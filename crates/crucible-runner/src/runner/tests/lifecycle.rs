//! Live toolset acquisition and cleanup around a turn.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crucible_core::{
    AgentId, Ancestry, Approved, Aside, Cancel, DescribeTool, Sensitivity, Steer, StopReason,
    Summary, Target, Tool, ToolArgs, ToolContext, ToolDescriptor, ToolEntry, ToolError, ToolOutput,
    ToolProvenance, ToolSnapshot, ToolSourceKind, Toolset, ToolsetContext, ToolsetError, Verdict,
};
use crucible_runtime::{BoxFuture, Bridge};

use super::*;

use crate::TurnError;
#[derive(Clone)]
struct Live {
    calls: Arc<Mutex<Vec<&'static str>>>,
    snapshot: ToolSnapshot,
    disposed: Arc<AtomicBool>,
    prepare_fails: bool,
    dispose_fails: bool,
    /// The step that never answers, so a turn that cannot wait drops it.
    waits: Option<Waits>,
}

/// The one step of a [`Live`] toolset that never answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Waits {
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
            waits: None,
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

    fn waiting(mut self, at: Waits) -> Self {
        self.waits = Some(at);
        self
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
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            self.disposed.store(false, Ordering::Release);
            self.saw("prepare");
            if self.waits == Some(Waits::Preparing) {
                std::future::pending::<()>().await;
            }
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
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            if !self.disposed.swap(true, Ordering::AcqRel) {
                self.saw("dispose");
            }
            if self.waits == Some(Waits::Disposing) {
                std::future::pending::<()>().await;
            }
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
    runner
        .turn("go", Box::new([]), &mut says, &context)
        .map(ran)
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

#[test]
fn a_preparation_that_never_answers_ends_the_turn_refused_and_is_still_disposed() {
    let live = Live::new().waiting(Waits::Preparing);

    let problem = run(live.clone(), Script::new(Vec::new())).unwrap_err();

    assert!(
        matches!(
            &problem,
            TurnError::Unready(unready) if unready.bridge() == Bridge::TurnTools
        ),
        "{problem:?}"
    );
    assert_eq!(live.calls(), ["prepare", "dispose"]);
}

#[test]
fn a_disposal_that_never_answers_does_not_hide_the_failure_that_required_it() {
    let live = Live::new().waiting(Waits::Disposing);

    let problem = run(live.clone(), Script::failing()).unwrap_err();
    let shown = problem.to_string();

    match problem {
        TurnError::ToolsetCleanupUnready { primary, cleanup } => {
            assert!(matches!(*primary, TurnError::Provider(_)), "{primary:?}");
            assert_eq!(cleanup.bridge(), Bridge::TurnTools);
            // The disposal was dropped before it answered: the reader is told
            // that it is unconfirmed, not that it failed, and not whether it
            // happened.
            assert_eq!(
                shown,
                "script: HTTP 401: no; then disposing of the toolset: the turn's tools would \
                 have had to wait, and the caller cannot; the waiting step was dropped before it \
                 answered, so whatever that step began is unconfirmed"
            );
        }
        other => panic!("the failure and the refusal together, not {other:?}"),
    }
    assert_eq!(live.calls(), ["prepare", "snapshot", "dispose"]);
}
