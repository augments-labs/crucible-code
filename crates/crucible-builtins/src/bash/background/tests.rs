//! Failed cleanup retains the registry's only process owner and capacity.

use std::io;
use std::process::ExitStatus;
use std::sync::atomic::AtomicUsize;
use std::thread;

use crucible_runtime::Cancel;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCleanup, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxInspection, SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy,
    SandboxResourceLimits, SandboxUsage, SandboxViolation,
};
use crucible_tools::Unwatched;

use super::*;
use crate::bash::output;

#[derive(Default)]
struct Observed {
    cleanup_allowed: AtomicBool,
    exited: AtomicBool,
    dropped: AtomicBool,
    stops: AtomicUsize,
    /// It has ended, while its ending waits on something else: a look at it
    /// says nothing until `exited` does.
    ended: AtomicBool,
    /// Its ending went wrong, and a look at it says how.
    failed: AtomicBool,
    /// Whether it was asked to stop before its ending was complete, which
    /// discards what it wrote.
    stopped_early: AtomicBool,
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
        if self.observed.failed.load(Ordering::Relaxed) {
            return Err(io::Error::other(
                "writable root changed after the command started",
            ));
        }
        Ok(self.observed.exited.load(Ordering::Relaxed).then(exited))
    }

    fn ended(&mut self) -> bool {
        self.observed.ended.load(Ordering::Relaxed) || self.observed.exited.load(Ordering::Relaxed)
    }

    fn stop(&mut self) -> io::Result<()> {
        self.observed.stops.fetch_add(1, Ordering::Relaxed);
        if !self.observed.exited.load(Ordering::Relaxed) {
            self.observed.stopped_early.store(true, Ordering::Relaxed);
        }
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

fn process(observed: &Arc<Observed>) -> Process {
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
        crucible_types::SandboxId::new(),
        identity,
        SandboxCapabilities::none(),
        &policy,
        &SandboxManifest::empty(),
        false,
        Some("synthetic cleanup fixture"),
        SandboxCleanup::Pending,
    )
    .expect("inspection");
    Process {
        observed: Arc::clone(observed),
        inspection,
    }
}

fn keep(left: &Background, observed: &Arc<Observed>, accepting: bool) -> Kept {
    let process = process(observed);
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

/// Marks `observed` as having completed its ending once `after` has passed.
fn completes(observed: &Arc<Observed>, after: Duration) -> thread::JoinHandle<()> {
    let later = Arc::clone(observed);
    thread::spawn(move || {
        thread::sleep(after);
        later.exited.store(true, Ordering::Relaxed);
    })
}

#[test]
fn a_command_that_ended_in_time_is_not_stopped_while_its_ending_completes() {
    // A command that finished before its deadline can still be waiting for
    // another command's publication when the deadline passes. Stopping it then
    // would discard what it wrote and report that it ran too long, and neither
    // is true.
    let observed = Arc::new(Observed::default());
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let completing = completes(&observed, Duration::from_millis(300));

    let answered = output::collect(
        Box::new(process(&observed)),
        &output::Waiting {
            allowed: Duration::from_millis(50),
            cancel: &Cancel::new(),
            watch: &Unwatched,
            leaving: None,
        },
    );
    completing.join().expect("the ending completed");

    assert!(
        !observed.stopped_early.load(Ordering::Relaxed),
        "a command that ended in time was stopped before its ending completed"
    );
    let Ok(output::Left::Answered(report)) = answered else {
        panic!("a command that ended in time did not answer");
    };
    assert!(!report.text().contains("ran too long"), "{}", report.text());
}

#[test]
fn a_cancelled_turn_keeps_a_command_that_had_already_ended() {
    // The cancel stops what is still running. A command that has ended is not,
    // and stopping it while its ending waits its turn would discard what it
    // wrote after it had done its work.
    let observed = Arc::new(Observed::default());
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let cancel = Cancel::new();
    cancel.request();
    let completing = completes(&observed, Duration::from_millis(300));

    let answered = output::collect(
        Box::new(process(&observed)),
        &output::Waiting {
            allowed: Duration::from_secs(10),
            cancel: &cancel,
            watch: &Unwatched,
            leaving: None,
        },
    );
    completing.join().expect("the ending completed");

    assert!(
        !observed.stopped_early.load(Ordering::Relaxed),
        "a cancel discarded a command that had already ended"
    );
    assert!(
        matches!(answered, Ok(output::Left::Answered(_))),
        "a command that had ended lost its result to the cancel"
    );
}

#[test]
fn stopping_a_command_that_has_ended_leaves_it_to_be_reported() {
    // The panel draws from a list a beat old, and a command whose ending waits
    // its turn still stands on it. Stopping that one would discard what it
    // wrote, and it is about to be reported anyway.
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    let number = keep(&left, &observed, false).number();
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    left.stop(number)
        .expect("a command that has ended needs no stopping");

    assert!(
        !observed.stopped_early.load(Ordering::Relaxed),
        "stopping it discarded what it wrote"
    );
    assert_eq!(left.count(), 1, "it went without being reported");
    observed.exited.store(true, Ordering::Relaxed);
    let ended = left.reap();
    assert_eq!(ended.first().map(|one| one.number), Some(number));
}

#[test]
fn a_command_whose_ending_went_wrong_is_reported_once_with_why() {
    // Kept as though still running, it would stand on the panel forever and
    // the model would never hear that what it wrote was not published.
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    let number = keep(&left, &observed, false).number();
    observed.ended.store(true, Ordering::Relaxed);
    observed.failed.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    let ended = left.reap();

    assert_eq!(
        ended.len(),
        1,
        "a command whose ending went wrong was kept as though it still ran"
    );
    let one = ended.first().expect("one ending");
    assert_eq!(one.number, number);
    assert!(
        one.unpublished
            .as_deref()
            .is_some_and(|why| why.contains("writable root changed")),
        "{one:?}"
    );
    assert_eq!(left.count(), 0);
    assert!(left.reap().is_empty(), "reported twice");
    assert_eq!(left.reported(), ended);
}

#[test]
fn letting_the_registry_go_waits_for_a_command_that_has_ended() {
    // The end of a run is when a command left running is most likely to have
    // just finished. Ending it before its ending completes would discard what
    // it wrote on the way out.
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    drop(keep(&left, &observed, false));
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let completing = completes(&observed, Duration::from_millis(200));

    drop(left);
    completing.join().expect("the ending completed");

    assert!(
        observed.dropped.load(Ordering::Relaxed),
        "the registry kept it"
    );
    assert!(
        !observed.stopped_early.load(Ordering::Relaxed),
        "the registry ended a command before its ending completed"
    );
}

#[test]
fn a_command_whose_publication_never_finishes_is_stopped_once_its_patience_has_passed() {
    let observed = Arc::new(Observed::default());
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let process = process(&observed);
    let (told, hears) = std::sync::mpsc::channel();
    let waiting = thread::spawn(move || {
        let answered = output::collect(
            Box::new(process),
            &output::Waiting {
                allowed: Duration::from_millis(50),
                cancel: &Cancel::new(),
                watch: &Unwatched,
                leaving: None,
            },
        );
        told.send(match answered {
            Ok(output::Left::Answered(report)) => report.text().to_owned(),
            Ok(output::Left::Running(_)) => "handed over".to_owned(),
            Err(problem) => format!("error: {problem}"),
        })
        .expect("the test hears how it went");
    });

    let said = hears.recv_timeout(Duration::from_secs(20));

    let said = said.expect("the deadline's wait for a publication has a ceiling");
    waiting.join().expect("the waiting thread");
    assert!(said.contains("ran too long"), "{said}");
}

#[test]
fn a_cancelled_turn_stops_a_command_whose_publication_never_finishes() {
    let observed = Arc::new(Observed::default());
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let process = process(&observed);
    let (told, hears) = std::sync::mpsc::channel();
    let watching = Arc::clone(&observed);
    let waiting = thread::spawn(move || {
        let cancel = Cancel::new();
        cancel.request();
        let answered = output::collect(
            Box::new(process),
            &output::Waiting {
                allowed: Duration::from_secs(30),
                cancel: &cancel,
                watch: &Unwatched,
                leaving: None,
            },
        );
        told.send(match answered {
            Ok(_) => "answered".to_owned(),
            Err(problem) => format!("error: {problem}"),
        })
        .expect("the test hears how it went");
        drop(watching);
    });

    let said = hears.recv_timeout(Duration::from_secs(20));

    let said = said.expect("a cancel waits for a publication only so long");
    waiting.join().expect("the waiting thread");
    assert!(said.starts_with("error:"), "{said}");
    assert!(observed.stops.load(Ordering::Relaxed) > 0, "{said}");
}

#[test]
fn letting_the_registry_go_stops_waiting_once_its_patience_has_passed() {
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    drop(keep(&left, &observed, false));
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let (told, hears) = std::sync::mpsc::channel();
    let letting_go = thread::spawn(move || {
        drop(left);
        told.send(()).expect("the test hears it let go");
    });

    let went = hears.recv_timeout(Duration::from_secs(20));

    went.expect("the registry's wait for a publication has a ceiling");
    letting_go.join().expect("the dropping thread");
    assert!(observed.dropped.load(Ordering::Relaxed));
}

#[test]
fn letting_the_registry_go_ends_a_command_whose_ending_went_wrong() {
    let left = Background::new();
    let observed = Arc::new(Observed::default());
    drop(keep(&left, &observed, false));
    observed.ended.store(true, Ordering::Relaxed);
    observed.failed.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    drop(left);

    assert_eq!(observed.stops.load(Ordering::Relaxed), 1);
    assert!(observed.dropped.load(Ordering::Relaxed));
}
