//! Running what the model asked for.
//!
//! One rule shapes this file: **every call the transcript records has a result
//! recorded with it.** A provider refuses a transcript containing a request
//! with no answer, so a turn that stops half way through a pass — because the
//! user cancelled, or said no — still writes a result for each remaining call
//! saying why there is nothing in it.
//!
//! Every call's run is spawned onto the runtime the turn is polled in, into a
//! bounded [`Group`], and awaited. The calls of a wave run beside one another,
//! at most [`TOOL_RUNS`] at once, and a call that runs alone is a wave of one.
//! Whatever order the runs answer in, the calls are answered in the order the
//! model asked for them. A run is never dropped part way through. A stop
//! raises the cancel its context holds; a call's deadline makes that cancel
//! read as raised from then on, which a run learns at its next look at it or
//! through a race on it, so a tool's deadline is cooperative. Either way the
//! call waits for the run's own answer — a run that ignores its cancel is
//! waited for until it answers — and is answered with what the run answered:
//! a run that did its work before it saw the stop is reported as having done
//! it, and one that heeded the stop as cut short. A stop still ends the pass
//! after the calls it reached, and no call of a wave is started once it is
//! raised.
//!
//! What a run prints while it runs comes back to the turn to be reported,
//! because only the turn holds where progress goes; [`Mailbox`] is how.

use std::collections::VecDeque;
use std::future::{Future, poll_fn};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::task::{Context, Poll, Waker};
use std::thread::{self, ThreadId};
use std::time::Instant;

use crucible_core::{
    Ancestry, Approved, Ask, CallResultStoreError, Cancel, InvocationId, InvocationRecord,
    JournalStore, PendingCallResult, Permission, RunItem, SandboxAudit, SandboxAuditRegistry,
    Settled, StopReason, TOOL_RESULT_BYTES, ToolCall, ToolContext, ToolEntry, ToolError,
    ToolExecutionMode, ToolId, ToolOutcome, ToolOutput, ToolOutputRetention, ToolReceipt,
    ToolResult, ToolSnapshot, ToolSourceReceipt, ToolWorker, Watch, Wrote,
};
use crucible_runtime::Group;

use crate::{Event, Reporter};
mod audit;

use audit::report_sandbox_audit;
pub(super) use audit::report_sandbox_registry;

/// How many of a turn's calls run at once, however wide a wave the run's
/// scheduler ceiling allows.
///
/// Each run is spawned onto the application's runtime, and a tool whose work
/// is still synchronous holds the worker it runs on for as long as its call
/// lasts. The runtime's workers are what its other owners run on too — among
/// them the status task of a confined process, which is where that process's
/// deadline and output-limit kills run — so this stays below the runtime's
/// worker count less one. However many calls are held, two workers stay free:
/// a status task's kill finds one even while something else is being polled
/// on the other, so a held worker never delays a kill. The application checks
/// this against its own worker count.
///
/// The bound is one turn's. The application takes one turn at a time on its
/// runtime, so it is the runtime's bound there too; two turns taken at once
/// on one runtime could hold twice as many workers.
pub const TOOL_RUNS: usize = 2;

/// How many pieces of what a batch's runs printed may wait for the turn to
/// report them.
///
/// A piece is whatever a tool hands over at once. A run that finds this many
/// waiting waits for the turn to take them, as it waited at the terminal's own
/// channel when it reported directly, so a run printing faster than the reader
/// can be shown is slowed rather than held in memory here.
const WRITTEN: usize = 16;

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
    /// The worker every call is lent for its blocking work, where the run
    /// was given one.
    pub(crate) worker: Option<&'a ToolWorker>,
    /// The widest wave of opt-in calls the run allows; at most [`TOOL_RUNS`]
    /// of them run at once.
    pub(crate) concurrency: usize,
}

impl Work<'_> {
    /// Runs `calls`, and answers every one of them in provider order.
    pub(crate) async fn pass(
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
                )
                .await;
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

                let decision = self.prepare(call).await;
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
                self.execute_wave(decisions).await
            };

            for (offset, invocation) in invocations.into_iter().enumerate() {
                if invocation.stops {
                    went = Went::Stopped(StopReason::Cancelled);
                }
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
                )
                .await;
            }
            at = end;
        }

        (results, went, produced)
    }

    /// Captures, validates, transforms, revalidates, classifies, guards, and
    /// approves one call without causing its tool effect.
    async fn prepare(&mut self, call: &ToolCall) -> Decision {
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
        let guarded = self
            .permission
            .decide_admitted_guarded(
                &admission,
                &sensitivity,
                |final_call, final_sensitivity| match entry.hooks().input() {
                    Some(guard) => guard.guard(final_call, final_sensitivity),
                    None => Ok(()),
                },
                self.ask,
            )
            .await;
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
                        .append_run_item(&RunItem::Invocation {
                            record: record.clone(),
                            preview: None,
                        })
                        .await;
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
    ///
    /// The approved calls run in batches of at most [`TOOL_RUNS`], each
    /// batch's runs spawned into a group of their own and every one of them
    /// awaited before the next batch begins. What comes back is in the wave's
    /// own order, whatever order the runs answered in.
    async fn execute_wave(&self, decisions: Vec<Decision>) -> Vec<Invocation> {
        let host = ExecutionHost {
            ancestry: self.ancestry,
            cancel: self.cancel,
            events: self.events,
            journal: self.journal,
            audits: self.audits,
            worker: self.worker,
        };
        let mut answered = Vec::with_capacity(decisions.len());
        let mut ready = Vec::new();
        for (index, decision) in decisions.into_iter().enumerate() {
            match decision {
                Decision::Ready(prepared) => ready.push((index, prepared)),
                Decision::Done(invocation)
                | Decision::Refused(invocation)
                | Decision::Stopped(invocation)
                | Decision::NotRun(invocation) => answered.push((index, invocation)),
            }
        }

        let mut ready = ready.into_iter();
        loop {
            // A stop raised while an earlier batch ran starts nothing more:
            // every call not yet started is answered as the stop cut it short.
            if self.cancel.requested() {
                answered.extend(ready.map(|(index, prepared)| (index, prepared.cut_short())));
                break;
            }
            let batch: Vec<(usize, Prepared)> = ready.by_ref().take(TOOL_RUNS).collect();
            if batch.is_empty() {
                break;
            }
            answered.extend(run_batch(batch, host).await);
        }
        // Committed by call index: the order the model asked in, never the
        // order the runs happened to answer in.
        answered.sort_by_key(|(index, _)| *index);
        answered
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
    async fn finish(
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
            let result = ToolResult {
                id: invocation.call.id.clone(),
                output: invocation.output.clone().into_recorded(),
            };
            if let Ok(receipt) = self.journal.put_call_result(pending.key(), &result).await {
                // The result is already durable and replayable. The
                // acceptance is awaited, so an executor that has to wait to
                // close its transition is waited for; one that fails
                // quarantines or stops its owned scope, but cannot replace
                // that sole accepted result.
                let _ = pending.accept(receipt).await;
            } else {
                // Dropping the unaccepted executor half hands its
                // application-owned process scope back to the registry that
                // owns its cleanup, as the acceptance contract says it does.
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
            // The preview travels beside the record rather than in it: what the
            // store keeps is the result the model was sent, and the change
            // lines are the reader's alone.
            let preview = invocation.output.diff().cloned();
            let _ = recovery.finish(
                invocation.outcome,
                invocation.output.clone().into_recorded(),
            );
            self.journal
                .append_run_item(&RunItem::Invocation {
                    record: recovery,
                    preview,
                })
                .await;
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

        results.push(ToolResult {
            id: invocation.call.id,
            output: invocation.output.into_recorded(),
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
    /// The call, answered as the stop cut it short before its run was
    /// started.
    fn cut_short(self) -> Invocation {
        Invocation::new(
            self.call,
            ToolOutput::failed(NOT_RUN),
            ToolOutcome::Cancelled,
            self.evidence,
        )
        .recovering(self.record)
    }

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
    /// Whether the pass ends on a stop at this call although its outcome says
    /// something else: its run answered after the turn was stopped, and is
    /// answered with what it answered rather than as cut short.
    stops: bool,
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
            stops: false,
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
    worker: Option<&'a ToolWorker>,
}

/// Runs one batch of approved calls, no more than [`TOOL_RUNS`] of them, each
/// spawned into one group, and answers every one once all their runs have
/// answered and the group holds none of them.
///
/// Each call is recorded as started and given its sandbox collector here, on
/// the turn, and is settled here once its run has answered; only the run
/// itself goes to the group. A run that comes apart, in whichever poll, is
/// answered as a contained panic once what its sandbox collected has been
/// reported, as one that runs alone always was.
///
/// The group is made on the turn's own cancel, so a stop reaches every run,
/// and nothing here drops a run that has begun: a stop raises the run's
/// cancel, a deadline makes it read as raised, and this waits for the run's
/// own answer. What bounds that wait is how soon the run heeds its cancel —
/// for work it handed a worker, the job's next look at its token — and a run
/// that never does is waited for until it answers. Settling a call, its
/// output hook included, is contained as its run is.
///
/// # Panics
///
/// Where the turn is polled outside a runtime, which is what spawning into a
/// [`Group`] does there. The application waits for a turn inside its own.
async fn run_batch(
    batch: Vec<(usize, Prepared)>,
    host: ExecutionHost<'_>,
) -> Vec<(usize, Invocation)> {
    let mut group = Group::new(TOOL_RUNS, host.cancel);
    let mailbox = Mailbox::new();
    let mut answered = Vec::with_capacity(batch.len());
    let mut running = Vec::with_capacity(batch.len());
    for (index, prepared) in batch {
        let fallback = PanicFallback::from(&prepared, host.cancel.clone());
        let audit = match host
            .audits
            .collector(host.ancestry, fallback.call.id.clone())
        {
            Ok(audit) => audit,
            Err(problem) => {
                answered.push((index, fallback.audit_failed(problem)));
                continue;
            }
        };
        let (started, approved) = Started::from(prepared, host).await;
        let slot = mailbox.expect();
        let run = Run {
            call: started.call.id.clone(),
            entry: started.entry.clone(),
            approved,
            ancestry: host.ancestry,
            parent: group.cancel().clone(),
            stop: host.cancel.clone(),
            deadline: started.deadline,
            invocation: started.record.id(),
            audit: audit.clone(),
            worker: host.worker.cloned(),
        };
        if let Err(full) = group.spawn(run.answering(mailbox.clone(), slot)) {
            let problem = ToolError::Io {
                tool: started.call.name.clone(),
                problem: "the turn could not start its run".into(),
                source: std::io::Error::other(full),
            };
            mailbox.answer(slot, Came::Returned(Returned::Unran(problem)));
        }
        running.push(Running {
            index,
            slot,
            fallback,
            audit,
            started,
        });
    }

    let mut came = mailbox.collected(&mut group, host.events).await;
    for Running {
        index,
        slot,
        fallback,
        audit,
        started,
    } in running
    {
        let invocation = match came.get_mut(slot).and_then(Option::take) {
            // Settling runs the call's own output hook and reports what its
            // sandbox collected, and either may come apart: that is answered
            // as a contained panic too, rather than unwinding out of the pass.
            Some(Came::Returned(returned)) => {
                match Contained(Box::pin(started.settled(returned, &audit, host))).await {
                    Some(invocation) => invocation,
                    None => fallback.panicked(),
                }
            }
            Some(Came::Apart) | None => fallback.contained(&audit, host).await,
        };
        answered.push((index, invocation));
    }
    answered
}

/// A call of a batch whose run is out, and what answering it needs.
struct Running {
    /// Where the call stands in its wave.
    index: usize,
    /// Where its run's answer arrives in the batch's [`Mailbox`].
    slot: usize,
    fallback: PanicFallback,
    audit: SandboxAudit,
    started: Started,
}

/// Everything one call's run owns once it is spawned: nothing it holds is
/// borrowed from the turn, so the task can be the group's.
struct Run {
    call: ToolId,
    entry: ToolEntry,
    approved: Approved,
    ancestry: Ancestry,
    /// The group's cancel, which the call's own is made a child of.
    parent: Cancel,
    /// The turn's own cancel, read once the run has answered.
    stop: Cancel,
    deadline: Option<Instant>,
    invocation: InvocationId,
    audit: SandboxAudit,
    worker: Option<ToolWorker>,
}

impl Run {
    /// The task a call's run is spawned as: the run, with a panic in any poll
    /// of it contained, and its answer handed back to the turn.
    async fn answering(self, mailbox: Mailbox, slot: usize) {
        let answering = Answering {
            mailbox: mailbox.clone(),
            slot,
            answered: false,
        };
        let came = match Contained(Box::pin(self.ran(mailbox))).await {
            Some(returned) => Came::Returned(returned),
            None => Came::Apart,
        };
        answering.answer(came);
    }

    /// Builds the call's context and awaits its run until the run answers.
    ///
    /// Nothing races it. A stop raises the context's cancel and the call's
    /// deadline passing makes it read as raised, and the run is still waited
    /// for until it answers: dropping it part way would leave whatever it had
    /// started unconfirmed, and any work it had handed a worker running with
    /// nobody told.
    async fn ran(self, mailbox: Mailbox) -> Returned {
        let Self {
            call,
            entry,
            approved,
            ancestry,
            parent,
            stop,
            deadline,
            invocation,
            audit,
            worker,
        } = self;
        let watched = Watched {
            mailbox,
            call: call.clone(),
        };
        let context = ToolContext::new(ancestry, call, &parent, deadline, &watched)
            .with_invocation(invocation);
        let context = match &worker {
            Some(worker) => context.with_worker(worker),
            None => context,
        };
        let context = match context.with_sandbox_audit(audit) {
            Ok(context) => context,
            Err(problem) => {
                return Returned::Unran(ToolError::Io {
                    tool: "sandbox audit".into(),
                    problem: "could not attach fixed lifecycle attribution".into(),
                    source: std::io::Error::other(problem),
                });
            }
        };
        let ran = entry.tool().run(approved, &context).await;
        Returned::Ran {
            ran,
            stopped: stop.requested(),
            timed_out: context.timed_out(),
            pending: context.take_call_result(),
        }
    }
}

/// What a call's run came to, as far as it can be known where it ran.
enum Returned {
    /// The run never began: its context could not be built, or the group
    /// would not take it.
    Unran(ToolError),
    /// The run answered.
    Ran {
        /// What it answered.
        ran: Result<ToolOutput, ToolError>,
        /// Whether the turn had been stopped by the time it did.
        stopped: bool,
        /// Whether the call's own deadline had passed by then.
        timed_out: bool,
        /// The executor half of a result it deferred, taken out of its
        /// context.
        pending: Result<Option<PendingCallResult>, CallResultStoreError>,
    },
}

/// What arrives in a run's slot of the [`Mailbox`].
enum Came {
    /// What the run came to.
    Returned(Returned),
    /// The run came apart, or its task was dropped before it answered.
    Apart,
}

/// Hands a run's answer back, and says it came apart if its task is dropped
/// first — unwinding, or taken away — so the turn waiting for it is never
/// left waiting for an answer that cannot come.
struct Answering {
    mailbox: Mailbox,
    slot: usize,
    answered: bool,
}

impl Answering {
    fn answer(mut self, came: Came) {
        self.answered = true;
        self.mailbox.answer(self.slot, came);
    }
}

impl Drop for Answering {
    fn drop(&mut self) {
        if !self.answered {
            self.mailbox.answer(self.slot, Came::Apart);
        }
    }
}

/// Where one batch's runs say what they printed and what they came to, for
/// the turn to report.
///
/// Only the turn holds where progress goes, and a run spawned into a group
/// can borrow nothing of the turn's, so what a run prints comes here and the
/// turn reports it — in the order it arrived, and before it answers the call
/// that printed it. At most [`WRITTEN`] pieces wait: a run that finds that
/// many waits for the turn to take them, and stops waiting once nobody is
/// left to take them. The one exception is a run polled on the very thread
/// that polls the turn, which a current-thread runtime does: the turn cannot
/// take anything until that run gives the thread back, so there a piece is
/// kept past the bound rather than waited on for ever. The application's
/// runtime polls runs on workers of their own, never on the turn's thread.
#[derive(Clone)]
struct Mailbox(Arc<Shared>);

struct Shared {
    held: Mutex<Held>,
    /// Signalled whenever the turn takes what was waiting, or leaves.
    room: Condvar,
}

struct Held {
    written: VecDeque<(ToolId, Wrote)>,
    /// One slot per run, filled once as it answers.
    answers: Vec<Option<Came>>,
    /// Wakes the turn waiting on this batch.
    waker: Option<Waker>,
    /// The thread polling the turn.
    turn: ThreadId,
    /// The turn stopped waiting, and nobody will take what arrives.
    closed: bool,
}

impl Mailbox {
    fn new() -> Self {
        Self(Arc::new(Shared {
            held: Mutex::new(Held {
                written: VecDeque::new(),
                answers: Vec::new(),
                waker: None,
                turn: thread::current().id(),
                closed: false,
            }),
            room: Condvar::new(),
        }))
    }

    fn held(&self) -> MutexGuard<'_, Held> {
        self.0.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A slot for one more run's answer.
    fn expect(&self) -> usize {
        let mut held = self.held();
        held.answers.push(None);
        held.answers.len() - 1
    }

    /// Fills `slot`, the first time only, and wakes the turn.
    fn answer(&self, slot: usize, came: Came) {
        let mut held = self.held();
        if let Some(empty @ None) = held.answers.get_mut(slot) {
            *empty = Some(came);
        }
        if let Some(waker) = &held.waker {
            waker.wake_by_ref();
        }
    }

    /// Leaves a piece a run printed for the turn to report.
    fn wrote(&self, call: &ToolId, text: Wrote) {
        let mut held = self.held();
        while held.written.len() >= WRITTEN && !held.closed && held.turn != thread::current().id() {
            held = self
                .0
                .room
                .wait(held)
                .unwrap_or_else(PoisonError::into_inner);
        }
        if held.closed {
            return;
        }
        held.written.push_back((call.clone(), text));
        if let Some(waker) = &held.waker {
            waker.wake_by_ref();
        }
    }

    /// Reports what the runs print as it arrives, until every run has
    /// answered and `group` holds none of them, and hands back their answers
    /// by slot.
    ///
    /// A run answers as the last thing its task does, and the task returns
    /// straight after; the few polls between the last answer and the group
    /// finding every task returned are spent asking again at once, so the
    /// batch never ends with a task of it still held.
    async fn collected<T: Send + 'static>(
        &self,
        group: &mut Group<T>,
        events: Reporter<'_>,
    ) -> Vec<Option<Came>> {
        let leaving = Leaving(self);
        poll_fn(|context| {
            let (written, answered) = {
                let mut held = self.held();
                held.turn = thread::current().id();
                match &mut held.waker {
                    Some(waker) => waker.clone_from(context.waker()),
                    None => held.waker = Some(context.waker().clone()),
                }
                let written = std::mem::take(&mut held.written);
                self.0.room.notify_all();
                (written, held.answers.iter().all(Option::is_some))
            };
            for (call, text) in written {
                events.post(Event::Wrote { call, text });
            }
            if !answered {
                return Poll::Pending;
            }
            if !group.is_empty() {
                context.waker().wake_by_ref();
                return Poll::Pending;
            }
            Poll::Ready(())
        })
        .await;
        drop(leaving);
        std::mem::take(&mut self.held().answers)
    }
}

/// Tells the runs of a batch that the turn has stopped waiting, however it
/// stops, so none of them waits for room nobody will make.
struct Leaving<'a>(&'a Mailbox);

impl Drop for Leaving<'_> {
    fn drop(&mut self) {
        self.0.held().closed = true;
        self.0.0.room.notify_all();
    }
}

/// A future whose panic, in whichever poll it comes, is caught and answered
/// as `None` rather than unwinding out of the task it runs in.
///
/// Polled with the waker of whoever polls it. It is awaited once, so nothing
/// asks it again once it has answered or come apart.
struct Contained<'a, T>(Pin<Box<dyn Future<Output = T> + Send + 'a>>);

impl<T> Future for Contained<'_, T> {
    type Output = Option<T>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        match catch_unwind(AssertUnwindSafe(|| self.0.as_mut().poll(cx))) {
            Ok(Poll::Ready(answer)) => Poll::Ready(Some(answer)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(_) => Poll::Ready(None),
        }
    }
}

/// A call whose run has been recorded as started, and what finishing it
/// needs.
struct Started {
    call: ToolCall,
    entry: ToolEntry,
    evidence: InvocationEvidence,
    record: InvocationRecord,
    deadline: Option<Instant>,
}

impl Started {
    /// Records the call as started, and hands back the approval to run it
    /// under.
    async fn from(prepared: Prepared, host: ExecutionHost<'_>) -> (Self, Approved) {
        let Prepared {
            call,
            entry,
            approved,
            evidence,
            mut record,
        } = prepared;
        let _ = record.start();
        host.journal
            .append_run_item(&RunItem::Invocation {
                record: record.clone(),
                preview: None,
            })
            .await;
        let deadline = entry
            .descriptor()
            .timeout()
            .and_then(|timeout| Instant::now().checked_add(timeout));
        (
            Self {
                call,
                entry,
                evidence,
                record,
                deadline,
            },
            approved,
        )
    }

    /// The call, answered with what its run came to, once what its sandbox
    /// collected has been reported, and marked to end the pass where the turn
    /// had been stopped by the time the run answered.
    ///
    /// The run's own answer decides what the call is answered with. A run
    /// that answered with an output did what it answered, whatever the turn
    /// was doing by then — a write already in place, a command that already
    /// exited — and is reported so; the stop still ends the pass through the
    /// mark. A run that answered it was cancelled is answered as the stop cut
    /// it short, and one that failed with its own failure; where the call's
    /// deadline had passed and the turn was not stopped, either is answered
    /// as timed out, since the deadline is what raised the run's cancel.
    async fn settled(
        self,
        returned: Returned,
        audit: &SandboxAudit,
        host: ExecutionHost<'_>,
    ) -> Invocation {
        let stopped = matches!(returned, Returned::Ran { stopped: true, .. });
        let mut invocation = self.answered(returned, audit, host).await;
        invocation.stops = stopped;
        invocation
    }

    /// [`Started::settled`], before the stop's mark.
    async fn answered(
        self,
        returned: Returned,
        audit: &SandboxAudit,
        host: ExecutionHost<'_>,
    ) -> Invocation {
        let Self {
            call,
            entry,
            evidence,
            record,
            ..
        } = self;
        let (ran, stopped, timed_out, pending) = match returned {
            Returned::Unran(problem) => {
                return Invocation::failed(call, &problem, ToolOutcome::Failed, evidence)
                    .recovering(record);
            }
            Returned::Ran {
                ran,
                stopped,
                timed_out,
                pending,
            } => (ran, stopped, timed_out, pending),
        };
        if let Err(problem) =
            report_sandbox_audit(audit, host.ancestry, &call.id, host.events, host.journal).await
        {
            return Invocation::failed(call, &problem, ToolOutcome::Failed, evidence)
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
            Err(_) if timed_out && !stopped => {
                return Invocation::new(
                    call,
                    ToolOutput::failed("tool timed out"),
                    ToolOutcome::TimedOut,
                    evidence,
                )
                .recovering(record);
            }
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
        let pending = match pending {
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
}

struct PanicFallback {
    call: ToolCall,
    evidence: InvocationEvidence,
    record: InvocationRecord,
    stop: Cancel,
}

impl PanicFallback {
    fn from(prepared: &Prepared, stop: Cancel) -> Self {
        Self {
            call: prepared.call.clone(),
            evidence: prepared.evidence.clone(),
            record: prepared.record.clone(),
            stop,
        }
    }
}

impl PanicFallback {
    /// The call, answered as a contained panic once whatever its sandbox
    /// audit collected has been reported.
    async fn contained(self, audit: &SandboxAudit, host: ExecutionHost<'_>) -> Invocation {
        let _ = report_sandbox_audit(
            audit,
            host.ancestry,
            &self.call.id,
            host.events,
            host.journal,
        )
        .await;
        self.panicked()
    }

    fn panicked(self) -> Invocation {
        let mut invocation = Invocation::new(
            self.call,
            ToolOutput::failed("tool panicked; the failure was contained"),
            ToolOutcome::Panicked,
            self.evidence,
        )
        .recovering(self.record);
        invocation.stops = self.stop.requested();
        invocation
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
/// not which call it is, and the turn, which needs both. It is made per call
/// rather than per pass so that the identifier cannot be the wrong one: there is
/// no moment at which this value exists beside a different call.
///
/// A piece of output goes to the batch's [`Mailbox`], and the turn turns it
/// into an event and posts it; what the drawing thread does with it is the
/// drawing thread's business. The mailbox holds only a bounded few, which is
/// what keeps a command printing a gigabyte from growing anything here.
struct Watched {
    /// Where it goes.
    mailbox: Mailbox,
    /// The call whose output this is.
    call: ToolId,
}

impl Watch for Watched {
    fn wrote(&self, text: Wrote) {
        self.mailbox.wrote(&self.call, text);
    }
}

/// A failure the model is meant to read and work around.
fn failure(problem: &ToolError) -> ToolOutput {
    ToolOutput::failed(problem.to_string())
}

#[cfg(test)]
mod tests;
