//! What waiting for a confined process to finish has to guarantee.

use std::io;
use std::process::ExitStatus;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

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

    fn stop(&mut self) -> io::Result<()> {
        self.ending.stops.fetch_add(1, Ordering::Relaxed);
        Ok(())
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
