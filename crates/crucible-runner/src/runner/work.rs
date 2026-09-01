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
    Ancestry, Approved, Ask, Cancel, Event, InvocationRecord, JournalStore, Permission, Reporter,
    RunItem, SandboxAudit, SandboxAuditRegistry, Settled, StopReason, TOOL_RESULT_BYTES, ToolCall,
    ToolContext, ToolEntry, ToolError, ToolExecutionMode, ToolId, ToolOutcome, ToolOutput,
    ToolOutputRetention, ToolReceipt, ToolResult, ToolSnapshot, ToolSourceReceipt, Watch, Wrote,
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
        if invocation.output.text().len() > room {
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
    let outcome = if output.is_failed() {
        ToolOutcome::Failed
    } else {
        ToolOutcome::Succeeded
    };
    Invocation::new(call, output, outcome, evidence).recovering(record)
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
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc::{Receiver, Sender, channel};
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::Duration;

    use crucible_core::{
        Ancestry, ArgumentTransform, Disposition, EventEnvelope, IdempotencyKey, InputGuard,
        InvocationState, JournalStore, Mode, OutputGuard, Post, RecoveryAction, Remember, Rules,
        SandboxCleanup, SandboxFactKind, SandboxId, SandboxLifecycle, Sensitivity, Summary, Target,
        Tool, ToolArgs, ToolDescriptor, ToolEffect, ToolExecutionMode, ToolHooks, ToolId,
        ToolProvenance, ToolResourceKey, ToolSourceKind, Verdict,
    };

    use super::*;
    use crate::Tools;
    use crate::fake::{Fixed, Says, changing};

    #[derive(Default)]
    struct KeepingJournal(Mutex<Vec<RunItem>>);

    impl JournalStore for KeepingJournal {
        fn append_run_item(&self, item: &RunItem) {
            self.0.lock().unwrap().push(item.clone());
        }
    }

    mod sandbox_audit;

    #[test]
    fn what_a_tool_prints_while_it_runs_arrives_under_its_own_call() {
        // Ordered against the event that ends the call, because that is the
        // whole of what a reader is owed: output while the call is out, then
        // the result. A piece arriving after `ToolFinished` would be drawn
        // under whatever call came next.
        let mut proof = Proof::new(Verdict::Allow)
            .offering(Fixed::new("bash").writing(&["Compiling one\n", "Compiling two\n"]));

        let call = ToolCall {
            id: ToolId::new("c-1"),
            name: "bash".into(),
            args: ToolArgs::new("{}"),
        };
        proof.pass(std::slice::from_ref(&call));

        let mut wrote = Vec::new();
        let mut finished = false;
        while let Ok(event) = proof.seen.try_recv() {
            match event {
                Event::Wrote { call, text } => {
                    assert!(!finished, "output arrived after the call had answered");
                    assert_eq!(call, ToolId::new("c-1"));
                    wrote.push(text.as_str().to_owned());
                }
                Event::ToolFinished { .. } => finished = true,
                Event::TurnStarted { .. }
                | Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Aged { .. }
                | Event::Unread { .. }
                | Event::Steered { .. }
                | Event::Spent { .. }
                | Event::TurnFinished { .. }
                | Event::Failed { .. } => {}
            }
        }

        assert_eq!(wrote, ["Compiling one\n", "Compiling two\n"]);
        assert!(finished, "the call never answered");
    }

    /// A destination that keeps the event and lets the attribution go: these
    /// assertions are about what a pass does, not about whose pass it was.
    struct Keeping(Sender<Event>);

    impl Post for Keeping {
        fn post(&self, reported: EventEnvelope) {
            drop(self.0.send(reported.into_event()));
        }
    }

    type Trace = Arc<Mutex<Vec<&'static str>>>;

    fn marked(trace: &Trace, stage: &'static str) {
        trace.lock().unwrap().push(stage);
    }

    struct PipelineTool {
        trace: Trace,
        invalid_final: bool,
        answer: Box<str>,
    }

    impl Tool for PipelineTool {
        fn validate(&self, args: &ToolArgs) -> Result<(), ToolError> {
            let stage = if args.as_str() == "raw" {
                "validate raw"
            } else {
                "validate transformed"
            };
            marked(&self.trace, stage);
            if self.invalid_final && args.as_str() == "transformed" {
                return Err(ToolError::Arguments {
                    tool: "pipeline".into(),
                    problem: "transformed arguments were invalid".into(),
                });
            }
            Ok(())
        }

        fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
            assert_eq!(args.as_str(), "transformed");
            marked(&self.trace, "sensitivity");
            changing()
        }

        fn summary(&self, _args: &ToolArgs) -> Summary {
            Summary::new("pipeline")
        }

        fn run(
            &self,
            approved: Approved,
            _context: &ToolContext<'_>,
        ) -> Result<ToolOutput, ToolError> {
            assert_eq!(approved.args().as_str(), "transformed");
            marked(&self.trace, "execute");
            Ok(ToolOutput::ok(self.answer.clone()))
        }
    }

    struct Transform(Trace);

    impl ArgumentTransform for Transform {
        fn transform(&self, _call: &ToolCall) -> Result<ToolArgs, ToolError> {
            marked(&self.0, "transform");
            Ok(ToolArgs::new("transformed"))
        }
    }

    struct GuardInput(Trace);

    impl InputGuard for GuardInput {
        fn guard(&self, call: &ToolCall, _sensitivity: &Sensitivity) -> Result<(), ToolError> {
            assert_eq!(call.args.as_str(), "transformed");
            marked(&self.0, "input guard");
            Ok(())
        }
    }

    struct GuardOutput(Trace);

    impl OutputGuard for GuardOutput {
        fn guard(&self, _call: &ToolCall, output: ToolOutput) -> Result<ToolOutput, ToolError> {
            marked(&self.0, "output guard");
            Ok(output)
        }
    }

    struct TracedAsk(Trace);

    impl Ask for TracedAsk {
        fn ask(&mut self, call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            assert_eq!(call.args.as_str(), "transformed");
            marked(&self.0, "approval");
            (Verdict::Allow, Remember::Never)
        }
    }

    fn pipeline_tools(
        trace: &Trace,
        invalid_final: bool,
        answer: impl Into<Box<str>>,
        result_bytes: Option<usize>,
    ) -> Tools {
        let provenance =
            ToolProvenance::new(ToolSourceKind::User, "test:pipeline", "pipeline test").unwrap();
        let mut descriptor = ToolDescriptor::new("pipeline", "{}", provenance).unwrap();
        if let Some(bytes) = result_bytes {
            descriptor = descriptor.limiting_result_to(bytes).unwrap();
        }
        let hooks = ToolHooks::new()
            .transforming(Arc::new(Transform(Arc::clone(trace))))
            .guarding_input(Arc::new(GuardInput(Arc::clone(trace))))
            .guarding_output(Arc::new(GuardOutput(Arc::clone(trace))));
        let mut tools = Tools::new();
        tools
            .add_with_hooks(
                descriptor,
                Arc::new(PipelineTool {
                    trace: Arc::clone(trace),
                    invalid_final,
                    answer: answer.into(),
                }),
                hooks,
            )
            .unwrap();
        tools
    }

    fn invoke(
        tools: &Tools,
        permission: &mut Permission,
        ask: &mut dyn Ask,
        call: ToolCall,
    ) -> (Vec<ToolResult>, Went, Vec<Event>) {
        invoke_many(tools, permission, ask, &[call], (1, usize::MAX))
    }

    fn invoke_many(
        tools: &Tools,
        permission: &mut Permission,
        ask: &mut dyn Ask,
        calls: &[ToolCall],
        limits: (usize, usize),
    ) -> (Vec<ToolResult>, Went, Vec<Event>) {
        let (concurrency, maximum) = limits;
        let (events, seen) = channel();
        let keeping = Keeping(events);
        let ancestry = Ancestry::new();
        let snapshot = tools.snapshot().unwrap();
        let cancel = Cancel::new();
        let journal = crucible_session::Session::nowhere();
        let (results, went, _) = Work {
            tools: &snapshot,
            permission,
            ask,
            events: Reporter::new(ancestry, &keeping),
            cancel: &cancel,
            ancestry,
            journal: &journal,
            audits: &SandboxAuditRegistry::new(),
            concurrency,
        }
        .pass(calls, 0, maximum);
        drop(keeping);
        (results, went, seen.try_iter().collect())
    }

    fn pipeline_call() -> ToolCall {
        ToolCall {
            id: ToolId::new("pipeline-call"),
            name: "pipeline".into(),
            args: ToolArgs::new("raw"),
        }
    }

    #[test]
    fn the_invocation_pipeline_runs_every_stage_in_its_declared_order() {
        let trace = Trace::default();
        let tools = pipeline_tools(&trace, false, "done", None);
        let mut permission = Permission::new();
        let mut ask = TracedAsk(Arc::clone(&trace));

        let (results, went, events) = invoke(&tools, &mut permission, &mut ask, pipeline_call());

        assert_eq!(
            results.first().map(|result| result.output.text()),
            Some("done")
        );
        assert!(matches!(went, Went::On));
        assert_eq!(
            *trace.lock().unwrap(),
            [
                "validate raw",
                "transform",
                "validate transformed",
                "sensitivity",
                "input guard",
                "approval",
                "execute",
                "output guard",
            ]
        );
        assert!(events.iter().any(|event| matches!(
            event,
            Event::ToolFinished {
                receipt: Some(receipt),
                ..
            } if receipt.outcome() == ToolOutcome::Succeeded
                && receipt.input_bytes() == "transformed".len()
        )));
    }

    #[test]
    fn transformed_arguments_are_revalidated_before_sensitivity_or_authority() {
        let trace = Trace::default();
        let tools = pipeline_tools(&trace, true, "never", None);
        let mut permission = Permission::new();
        let mut ask = TracedAsk(Arc::clone(&trace));

        let (results, went, _) = invoke(&tools, &mut permission, &mut ask, pipeline_call());

        assert!(
            results
                .first()
                .is_some_and(|result| result.output.is_failed())
        );
        assert!(matches!(went, Went::On));
        assert_eq!(
            *trace.lock().unwrap(),
            ["validate raw", "transform", "validate transformed"]
        );
    }

    struct KeyedEffect;

    impl Tool for KeyedEffect {
        fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
            Ok(())
        }

        fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
            Sensitivity::ReadOnly {
                target: Target::unresolved(),
            }
        }

        fn idempotency_key(&self, _args: &ToolArgs) -> Option<IdempotencyKey> {
            Some(IdempotencyKey::new("operation-42").unwrap())
        }

        fn summary(&self, _args: &ToolArgs) -> Summary {
            Summary::new("keyed effect")
        }

        fn run(
            &self,
            _approved: Approved,
            _context: &ToolContext<'_>,
        ) -> Result<ToolOutput, ToolError> {
            Ok(ToolOutput::ok("one effect"))
        }
    }

    #[test]
    fn an_approved_effect_journals_one_stable_prepared_started_and_finished_invocation() {
        let descriptor = ToolDescriptor::new(
            "keyed",
            "{}",
            ToolProvenance::new(ToolSourceKind::User, "test:keyed", "keyed test").unwrap(),
        )
        .unwrap()
        .causing(ToolEffect::Idempotent);
        let mut tools = Tools::new();
        tools.add(descriptor, Arc::new(KeyedEffect)).unwrap();
        let snapshot = tools.snapshot().unwrap();
        let journal = KeepingJournal::default();
        let (events, _seen) = channel();
        let keeping = Keeping(events);
        let ancestry = Ancestry::new();
        let cancel = Cancel::new();
        let mut permission = Permission::new();
        let mut ask = Says::new(Verdict::Allow);

        let (results, went, _) = Work {
            tools: &snapshot,
            permission: &mut permission,
            ask: &mut ask,
            events: Reporter::new(ancestry, &keeping),
            cancel: &cancel,
            ancestry,
            journal: &journal,
            audits: &SandboxAuditRegistry::new(),
            concurrency: 1,
        }
        .pass(&[call("keyed-call", "keyed")], 0, usize::MAX);

        assert!(matches!(went, Went::On));
        assert_eq!(results.len(), 1);
        let held = journal.0.lock().unwrap();
        let invocations = held
            .iter()
            .filter_map(|item| match item {
                RunItem::Invocation(invocation) => Some(invocation),
                _ => None,
            })
            .collect::<Vec<_>>();
        let [prepared, started, finished] = invocations.as_slice() else {
            panic!("expected prepared, started and finished records");
        };
        assert_eq!(prepared.id(), started.id());
        assert_eq!(started.id(), finished.id());
        assert!(matches!(prepared.state(), InvocationState::Prepared));
        assert!(matches!(started.state(), InvocationState::Started));
        assert!(matches!(finished.state(), InvocationState::Finished { .. }));
        assert_eq!(prepared.effect(), ToolEffect::Idempotent);
        assert!(prepared.idempotency_key().is_some());
        assert_eq!(prepared.recovery(), RecoveryAction::Retry);
        assert_eq!(started.recovery(), RecoveryAction::RetryWithIdempotencyKey);
        assert_eq!(finished.recovery(), RecoveryAction::UseRecordedResult);
    }

    #[test]
    fn standing_denial_runs_before_the_input_guard_and_never_executes() {
        let trace = Trace::default();
        let tools = pipeline_tools(&trace, false, "never", None);
        let mut rules = Rules::new();
        rules.add(Disposition::Deny, "pipeline").unwrap();
        let mut permission = Permission::with(Mode::FullAccess, rules);
        let mut ask = TracedAsk(Arc::clone(&trace));

        let (results, went, _) = invoke(&tools, &mut permission, &mut ask, pipeline_call());

        assert_eq!(
            results.first().map(|result| result.output.text()),
            Some(FORBIDDEN)
        );
        assert!(matches!(went, Went::On));
        assert_eq!(
            *trace.lock().unwrap(),
            [
                "validate raw",
                "transform",
                "validate transformed",
                "sensitivity",
            ]
        );
    }

    #[test]
    fn encoded_output_is_bounded_after_the_output_guard_and_before_the_event() {
        let trace = Trace::default();
        let raw = "\"".repeat(300);
        let tools = pipeline_tools(&trace, false, raw, Some(512));
        let mut permission = Permission::new();
        let mut ask = TracedAsk(Arc::clone(&trace));

        let (results, _, events) = invoke(&tools, &mut permission, &mut ask, pipeline_call());
        let (event_output, receipt) = events
            .iter()
            .find_map(|event| match event {
                Event::ToolFinished {
                    output,
                    receipt: Some(receipt),
                    ..
                } => Some((output, receipt)),
                _ => None,
            })
            .unwrap();

        assert_eq!(
            Some(event_output.text()),
            results.first().map(|result| result.output.text())
        );
        assert!(event_output.text().contains("encoded bytes"));
        assert!(receipt.output().omitted() > 0);
        assert!(receipt.output().retained() <= 512);
        assert_eq!(trace.lock().unwrap().last(), Some(&"output guard"));
    }

    struct TimesOut(Arc<AtomicUsize>);

    impl Tool for TimesOut {
        fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
            Ok(())
        }

        fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
            Sensitivity::ReadOnly {
                target: Target::unresolved(),
            }
        }

        fn summary(&self, _args: &ToolArgs) -> Summary {
            Summary::new("timeout")
        }

        fn run(
            &self,
            _approved: Approved,
            context: &ToolContext<'_>,
        ) -> Result<ToolOutput, ToolError> {
            self.0.fetch_add(1, Ordering::SeqCst);
            while !context.cancel().requested() {
                thread::yield_now();
            }
            Err(ToolError::Cancelled("timeout".into()))
        }
    }

    #[test]
    fn a_descriptor_timeout_finalizes_once_without_stopping_the_run() {
        let ran = Arc::new(AtomicUsize::new(0));
        let descriptor = ToolDescriptor::new(
            "timeout",
            "{}",
            ToolProvenance::new(ToolSourceKind::User, "test:timeout", "timeout test").unwrap(),
        )
        .unwrap()
        .timing_out_after(Duration::from_millis(5))
        .unwrap();
        let mut tools = Tools::new();
        tools
            .add(descriptor, Arc::new(TimesOut(Arc::clone(&ran))))
            .unwrap();
        let mut permission = Permission::new();
        let mut ask = Says::new(Verdict::Allow);

        let (results, went, events) = invoke(
            &tools,
            &mut permission,
            &mut ask,
            call("timeout-call", "timeout"),
        );

        assert_eq!(ran.load(Ordering::SeqCst), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results.first().map(|result| result.output.text()),
            Some("tool timed out")
        );
        assert!(matches!(went, Went::On));
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::ToolFinished { .. }))
                .count(),
            1
        );
        assert!(events.iter().any(|event| matches!(
            event,
            Event::ToolFinished {
                receipt: Some(receipt),
                ..
            } if receipt.outcome() == ToolOutcome::TimedOut
        )));
    }

    #[derive(Default)]
    struct ScheduleState {
        active: AtomicUsize,
        peak: AtomicUsize,
        ran: AtomicUsize,
        approvals: AtomicUsize,
        completed: Mutex<Vec<String>>,
    }

    struct Scheduled {
        state: Arc<ScheduleState>,
        barrier: Option<Arc<Barrier>>,
        approvals_before_effects: usize,
    }

    impl Tool for Scheduled {
        fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
            Ok(())
        }

        fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
            changing()
        }

        fn summary(&self, args: &ToolArgs) -> Summary {
            Summary::new(args.as_str())
        }

        fn run(
            &self,
            approved: Approved,
            _context: &ToolContext<'_>,
        ) -> Result<ToolOutput, ToolError> {
            assert!(
                self.state.approvals.load(Ordering::SeqCst) >= self.approvals_before_effects,
                "an effect began before its scheduler wave finished approval"
            );
            self.state.ran.fetch_add(1, Ordering::SeqCst);
            let active = self.state.active.fetch_add(1, Ordering::SeqCst) + 1;
            self.state.peak.fetch_max(active, Ordering::SeqCst);
            if let Some(barrier) = &self.barrier {
                barrier.wait();
            }
            if approved.args().as_str().contains("slow") {
                thread::sleep(Duration::from_millis(20));
            }
            self.state
                .completed
                .lock()
                .unwrap()
                .push(approved.args().as_str().to_owned());
            self.state.active.fetch_sub(1, Ordering::SeqCst);
            Ok(ToolOutput::ok(approved.args().as_str()))
        }
    }

    struct CountedAsk {
        state: Arc<ScheduleState>,
        answers: Vec<Verdict>,
        at: usize,
    }

    impl CountedAsk {
        fn allowing(state: Arc<ScheduleState>) -> Self {
            Self {
                state,
                answers: vec![Verdict::Allow],
                at: 0,
            }
        }
    }

    impl Ask for CountedAsk {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            self.state.approvals.fetch_add(1, Ordering::SeqCst);
            let answer = self
                .answers
                .get(self.at)
                .copied()
                .or_else(|| self.answers.last().copied())
                .unwrap_or(Verdict::Deny);
            self.at += 1;
            (answer, Remember::Never)
        }
    }

    fn scheduled_tools(
        registrations: &[(&str, ToolExecutionMode)],
        state: &Arc<ScheduleState>,
        barrier: Option<Arc<Barrier>>,
        approvals_before_effects: usize,
    ) -> Tools {
        let executor: Arc<dyn Tool> = Arc::new(Scheduled {
            state: Arc::clone(state),
            barrier,
            approvals_before_effects,
        });
        let mut tools = Tools::new();
        for (name, mode) in registrations {
            let descriptor = ToolDescriptor::new(
                *name,
                "{}",
                ToolProvenance::new(
                    ToolSourceKind::User,
                    format!("test:{name}"),
                    format!("{name} scheduler test"),
                )
                .unwrap(),
            )
            .unwrap()
            .executing(mode.clone());
            tools.add(descriptor, Arc::clone(&executor)).unwrap();
        }
        tools
    }

    fn scheduled_call(id: &str, name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new(args),
        }
    }

    #[test]
    fn parallel_calls_obey_the_run_ceiling_and_finish_in_provider_order() {
        let state = Arc::new(ScheduleState::default());
        let tools = scheduled_tools(
            &[("parallel", ToolExecutionMode::Parallel)],
            &state,
            Some(Arc::new(Barrier::new(2))),
            2,
        );
        let calls = [
            scheduled_call("a", "parallel", "slow-a"),
            scheduled_call("b", "parallel", "fast-b"),
            scheduled_call("c", "parallel", "slow-c"),
            scheduled_call("d", "parallel", "fast-d"),
        ];
        let mut permission = Permission::new();
        let mut ask = CountedAsk::allowing(Arc::clone(&state));

        let (results, went, events) =
            invoke_many(&tools, &mut permission, &mut ask, &calls, (2, usize::MAX));

        assert!(matches!(went, Went::On));
        assert_eq!(state.peak.load(Ordering::SeqCst), 2);
        assert_eq!(
            results
                .iter()
                .map(|result| result.output.text())
                .collect::<Vec<_>>(),
            ["slow-a", "fast-b", "slow-c", "fast-d"]
        );
        assert_eq!(
            events
                .iter()
                .filter_map(|event| match event {
                    Event::ToolFinished { call, .. } => Some(call.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>(),
            ["a", "b", "c", "d"]
        );
        assert_eq!(
            *state.completed.lock().unwrap(),
            ["fast-b", "slow-a", "fast-d", "slow-c"],
            "the fixture did not actually complete out of provider order"
        );
    }

    #[test]
    fn one_exclusive_resource_key_never_overlaps_itself() {
        let state = Arc::new(ScheduleState::default());
        let key = ToolResourceKey::new("workspace:index").unwrap();
        let tools = scheduled_tools(
            &[
                ("exclusive_a", ToolExecutionMode::Exclusive(key.clone())),
                ("exclusive_b", ToolExecutionMode::Exclusive(key)),
            ],
            &state,
            None,
            1,
        );
        let calls = [
            scheduled_call("a", "exclusive_a", "slow-a"),
            scheduled_call("b", "exclusive_b", "slow-b"),
        ];
        let mut permission = Permission::new();
        let mut ask = CountedAsk::allowing(Arc::clone(&state));

        let (results, went, _) =
            invoke_many(&tools, &mut permission, &mut ask, &calls, (2, usize::MAX));

        assert!(matches!(went, Went::On));
        assert_eq!(results.len(), 2);
        assert_eq!(state.peak.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn sequential_mode_remains_a_barrier_even_under_a_wider_run() {
        let state = Arc::new(ScheduleState::default());
        let tools = scheduled_tools(
            &[("sequential", ToolExecutionMode::Sequential)],
            &state,
            None,
            1,
        );
        let calls = [
            scheduled_call("a", "sequential", "slow-a"),
            scheduled_call("b", "sequential", "slow-b"),
        ];
        let mut permission = Permission::new();
        let mut ask = CountedAsk::allowing(Arc::clone(&state));

        let (_, went, _) = invoke_many(&tools, &mut permission, &mut ask, &calls, (8, usize::MAX));

        assert!(matches!(went, Went::On));
        assert_eq!(state.peak.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_refusal_in_a_parallel_wave_happens_before_any_effect() {
        let state = Arc::new(ScheduleState::default());
        let tools = scheduled_tools(
            &[("parallel", ToolExecutionMode::Parallel)],
            &state,
            None,
            0,
        );
        let calls = [
            scheduled_call("a", "parallel", "a"),
            scheduled_call("b", "parallel", "b"),
        ];
        let mut permission = Permission::new();
        let mut ask = CountedAsk {
            state: Arc::clone(&state),
            answers: vec![Verdict::Allow, Verdict::Deny],
            at: 0,
        };

        let (results, went, _) =
            invoke_many(&tools, &mut permission, &mut ask, &calls, (2, usize::MAX));

        assert_eq!(state.ran.load(Ordering::SeqCst), 0);
        assert_eq!(results.len(), 2);
        assert_eq!(texts(&results), [NOT_RUN, DENIED]);
        assert!(matches!(went, Went::Refused(ref name) if &**name == "parallel"));
    }

    #[test]
    fn result_budget_is_reserved_before_a_parallel_wave_is_admitted() {
        let state = Arc::new(ScheduleState::default());
        let tools = scheduled_tools(
            &[("parallel", ToolExecutionMode::Parallel)],
            &state,
            None,
            0,
        );
        let calls = [
            scheduled_call("a", "parallel", "a"),
            scheduled_call("b", "parallel", "b"),
        ];
        let mut permission = Permission::new();
        let mut ask = CountedAsk::allowing(Arc::clone(&state));

        let (results, went, _) = invoke_many(
            &tools,
            &mut permission,
            &mut ask,
            &calls,
            (2, NOT_RUN.len().saturating_mul(2).saturating_sub(1)),
        );

        assert_eq!(state.ran.load(Ordering::SeqCst), 0);
        assert_eq!(state.approvals.load(Ordering::SeqCst), 0);
        assert_eq!(results.len(), 2);
        assert!(matches!(went, Went::OutputLimit));
    }

    /// One pass, with everything it needed set up around it.
    struct Proof {
        tools: Tools,
        permission: Permission,
        says: Says,
        cancel: Cancel,
        events: Keeping,
        seen: Receiver<Event>,
    }

    impl Proof {
        fn new(verdict: Verdict) -> Self {
            Self::asking(Says::new(verdict))
        }

        fn asking(says: Says) -> Self {
            let (events, seen) = channel();

            Self {
                tools: Tools::new(),
                permission: Permission::new(),
                says,
                cancel: Cancel::new(),
                events: Keeping(events),
                seen,
            }
        }

        fn offering(mut self, tool: Fixed) -> Self {
            self.tools.add_builtin(tool).unwrap();
            self
        }

        fn pass(&mut self, calls: &[ToolCall]) -> (Vec<ToolResult>, Went) {
            let (results, went, _) = self.within(calls, 0, usize::MAX);
            (results, went)
        }

        fn within(
            &mut self,
            calls: &[ToolCall],
            held: usize,
            maximum: usize,
        ) -> (Vec<ToolResult>, Went, usize) {
            let events = Reporter::new(Ancestry::new(), &self.events);
            let tools = self.tools.snapshot().unwrap();
            let journal = crucible_session::Session::nowhere();

            Work {
                tools: &tools,
                permission: &mut self.permission,
                ask: &mut self.says,
                events,
                cancel: &self.cancel,
                ancestry: Ancestry::new(),
                journal: &journal,
                audits: &SandboxAuditRegistry::new(),
                concurrency: 1,
            }
            .pass(calls, held, maximum)
        }
    }

    fn call(id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: ToolId::new(id),
            name: name.into(),
            args: ToolArgs::new("{}"),
        }
    }

    fn texts(results: &[ToolResult]) -> Vec<&str> {
        results.iter().map(|result| result.output.text()).collect()
    }

    fn outcomes(proof: &Proof) -> Vec<ToolOutcome> {
        proof
            .seen
            .try_iter()
            .filter_map(|event| match event {
                Event::ToolFinished {
                    receipt: Some(receipt),
                    ..
                } => Some(receipt.outcome()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_call_that_runs_comes_back_with_what_the_tool_produced() {
        let mut proof =
            Proof::new(Verdict::Allow).offering(Fixed::new("read").answering("fn main() {}"));

        let (results, went) = proof.pass(&[call("a", "read")]);

        assert_eq!(texts(&results), ["fn main() {}"]);
        assert!(matches!(went, Went::On), "the turn should carry on");
    }

    #[test]
    fn the_tool_that_runs_is_the_one_the_verdict_was_reached_about() {
        // The name is dispatched on out of the approval, beside the arguments
        // and the proof. Two tools answering differently are how a pass can
        // say which of them ran.
        let mut proof = Proof::new(Verdict::Allow)
            .offering(Fixed::new("read").answering("what read produced"))
            .offering(Fixed::new("grep").answering("what grep produced"));

        let (results, went) = proof.pass(&[call("a", "grep")]);

        assert_eq!(texts(&results), ["what grep produced"]);
        assert!(matches!(went, Went::On));
    }

    #[test]
    fn a_name_no_tool_answers_to_is_reported_to_the_model_rather_than_ending_the_turn() {
        // The model invented it, so the model is the one that can fix it.
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read"));

        let (results, went) = proof.pass(&[call("a", "frobnicate")]);

        assert_eq!(texts(&results), ["no tool named frobnicate"]);
        assert!(results.first().is_some_and(|r| r.output.is_failed()));
        assert!(matches!(went, Went::On));
    }

    #[test]
    fn a_tool_that_fails_reports_it_to_the_model_rather_than_ending_the_turn() {
        let mut proof =
            Proof::new(Verdict::Allow).offering(Fixed::new("read").breaking("unreadable"));

        let (results, went) = proof.pass(&[call("a", "read")]);

        assert_eq!(texts(&results), ["read: unreadable"]);
        assert!(matches!(went, Went::On));
        assert_eq!(outcomes(&proof), [ToolOutcome::Failed]);
    }

    #[test]
    fn a_denied_call_ends_the_turn_and_says_so_in_its_result() {
        let mut proof = Proof::new(Verdict::Deny).offering(Fixed::new("write").risking(changing()));

        let (results, went) = proof.pass(&[call("a", "write")]);

        assert_eq!(texts(&results), [DENIED]);
        assert!(
            matches!(went, Went::Refused(ref name) if &**name == "write"),
            "the turn should name the tool that was refused"
        );
        assert_eq!(outcomes(&proof), [ToolOutcome::Refused]);
    }

    #[test]
    fn every_call_is_answered_even_after_the_turn_is_over() {
        // A call with no result is a transcript the provider refuses, so the
        // ones that never ran are answered too.
        let mut proof = Proof::new(Verdict::Deny).offering(Fixed::new("write").risking(changing()));

        let (results, _) = proof.pass(&[call("a", "write"), call("b", "write")]);

        assert_eq!(results.len(), 2);
        assert_eq!(texts(&results), [DENIED, NOT_RUN]);
    }

    #[test]
    fn a_call_allowed_for_the_session_is_not_put_to_the_user_again() {
        // One permission engine covers the pass, so the second call finds what
        // the first was allowed. A fresh engine per call would ask twice and
        // make `always` mean `once`.
        let mut proof = Proof::asking(Says::for_the_session())
            .offering(Fixed::new("write").risking(changing()));

        proof.pass(&[call("a", "write"), call("b", "write")]);

        assert_eq!(proof.says.asked, 1);
    }

    #[test]
    fn a_call_after_a_denial_is_never_put_to_the_user() {
        let mut proof = Proof::new(Verdict::Deny).offering(Fixed::new("write").risking(changing()));

        proof.pass(&[call("a", "write"), call("b", "write")]);

        assert_eq!(proof.says.asked, 1, "the user was asked about a dead turn");
    }

    #[test]
    fn a_cancelled_pass_stops_the_turn_without_running_anything_more() {
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read"));
        proof.cancel.request();

        let (results, went) = proof.pass(&[call("a", "read")]);

        assert_eq!(texts(&results), [NOT_RUN]);
        assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
        assert_eq!(outcomes(&proof), [ToolOutcome::NotRun]);
    }

    #[test]
    fn a_cancellation_is_not_relabelled_when_its_stand_in_crosses_the_boundary() {
        // The turn ended because the user stopped it. That the stand-in answer
        // did not fit the room left is a fact about the room, and reporting the
        // boundary instead would turn "the user stopped this" into a limit the
        // model would try to work around.
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read"));
        proof.cancel.request();

        let (results, went, _) = proof.within(&[call("a", "read")], 0, 0);

        assert_eq!(texts(&results), [""]);
        assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
    }

    #[test]
    fn a_tool_that_noticed_the_cancellation_itself_stops_the_turn() {
        // A long-running tool checks the flag mid-work and returns. That is not
        // a failure to report to the model — the user stopped the turn.
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("bash").cancelling());

        let (_, went) = proof.pass(&[call("a", "bash")]);

        assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
        assert_eq!(outcomes(&proof), [ToolOutcome::Cancelled]);
    }

    #[test]
    fn every_call_reports_that_it_finished() {
        // The renderer redraws the line it drew when the call was requested, so
        // a call with no finish stays on screen as if it were still running.
        let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read"));

        proof.pass(&[call("a", "read"), call("b", "read")]);

        let finished: Vec<(String, ToolOutcome)> = proof
            .seen
            .try_iter()
            .filter_map(|event| match event {
                Event::ToolFinished {
                    call,
                    receipt: Some(receipt),
                    ..
                } => Some((call.to_string(), receipt.outcome())),
                Event::TurnStarted { .. }
                | Event::PromptCache { .. }
                | Event::Sandbox { .. }
                | Event::Delta { .. }
                | Event::ToolRequested { .. }
                | Event::Wrote { .. }
                | Event::Carried { .. }
                | Event::Compacting { .. }
                | Event::Compacted { .. }
                | Event::Retrying
                | Event::Aged { .. }
                | Event::Unread { .. }
                | Event::Steered { .. }
                | Event::TurnFinished { .. }
                | Event::Spent { .. }
                | Event::Failed { .. }
                | Event::ToolFinished { receipt: None, .. } => None,
            })
            .collect();

        assert_eq!(
            finished,
            [
                ("a".to_owned(), ToolOutcome::Succeeded),
                ("b".to_owned(), ToolOutcome::Succeeded),
            ]
        );
    }

    #[test]
    fn an_output_limit_still_answers_every_recorded_call() {
        let oversized = "x".repeat(OUTPUT_LIMIT.len() + 1);
        let maximum = OUTPUT_LIMIT.len() + NOT_RUN.len();
        let mut proof =
            Proof::new(Verdict::Allow).offering(Fixed::new("read").answering(&oversized));

        let (results, went, produced) =
            proof.within(&[call("a", "read"), call("b", "read")], 0, maximum);

        assert_eq!(results.len(), 2);
        assert_eq!(texts(&results), [OUTPUT_LIMIT, ""]);
        assert!(matches!(went, Went::OutputLimit));
        assert!(produced <= maximum);
    }
}
