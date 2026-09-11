//! Failed cleanup retains the registry's only process owner and capacity.

use std::io;
use std::process::ExitStatus;
use std::sync::atomic::AtomicUsize;

use crucible_core::{
    Cancel, SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance,
    SandboxCapabilities, SandboxCleanup, SandboxFilesystemAccess, SandboxFilesystemProvenance,
    SandboxFilesystemRule, SandboxInspection, SandboxManifest, SandboxNetworkPolicy, SandboxOutput,
    SandboxPolicy, SandboxResourceLimits, SandboxUsage, SandboxViolation, Unwatched,
};

use super::*;
use crate::bash::output;

#[derive(Default)]
struct Observed {
    cleanup_allowed: AtomicBool,
    exited: AtomicBool,
    dropped: AtomicBool,
    stops: AtomicUsize,
}

struct Process {
    observed: Arc<Observed>,
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
        Ok(self.observed.exited.load(Ordering::Relaxed).then(exited))
    }

    fn stop(&mut self) -> io::Result<()> {
        self.observed.stops.fetch_add(1, Ordering::Relaxed);
        if self.observed.cleanup_allowed.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(io::Error::other("synthetic cleanup failure"))
        }
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

impl Drop for Process {
    fn drop(&mut self) {
        self.observed.dropped.store(true, Ordering::Relaxed);
    }
}

fn exited() -> ExitStatus {
    #[cfg(unix)]
    use std::os::unix::process::ExitStatusExt as _;
    #[cfg(windows)]
    use std::os::windows::process::ExitStatusExt as _;

    ExitStatus::from_raw(0)
}

fn keep(left: &Background, observed: &Arc<Observed>, accepting: bool) -> Kept {
    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("background-test").expect("backend name"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("backend identity");
    let root = std::env::current_dir().expect("working directory");
    let rule = SandboxFilesystemRule::new(
        &root,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .expect("workspace rule");
    let policy = SandboxPolicy::new(
        false,
        [rule],
        root,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("policy");
    let inspection = SandboxInspection::new(
        crucible_core::SandboxId::new(),
        identity,
        SandboxCapabilities::none(),
        &policy,
        &SandboxManifest::empty(),
        false,
        Some("synthetic cleanup fixture"),
        SandboxCleanup::Pending,
    )
    .expect("inspection");
    let process = Process {
        observed: Arc::clone(observed),
        inspection,
    };
    let taken = output::collect(
        Box::new(process),
        &output::Waiting {
            allowed: Duration::from_secs(1),
            cancel: &Cancel::new(),
            watch: &Unwatched,
            leaving: Some(output::Leaving {
                left,
                after: Some(Duration::ZERO),
            }),
        },
    )
    .expect("immediate background handover");
    let output::Left::Running(taking) = taken else {
        panic!("fixture must enter the registry before observing exit");
    };
    left.keep(
        taking,
        Keep {
            called: "synthetic command",
            said: "cleanup ownership",
            lease: None,
            accepting,
        },
    )
    .expect("registry capacity")
}

#[test]
fn failed_stop_keeps_the_process_and_its_capacity_until_cleanup_succeeds() {
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    let kept = keep(&left, &observed, false);
    let number = kept.number();
    for _ in 1..MOST {
        drop(keep(&left, &Arc::new(Observed::default()), false));
    }

    let _ = left.stop(number);
    assert_eq!(left.count(), MOST, "failed cleanup released capacity");
    assert!(!observed.dropped.load(Ordering::Relaxed));
    assert!(
        left.reserve().is_none(),
        "an uncleaned command still owns its slot"
    );
    assert!(left.running().iter().any(|entry| entry.number == number));
    let _ = left.stop(number);
    assert_eq!(
        observed.stops.load(Ordering::Relaxed),
        2,
        "cleanup was not retried"
    );

    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let _ = left.stop(number);
    assert_eq!(left.count(), MOST - 1);
    assert!(observed.dropped.load(Ordering::Relaxed));
    assert!(left.reserve().is_some());
    assert!(
        left.reported().is_empty(),
        "explicit stop is not a natural completion"
    );
}

#[test]
fn reaping_waits_for_cleanup_before_reporting_exactly_one_completion() {
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    let number = keep(&left, &observed, false).number();
    observed.exited.store(true, Ordering::Relaxed);

    assert!(
        left.reap().is_empty(),
        "failed cleanup was reported as complete"
    );
    assert!(left.reported().is_empty());
    assert_eq!(left.count(), 1);
    assert!(!observed.dropped.load(Ordering::Relaxed));
    assert!(left.reap().is_empty());

    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let ended = left.reap();
    assert_eq!(ended.len(), 1);
    assert_eq!(ended.first().map(|one| one.number), Some(number));
    assert_eq!(left.reported(), ended);
    assert!(left.reported().is_empty());
    assert!(left.reap().is_empty());
    assert!(observed.dropped.load(Ordering::Relaxed));
}

#[test]
fn abandoned_start_keeps_failed_cleanup_visible_and_retryable() {
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    let kept = keep(&left, &observed, true);
    let number = kept.number();
    drop(kept);

    assert_eq!(left.count(), 1, "abandoned start lost cleanup ownership");
    assert_eq!(
        left.running().first().map(|entry| entry.number),
        Some(number)
    );
    assert!(!observed.dropped.load(Ordering::Relaxed));
    observed.exited.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    assert_eq!(left.reap().len(), 1, "abandoned acceptance remained stuck");
}

#[test]
fn abandoned_receipt_keeps_failed_cleanup_visible_and_retryable() {
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    let kept = keep(&left, &observed, true);
    let number = kept.number();
    drop(kept.acceptance().expect("pending receipt"));

    assert_eq!(left.count(), 1, "abandoned receipt lost cleanup ownership");
    assert_eq!(
        left.running().first().map(|entry| entry.number),
        Some(number)
    );
    assert!(!observed.dropped.load(Ordering::Relaxed));
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let _ = left.stop(number);
    assert_eq!(left.count(), 0);
    assert!(left.reported().is_empty());
}
