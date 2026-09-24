//! What waiting for a confined process to finish has to guarantee.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crucible_runtime::{BoxFuture, Unready};
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection,
    SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRequest, SandboxResourceLimits, SandboxUsage, SandboxViolation,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

use super::{Finish, PUBLICATION, STOPPING};

/// An absolute path spelled the way the running platform's path type accepts.
#[cfg(unix)]
const ROOT: &str = "/workspace";
#[cfg(windows)]
const ROOT: &str = r"C:\workspace";

/// How a test's process ends, decided by the test.
#[derive(Default)]
struct Ending {
    /// It has ended, while its ending waits on something else.
    ended: AtomicBool,
    /// Its ending is complete, and a look at it says so.
    exited: AtomicBool,
    /// Its ending went wrong, and a look at it says how.
    failed: AtomicBool,
    /// How many times it was asked to stop.
    stops: AtomicUsize,
    /// Stopping it does not confirm that its scope ended.
    unstoppable: AtomicBool,
    /// Stopping it never answers, so an awaited caller gives up on it at the
    /// bound on a stop that does not answer.
    waits: AtomicBool,
    /// Stopping it answers only after a wait on the runtime's clock, inside the
    /// bound on a stop that does not answer, which a caller that can wait sits
    /// through.
    slow: AtomicBool,
}

struct Process {
    ending: Arc<Ending>,
    inspection: SandboxInspection,
}

impl SandboxProcess for Process {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        None
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.ending.failed.load(Ordering::Relaxed) {
            return Err(io::Error::other(
                "writable root changed after the command started",
            ));
        }
        Ok(self.ending.exited.load(Ordering::Relaxed).then(exited))
    }

    fn ended(&mut self) -> bool {
        self.ending.ended.load(Ordering::Relaxed) || self.ending.exited.load(Ordering::Relaxed)
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            self.ending.stops.fetch_add(1, Ordering::Relaxed);
            if self.ending.waits.load(Ordering::Relaxed) {
                std::future::pending::<()>().await;
            }
            if self.ending.slow.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            if self.ending.unstoppable.load(Ordering::Relaxed) {
                // Words that say what went wrong and not that the end is
                // unconfirmed, as a backend's failure need not say it.
                return Err(io::Error::other("the scope could not be reaped"));
            }
            Ok(())
        })
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage::default()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        None
    }
}

fn exited() -> ExitStatus {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt as _;

    ExitStatus::from_raw(0)
}

/// The refusal somewhere beneath `error`, however it was carried.
///
/// An [`io::Error`] hides the error it holds from `source`, so the walk looks
/// inside one before stepping past it.
fn refusal(error: &(dyn std::error::Error + 'static)) -> Option<Unready> {
    let mut next = Some(error);
    while let Some(link) = next {
        if let Some(unready) = link.downcast_ref::<Unready>() {
            return Some(*unready);
        }
        next = match link
            .downcast_ref::<io::Error>()
            .and_then(io::Error::get_ref)
        {
            Some(inner) => Some(inner as &(dyn std::error::Error + 'static)),
            None => link.source(),
        };
    }
    None
}

fn process(ending: &Arc<Ending>) -> Process {
    let policy = SandboxPolicy::new(
        false,
        [SandboxFilesystemRule::new(
            ROOT,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("a rule over an absolute root")],
        ROOT,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("a policy whose rules are all rooted");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("finish"),
        policy,
        SandboxManifest::empty(),
    );
    let backend = SandboxBackendIdentity::new(
        SandboxBackendId::new("test").expect("a backend name"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("a backend identity");
    let inspection = SandboxInspection::unconfined_for_request(
        backend,
        SandboxCapabilities::none(),
        &request,
        "a test, which confines nothing",
    )
    .expect("an inspection of a request a test built");
    Process {
        ending: Arc::clone(ending),
        inspection,
    }
}

// The asynchronous finish, on a paused clock: every wait below is the rule's
// own and costs no wall time, so a bound is asserted exactly rather than
// against a scheduler's delay.

#[tokio::test(start_paused = true)]
async fn a_process_that_ended_in_its_grace_is_waited_for_asynchronously_rather_than_stopped() {
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let later = Arc::clone(&ending);
    let completing = async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        later.exited.store(true, Ordering::Relaxed);
    };

    let (finish, ()) = tokio::join!(
        Finish::after_async(&mut process, Duration::ZERO),
        completing
    );

    assert!(matches!(finish, Finish::Exited(_)), "{finish:?}");
    assert_eq!(ending.stops.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn a_process_whose_ending_went_wrong_says_why_asynchronously_rather_than_being_stopped() {
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        failed: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);

    let finish = Finish::after_async(&mut process, Duration::from_millis(50)).await;

    match finish {
        Finish::Unpublished(problem) => assert!(
            problem.to_string().contains("writable root changed"),
            "{problem}"
        ),
        other => panic!("an ending that went wrong was reported as {other:?}"),
    }
    assert_eq!(ending.stops.load(Ordering::Relaxed), 0);
}

#[tokio::test(start_paused = true)]
async fn a_process_that_will_not_go_is_stopped_asynchronously_once_its_grace_has_passed() {
    let ending = Arc::new(Ending::default());
    let mut process = process(&ending);
    let grace = Duration::from_millis(50);
    let began = tokio::time::Instant::now();

    let finish = Finish::after_async(&mut process, grace).await;

    assert!(matches!(finish, Finish::Stopped), "{finish:?}");
    assert_eq!(ending.stops.load(Ordering::Relaxed), 1);
    assert!(
        began.elapsed() >= grace,
        "stopped after {:?}",
        began.elapsed()
    );
}

#[tokio::test(start_paused = true)]
async fn a_stop_that_takes_a_while_is_waited_for_asynchronously_rather_than_dropped() {
    // A stop that answers inside its bound is a stop that was confirmed.
    let ending = Arc::new(Ending {
        slow: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);

    let finish = Finish::after_async(&mut process, Duration::ZERO).await;

    assert!(matches!(finish, Finish::Stopped), "{finish:?}");
    assert_eq!(ending.stops.load(Ordering::Relaxed), 1);
}

#[tokio::test(start_paused = true)]
async fn a_process_whose_publication_never_finishes_says_so_asynchronously_at_the_ceiling() {
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let began = tokio::time::Instant::now();

    let finish = Finish::after_async(&mut process, Duration::ZERO).await;

    assert!(
        matches!(&finish, Finish::Unpublished(problem) if problem.to_string()
            == "its publication did not finish in time"),
        "{finish:?}"
    );
    assert_eq!(ending.stops.load(Ordering::Relaxed), 1);
    let waited = began.elapsed();
    assert!(waited >= PUBLICATION, "stopped after {waited:?}");
    assert!(
        waited < PUBLICATION + Duration::from_millis(100),
        "stopped after {waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_stop_that_fails_asynchronously_after_the_ceiling_says_both_things() {
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        unstoppable: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);

    let finish = Finish::after_async(&mut process, Duration::ZERO).await;

    match finish {
        Finish::Unreaped(problem) => assert_eq!(
            problem.to_string(),
            "its publication did not finish in time, and stopping it could not be \
             confirmed: the scope could not be reaped"
        ),
        other => panic!("a stop that failed after the ceiling was reported as {other:?}"),
    }
}

#[tokio::test(start_paused = true)]
async fn a_stop_that_never_answers_is_given_up_on_and_reported_as_failed_cleanup() {
    // The finish is bounded end to end: a stop that is never going to answer is
    // waited on for as long as a stop can be worth waiting on and no longer,
    // and what that leaves is a program whose end nobody confirmed.
    let ending = Arc::new(Ending {
        waits: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let grace = Duration::from_millis(50);
    let began = tokio::time::Instant::now();

    let finish = Finish::after_async(&mut process, grace).await;

    let waited = began.elapsed();
    assert_eq!(ending.stops.load(Ordering::Relaxed), 1);
    match finish {
        Finish::Unreaped(problem) => {
            assert_eq!(problem.kind(), io::ErrorKind::TimedOut, "{problem:?}");
            assert_eq!(
                problem.to_string(),
                "stopping a hosted program did not answer within 50ms, so whatever it \
                 began is unconfirmed"
            );
            // Not a refusal to wait: this finish did wait, and ran out.
            assert!(refusal(&problem).is_none(), "{problem:?}");
        }
        other => panic!("a stop that never answered was reported as {other:?}"),
    }
    assert!(waited >= grace + STOPPING, "gave up after {waited:?}");
    assert!(
        waited < grace + STOPPING + Duration::from_millis(100),
        "gave up after {waited:?}"
    );
}

#[tokio::test(start_paused = true)]
async fn a_stop_that_never_answers_after_the_ceiling_keeps_both_facts() {
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        waits: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let began = tokio::time::Instant::now();

    let finish = Finish::after_async(&mut process, Duration::ZERO).await;

    let waited = began.elapsed();
    match finish {
        Finish::Unreaped(problem) => {
            assert_eq!(problem.kind(), io::ErrorKind::TimedOut, "{problem:?}");
            // The stop's words already say that its end is unconfirmed, and
            // the message does not say it a second time.
            assert_eq!(
                problem.to_string(),
                "its publication did not finish in time, and stopping a hosted program \
                 did not answer within 50ms, so whatever it began is unconfirmed"
            );
        }
        other => panic!("a stop that never answered was reported as {other:?}"),
    }
    assert!(
        waited < PUBLICATION + STOPPING + Duration::from_millis(100),
        "gave up after {waited:?}"
    );
}

#[test]
fn an_asynchronous_finish_can_be_awaited_on_a_task_of_its_own() {
    // A runtime moves a spawned task between its threads, so a finish that is
    // not `Send` could only be awaited where it was made.
    fn sendable<T: Send>(_: &T) {}
    let ending = Arc::new(Ending::default());
    let mut process = process(&ending);
    let finishing = Finish::after_async(&mut process, Duration::ZERO);
    sendable(&finishing);
}
