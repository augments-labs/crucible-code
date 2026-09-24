use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crucible_runtime::{Cancel, NOTICED};
use tokio::runtime::{Builder, Runtime};
use tokio::task::JoinHandle;

use super::{ToolWorker, Unrun};

/// How long a test waits for something the code under test should do at once,
/// before deciding it never will. Far longer than [`NOTICED`], so a pass is a
/// fact about the code rather than about how busy the machine is.
const PATIENCE: Duration = Duration::from_secs(5);

/// How often a job held on a gate looks at it.
const TICK: Duration = Duration::from_millis(1);

/// The longest any job here runs, whether or not what it waits for comes.
/// Dropping a runtime waits for its blocking threads, so a job that outlived a
/// failed assertion would turn the failure into a hang.
const LIFETIME: Duration = Duration::from_secs(10);

/// Waits on a blocking thread until `gate` is raised, or [`LIFETIME`] passes;
/// says which.
fn held_until(gate: &Cancel) -> &'static str {
    let began = std::time::Instant::now();
    while !gate.requested() {
        if began.elapsed() >= LIFETIME {
            return "gave up";
        }
        std::thread::sleep(TICK);
    }
    "released"
}

/// A runtime of the test's own, with the timer the worker's wait looks at its
/// cancellation on, as the application's has.
fn runtime() -> Runtime {
    Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a runtime for the test")
}

/// Waits until `done` holds, or fails the test once [`PATIENCE`] has passed.
async fn until(what: &str, done: impl Fn() -> bool) {
    let began = tokio::time::Instant::now();
    while !done() {
        assert!(began.elapsed() < PATIENCE, "{what} never happened");
        tokio::time::sleep(TICK).await;
    }
}

/// Fills every place on `worker` with a job that runs until `gate` is raised,
/// and waits until every one of them has started.
async fn occupy(
    worker: &ToolWorker,
    gate: &Cancel,
) -> Vec<JoinHandle<Result<&'static str, Unrun>>> {
    let started = Arc::new(AtomicUsize::new(0));
    let holding = (0..ToolWorker::CAPACITY)
        .map(|_| {
            let worker = worker.clone();
            let gate = gate.clone();
            let started = Arc::clone(&started);
            tokio::spawn(async move {
                worker
                    .run(&Cancel::new(), move |_| {
                        started.fetch_add(1, Ordering::AcqRel);
                        held_until(&gate)
                    })
                    .await
            })
        })
        .collect();
    until("every place on the worker being taken", || {
        started.load(Ordering::Acquire) == ToolWorker::CAPACITY
    })
    .await;
    holding
}

#[test]
fn a_call_cancelled_while_waiting_for_room_leaves_and_its_work_never_starts() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());

    runtime.block_on(async {
        let gate = Cancel::new();
        let holding = occupy(&worker, &gate).await;

        let call = Cancel::new();
        let ran = Arc::new(AtomicBool::new(false));
        let kept = Arc::new(());
        let waiting = {
            let worker = worker.clone();
            let call = call.clone();
            let ran = Arc::clone(&ran);
            let kept = Arc::clone(&kept);
            tokio::spawn(async move {
                worker
                    .run(&call, move |_| {
                        let _ = &kept;
                        ran.store(true, Ordering::Release);
                    })
                    .await
            })
        };
        // Long enough for the call to have asked for room and been told to
        // wait, so what is cancelled is a call that is waiting.
        tokio::time::sleep(NOTICED * 2).await;
        assert!(!waiting.is_finished(), "the call did not wait for room");

        call.request();
        let answered = tokio::time::timeout(PATIENCE, waiting).await;
        assert!(
            matches!(answered, Ok(Ok(Err(Unrun::Cancelled)))),
            "the cancelled call was still waiting for room: {answered:?}"
        );
        assert_eq!(
            Arc::strong_count(&kept),
            1,
            "the cancelled call's work was still held after the call answered"
        );

        gate.request();
        for held in holding {
            assert_eq!(held.await.expect("a held job's task"), Ok("released"));
        }
        assert!(
            !ran.load(Ordering::Acquire),
            "work cancelled while it waited for room ran once room was made"
        );
        assert_eq!(
            worker.capacity.available_permits(),
            ToolWorker::CAPACITY,
            "a place on the worker was still taken once every job had ended"
        );
    });
}

/// A job that runs until the token it is handed is raised, and says that it
/// started and that it ended.
fn until_stopped(
    started: &Arc<AtomicBool>,
    ended: &Arc<AtomicBool>,
) -> impl FnOnce(&Cancel) -> &'static str + Send + 'static {
    let started = Arc::clone(started);
    let ended = Arc::clone(ended);
    move |stop| {
        started.store(true, Ordering::Release);
        let said = held_until(stop);
        ended.store(true, Ordering::Release);
        said
    }
}

#[test]
fn a_call_cancelled_while_its_job_runs_waits_for_the_job_and_gives_its_place_back() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());

    runtime.block_on(async {
        let call = Cancel::new();
        let started = Arc::new(AtomicBool::new(false));
        let ended = Arc::new(AtomicBool::new(false));
        let running = {
            let worker = worker.clone();
            let call = call.clone();
            let job = until_stopped(&started, &ended);
            tokio::spawn(async move { worker.run(&call, job).await })
        };
        until("the job starting", || started.load(Ordering::Acquire)).await;

        call.request();
        let answered = tokio::time::timeout(PATIENCE, running).await;
        assert!(
            matches!(answered, Ok(Ok(Ok("released")))),
            "the call's cancellation did not reach its running job: {answered:?}"
        );
        assert!(
            ended.load(Ordering::Acquire),
            "the call answered before the job it started had returned"
        );
        assert_eq!(
            worker.capacity.available_permits(),
            ToolWorker::CAPACITY,
            "the job's place was not given back when it returned"
        );
    });
}

#[test]
fn a_run_asked_once_from_outside_any_runtime_waits_and_its_job_stops_when_it_is_dropped() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let started = Arc::new(AtomicBool::new(false));
    let ended = Arc::new(AtomicBool::new(false));

    // Asked once, on a thread no runtime runs and none is entered on, the way
    // a crossing that polls once asks a tool's run.
    {
        let cancel = Cancel::new();
        let mut asked = std::pin::pin!(worker.run(&cancel, until_stopped(&started, &ended)));
        let polled = asked
            .as_mut()
            .poll(&mut std::task::Context::from_waker(std::task::Waker::noop()));
        assert!(polled.is_pending(), "a job that waits answered at once");
    }

    runtime.block_on(until(
        "the dropped run's job stopping and giving its place back",
        || {
            ended.load(Ordering::Acquire)
                && worker.capacity.available_permits() == ToolWorker::CAPACITY
        },
    ));
}

#[test]
fn no_more_jobs_run_at_once_than_the_worker_has_places() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());

    runtime.block_on(async {
        let gate = Cancel::new();
        let holding = occupy(&worker, &gate).await;

        let ran = Arc::new(AtomicBool::new(false));
        let extra = {
            let worker = worker.clone();
            let ran = Arc::clone(&ran);
            tokio::spawn(async move {
                worker
                    .run(&Cancel::new(), move |_| ran.store(true, Ordering::Release))
                    .await
            })
        };
        tokio::time::sleep(NOTICED * 2).await;
        assert!(
            !ran.load(Ordering::Acquire),
            "a job started while every place on the worker was taken"
        );

        gate.request();
        for held in holding {
            assert_eq!(held.await.expect("a held job's task"), Ok("released"));
        }
        assert_eq!(
            tokio::time::timeout(PATIENCE, extra)
                .await
                .ok()
                .map(Result::ok),
            Some(Some(Ok(()))),
            "the waiting job never got the place given back"
        );
        assert!(ran.load(Ordering::Acquire));
    });
}

#[test]
fn a_call_cancelled_before_it_asks_starts_nothing_even_with_room() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());
    let cancel = Cancel::new();
    cancel.request();
    let ran = Arc::new(AtomicBool::new(false));

    let answered = runtime.block_on({
        let ran = Arc::clone(&ran);
        worker.run(&cancel, move |_| ran.store(true, Ordering::Release))
    });

    assert_eq!(answered, Err(Unrun::Cancelled));
    assert!(!ran.load(Ordering::Acquire), "a cancelled call's job ran");
}

#[test]
fn a_job_that_comes_apart_gives_its_place_back_and_says_so() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());

    let answered: Result<(), Unrun> =
        runtime.block_on(worker.run(&Cancel::new(), |_| panic!("the job came apart")));

    assert_eq!(answered, Err(Unrun::Panicked));
    assert_eq!(
        worker.capacity.available_permits(),
        ToolWorker::CAPACITY,
        "a job that came apart kept its place"
    );
}

#[test]
fn a_worker_whose_runtime_has_stopped_says_so_and_starts_nothing() {
    let stopped = runtime();
    let worker = ToolWorker::new(stopped.handle().clone());
    stopped.shutdown_background();
    let ran = Arc::new(AtomicBool::new(false));
    let cancel = Cancel::new();

    let answered = runtime().block_on({
        let ran = Arc::clone(&ran);
        worker.run(&cancel, move |_| ran.store(true, Ordering::Release))
    });

    assert_eq!(answered, Err(Unrun::Stopped));
    assert!(
        !ran.load(Ordering::Acquire),
        "a job ran on a runtime that had stopped"
    );
    assert_eq!(worker.capacity.available_permits(), ToolWorker::CAPACITY);
}

#[test]
fn a_dropped_call_raises_its_job_s_token_and_the_place_comes_back_only_when_the_job_returns() {
    let runtime = runtime();
    let worker = ToolWorker::new(runtime.handle().clone());

    runtime.block_on(async {
        // A job that sees its token raised and then goes on working, on a gate
        // of its own, the way a job between two checks does: what a dropped
        // call leaves is a job that has been asked to stop and has not yet
        // returned.
        let gate = Cancel::new();
        let started = Arc::new(AtomicBool::new(false));
        let told = Arc::new(AtomicBool::new(false));
        let running = {
            let worker = worker.clone();
            let gate = gate.clone();
            let started = Arc::clone(&started);
            let told = Arc::clone(&told);
            tokio::spawn(async move {
                worker
                    .run(&Cancel::new(), move |stop| {
                        started.store(true, Ordering::Release);
                        if held_until(stop) == "released" {
                            told.store(true, Ordering::Release);
                        }
                        held_until(&gate)
                    })
                    .await
            })
        };
        until("the job starting", || started.load(Ordering::Acquire)).await;
        assert!(
            !told.load(Ordering::Acquire),
            "the job's token was raised before the call was dropped"
        );

        running.abort();
        let _ = running.await;
        until("the dropped call's job seeing its token raised", || {
            told.load(Ordering::Acquire)
        })
        .await;
        assert_eq!(
            worker.capacity.available_permits(),
            ToolWorker::CAPACITY - 1,
            "the dropped call's place came back while its job was still running"
        );

        gate.request();
        until("the job returning and its place coming back", || {
            worker.capacity.available_permits() == ToolWorker::CAPACITY
        })
        .await;
    });
}
