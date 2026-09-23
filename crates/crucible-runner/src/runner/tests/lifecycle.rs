//! Live toolset acquisition and cleanup around a turn.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crucible_core::{
    AgentId, Ancestry, Approved, Aside, Cancel, DescribeTool, Sensitivity, Steer, StopReason,
    Summary, Target, Tool, ToolArgs, ToolContext, ToolDescriptor, ToolEntry, ToolError, ToolOutput,
    ToolProvenance, ToolSnapshot, ToolSourceKind, Toolset, ToolsetContext, ToolsetError, Verdict,
};
use crucible_runtime::BoxFuture;

use super::*;

use crate::TurnError;
#[derive(Clone)]
struct Live {
    calls: Arc<Mutex<Vec<&'static str>>>,
    snapshot: ToolSnapshot,
    disposed: Arc<AtomicBool>,
    prepare_fails: bool,
    dispose_fails: bool,
    /// Whether preparing and disposing each wait once before they answer.
    unhurried: bool,
    /// The step that waits until the turn is stopped, stopping it itself,
    /// and then answers.
    stops: Option<Step>,
}

/// A step of a [`Live`] toolset's lifecycle the turn awaits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Preparing,
    Disposing,
}

impl Live {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            snapshot: ToolSnapshot::empty(),
            disposed: Arc::new(AtomicBool::new(true)),
            prepare_fails: false,
            dispose_fails: false,
            unhurried: false,
            stops: None,
        }
    }

    fn offering(tool: Fixed) -> Self {
        let provenance = ToolProvenance::new(
            ToolSourceKind::Other,
            "test:live-toolset",
            "live toolset test fixture",
        )
        .unwrap();
        let descriptor = tool.descriptor(provenance).unwrap();
        Self {
            snapshot: ToolSnapshot::new([ToolEntry::new(descriptor, Arc::new(tool))]).unwrap(),
            ..Self::new()
        }
    }

    fn failing_prepare(mut self) -> Self {
        self.prepare_fails = true;
        self
    }

    fn failing_dispose(mut self) -> Self {
        self.dispose_fails = true;
        self
    }

    fn unhurried(mut self) -> Self {
        self.unhurried = true;
        self
    }

    fn stopping_at(mut self, step: Step) -> Self {
        self.stops = Some(step);
        self
    }

    /// Waits once where this toolset is unhurried, and not at all otherwise.
    async fn pause(&self) {
        if self.unhurried {
            super::waiting::Later::new(async {}).await;
        }
    }

    /// Where `step` is the one this toolset stops at, waits until the turn
    /// is stopped, stopping it, and notes that the step then answered.
    async fn stopping(&self, step: Step, context: &ToolsetContext) {
        if self.stops == Some(step) {
            super::waiting::UntilStopped {
                cancel: context.cancel().clone(),
                answer: Some(()),
            }
            .await;
            self.saw("answered once stopped");
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn saw(&self, call: &'static str) {
        self.calls.lock().unwrap().push(call);
    }
}

impl Toolset for Live {
    fn prepare<'a>(
        &'a self,
        context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            self.disposed.store(false, Ordering::Release);
            self.saw("prepare");
            self.pause().await;
            self.stopping(Step::Preparing, context).await;
            if self.prepare_fails {
                Err(ToolsetError::Entries {
                    maximum: 0,
                    actual: 1,
                })
            } else {
                Ok(())
            }
        })
    }

    fn snapshot<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(async move {
            self.saw("snapshot");
            Ok(self.snapshot.clone())
        })
    }

    fn refresh<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(async move {
            self.saw("refresh");
            Ok(self.snapshot.clone())
        })
    }

    fn dispose<'a>(
        &'a self,
        context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            if !self.disposed.swap(true, Ordering::AcqRel) {
                self.saw("dispose");
            }
            self.pause().await;
            self.stopping(Step::Disposing, context).await;
            if self.dispose_fails {
                Err(ToolsetError::Bytes {
                    maximum: 0,
                    actual: 1,
                })
            } else {
                Ok(())
            }
        })
    }
}

fn run<T>(live: T, script: Script) -> Result<StopReason, TurnError>
where
    T: Toolset + 'static,
{
    running(live, script).0
}

/// [`run`], handing back the transcript the turn left beside how it ended.
fn running<T>(live: T, script: Script) -> (Result<StopReason, TurnError>, Transcript)
where
    T: Toolset + 'static,
{
    let (events, _seen) = channel();
    let events = Watching(events);
    let cancel = Cancel::new();
    let steer = Steer::new();
    let aside = Aside::new();
    let mut says = Says::new(Verdict::Allow);
    let mut runner = Runner::with_toolset(
        Box::new(script),
        live,
        Agent::new(
            AgentId::new("test"),
            Model {
                name: "test".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                effort: None,
            },
        ),
        ContextInputs::new(std::env::temp_dir())
            .dated(std::time::UNIX_EPOCH + std::time::Duration::from_hours(496_704)),
        Recording::nowhere(),
    );
    let context = runner.starting(&events, &cancel, &steer, &aside);
    let turned = runner
        .turn("go", Box::new([]), &mut says, &context)
        .awaited()
        .map(ran);
    (turned, runner.transcript().clone())
}

struct Marks {
    version: &'static str,
    ran: Arc<Mutex<Vec<&'static str>>>,
}

impl Tool for Marks {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new(self.version)
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.ran.lock().unwrap().push(self.version);
            Ok(ToolOutput::ok(self.version))
        })
    }
}

#[derive(Clone)]
struct Changing {
    calls: Arc<Mutex<Vec<&'static str>>>,
    old: ToolSnapshot,
    new: ToolSnapshot,
    ran: Arc<Mutex<Vec<&'static str>>>,
}

impl Changing {
    fn new() -> Self {
        let ran = Arc::new(Mutex::new(Vec::new()));
        let snapshot = |version: &'static str| {
            let descriptor = ToolDescriptor::new(
                "version",
                "{}",
                ToolProvenance::new(
                    ToolSourceKind::Other,
                    format!("test:{version}"),
                    format!("{version} generation"),
                )
                .unwrap(),
            )
            .unwrap();
            ToolSnapshot::new([ToolEntry::new(
                descriptor,
                Arc::new(Marks {
                    version,
                    ran: Arc::clone(&ran),
                }),
            )])
            .unwrap()
        };
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            old: snapshot("old"),
            new: snapshot("new"),
            ran,
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn ran(&self) -> Vec<&'static str> {
        self.ran.lock().unwrap().clone()
    }
}

impl Toolset for Changing {
    fn prepare<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("prepare");
            Ok(())
        })
    }

    fn snapshot<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("snapshot");
            Ok(self.old.clone())
        })
    }

    fn refresh<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("refresh");
            Ok(self.new.clone())
        })
    }

    fn dispose<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push("dispose");
            Ok(())
        })
    }
}

#[test]
fn refresh_changes_only_later_admissions_and_each_generation_runs_once() {
    let changing = Changing::new();

    let stopped = run(
        changing.clone(),
        Script::new(vec![
            calling("one", "version", "{}"),
            calling("two", "version", "{}"),
            saying("done"),
        ]),
    )
    .expect("the turn");

    assert_eq!(stopped, StopReason::Yielded);
    assert_eq!(changing.ran(), ["old", "new"]);
    assert_eq!(
        changing.calls(),
        ["prepare", "snapshot", "refresh", "refresh", "dispose"]
    );
}

#[test]
fn a_toolset_that_waits_to_prepare_and_to_dispose_is_awaited() {
    let live = Live::new().unhurried();

    let stopped = run(live.clone(), Script::new(vec![saying("done")]));

    assert_eq!(stopped.unwrap(), StopReason::Yielded);
    assert_eq!(live.calls(), ["prepare", "snapshot", "dispose"]);
}

#[test]
fn a_preparation_still_waiting_when_the_turn_is_stopped_ends_the_turn_stopped() {
    // The preparation is awaited across the stop, answers, and the turn ends
    // the way a stopped turn ends: the model's calls are never run and never
    // recorded without the results a provider would require beside them.
    //
    // The model is still asked once after the stop, and its answer is what
    // the transcript records. That is the turn's own ordering and not the
    // waiting's: nothing between a successful preparation and the request
    // looks at the stop, and the synchronous turn before this one asked the
    // model the same way after a preparation that raised the stop and
    // answered at once. It is pinned here as it stands, so a change to it is
    // a decision that shows.
    let live = Live::offering(Fixed::new("read")).stopping_at(Step::Preparing);
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let sent = script.sent();

    let (turned, transcript) = running(live.clone(), script);

    assert_eq!(turned.unwrap(), StopReason::Cancelled);
    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "the model was not asked exactly once after the stop"
    );
    assert_eq!(
        live.calls(),
        ["prepare", "answered once stopped", "snapshot", "dispose"]
    );
    assert_eq!(
        super::unanswered::shape(&conversation(&transcript)),
        ["user go", "answer"],
        "a call was recorded, or recorded without its result, or the answer asked for after \
         the stop was not recorded"
    );
}

#[test]
fn a_disposal_still_waiting_when_the_turn_is_stopped_answers_and_the_ending_stands() {
    // The turn had ended by the time it disposed of its toolset. The disposal
    // is awaited across the stop to its answer, and the ending the turn had
    // reached is the one it hands back.
    let live = Live::new().stopping_at(Step::Disposing);

    let (turned, transcript) = running(live.clone(), Script::new(vec![saying("done")]));

    assert_eq!(turned.unwrap(), StopReason::Yielded);
    assert_eq!(
        live.calls(),
        ["prepare", "snapshot", "dispose", "answered once stopped"]
    );
    assert_eq!(
        super::unanswered::shape(&conversation(&transcript)),
        ["user go", "answer"]
    );
}

#[test]
fn a_turn_prepares_snapshots_and_disposes_its_live_toolset() {
    let live = Live::new();
    run(live.clone(), Script::new(vec![saying("done")])).expect("the turn");

    assert_eq!(live.calls(), ["prepare", "snapshot", "dispose"]);
}

#[test]
fn setup_failure_is_disposed_before_it_is_returned() {
    let live = Live::new().failing_prepare();

    let problem = run(live.clone(), Script::new(Vec::new())).unwrap_err();

    assert!(matches!(problem, TurnError::Toolset(_)));
    assert_eq!(live.calls(), ["prepare", "dispose"]);
}

#[test]
fn provider_failure_disposes_the_prepared_toolset() {
    let live = Live::new();

    let problem = run(live.clone(), Script::failing()).unwrap_err();

    assert!(matches!(problem, TurnError::Provider(_)));
    assert_eq!(live.calls(), ["prepare", "snapshot", "dispose"]);
}

#[test]
fn execution_failure_disposes_the_prepared_toolset() {
    let live = Live::offering(Fixed::new("break").breaking("broken"));

    let stopped = run(
        live.clone(),
        Script::new(vec![calling("one", "break", "{}"), saying("done")]),
    )
    .expect("an executor failure is a model-readable result");

    assert_eq!(stopped, StopReason::Yielded);
    assert_eq!(live.calls(), ["prepare", "snapshot", "refresh", "dispose"]);
}

#[test]
fn cancellation_from_a_running_tool_disposes_the_prepared_toolset() {
    let live = Live::offering(Fixed::new("stop").cancelling());

    let stopped = run(
        live.clone(),
        Script::new(vec![calling("one", "stop", "{}")]),
    )
    .expect("cancellation is an expected stop");

    assert_eq!(stopped, StopReason::Cancelled);
    assert_eq!(live.calls(), ["prepare", "snapshot", "dispose"]);
}

#[test]
fn repeated_disposal_reuses_the_first_cleanup_outcome_without_repeating_effects() {
    let live = Live::new().failing_dispose();
    let context = ToolsetContext::new(Ancestry::new(), Cancel::new(), None);
    crucible_runtime::answered!(live.prepare(&context)).unwrap();

    assert!(crucible_runtime::answered!(live.dispose(&context)).is_err());
    assert!(crucible_runtime::answered!(live.dispose(&context)).is_err());

    assert_eq!(live.calls(), ["prepare", "dispose"]);
}

#[test]
fn cleanup_failure_does_not_hide_the_failure_that_required_cleanup() {
    let live = Live::new().failing_prepare().failing_dispose();

    let problem = run(live.clone(), Script::new(Vec::new())).unwrap_err();

    assert!(matches!(problem, TurnError::ToolsetCleanup { .. }));
    assert_eq!(live.calls(), ["prepare", "dispose"]);
}
