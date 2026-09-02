//! Running what the model asked for.
//!
//! One rule shapes this file: **every call the transcript records has a result
//! recorded with it.** A provider refuses a transcript containing a request
//! with no answer, so a turn that stops half way through a pass — because the
//! user cancelled, or said no — still writes a result for each remaining call
//! saying why there is nothing in it.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::thread;
use std::time::Instant;

use crucible_core::{
    Ancestry, Approved, Ask, Cancel, Event, InvocationRecord, JournalStore, PendingCallResult,
    Permission, Reporter, RunItem, SandboxAudit, SandboxAuditRegistry, Settled, StopReason,
    TOOL_RESULT_BYTES, ToolCall, ToolContext, ToolEntry, ToolError, ToolExecutionMode, ToolId,
    ToolOutcome, ToolOutput, ToolOutputRetention, ToolReceipt, ToolResult, ToolSnapshot,
    ToolSourceReceipt, Watch, Wrote,
};

mod audit;

pub(super) use audit::report_sandbox_registry;
use audit::{report_sandbox_audit, report_sandbox_facts};

/// What a call is answered with when the turn ended before it could run.
const NOT_RUN: &str = "not run: the turn ended first";

/// What a call is answered with when the user said no.
const DENIED: &str = "the user did not allow this";

/// What a call is answered with when its output would cross the turn boundary.
const OUTPUT_LIMIT: &str = "not run: the turn output limit was reached";

/// What replaces a background acceptance whose protected result write failed.
const RESULT_STORAGE_FAILED: &str = "background command could not be durably accepted";

/// What a call is answered with when standing policy forbids it — a rule, or
/// the engine keeping its own configuration out of reach. Phrased for the
/// model, which is what reads it: it says the wall is standing rather than
/// momentary, so the answer is to do something else and not to rephrase this.
const FORBIDDEN: &str = "permission policy does not allow this; asking again will not change it";

/// What one pass of calls decided about the turn.
pub(crate) enum Went {
    /// Every call ran. Ask the model again.
    On,
    /// The user cancelled part way through.
    Stopped(StopReason),
    /// The user refused this tool.
    Refused(Box<str>),
    /// A tool result would have crossed the retained-output boundary.
    OutputLimit,
}

/// Everything a pass of calls needs, gathered so the runner reads as one thing.
pub(crate) struct Work<'a> {
    /// What may be called.
    pub(crate) tools: &'a ToolSnapshot,
    /// The session's memory of what was already allowed.
    pub(crate) permission: &'a mut Permission,
    /// How to put a call to the user.
    pub(crate) ask: &'a mut dyn Ask,
    /// Where progress is reported.
    pub(crate) events: Reporter<'a>,
    /// Whether the user has asked everything to stop.
    pub(crate) cancel: &'a Cancel,
    /// The run identity placed in each per-call context.
    pub(crate) ancestry: Ancestry,
    /// Durable framework history, distinct from provider messages.
    pub(crate) journal: &'a dyn JournalStore,
    /// Bounded owner of collectors retained by detached sandbox processes.
    pub(crate) audits: &'a SandboxAuditRegistry,
    /// The most opt-in calls that may execute at once.
    pub(crate) concurrency: usize,
}

impl Work<'_> {
    /// Runs `calls`, and answers every one of them in provider order.
    pub(crate) fn pass(
        &mut self,
        calls: &[ToolCall],
        held: usize,
        maximum: usize,
    ) -> (Vec<ToolResult>, Went, usize) {
        let mut results = Vec::with_capacity(calls.len());
        let mut went = Went::On;
        let mut produced = 0_usize;

        let mut at = 0;
        while at < calls.len() {
            let Some(call) = calls.get(at) else {
                break;
            };
            if let Some(invocation) = self.after_turn(call, &went) {
                self.finish(
                    invocation,
                    at,
                    calls.len(),
                    held,
                    maximum,
                    &mut produced,
                    &mut went,
                    &mut results,
                );
                at += 1;
                continue;
            }

            // Cancellation is the reason the turn ended even when there is
            // too little room left for its stand-in. It is also known before
            // admission, so budget reservation must not relabel it.
            if self.cancel.requested() {
                went = Went::Stopped(StopReason::Cancelled);
                continue;
            }

            let end = self.wave_end(calls, at);

            // Reserve the model-readable stand-ins for every recorded call
            // before this wave is admitted. This is the budget that cannot be
            // recovered later: even a refusal or cancellation must answer all
            // of those calls. Resource keys and worker slots were reserved by
            // `wave_end` before any permission or executor code runs.
            let required = calls.len().saturating_sub(at).saturating_mul(NOT_RUN.len());
            if held.saturating_add(produced).saturating_add(required) > maximum {
                went = Went::OutputLimit;
                continue;
            }

            let mut decisions = Vec::with_capacity(end - at);
            let mut ended = None;
            let Some(wave) = calls.get(at..end) else {
                break;
            };
            for call in wave {
                if ended.is_some() {
                    decisions.push(Decision::NotRun(self.stand_in(
                        call,
                        NOT_RUN,
                        ToolOutcome::NotRun,
                    )));
                    continue;
                }

                let decision = self.prepare(call);
                match &decision {
                    Decision::Refused(_) => {
                        ended = Some(WaveEnd::Refused);
                    }
                    Decision::Stopped(_) => ended = Some(WaveEnd::Stopped),
                    Decision::Ready(_) | Decision::Done(_) | Decision::NotRun(_) => {}
                }
                decisions.push(decision);
            }

            let invocations = if ended.is_some() {
                decisions
                    .into_iter()
                    .map(|decision| match decision {
                        Decision::Ready(prepared) => prepared.not_run(),
                        Decision::Done(invocation)
                        | Decision::Refused(invocation)
                        | Decision::Stopped(invocation)
                        | Decision::NotRun(invocation) => invocation,
                    })
                    .collect()
            } else {
                self.execute_wave(decisions)
            };

            for (offset, invocation) in invocations.into_iter().enumerate() {
                match invocation.outcome {
                    ToolOutcome::Refused => {
                        went = Went::Refused(invocation.call.name.clone());
                    }
                    ToolOutcome::Cancelled => {
                        went = Went::Stopped(StopReason::Cancelled);
                    }
                    ToolOutcome::Succeeded
                    | ToolOutcome::Failed
                    | ToolOutcome::Forbidden
                    | ToolOutcome::TimedOut
                    | ToolOutcome::Rejected
                    | ToolOutcome::NotRun
                    | ToolOutcome::OutputLimit
                    | ToolOutcome::Panicked => {}
                }
                self.finish(
                    invocation,
                    at + offset,
                    calls.len(),
                    held,
                    maximum,
                    &mut produced,
                    &mut went,
                    &mut results,
                );
            }
            at = end;
        }

        (results, went, produced)
    }

    /// Captures, validates, transforms, revalidates, classifies, guards, and
    /// approves one call without causing its tool effect.
    fn prepare(&mut self, call: &ToolCall) -> Decision {
        if self.cancel.requested() {
            return Decision::Stopped(self.stand_in(call, NOT_RUN, ToolOutcome::Cancelled));
        }

        let admission = match self.tools.admit(call) {
            Ok(admission) => admission,
            Err(problem) => return Decision::Done(Self::rejected(call, None, &problem)),
        };
        let entry = match self.tools.resolve(&admission) {
            Ok(entry) => entry,
            Err(problem) => return Decision::Done(Self::rejected(call, None, &problem)),
        };
        let source = Some(entry.descriptor().provenance().receipt());
        let result_limit = result_limit(entry);
        let raw_evidence =
            InvocationEvidence::new(source.clone(), call.args.as_str().len(), result_limit);

        if let Err(problem) = entry.tool().validate(&call.args) {
            return Decision::Done(Invocation::failed(
                call.clone(),
                &problem,
                ToolOutcome::Rejected,
                raw_evidence,
            ));
        }

        let args = match entry.hooks().argument() {
            Some(transform) => match transform.transform(call) {
                Ok(args) => args,
                Err(problem) => {
                    return Decision::Done(Invocation::failed(
                        call.clone(),
                        &problem,
                        ToolOutcome::Rejected,
                        raw_evidence,
                    ));
                }
            },
            None => call.args.clone(),
        };
        let transformed = ToolCall {
            id: call.id.clone(),
            name: call.name.clone(),
            args,
        };
        let evidence =
            InvocationEvidence::new(source, transformed.args.as_str().len(), result_limit);
        let admission = match self.tools.admit(&transformed) {
            Ok(admission) => admission,
            Err(problem) => {
                return Decision::Done(Invocation::failed(
                    transformed.clone(),
                    &problem,
                    ToolOutcome::Rejected,
                    evidence,
                ));
            }
        };
        let entry = match self.tools.resolve(&admission) {
            Ok(entry) => entry,
            Err(problem) => {
                return Decision::Done(Invocation::failed(
                    transformed.clone(),
                    &problem,
                    ToolOutcome::Rejected,
                    evidence,
                ));
            }
        };
        if let Err(problem) = entry.tool().validate(&transformed.args) {
            return Decision::Done(Invocation::failed(
                transformed.clone(),
                &problem,
                ToolOutcome::Rejected,
                evidence,
            ));
        }

        let sensitivity = entry.tool().sensitivity(&transformed.args);
        let guarded = self.permission.decide_admitted_guarded(
            &admission,
            &sensitivity,
            |final_call, final_sensitivity| match entry.hooks().input() {
                Some(guard) => guard.guard(final_call, final_sensitivity),
                None => Ok(()),
            },
            self.ask,
        );
        let settled = match guarded {
            Ok(settled) => settled,
            Err(problem) => {
                return Decision::Done(Invocation::failed(
                    transformed.clone(),
                    &problem,
                    ToolOutcome::Rejected,
                    evidence,
                ));
            }
        };

        match settled {
            Settled::Approved(approved) => match self.tools.resolve_approved(&approved) {
                Ok(approved_entry) => {
                    let record = InvocationRecord::new(
                        transformed.clone(),
                        self.ancestry,
                        approved_entry.descriptor().effect(),
                        approved_entry.tool().idempotency_key(&transformed.args),
                    );
                    self.journal
                        .append_run_item(&RunItem::Invocation(record.clone()));
                    Decision::Ready(Prepared {
                        call: transformed,
                        entry: approved_entry.clone(),
                        approved,
                        evidence,
                        record,
                    })
                }
                Err(problem) => Decision::Done(Invocation::failed(
                    transformed.clone(),
                    &problem,
                    ToolOutcome::Rejected,
                    evidence,
                )),
            },
            Settled::Forbidden => Decision::Done(Invocation::new(
                transformed.clone(),
                ToolOutput::failed(FORBIDDEN),
                ToolOutcome::Forbidden,
                evidence,
            )),
            Settled::Refused => Decision::Refused(Invocation::new(
                transformed.clone(),
                ToolOutput::failed(DENIED),
                ToolOutcome::Refused,
                evidence,
            )),
        }
    }

    /// Executes every approved call in one conflict-free scheduler wave.
    fn execute_wave(&self, decisions: Vec<Decision>) -> Vec<Invocation> {
        let host = ExecutionHost {
            ancestry: self.ancestry,
            cancel: self.cancel,
            events: self.events,
            journal: self.journal,
            audits: self.audits,
        };
        let ready = decisions
            .iter()
            .filter(|decision| matches!(decision, Decision::Ready(_)))
            .count();
        if ready <= 1 {
            return decisions
                .into_iter()
                .map(|decision| match decision {
                    Decision::Ready(prepared) => execute_contained(prepared, host),
                    Decision::Done(invocation)
                    | Decision::Refused(invocation)
                    | Decision::Stopped(invocation)
                    | Decision::NotRun(invocation) => invocation,
                })
                .collect();
        }

        let mut completed = Vec::with_capacity(decisions.len());
        thread::scope(|scope| {
            let mut running = Vec::with_capacity(ready);
            for (index, decision) in decisions.into_iter().enumerate() {
                match decision {
                    Decision::Ready(prepared) => {
                        let fallback = PanicFallback::from(&prepared);
                        running.push((
                            index,
                            fallback,
                            scope.spawn(move || execute_contained(prepared, host)),
                        ));
                    }
                    Decision::Done(invocation)
                    | Decision::Refused(invocation)
                    | Decision::Stopped(invocation)
                    | Decision::NotRun(invocation) => completed.push((index, invocation)),
                }
            }
            for (index, fallback, worker) in running {
                let invocation = match worker.join() {
                    Ok(invocation) => invocation,
                    Err(_) => fallback.panicked(),
                };
                completed.push((index, invocation));
            }
        });
        completed.sort_by_key(|(index, _)| *index);
        completed
            .into_iter()
            .map(|(_, invocation)| invocation)
            .collect()
    }

    /// Ends the next conflict-free, bounded wave before any call is admitted.
    fn wave_end(&self, calls: &[ToolCall], start: usize) -> usize {
        let ceiling = self.concurrency.max(1);
        let Some(first) = calls.get(start) else {
            return start;
        };
        if ceiling == 1 || matches!(self.mode(first), ToolExecutionMode::Sequential) {
            return start + 1;
        }

        let mut end = start;
        let mut exclusive = Vec::<Box<str>>::new();
        while end < calls.len() && end - start < ceiling {
            let Some(call) = calls.get(end) else {
                break;
            };
            match self.mode(call) {
                ToolExecutionMode::Sequential => break,
                ToolExecutionMode::Parallel => end += 1,
                ToolExecutionMode::Exclusive(key) => {
                    if exclusive.iter().any(|held| &**held == key.as_str()) {
                        break;
                    }
                    exclusive.push(key.as_str().into());
                    end += 1;
                }
            }
        }
        end.max(start + 1)
    }

    fn mode(&self, call: &ToolCall) -> ToolExecutionMode {
        self.tools
            .find(&call.name)
            .map_or(ToolExecutionMode::Sequential, |entry| {
                entry.descriptor().execution().clone()
            })
    }

    fn rejected(call: &ToolCall, entry: Option<&ToolEntry>, problem: &ToolError) -> Invocation {
        let evidence = InvocationEvidence::new(
            entry.map(|entry| entry.descriptor().provenance().receipt()),
            call.args.as_str().len(),
            entry.map_or(TOOL_RESULT_BYTES, result_limit),
        );
        Invocation::failed(call.clone(), problem, ToolOutcome::Rejected, evidence)
    }

    fn stand_in(&self, call: &ToolCall, text: &str, outcome: ToolOutcome) -> Invocation {
        let entry = self.tools.find(&call.name);
        let evidence = InvocationEvidence::new(
            entry.map(|entry| entry.descriptor().provenance().receipt()),
            call.args.as_str().len(),
            entry.map_or(TOOL_RESULT_BYTES, result_limit),
        );
        Invocation::new(call.clone(), ToolOutput::failed(text), outcome, evidence)
    }

    fn after_turn(&self, call: &ToolCall, went: &Went) -> Option<Invocation> {
        match went {
            Went::Stopped(_) | Went::Refused(_) => {
                Some(self.stand_in(call, NOT_RUN, ToolOutcome::NotRun))
            }
            Went::OutputLimit => Some(self.stand_in(call, "", ToolOutcome::OutputLimit)),
            Went::On => None,
        }
    }

    // These are the two sides of one atomic finalization: the shared turn
    // budget/state and both ordered sinks. Wrapping references in a carrier
    // would shorten the signature without reducing the operation's inputs.
    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        mut invocation: Invocation,
        index: usize,
        total: usize,
        held: usize,
        maximum: usize,
        produced: &mut usize,
        went: &mut Went,
        results: &mut Vec<ToolResult>,
    ) {
        // Leave enough room to answer every later call even when this one
        // fills the budget. The provider requires a result for every call
        // already recorded, so dropping the tail is not a valid bound.
        let later = total.saturating_sub(index + 1);
        let reserved = later.saturating_mul(NOT_RUN.len());
        let room = maximum
            .saturating_sub(held)
            .saturating_sub(*produced)
            .saturating_sub(reserved);
        let turn_limited = invocation.output.text().len() > room;
        if turn_limited {
            invocation.output = ToolOutput::failed(if OUTPUT_LIMIT.len() <= room {
                OUTPUT_LIMIT
            } else {
                ""
            });
            invocation.retention = invocation
                .output
                .limit_encoded(invocation.evidence.result_limit);
            if matches!(went, Went::On | Went::OutputLimit) {
                *went = Went::OutputLimit;
                invocation.outcome = ToolOutcome::OutputLimit;
            }
        }
        if let Some(pending) = invocation.pending_result.take()
            && !turn_limited
        {
            let mut durable_output = invocation.output.clone();
            durable_output.forget_diff();
            let result = ToolResult {
                id: invocation.call.id.clone(),
                output: durable_output,
            };
            if let Ok(receipt) = self.journal.put_call_result(pending.key(), &result) {
                // The result is already durable and replayable. A failed
                // executor acknowledgement quarantines/stops its owned scope,
                // but cannot replace that sole accepted result.
                let _ = pending.accept(receipt);
            } else {
                // Dropping the unaccepted executor half reclaims its
                // application-owned process scope.
                drop(pending);
                invocation.output = ToolOutput::failed(if RESULT_STORAGE_FAILED.len() <= room {
                    RESULT_STORAGE_FAILED
                } else {
                    ""
                });
                invocation.retention = invocation
                    .output
                    .limit_encoded(invocation.evidence.result_limit);
                invocation.outcome = ToolOutcome::Failed;
            }
        }
        *produced = produced.saturating_add(invocation.output.text().len());

        if let Some(mut recovery) = invocation.recovery.take() {
            let _ = recovery.finish(invocation.outcome, invocation.output.clone());
            self.journal.append_run_item(&RunItem::Invocation(recovery));
        }

        let receipt = ToolReceipt::new(
            self.tools.generation().clone(),
            invocation.evidence.source,
            invocation.evidence.input_bytes,
            invocation.retention,
            invocation.outcome,
        );
        self.events.post(Event::ToolFinished {
            call: invocation.call.id.clone(),
            output: invocation.output.clone(),
            receipt: Some(receipt),
        });

        invocation.output.forget_diff();
        results.push(ToolResult {
            id: invocation.call.id,
            output: invocation.output,
        });
    }
}

enum WaveEnd {
    Refused,
    Stopped,
}

enum Decision {
    Ready(Prepared),
    Done(Invocation),
    Refused(Invocation),
    Stopped(Invocation),
    NotRun(Invocation),
}

struct Prepared {
    call: ToolCall,
    entry: ToolEntry,
    approved: Approved,
    evidence: InvocationEvidence,
    record: InvocationRecord,
}

impl Prepared {
    fn not_run(self) -> Invocation {
        Invocation::new(
            self.call,
            ToolOutput::failed(NOT_RUN),
            ToolOutcome::NotRun,
            self.evidence,
        )
        .recovering(self.record)
    }
}

#[derive(Clone)]
struct InvocationEvidence {
    source: Option<ToolSourceReceipt>,
    input_bytes: usize,
    result_limit: usize,
}

impl InvocationEvidence {
    fn new(source: Option<ToolSourceReceipt>, input_bytes: usize, result_limit: usize) -> Self {
        Self {
            source,
            input_bytes,
            result_limit,
        }
    }
}

struct Invocation {
    call: ToolCall,
    output: ToolOutput,
    outcome: ToolOutcome,
    evidence: InvocationEvidence,
    retention: ToolOutputRetention,
    recovery: Option<InvocationRecord>,
    pending_result: Option<PendingCallResult>,
}

impl Invocation {
    fn new(
        call: ToolCall,
        mut output: ToolOutput,
        outcome: ToolOutcome,
        evidence: InvocationEvidence,
    ) -> Self {
        let retention = output.limit_encoded(evidence.result_limit);
        Self {
            call,
            output,
            outcome,
            evidence,
            retention,
            recovery: None,
            pending_result: None,
        }
    }

    fn failed(
        call: ToolCall,
        problem: &ToolError,
        outcome: ToolOutcome,
        evidence: InvocationEvidence,
    ) -> Self {
        Self::new(call, failure(problem), outcome, evidence)
    }

    fn recovering(mut self, record: InvocationRecord) -> Self {
        self.recovery = Some(record);
        self
    }

    fn accepting(mut self, pending: Option<PendingCallResult>) -> Self {
        self.pending_result = pending;
        self
    }
}

#[derive(Clone, Copy)]
struct ExecutionHost<'a> {
    ancestry: Ancestry,
    cancel: &'a Cancel,
    events: Reporter<'a>,
    journal: &'a dyn JournalStore,
    audits: &'a SandboxAuditRegistry,
}

fn execute_contained(prepared: Prepared, host: ExecutionHost<'_>) -> Invocation {
    let fallback = PanicFallback::from(&prepared);
    let audit = match host
        .audits
        .collector(host.ancestry, fallback.call.id.clone())
    {
        Ok(audit) => audit,
        Err(problem) => return fallback.audit_failed(problem),
    };
    if let Ok(invocation) =
        catch_unwind(AssertUnwindSafe(|| execute(prepared, host, audit.clone())))
    {
        invocation
    } else {
        let _ = report_sandbox_audit(
            &audit,
            host.ancestry,
            &fallback.call.id,
            host.events,
            host.journal,
        );
        fallback.panicked()
    }
}

fn execute(prepared: Prepared, host: ExecutionHost<'_>, audit: SandboxAudit) -> Invocation {
    let Prepared {
        call,
        entry,
        approved,
        evidence,
        mut record,
    } = prepared;
    let _ = record.start();
    host.journal
        .append_run_item(&RunItem::Invocation(record.clone()));
    let deadline = entry
        .descriptor()
        .timeout()
        .and_then(|timeout| Instant::now().checked_add(timeout));
    let watching = Watching {
        call: call.id.clone(),
        events: host.events,
    };
    let context = match ToolContext::new(
        host.ancestry,
        call.id.clone(),
        host.cancel,
        deadline,
        &watching,
    )
    .with_call_result_store(record.id(), host.journal)
    .with_sandbox_audit(audit)
    {
        Ok(context) => context,
        Err(problem) => {
            let problem = ToolError::Io {
                tool: "sandbox audit".into(),
                problem: "could not attach fixed lifecycle attribution".into(),
                source: std::io::Error::other(problem),
            };
            return Invocation::failed(call, &problem, ToolOutcome::Failed, evidence)
                .recovering(record);
        }
    };
    let ran = entry.tool().run(approved, &context);
    if let Err(problem) = report_sandbox_facts(&context, host.events, host.journal) {
        return Invocation::failed(call, &problem, ToolOutcome::Failed, evidence)
            .recovering(record);
    }

    if host.cancel.requested() {
        return Invocation::new(
            call,
            ToolOutput::failed(NOT_RUN),
            ToolOutcome::Cancelled,
            evidence,
        )
        .recovering(record);
    }
    if context.timed_out() {
        return Invocation::new(
            call,
            ToolOutput::failed("tool timed out"),
            ToolOutcome::TimedOut,
            evidence,
        )
        .recovering(record);
    }

    let output = match ran {
        Ok(output) => match entry.hooks().output() {
            Some(guard) => match guard.guard(&call, output) {
                Ok(output) => output,
                Err(problem) => {
                    return Invocation::failed(call, &problem, ToolOutcome::Failed, evidence)
                        .recovering(record);
                }
            },
            None => output,
        },
        Err(ToolError::Cancelled(_)) => {
            return Invocation::new(
                call,
                ToolOutput::failed(NOT_RUN),
                ToolOutcome::Cancelled,
                evidence,
            )
            .recovering(record);
        }
        Err(problem) => {
            return Invocation::failed(call, &problem, ToolOutcome::Failed, evidence)
                .recovering(record);
        }
    };
    let pending = match context.take_call_result() {
        Ok(pending) => pending,
        Err(problem) => {
            let problem = ToolError::Io {
                tool: call.name.clone(),
                problem: "could not transfer deferred result ownership".into(),
                source: std::io::Error::other(problem),
            };
            return Invocation::failed(call, &problem, ToolOutcome::Failed, evidence)
                .recovering(record);
        }
    };
    let outcome = if output.is_failed() {
        ToolOutcome::Failed
    } else {
        ToolOutcome::Succeeded
    };
    Invocation::new(call, output, outcome, evidence)
        .recovering(record)
        .accepting(pending)
}

struct PanicFallback {
    call: ToolCall,
    evidence: InvocationEvidence,
    record: InvocationRecord,
}

impl From<&Prepared> for PanicFallback {
    fn from(prepared: &Prepared) -> Self {
        Self {
            call: prepared.call.clone(),
            evidence: prepared.evidence.clone(),
            record: prepared.record.clone(),
        }
    }
}

impl PanicFallback {
    fn panicked(self) -> Invocation {
        Invocation::new(
            self.call,
            ToolOutput::failed("tool panicked; the failure was contained"),
            ToolOutcome::Panicked,
            self.evidence,
        )
        .recovering(self.record)
    }

    fn audit_failed(self, problem: crucible_core::SandboxAuditError) -> Invocation {
        let error = ToolError::Io {
            tool: "sandbox audit".into(),
            problem: "could not register the bounded sandbox lifecycle".into(),
            source: std::io::Error::other(problem),
        };
        Invocation::failed(self.call, &error, ToolOutcome::Failed, self.evidence)
            .recovering(self.record)
    }
}

fn result_limit(entry: &ToolEntry) -> usize {
    entry
        .descriptor()
        .result_bytes()
        .unwrap_or(TOOL_RESULT_BYTES)
        .min(TOOL_RESULT_BYTES)
}

/// Where one call's output goes while its tool is still running.
///
/// The whole of the bridge between a tool, which knows what it has printed and
/// not which call it is, and the channel, which needs both. It is made per call
/// rather than per pass so that the identifier cannot be the wrong one: there is
/// no moment at which this value exists beside a different call.
///
/// Nothing is held. A piece of output is turned into an event and posted, and
/// what the drawing thread does with it is the drawing thread's business — which
/// is what keeps a command printing a gigabyte from growing anything here.
struct Watching<'a> {
    /// The call whose output this is.
    call: ToolId,
    /// Where it goes.
    events: Reporter<'a>,
}

impl Watch for Watching<'_> {
    fn wrote(&self, text: Wrote) {
        self.events.post(Event::Wrote {
            call: self.call.clone(),
            text,
        });
    }
}

/// A failure the model is meant to read and work around.
fn failure(problem: &ToolError) -> ToolOutput {
    ToolOutput::failed(problem.to_string())
}

#[cfg(test)]
mod tests;
