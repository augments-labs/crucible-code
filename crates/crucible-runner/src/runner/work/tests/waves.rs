//! A pass's runs as the tasks of a group: awaited whatever they wait on, never
//! dropped part way, committed in call order, and lent the run's worker.

use super::*;

/// Not ready the first time it is asked, having woken whoever asked, and
/// ready every time after: the smallest wait there is.
#[derive(Default)]
struct WaitsOnce(bool);

impl Future for WaitsOnce {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            return Poll::Ready(());
        }
        self.0 = true;
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// An acceptance that waits once before it binds the receipt it was given,
/// and says so if it is dropped instead.
struct AcceptedAfterWaiting(Arc<Mutex<Option<CallResultReceipt>>>);

impl Drop for AcceptedAfterWaiting {
    fn drop(&mut self) {
        let Ok(mut accepted) = self.0.lock() else {
            return;
        };
        if accepted.is_none() {
            *accepted = Some(CallResultReceipt::from_digest([0xdd; 32]));
        }
    }
}

impl CallResultAcceptance for AcceptedAfterWaiting {
    fn accept<'a>(
        self: Box<Self>,
        receipt: CallResultReceipt,
    ) -> BoxFuture<'a, Result<(), crucible_core::SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            WaitsOnce::default().await;
            *self.0.lock().unwrap() = Some(receipt);
            Ok(())
        })
    }
}

/// Defers its result to an acceptance that waits once.
struct DeferredAfterWaiting(Arc<Mutex<Option<CallResultReceipt>>>);

impl Tool for DeferredAfterWaiting {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("deferred after waiting")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            context
                .defer_call_result(Box::new(AcceptedAfterWaiting(Arc::clone(&self.0))))
                .map_err(|problem| ToolError::Io {
                    tool: "deferred".into(),
                    problem: "could not defer the final result".into(),
                    source: std::io::Error::other(problem),
                })?;
            Ok(ToolOutput::ok("raw executor output"))
        })
    }
}

#[test]
fn a_background_acceptance_that_waits_before_it_answers_is_awaited() {
    let accepted = Arc::new(Mutex::new(None));
    let descriptor = ToolDescriptor::new(
        "deferred",
        "{}",
        ToolProvenance::new(ToolSourceKind::User, "test:deferred", "deferred test").unwrap(),
    )
    .unwrap();
    let mut tools = Tools::new();
    tools
        .add(
            descriptor,
            Arc::new(DeferredAfterWaiting(Arc::clone(&accepted))),
        )
        .unwrap();
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
        worker: None,
        concurrency: 1,
    }
    .pass(&[call("deferred-call", "deferred")], 0, usize::MAX)
    .awaited();

    assert!(matches!(went, Went::On));
    assert_eq!(texts(&results), ["raw executor output"]);
    assert_eq!(
        *accepted.lock().unwrap(),
        Some(CallResultReceipt::from_digest([0x44; 32])),
        "the acceptance was dropped rather than awaited, and its scope handed back"
    );
}

/// A runtime of the test's own, with a timer and blocking threads, for a
/// test whose tool hands work to a worker on it.
fn worker_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
}

/// Hands a job to a worker that goes on past its last look at its token:
/// once the token is raised it still takes a moment to finish the step it
/// is in, and only then says it ended. A job whose call was dropped goes on
/// doing that with nobody waiting.
pub(super) struct Effecting {
    pub(super) worker: crucible_core::ToolWorker,
    /// Raised by the job once it has started, as a reader pressing the key
    /// while it runs would.
    pub(super) stops: Option<Cancel>,
    pub(super) started: Arc<AtomicUsize>,
    pub(super) ended: Arc<AtomicUsize>,
}

impl Tool for Effecting {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("effecting")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        let stops = self.stops.clone();
        let started = Arc::clone(&self.started);
        let ended = Arc::clone(&self.ended);
        Box::pin(async move {
            let said = self
                .worker
                .run(context.cancel(), move |token| {
                    started.fetch_add(1, Ordering::SeqCst);
                    if let Some(stops) = stops {
                        stops.request();
                    }
                    while !token.requested() {
                        thread::sleep(Duration::from_millis(1));
                    }
                    thread::sleep(Duration::from_millis(100));
                    ended.fetch_add(1, Ordering::SeqCst);
                    "ended after its last look"
                })
                .await
                .map_err(|problem| ToolError::Io {
                    tool: "effecting".into(),
                    problem: "its job did not answer".into(),
                    source: std::io::Error::other(problem),
                })?;
            Ok(ToolOutput::ok(said))
        })
    }
}

fn effecting_tools(tool: Effecting, descriptor: ToolDescriptor) -> Tools {
    let mut tools = Tools::new();
    tools.add(descriptor, Arc::new(tool)).unwrap();
    tools
}

fn effecting_descriptor() -> ToolDescriptor {
    ToolDescriptor::new(
        "effecting",
        "{}",
        ToolProvenance::new(ToolSourceKind::User, "test:effecting", "effecting test").unwrap(),
    )
    .unwrap()
}

#[test]
fn a_worker_job_is_awaited_through_the_turn_s_cancel_and_never_dropped_mid_run() {
    // The reader stops the turn while each call's job is running. The job is
    // asked to stop and still finishes the step it is in; the pass waits for
    // that and for the run's own answer, so nothing a job did is left
    // untold behind a turn that already ended.
    let runtime = worker_runtime();
    let cancel = Cancel::new();
    let started = Arc::new(AtomicUsize::new(0));
    let ended = Arc::new(AtomicUsize::new(0));
    let tools = effecting_tools(
        Effecting {
            worker: crucible_core::ToolWorker::new(runtime.handle().clone()),
            stops: Some(cancel.clone()),
            started: Arc::clone(&started),
            ended: Arc::clone(&ended),
        },
        effecting_descriptor().executing(ToolExecutionMode::Parallel),
    );
    let snapshot = tools.snapshot().unwrap();
    let (events, _seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let journal = Recording::nowhere();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = runtime.block_on(
        Work {
            tools: &snapshot,
            permission: &mut permission,
            ask: &mut ask,
            events: Reporter::new(ancestry, &keeping),
            cancel: &cancel,
            ancestry,
            journal: &*journal,
            audits: &SandboxAuditRegistry::new(),
            worker: None,
            concurrency: 2,
        }
        .pass(
            &[call("effect-a", "effecting"), call("effect-b", "effecting")],
            0,
            usize::MAX,
        ),
    );
    let (started, ended) = (started.load(Ordering::SeqCst), ended.load(Ordering::SeqCst));

    assert!(started >= 1, "no job started, so nothing was proved");
    assert_eq!(
        ended, started,
        "the pass returned while a job it started was still running"
    );
    assert!(matches!(went, Went::Stopped(StopReason::Cancelled)));
    assert_eq!(results.len(), 2);
}

#[test]
fn a_worker_job_still_running_at_its_deadline_is_asked_to_stop_awaited_and_reported_as_done() {
    // A deadline makes the call's cancel read as raised, so the job, which
    // looks at its token, stops at its next look; the call waits for the job
    // to finish the step it is in and for the run's own answer. The run
    // answered with what it did, so that is what the call is answered with:
    // an effect the job committed is never reported as not done.
    let runtime = worker_runtime();
    let started = Arc::new(AtomicUsize::new(0));
    let ended = Arc::new(AtomicUsize::new(0));
    let tools = effecting_tools(
        Effecting {
            worker: crucible_core::ToolWorker::new(runtime.handle().clone()),
            stops: None,
            started: Arc::clone(&started),
            ended: Arc::clone(&ended),
        },
        effecting_descriptor()
            .timing_out_after(Duration::from_millis(50))
            .unwrap(),
    );
    let snapshot = tools.snapshot().unwrap();
    let (events, seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let cancel = Cancel::new();
    let journal = Recording::nowhere();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = runtime.block_on(
        Work {
            tools: &snapshot,
            permission: &mut permission,
            ask: &mut ask,
            events: Reporter::new(ancestry, &keeping),
            cancel: &cancel,
            ancestry,
            journal: &*journal,
            audits: &SandboxAuditRegistry::new(),
            worker: None,
            concurrency: 1,
        }
        .pass(&[call("effect", "effecting")], 0, usize::MAX),
    );

    assert_eq!(started.load(Ordering::SeqCst), 1);
    assert_eq!(
        ended.load(Ordering::SeqCst),
        1,
        "the call was answered while the job it started was still running"
    );
    assert!(matches!(went, Went::On));
    assert_eq!(texts(&results), ["ended after its last look"]);
    drop(keeping);
    assert!(seen.try_iter().any(|event| matches!(
        event,
        Event::ToolFinished {
            receipt: Some(receipt),
            ..
        } if receipt.outcome() == ToolOutcome::Succeeded
    )));
}

/// Answers once the call named `after` has answered, so a wave's runs answer
/// in the order the test chooses rather than the order they were asked.
struct AnswersAfter {
    answered: Arc<Mutex<Vec<String>>>,
}

impl Tool for AnswersAfter {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run<'a>(
        &'a self,
        approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let own = approved.args().as_str().to_owned();
            let after = own.strip_prefix("after-").map(str::to_owned);
            if let Some(after) = after {
                std::future::poll_fn(|cx| {
                    if self.answered.lock().unwrap().contains(&after) {
                        Poll::Ready(())
                    } else {
                        cx.waker().wake_by_ref();
                        Poll::Pending
                    }
                })
                .await;
            }
            self.answered.lock().unwrap().push(own.clone());
            Ok(ToolOutput::ok(own))
        })
    }
}

#[test]
fn a_wave_commits_in_call_order_whatever_order_its_runs_answer_in() {
    let answered = Arc::new(Mutex::new(Vec::new()));
    let descriptor = ToolDescriptor::new(
        "ordered",
        "{}",
        ToolProvenance::new(ToolSourceKind::User, "test:ordered", "ordered test").unwrap(),
    )
    .unwrap()
    .executing(ToolExecutionMode::Parallel);
    let mut tools = Tools::new();
    tools
        .add(
            descriptor,
            Arc::new(AnswersAfter {
                answered: Arc::clone(&answered),
            }),
        )
        .unwrap();
    let calls = [
        scheduled_call("a", "ordered", "after-b"),
        scheduled_call("b", "ordered", "b"),
    ];

    let (results, went, events) = invoke_many(
        &tools,
        &mut Permission::new(),
        &mut Says::new(Verdict::Allow),
        &calls,
        (2, usize::MAX),
    );

    assert_eq!(
        *answered.lock().unwrap(),
        ["b", "after-b"],
        "the fixture did not answer out of call order"
    );
    assert!(matches!(went, Went::On));
    assert_eq!(texts(&results), ["after-b", "b"]);
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                Event::ToolFinished { call, .. } => Some(call.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["a", "b"]
    );
}

/// Runs a job on the worker its call was lent, and answers with how the
/// worker `seen` — a copy the test kept of the one it lent — looked while
/// that job held its place.
struct Borrows {
    seen: crucible_core::ToolWorker,
}

impl Tool for Borrows {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("borrows")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            let Some(worker) = context.worker() else {
                return Ok(ToolOutput::failed("no worker was lent"));
            };
            let seen = self.seen.clone();
            let said = worker
                .run(context.cancel(), move |_| format!("{seen:?}"))
                .await
                .map_err(|problem| ToolError::Io {
                    tool: "borrows".into(),
                    problem: "its job did not answer".into(),
                    source: std::io::Error::other(problem),
                })?;
            Ok(ToolOutput::ok(said))
        })
    }
}

#[test]
fn a_call_runs_its_blocking_work_on_the_worker_the_run_lent() {
    // The job holds one place on the worker it runs on. The copy the test
    // kept shares the lent worker's bound, so it shows that place taken only
    // if the job ran on the very worker that was lent.
    let runtime = worker_runtime();
    let worker = crucible_core::ToolWorker::new(runtime.handle().clone());
    let descriptor = ToolDescriptor::new(
        "borrows",
        "{}",
        ToolProvenance::new(ToolSourceKind::User, "test:borrows", "borrows test").unwrap(),
    )
    .unwrap();
    let mut tools = Tools::new();
    tools
        .add(
            descriptor,
            Arc::new(Borrows {
                seen: worker.clone(),
            }),
        )
        .unwrap();
    let snapshot = tools.snapshot().unwrap();
    let (events, _seen) = channel();
    let keeping = Keeping(events);
    let ancestry = Ancestry::new();
    let cancel = Cancel::new();
    let journal = Recording::nowhere();
    let mut permission = Permission::new();
    let mut ask = Says::new(Verdict::Allow);

    let (results, went, _) = runtime.block_on(
        Work {
            tools: &snapshot,
            permission: &mut permission,
            ask: &mut ask,
            events: Reporter::new(ancestry, &keeping),
            cancel: &cancel,
            ancestry,
            journal: &*journal,
            audits: &SandboxAuditRegistry::new(),
            worker: Some(&worker),
            concurrency: 1,
        }
        .pass(&[call("borrow", "borrows")], 0, usize::MAX),
    );

    assert!(matches!(went, Went::On));
    let said = texts(&results).concat();
    assert!(
        said.contains("available: 3"),
        "the job did not hold a place on the lent worker: {said}"
    );
}

/// Prints `pieces` pieces one after another, counting each one it has handed
/// over.
struct Chatty {
    pieces: usize,
    handed: Arc<AtomicUsize>,
}

impl Tool for Chatty {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("chatty")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            for piece in 0..self.pieces {
                context.wrote(Wrote::new(format!("{piece}\n")));
                self.handed.fetch_add(1, Ordering::SeqCst);
            }
            Ok(ToolOutput::ok("said it all"))
        })
    }
}

/// Keeps every event, holding the first one until it is let go, as a
/// terminal that has fallen behind holds the turn at its channel.
struct Held {
    kept: Sender<Event>,
    go: Mutex<Option<Receiver<()>>>,
}

impl Post for Held {
    fn post(&self, reported: EventEnvelope) {
        if let Some(go) = self.go.lock().unwrap().take() {
            let _ = go.recv();
        }
        drop(self.kept.send(reported.into_event()));
    }
}

#[test]
fn a_run_printing_faster_than_the_turn_reports_waits_for_room_and_loses_nothing() {
    const PIECES: usize = 200;
    let handed = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&handed);
    let (kept, seen) = channel();
    let (release, go) = channel();

    // The pass runs on a thread of its own, since the test holds its reporting
    // back and has to be free to let it go.
    let passing = thread::spawn(move || {
        let descriptor = ToolDescriptor::new(
            "chatty",
            "{}",
            ToolProvenance::new(ToolSourceKind::User, "test:chatty", "chatty test").unwrap(),
        )
        .unwrap();
        let mut tools = Tools::new();
        tools
            .add(
                descriptor,
                Arc::new(Chatty {
                    pieces: PIECES,
                    handed: counted,
                }),
            )
            .unwrap();
        let snapshot = tools.snapshot().unwrap();
        let held = Held {
            kept,
            go: Mutex::new(Some(go)),
        };
        let ancestry = Ancestry::new();
        let cancel = Cancel::new();
        let journal = Recording::nowhere();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(TOOL_RUNS)
            .enable_time()
            .build()
            .unwrap();
        let (results, _, _) = runtime.block_on(
            Work {
                tools: &snapshot,
                permission: &mut Permission::new(),
                ask: &mut Says::new(Verdict::Allow),
                events: Reporter::new(ancestry, &held),
                cancel: &cancel,
                ancestry,
                journal: &*journal,
                audits: &SandboxAuditRegistry::new(),
                worker: None,
                concurrency: 1,
            }
            .pass(&[call("chatty-call", "chatty")], 0, usize::MAX),
        );
        texts(&results).concat()
    });

    thread::sleep(Duration::from_millis(200));
    let while_held = handed.load(Ordering::SeqCst);
    release.send(()).unwrap();
    let answered = passing.join().unwrap();

    assert!(
        while_held <= 2 * WRITTEN,
        "{while_held} pieces were handed over while the turn could report none of them"
    );
    assert_eq!(answered, "said it all");
    let events: Vec<Event> = seen.try_iter().collect();
    let wrote: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            Event::Wrote { text, .. } => Some(text.as_str().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        wrote,
        (0..PIECES)
            .map(|piece| format!("{piece}\n"))
            .collect::<Vec<_>>(),
        "a piece was lost or reordered on its way to the turn"
    );
    assert!(
        matches!(events.last(), Some(Event::ToolFinished { .. })),
        "the call answered before what it printed was reported"
    );
}
