//! What waiting for a confined process to finish has to guarantee.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use crucible_runtime::{BoxFuture, Bridge, Unready};
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection,
    SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRequest, SandboxResourceLimits, SandboxUsage, SandboxViolation,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

use super::Finish;

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
    /// Stopping it never answers, so a caller that cannot wait drops the stop.
    waits: AtomicBool,
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

#[test]
fn a_process_that_ended_in_its_grace_is_waited_for_rather_than_stopped() {
    // Its ending can wait on another command's publication for longer than the
    // grace. Stopping it then would discard what it wrote, from a process that
    // did exactly what the grace was for.
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let later = Arc::clone(&ending);
    let completing = thread::spawn(move || {
        thread::sleep(Duration::from_millis(100));
        later.exited.store(true, Ordering::Relaxed);
    });

    let finish = Finish::after(&mut process, Duration::ZERO);
    completing.join().expect("the ending completed");

    assert!(matches!(finish, Finish::Exited(_)), "{finish:?}");
    assert_eq!(
        ending.stops.load(Ordering::Relaxed),
        0,
        "stopping it would have discarded what it wrote"
    );
}

#[test]
fn a_process_whose_ending_went_wrong_says_why_rather_than_being_stopped() {
    // An error from a process that has ended is not a status nobody could
    // read: it is how that ending went wrong, and stopping it afterwards would
    // report an ordinary stop in its place.
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        failed: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);

    let finish = Finish::after(&mut process, Duration::from_millis(50));

    match finish {
        Finish::Unpublished(problem) => assert!(
            problem.to_string().contains("writable root changed"),
            "{problem}"
        ),
        other => panic!("an ending that went wrong was reported as {other:?}"),
    }
    assert_eq!(ending.stops.load(Ordering::Relaxed), 0);
}

#[test]
fn a_process_whose_publication_never_finishes_says_so_once_its_patience_has_passed() {
    // The wait for an ending is worth making only while it can end. A lock held
    // by something outside this process — an older crucible, say — would
    // otherwise keep a turn and a shutdown waiting for ever.
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let (told, hears) = std::sync::mpsc::channel();
    let waiting = thread::spawn(move || {
        let finish = Finish::after(&mut process, Duration::ZERO);
        told.send(format!("{finish:?}")).expect("the test hears it");
    });

    let finish = hears.recv_timeout(Duration::from_secs(20));

    let finish = finish.expect("the wait for a publication that never ends has a ceiling");
    waiting.join().expect("the waiting thread");
    // Stopped, and said so: the ceiling discarded what it wrote, and an ending
    // reported as a clean stop tells the caller nothing was lost.
    assert!(finish.starts_with("Unpublished"), "{finish}");
    assert_eq!(ending.stops.load(Ordering::Relaxed), 1);
}

#[test]
fn a_stop_that_fails_after_the_ceiling_says_both_things() {
    // Two facts, and a caller told only the second retires the program as
    // though it might still be running: its publication did not finish in time,
    // and the stop that followed could not be confirmed either.
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        unstoppable: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let (told, hears) = std::sync::mpsc::channel();
    let waiting = thread::spawn(move || {
        let finish = Finish::after(&mut process, Duration::ZERO);
        told.send(finish).expect("the test hears it");
    });

    let finish = hears.recv_timeout(Duration::from_secs(20));

    let finish = finish.expect("the wait for a publication that never ends has a ceiling");
    waiting.join().expect("the waiting thread");
    match finish {
        // The message, not the error's `Debug`, which names a field for the
        // publication whatever the message says.
        Finish::Unreaped(problem) => {
            assert!(problem.to_string().contains("publication"), "{problem}");
            // A failed stop's words do not say that its end is unconfirmed, so
            // the message says it, once.
            assert_eq!(
                problem.to_string(),
                "its publication did not finish in time, and stopping it could not be \
                 confirmed: the scope could not be reaped"
            );
        }
        other => panic!("a stop that failed after the ceiling was reported as {other:?}"),
    }
}

#[test]
fn a_stop_that_would_have_had_to_wait_after_the_ceiling_keeps_both_facts_and_the_refusal() {
    // The stop is dropped rather than waited on, so its end is not known; and a
    // caller deciding what to do about that has to be able to tell a refused
    // stop from a failed one, under the words that still say both facts.
    let ending = Arc::new(Ending {
        ended: AtomicBool::new(true),
        waits: AtomicBool::new(true),
        ..Ending::default()
    });
    let mut process = process(&ending);
    let (told, hears) = std::sync::mpsc::channel();
    let waiting = thread::spawn(move || {
        let finish = Finish::after(&mut process, Duration::ZERO);
        told.send(finish).expect("the test hears it");
    });

    let finish = hears.recv_timeout(Duration::from_secs(20));

    let finish = finish.expect("the wait for a publication that never ends has a ceiling");
    waiting.join().expect("the waiting thread");
    assert_eq!(ending.stops.load(Ordering::Relaxed), 1);
    match finish {
        Finish::Unreaped(problem) => {
            assert_eq!(
                refusal(&problem).map(|unready| unready.bridge()),
                Some(Bridge::TransportProcess),
                "the refusal is carried as itself, not as its words: {problem:?}"
            );
            assert!(problem.to_string().contains("publication"), "{problem}");
            // The refusal's words already say that the stop's end is
            // unconfirmed, and the message does not say it a second time.
            assert_eq!(
                problem.to_string(),
                "its publication did not finish in time, and stopping a hosted program \
                 would have had to wait, and the caller cannot; the waiting step was \
                 dropped before it answered, so whatever that step began is unconfirmed"
            );
        }
        other => panic!("a stop nobody confirmed was reported as {other:?}"),
    }
}
