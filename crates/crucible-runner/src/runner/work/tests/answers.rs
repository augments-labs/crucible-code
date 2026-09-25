//! What a call is answered with once its run has answered: the run's own
//! answer, whatever the turn was doing by then, and a contained panic
//! wherever settling it comes apart.

use super::*;

/// One pass over `calls` under `cancel`, on a multi-thread runtime of the
/// test's own, as [`invoke_many`] runs one.
fn stopped_pass(
    tools: &Tools,
    cancel: &Cancel,
    calls: &[ToolCall],
    concurrency: usize,
) -> (Vec<ToolResult>, Went, Vec<Event>) {
    let (events, seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let snapshot = tools.snapshot().unwrap();
    let journal = Recording::nowhere();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(TOOL_RUNS)
        .enable_time()
        .build()
        .unwrap();
    let (results, went, _) = runtime.block_on(
        Work {
            tools: &snapshot,
            permission: &mut Permission::new(),
            ask: &mut Says::new(Verdict::Allow),
            events: Reporter::new(ancestry, &keeping),
            cancel,
            ancestry,
            journal: &*journal,
            audits: &SandboxAuditRegistry::new(),
            worker: None,
            concurrency,
        }
        .pass(calls, 0, usize::MAX),
    );
    drop(keeping);
    (results, went, seen.try_iter().collect())
}

fn finished(events: &[Event]) -> Vec<ToolOutcome> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::ToolFinished {
                receipt: Some(receipt),
                ..
            } => Some(receipt.outcome()),
            _ => None,
        })
        .collect()
}

/// Raises the turn's stop, then commits its effect and says it did, never
/// looking at its cancel: a command that had already exited, a write already
/// renamed into place.
struct Commits {
    stop: Cancel,
    committed: Arc<AtomicUsize>,
}

impl Tool for Commits {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("commits")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.stop.request();
            self.committed.fetch_add(1, Ordering::SeqCst);
            Ok(ToolOutput::ok("committed"))
        })
    }
}

fn descriptor(name: &str) -> ToolDescriptor {
    ToolDescriptor::new(
        name,
        "{}",
        ToolProvenance::new(
            ToolSourceKind::User,
            format!("test:{name}"),
            format!("{name} test"),
        )
        .unwrap(),
    )
    .unwrap()
}

#[test]
fn a_run_that_committed_after_the_stop_is_answered_as_done_and_the_pass_still_stops() {
    let cancel = Cancel::new();
    let committed = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools
        .add(
            descriptor("commits"),
            Arc::new(Commits {
                stop: cancel.clone(),
                committed: Arc::clone(&committed),
            }),
        )
        .unwrap();

    let (results, went, events) = stopped_pass(
        &tools,
        &cancel,
        &[call("commit", "commits"), call("after", "commits")],
        1,
    );

    assert_eq!(committed.load(Ordering::SeqCst), 1);
    assert_eq!(
        texts(&results),
        ["committed", NOT_RUN],
        "an effect the run committed was reported as not done"
    );
    assert_eq!(
        finished(&events),
        [ToolOutcome::Succeeded, ToolOutcome::NotRun]
    );
    assert!(
        matches!(went, Went::Stopped(StopReason::Cancelled)),
        "the stop no longer ended the pass"
    );
}

#[test]
fn a_worker_job_that_committed_after_the_stop_is_answered_as_done() {
    // The job sees its token raised, finishes the step it is in, and ends;
    // the run answers what the job answered, and so does the call.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let cancel = Cancel::new();
    let started = Arc::new(AtomicUsize::new(0));
    let ended = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools
        .add(
            ToolDescriptor::new(
                "effecting",
                "{}",
                ToolProvenance::new(ToolSourceKind::User, "test:effecting", "effecting test")
                    .unwrap(),
            )
            .unwrap(),
            Arc::new(super::waves::Effecting {
                worker: crucible_core::ToolWorker::new(runtime.handle().clone()),
                stops: Some(cancel.clone()),
                started: Arc::clone(&started),
                ended: Arc::clone(&ended),
            }),
        )
        .unwrap();

    let (results, went, events) = stopped_pass(&tools, &cancel, &[call("effect", "effecting")], 1);

    assert_eq!(ended.load(Ordering::SeqCst), 1);
    assert_eq!(texts(&results), ["ended after its last look"]);
    assert_eq!(finished(&events), [ToolOutcome::Succeeded]);
    assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
}

/// Raises the turn's stop, waits until its own cancel says so, and answers
/// that it was cancelled: a run that heeded the stop.
struct Heeds {
    stop: Cancel,
}

impl Tool for Heeds {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("heeds")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.stop.request();
            let _ = context.cancel().race(std::future::pending::<()>()).await;
            Err(ToolError::Cancelled("heeds".into()))
        })
    }
}

#[test]
fn a_run_that_heeded_the_stop_is_still_answered_as_cancelled() {
    let cancel = Cancel::new();
    let mut tools = Tools::new();
    tools
        .add(
            descriptor("heeds"),
            Arc::new(Heeds {
                stop: cancel.clone(),
            }),
        )
        .unwrap();

    let (results, went, events) = stopped_pass(&tools, &cancel, &[call("heed", "heeds")], 1);

    assert_eq!(texts(&results), [NOT_RUN]);
    assert_eq!(finished(&events), [ToolOutcome::Cancelled]);
    assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
}

/// Counts every run that starts, and raises the turn's stop from the first.
struct CountsStarts {
    stop: Cancel,
    started: Arc<AtomicUsize>,
}

impl Tool for CountsStarts {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("counts")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.started.fetch_add(1, Ordering::SeqCst);
            self.stop.request();
            Ok(ToolOutput::ok("ran"))
        })
    }
}

#[test]
fn a_wave_s_later_batch_is_not_started_once_the_turn_is_stopped() {
    // A wave wider than the runs held at once goes out in batches. A stop
    // raised during one batch is looked at before the next, whose calls are
    // answered as cut short without being started.
    let cancel = Cancel::new();
    let started = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools
        .add(
            descriptor("counts").executing(ToolExecutionMode::Parallel),
            Arc::new(CountsStarts {
                stop: cancel.clone(),
                started: Arc::clone(&started),
            }),
        )
        .unwrap();
    let calls: Vec<ToolCall> = (0..2 * TOOL_RUNS)
        .map(|at| call(&format!("count-{at}"), "counts"))
        .collect();

    let (results, went, events) = stopped_pass(&tools, &cancel, &calls, calls.len());

    assert_eq!(
        started.load(Ordering::SeqCst),
        TOOL_RUNS,
        "a batch was started after the stop"
    );
    assert_eq!(results.len(), calls.len());
    let answered = texts(&results);
    assert!(answered.iter().take(TOOL_RUNS).all(|text| *text == "ran"));
    assert!(answered.iter().skip(TOOL_RUNS).all(|text| *text == NOT_RUN));
    assert!(
        finished(&events)
            .iter()
            .skip(TOOL_RUNS)
            .all(|outcome| *outcome == ToolOutcome::Cancelled)
    );
    assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
}

/// Answers at once, before any hook sees it.
struct Plain;

impl Tool for Plain {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("plain")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async { Ok(ToolOutput::ok("plain")) })
    }
}

/// An output hook that comes apart.
struct ComesApart;

impl OutputGuard for ComesApart {
    fn guard(&self, _call: &ToolCall, _output: ToolOutput) -> Result<ToolOutput, ToolError> {
        panic!("the output guard came apart")
    }
}

#[test]
fn a_call_whose_output_hook_comes_apart_is_answered_as_a_contained_panic() {
    let mut tools = Tools::new();
    tools
        .add_with_hooks(
            descriptor("guarded"),
            Arc::new(Plain),
            ToolHooks::new().guarding_output(Arc::new(ComesApart)),
        )
        .unwrap();

    let passed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        stopped_pass(
            &tools,
            &Cancel::new(),
            &[call("guarded", "guarded"), call("next", "guarded")],
            1,
        )
    }));

    let Ok((results, went, events)) = passed else {
        panic!("the output hook's panic unwound out of the pass");
    };
    assert_eq!(
        texts(&results),
        [
            "tool panicked; the failure was contained",
            "tool panicked; the failure was contained"
        ]
    );
    assert_eq!(
        finished(&events),
        [ToolOutcome::Panicked, ToolOutcome::Panicked]
    );
    assert!(matches!(went, Went::On));
}
