use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Barrier, Mutex};
use std::time::Duration;

use crucible_core::{
    Ancestry, ArgumentTransform, CallResultAcceptance, CallResultKey, CallResultReceipt,
    CallResultStoreError, Disposition, EventEnvelope, IdempotencyKey, InputGuard, InvocationState,
    JournalStore, Mode, OutputGuard, Post, RecoveryAction, Remember, Rules, SandboxCleanup,
    SandboxFactKind, SandboxId, SandboxLifecycle, Sensitivity, Summary, Target, Tool, ToolArgs,
    ToolDescriptor, ToolEffect, ToolExecutionMode, ToolHooks, ToolId, ToolProvenance,
    ToolResourceKey, ToolSourceKind, Verdict,
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

#[derive(Default)]
struct ResultJournal {
    items: Mutex<Vec<RunItem>>,
    results: Mutex<Vec<(CallResultKey, ToolResult)>>,
}

impl JournalStore for ResultJournal {
    fn append_run_item(&self, item: &RunItem) {
        self.items.lock().unwrap().push(item.clone());
    }

    fn put_call_result(
        &self,
        key: CallResultKey,
        result: &ToolResult,
    ) -> Result<CallResultReceipt, CallResultStoreError> {
        self.results.lock().unwrap().push((key, result.clone()));
        Ok(CallResultReceipt::from_digest([0x44; 32]))
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

    fn run(&self, approved: Approved, _context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
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

struct ReplaceOutput;

impl OutputGuard for ReplaceOutput {
    fn guard(&self, _call: &ToolCall, _output: ToolOutput) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok("guarded final output"))
    }
}

struct AcceptedResult(Arc<Mutex<Option<CallResultReceipt>>>);

impl Drop for AcceptedResult {
    fn drop(&mut self) {
        let Ok(mut accepted) = self.0.lock() else {
            return;
        };
        if accepted.is_none() {
            *accepted = Some(CallResultReceipt::from_digest([0xdd; 32]));
        }
    }
}

impl CallResultAcceptance for AcceptedResult {
    fn accept(
        self: Box<Self>,
        receipt: CallResultReceipt,
    ) -> Result<(), crucible_core::SandboxError> {
        *self.0.lock().unwrap() = Some(receipt);
        Ok(())
    }
}

struct DeferredResultTool(Arc<Mutex<Option<CallResultReceipt>>>);

impl Tool for DeferredResultTool {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("deferred result")
    }

    fn run(&self, _approved: Approved, context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        context
            .defer_call_result(Box::new(AcceptedResult(Arc::clone(&self.0))))
            .map_err(|problem| ToolError::Io {
                tool: "deferred".into(),
                problem: "could not defer the final result".into(),
                source: std::io::Error::other(problem),
            })?;
        Ok(ToolOutput::ok("raw executor output"))
    }
}

struct LongOutput;

impl OutputGuard for LongOutput {
    fn guard(&self, _call: &ToolCall, _output: ToolOutput) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok("x".repeat(100)))
    }
}

#[derive(Default)]
struct FailingResultJournal;

impl JournalStore for FailingResultJournal {
    fn append_run_item(&self, _item: &RunItem) {}

    fn put_call_result(
        &self,
        _key: CallResultKey,
        _result: &ToolResult,
    ) -> Result<CallResultReceipt, CallResultStoreError> {
        Err(CallResultStoreError::Storage)
    }
}

fn deferred_tools(
    accepted: &Arc<Mutex<Option<CallResultReceipt>>>,
    guard: Arc<dyn OutputGuard>,
) -> Tools {
    let descriptor = ToolDescriptor::new(
        "deferred",
        "{}",
        ToolProvenance::new(ToolSourceKind::User, "test:deferred", "deferred test").unwrap(),
    )
    .unwrap();
    let mut tools = Tools::new();
    tools
        .add_with_hooks(
            descriptor,
            Arc::new(DeferredResultTool(Arc::clone(accepted))),
            ToolHooks::new().guarding_output(guard),
        )
        .unwrap();
    tools
}

#[test]
fn deferred_results_commit_the_exact_guarded_runner_output() {
    let accepted = Arc::new(Mutex::new(None));
    let tools = deferred_tools(&accepted, Arc::new(ReplaceOutput));
    let snapshot = tools.snapshot().unwrap();
    let journal = ResultJournal::default();
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
    .pass(&[call("deferred-call", "deferred")], 0, usize::MAX);

    assert!(matches!(went, Went::On));
    let result = results.first().expect("one result");
    assert_eq!(result.output.text(), "guarded final output");
    let stored = journal.results.lock().unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored.first().map(|(_, stored)| stored), Some(result));
    assert_eq!(
        *accepted.lock().unwrap(),
        Some(CallResultReceipt::from_digest([0x44; 32]))
    );
}

#[test]
fn a_turn_output_refusal_reclaims_the_unaccepted_background_scope() {
    let accepted = Arc::new(Mutex::new(None));
    let tools = deferred_tools(&accepted, Arc::new(LongOutput));
    let snapshot = tools.snapshot().unwrap();
    let journal = ResultJournal::default();
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
    .pass(&[call("deferred-call", "deferred")], 0, 40);

    assert!(matches!(went, Went::OutputLimit));
    assert!(results.first().expect("one result").output.is_failed());
    assert!(journal.results.lock().unwrap().is_empty());
    assert_eq!(
        *accepted.lock().unwrap(),
        Some(CallResultReceipt::from_digest([0xdd; 32]))
    );
}

#[test]
fn a_failed_durable_result_write_reclaims_the_background_scope() {
    let accepted = Arc::new(Mutex::new(None));
    let tools = deferred_tools(&accepted, Arc::new(ReplaceOutput));
    let snapshot = tools.snapshot().unwrap();
    let journal = FailingResultJournal;
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
    .pass(&[call("deferred-call", "deferred")], 0, usize::MAX);

    assert!(matches!(went, Went::On));
    let result = results.first().expect("one result");
    assert!(result.output.is_failed());
    assert_eq!(result.output.text(), RESULT_STORAGE_FAILED);
    assert_eq!(
        *accepted.lock().unwrap(),
        Some(CallResultReceipt::from_digest([0xdd; 32]))
    );
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

    fn run(&self, _approved: Approved, context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
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

    fn run(&self, approved: Approved, _context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
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
    let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read").breaking("unreadable"));

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
    let mut proof =
        Proof::asking(Says::for_the_session()).offering(Fixed::new("write").risking(changing()));

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
    let mut proof = Proof::new(Verdict::Allow).offering(Fixed::new("read").answering(&oversized));

    let (results, went, produced) =
        proof.within(&[call("a", "read"), call("b", "read")], 0, maximum);

    assert_eq!(results.len(), 2);
    assert_eq!(texts(&results), [OUTPUT_LIMIT, ""]);
    assert!(matches!(went, Went::OutputLimit));
    assert!(produced <= maximum);
}
