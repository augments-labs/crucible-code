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
    SandboxRead, SandboxResourceLimits, SandboxUsage, SandboxViolation,
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
    /// How many times it was asked how it ended.
    looks: AtomicUsize,
    /// While set, every look at it waits until it is cleared: a process
    /// whose backend has stopped answering.
    looks_held: AtomicBool,
    /// While set, every stop of it waits until it is cleared.
    stops_held: AtomicBool,
    /// How many looks or stops have begun waiting on either.
    stalled: AtomicUsize,
}

impl Observed {
    /// Waits while `held` is set, counting the wait once it begins.
    fn stall(&self, held: &AtomicBool) {
        if !held.load(Ordering::Acquire) {
            return;
        }
        self.stalled.fetch_add(1, Ordering::AcqRel);
        while held.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// Lets every stalled look and stop go.
    fn release(&self) {
        self.looks_held.store(false, Ordering::Release);
        self.stops_held.store(false, Ordering::Release);
    }
}

struct Process {
    observed: Arc<Observed>,
    inspection: SandboxInspection,
    /// What its standard output reads as, where a test gives it one.
    stdout: Option<Box<dyn SandboxOutput>>,
}

impl SandboxProcess for Process {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        None
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.observed.stall(&self.observed.looks_held);
        self.observed.looks.fetch_add(1, Ordering::Relaxed);
        if self.observed.failed.load(Ordering::Relaxed) {
            return Err(io::Error::other(
                "writable root changed after the command started",
            ));
        }
        Ok(self.observed.exited.load(Ordering::Relaxed).then(exited))
    }

    fn ended(&mut self) -> bool {
        self.observed.stall(&self.observed.looks_held);
        self.observed.ended.load(Ordering::Relaxed) || self.observed.exited.load(Ordering::Relaxed)
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            self.observed.stall(&self.observed.stops_held);
            self.observed.stops.fetch_add(1, Ordering::Relaxed);
            if !self.observed.exited.load(Ordering::Relaxed) {
                self.observed.stopped_early.store(true, Ordering::Relaxed);
            }
            if self.observed.cleanup_allowed.load(Ordering::Relaxed) {
                // A stop that confirms the scope ended has reaped the leader, so a
                // look after it answers, the way the real one does.
                self.observed.exited.store(true, Ordering::Relaxed);
                Ok(())
            } else {
                Err(io::Error::other("synthetic cleanup failure"))
            }
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
        stdout: None,
    }
}

fn keep(left: &Background, observed: &Arc<Observed>, accepting: bool) -> Kept {
    keeping(left, process(observed), accepting)
}

fn keeping(left: &Background, process: Process, accepting: bool) -> Kept {
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
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let kept = keep(&left, &observed, false);
    let number = kept.number();
    for _ in 1..MOST {
        drop(keep(&left, &Arc::new(Observed::default()), false));
    }

    left.stop(number).expect("the stop was asked for");
    waiting_until("the refused stop", || refused(&left, number));
    assert_eq!(left.count(), MOST, "failed cleanup released capacity");
    assert!(!observed.dropped.load(Ordering::Relaxed));
    assert!(
        left.reserve().is_none(),
        "an uncleaned command still owns its slot"
    );
    assert!(left.running().iter().any(|entry| entry.number == number));
    left.stop(number).expect("the stop was asked for again");
    waiting_until("the retried stop, refused", || {
        observed.stops.load(Ordering::Relaxed) >= 2 && refused(&left, number)
    });
    assert_eq!(
        observed.stops.load(Ordering::Relaxed),
        2,
        "cleanup was not retried"
    );

    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    left.stop(number)
        .expect("the stop was asked for a third time");
    waiting_until("the stopped command's entry going", || {
        left.count() == MOST - 1
    });
    assert!(observed.dropped.load(Ordering::Relaxed));
    assert!(left.reserve().is_some());
    assert!(
        left.reported().is_empty(),
        "explicit stop is not a natural completion"
    );
}

#[test]
fn reaping_waits_for_cleanup_before_reporting_exactly_one_completion() {
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let number = keep(&left, &observed, false).number();
    observed.exited.store(true, Ordering::Relaxed);
    // Its owner has asked for what it left running to end, and been refused,
    // more than once.
    waiting_until("two refused stops", || {
        observed.stops.load(Ordering::Relaxed) >= 2
    });

    assert!(
        left.reap().is_empty(),
        "failed cleanup was reported as complete"
    );
    assert!(left.reported().is_empty());
    assert_eq!(left.count(), 1);
    assert!(!observed.dropped.load(Ordering::Relaxed));
    assert!(left.reap().is_empty());

    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let ended = vec![reaped(&left)];
    assert_eq!(ended.len(), 1);
    assert_eq!(ended.first().map(|one| one.number), Some(number));
    assert_eq!(left.reported(), ended);
    assert!(left.reported().is_empty());
    assert!(left.reap().is_empty());
    assert!(observed.dropped.load(Ordering::Relaxed));
}

#[test]
fn abandoned_start_keeps_failed_cleanup_visible_and_retryable() {
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let kept = keep(&left, &observed, true);
    let number = kept.number();
    drop(kept);
    waiting_until("the abandoned start's refused stop", || {
        observed.stops.load(Ordering::Relaxed) >= 1
    });

    assert_eq!(left.count(), 1, "abandoned start lost cleanup ownership");
    assert_eq!(
        left.running().first().map(|entry| entry.number),
        Some(number)
    );
    assert!(!observed.dropped.load(Ordering::Relaxed));
    observed.exited.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    assert_eq!(
        reaped(&left).number,
        number,
        "abandoned acceptance remained stuck"
    );
}

#[test]
fn an_abandoned_result_ends_its_command_though_it_has_ended_and_reports_nothing() {
    // Nothing recorded that the command was started, so nothing may report
    // that it ended: it is ended, what it wrote with it, and it goes.
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    drop(keep(&left, &observed, true));

    waiting_until("the abandoned command's entry going", || left.count() == 0);

    assert_eq!(observed.stops.load(Ordering::Relaxed), 1);
    assert!(observed.dropped.load(Ordering::Relaxed));
    assert!(left.reap().is_empty(), "an abandoned command was reported");
    assert!(left.reported().is_empty());
}

#[test]
fn abandoned_receipt_keeps_failed_cleanup_visible_and_retryable() {
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let kept = keep(&left, &observed, true);
    let number = kept.number();
    drop(kept.acceptance().expect("pending receipt"));
    waiting_until("the abandoned receipt's refused stop", || {
        observed.stops.load(Ordering::Relaxed) >= 1
    });

    assert_eq!(left.count(), 1, "abandoned receipt lost cleanup ownership");
    assert_eq!(
        left.running().first().map(|entry| entry.number),
        Some(number)
    );
    assert!(!observed.dropped.load(Ordering::Relaxed));
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    left.stop(number).expect("the stop was asked for");
    waiting_until("the stopped command's entry going", || left.count() == 0);
    assert!(left.reported().is_empty());
}

/// Marks `observed` as having completed its ending once `after` has passed.
///
/// `after` sits in a band: longer than whatever deadline the test wants to watch
/// pass first, and well short of the publication ceiling the code under test
/// allows — `PUBLICATION` in test builds, and `CANCELLATION` for a cancel. A sleep only ever
/// overshoots, so the lower end holds by construction and only the ceiling can be
/// lost. Timing an ending to land *at* that ceiling is what made one of these fail
/// on one loaded runner while passing on three others, so leave the slack in.
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
    let completing = completes(&observed, Duration::from_millis(100));

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
    // wrote after it had done its work. Its ending completes inside the cancel's
    // own patience, which is shorter than the deadline's because somebody is
    // waiting for the turn to end; how long that patience is belongs to
    // `a_cancel_waits_less_for_a_publication_than_a_deadline_does`.
    let observed = Arc::new(Observed::default());
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let cancel = Cancel::new();
    cancel.request();
    let completing = completes(&observed, Duration::from_millis(20));

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
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let number = keep(&left, &observed, false).number();
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    left.stop(number)
        .expect("a command that has ended needs no stopping");
    waiting_until("the owner taking up the stop", || !asked(&left, number));

    assert!(
        !observed.stopped_early.load(Ordering::Relaxed),
        "stopping it discarded what it wrote"
    );
    assert_eq!(left.count(), 1, "it went without being reported");
    observed.exited.store(true, Ordering::Relaxed);
    assert_eq!(reaped(&left).number, number);
}

#[test]
fn a_command_whose_ending_went_wrong_is_reported_once_with_why() {
    // Kept as though still running, it would stand on the panel forever and
    // the model would never hear that what it wrote was not published.
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let number = keep(&left, &observed, false).number();
    observed.ended.store(true, Ordering::Relaxed);
    observed.failed.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    let ended = vec![reaped(&left)];

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
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    drop(keep(&left, &observed, false));
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let completing = completes(&observed, Duration::from_millis(100));

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
    // The ceiling discarded what it wrote. A command told only that it ran too
    // long is told the opposite of what happened: it finished, and lost its files.
    assert!(said.contains("nothing it wrote was published"), "{said}");
}

#[test]
fn a_kept_command_that_cannot_publish_is_reported_once_its_patience_has_passed() {
    // Before the ceiling this test is named for, nothing else ended it: the
    // panel's stop leaves anything that has ended to be reported, and only the
    // registry's drop ever looked again, so it would keep one of the slots for
    // the rest of the run and say nothing. The assertions below are what
    // happens now.
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    drop(keep(&left, &observed, false));
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    let one = reaped(&left);

    assert!(one.unpublished.is_some(), "{one:?}");
    assert_eq!(left.count(), 0, "it kept its slot");
}

#[test]
fn a_publication_that_never_finishes_keeps_its_reason_after_end_stops_it() {
    // The fake's own `stop`, like the real backend's, resolves the process as
    // exited on success. What reap decided on the beat that called `end` —
    // that nothing it wrote was published — must still be what is reported,
    // not a status a later beat reads back off the very leader `end` itself
    // just resolved.
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let mut printing = process(&observed);
    printing.stdout = Some(Box::new(Printing {
        said: None,
        then: || Ok(SandboxRead::Pending),
        told: None,
    }));
    drop(keeping(&left, printing, false));
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    let one = reaped(&left);

    assert_eq!(one.code, None, "{one:?}");
    assert_eq!(
        one.unpublished.as_deref(),
        Some("its publication did not finish in time"),
        "{one:?}"
    );
}

#[test]
fn a_cancel_waits_less_for_a_publication_than_a_deadline_does() {
    // A deadline's patience is a machine's; a cancel's is a person's, who is
    // waiting for the turn to end.
    let observed = Arc::new(Observed::default());
    observed.ended.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    let process = process(&observed);
    let (told, hears) = std::sync::mpsc::channel();
    let waiting = thread::spawn(move || {
        let cancel = Cancel::new();
        cancel.request();
        let began = Instant::now();
        let answered = output::collect(
            Box::new(process),
            &output::Waiting {
                allowed: Duration::from_secs(30),
                cancel: &cancel,
                watch: &Unwatched,
                leaving: None,
            },
        );
        told.send((
            began.elapsed(),
            match answered {
                Ok(_) => "answered".to_owned(),
                Err(problem) => format!("error: {problem}"),
            },
        ))
        .expect("the test hears how it went");
    });

    let (took, said) = hears
        .recv_timeout(Duration::from_secs(20))
        .expect("a cancel answers");

    waiting.join().expect("the waiting thread");
    assert!(said.contains("cancelled"), "{said}");
    // Held to the two ceilings themselves rather than a figure between them. A wait
    // only ever overshoots, so a cancel that took the deadline's patience is at
    // least that long every time, and one that took none is shorter than its own.
    // A midpoint left a correct cancel half the room, and a loaded runner used it.
    assert!(
        took >= output::CANCELLATION,
        "a cancel gave up before its own patience: {took:?}"
    );
    assert!(
        took < output::PUBLICATION,
        "a cancel waited the deadline's ceiling: {took:?}"
    );
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
    // It had ended, and the stop discarded what it wrote. Answered as a
    // cancellation, the runner tells the model the call was not run at all —
    // which is false of a command that finished and lost its files.
    assert!(said.contains("nothing it wrote was published"), "{said}");
    assert!(observed.stops.load(Ordering::Relaxed) > 0, "{said}");
}

#[test]
fn letting_the_registry_go_stops_waiting_once_its_patience_has_passed() {
    let runtime = runtime();
    let left = registry(&runtime);
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
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    drop(keep(&left, &observed, false));
    observed.ended.store(true, Ordering::Relaxed);
    observed.failed.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    drop(left);

    assert_eq!(observed.stops.load(Ordering::Relaxed), 1);
    assert!(observed.dropped.load(Ordering::Relaxed));
    // Left at once rather than waited out: an ending that went wrong is an
    // answer, and the patience is for one that has not answered yet.
    let looks = observed.looks.load(Ordering::Relaxed);
    assert!(
        looks <= 5,
        "the registry kept asking a command that had answered: {looks}"
    );
}

/// A standard output that hands over `said` and then ends the way `then` says,
/// telling `told` once it has.
struct Printing {
    said: Option<&'static [u8]>,
    then: fn() -> io::Result<SandboxRead>,
    told: Option<std::sync::mpsc::Sender<()>>,
}

impl SandboxOutput for Printing {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        let Some(said) = self.said.take() else {
            let ending = (self.then)();
            if let Some(told) = self.told.take() {
                let _ = told.send(());
            }
            return ending;
        };
        let read = said.len().min(buffer.len());
        buffer
            .get_mut(..read)
            .expect("room for what it says")
            .copy_from_slice(said.get(..read).expect("what it says"));
        Ok(SandboxRead::Bytes(read))
    }
}

/// What reaping reports of a background command that printed `said` on its
/// standard output, whose reading then ended the way `then` says, and which then
/// exited.
///
/// The command exits only once its reader has met that ending. Reaping reports
/// an ending whose readers are still going once a short grace has passed, and a
/// reader starved past it would be stopped before it met the ending under test.
fn ended_after_printing(said: &'static [u8], then: fn() -> io::Result<SandboxRead>) -> Ended {
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let (told, heard) = std::sync::mpsc::channel();
    let mut printing = process(&observed);
    printing.stdout = Some(Box::new(Printing {
        said: Some(said),
        then,
        told: Some(told),
    }));
    drop(keeping(&left, printing, false));
    heard
        .recv_timeout(Duration::from_secs(20))
        .expect("the reader met the ending under test");
    observed.exited.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    reaped(&left)
}

#[test]
fn a_background_command_whose_output_read_failed_says_it_is_incomplete() {
    // A reader that failed part-way stops the way one that reached the end
    // does. Reported as it stood, the model reads the part as the whole.
    let one = ended_after_printing(b"compiled 3 of 7 crates", || {
        Err(io::Error::other("synthetic read failure canary 5f3a"))
    });

    assert!(
        one.printed.starts_with("compiled 3 of 7 crates"),
        "{:?}",
        one.printed
    );
    assert!(
        one.printed
            .contains("[output is incomplete: reading it failed before the end]"),
        "a reader that failed left output that looks complete: {:?}",
        one.printed
    );
    assert!(
        !one.printed.contains("canary"),
        "the read failure's own words reached the note: {:?}",
        one.printed
    );
}

/// Whether the command running as `number` has had both its readers reach the
/// end of their pipes. Holds the registry lock only for the lookup, and reads
/// `false` for a number with no entry — so a test can wait on the fact itself
/// instead of racing the beat that would otherwise report it.
fn drained(left: &Background, number: usize) -> bool {
    left.standing.lock().is_ok_and(|standing| {
        standing
            .left
            .iter()
            .find(|entry| entry.number == number)
            .is_some_and(Left::drained)
    })
}

#[test]
fn a_background_command_whose_output_was_read_to_the_end_says_only_what_it_printed() {
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let (told, heard) = std::sync::mpsc::channel();
    let mut printing = process(&observed);
    printing.stdout = Some(Box::new(Printing {
        said: Some(b"compiled 7 of 7 crates\n"),
        then: || Ok(SandboxRead::End),
        told: Some(told),
    }));
    let kept = keeping(&left, printing, false);
    let number = kept.number();
    drop(kept);
    heard
        .recv_timeout(Duration::from_secs(20))
        .expect("the reader met the ending under test");

    // `told` fires as the reader decides to end, a moment before its thread
    // has actually finished. Waited out here, rather than left to the grace
    // `reap` gives a reader that has not: that grace is what the other two
    // tests below mean to exercise, and this one would otherwise pass or fail
    // on how much of it a loaded runner left to close that moment's gap.
    let deadline = Instant::now() + Duration::from_secs(20);
    while !drained(&left, number) {
        assert!(
            Instant::now() < deadline,
            "the reader never reached the end"
        );
        thread::sleep(Duration::from_millis(10));
    }

    observed.exited.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    let one = reaped(&left);

    assert_eq!(&*one.printed, "compiled 7 of 7 crates");
    assert_eq!(one.lines, 1);
}

#[test]
fn a_background_command_whose_reader_never_reached_the_end_says_it_is_incomplete() {
    // Something the command started keeps its pipe from ever showing an end,
    // the way a held-open grandchild's would. Every hold on reporting its
    // ending runs out rather than waiting forever, and what it kept must not
    // then read as the whole.
    let one = ended_after_printing(b"still building", || Ok(SandboxRead::Pending));

    assert_eq!(
        &*one.printed,
        format!("still building{BEFORE_NOTE}{UNDRAINED}")
    );
}

/// A standard output that hands over `said`, then parks on [`SandboxRead::Pending`]
/// — signalling once, on `parked`, the first time it does — until both
/// `observed` shows `end` has stopped this command and the test has set
/// `let_go`, and only then hands over `rest` before ending: the shape of a pipe
/// a descendant kept open past the shell's own exit, which `end` is what makes
/// let go. The second gate lets the test hold the release back until the
/// reader has parked.
struct Releasing {
    said: Option<&'static [u8]>,
    observed: Arc<Observed>,
    let_go: Arc<AtomicBool>,
    rest: Option<&'static [u8]>,
    parked: Option<std::sync::mpsc::Sender<()>>,
}

impl SandboxOutput for Releasing {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if let Some(said) = self.said.take() {
            let read = said.len().min(buffer.len());
            buffer
                .get_mut(..read)
                .expect("room for what it says")
                .copy_from_slice(said.get(..read).expect("what it says"));
            return Ok(SandboxRead::Bytes(read));
        }
        if self.observed.stops.load(Ordering::Relaxed) == 0 || !self.let_go.load(Ordering::Relaxed)
        {
            if let Some(parked) = self.parked.take() {
                let _ = parked.send(());
            }
            return Ok(SandboxRead::Pending);
        }
        let Some(rest) = self.rest.take() else {
            return Ok(SandboxRead::End);
        };
        let read = rest.len().min(buffer.len());
        buffer
            .get_mut(..read)
            .expect("room for what it says")
            .copy_from_slice(rest.get(..read).expect("what it says"));
        Ok(SandboxRead::Bytes(read))
    }
}

#[test]
fn a_background_command_whose_reader_catches_up_once_stopped_says_the_whole() {
    // A grandchild can keep a pipe open past the shell's own exit; `end` is
    // what is expected to make it let go. The bytes that then arrive are
    // still worth keeping, not losing to a check made before the reader had
    // caught up with them.
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let let_go = Arc::new(AtomicBool::new(false));
    let (parked, settled) = std::sync::mpsc::channel();
    let mut printing = process(&observed);
    printing.stdout = Some(Box::new(Releasing {
        said: Some(b"still building"),
        observed: Arc::clone(&observed),
        let_go: Arc::clone(&let_go),
        rest: Some(b": done"),
        parked: Some(parked),
    }));
    let kept = keeping(&left, printing, false);
    let number = kept.number();
    drop(kept);
    settled
        .recv_timeout(Duration::from_secs(20))
        .expect("the reader parked waiting for the release");
    // Released the moment `end` has stopped the command and not before: the
    // reader then catches up within its own pause, well inside the grace its
    // owner gives it from the stop. An owner that judged the pipe before that
    // grace would report the marker instead.
    let_go.store(true, Ordering::Relaxed);
    observed.exited.store(true, Ordering::Relaxed);
    observed.cleanup_allowed.store(true, Ordering::Relaxed);

    let one = reaped(&left);

    assert_eq!(one.number, number);
    assert_eq!(&*one.printed, "still building: done");
}

/// A runtime of this test's own for the registry to own its commands on, so a
/// command a test leaves stalled holds none of another test's workers.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_time()
        .build()
        .expect("a runtime to own commands on")
}

/// A registry whose commands are owned on `runtime`.
fn registry(runtime: &tokio::runtime::Runtime) -> Background {
    let left = Background::new();
    left.watching_on(runtime.handle().clone());
    left
}

/// Waits, up to a ceiling no passing run comes near, until `until` holds.
fn waiting_until(what: &str, until: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !until() {
        assert!(Instant::now() < deadline, "{what} never happened");
        thread::sleep(Duration::from_millis(1));
    }
}

/// How long the thread that draws may wait for an answer about what is
/// running: far above taking a lock and copying a few rows, on a loaded runner
/// too, and far below a process that never answers.
const DRAWN: Duration = Duration::from_millis(500);

#[test]
fn what_is_running_is_answered_while_its_process_stalls() {
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let number = keep(&left, &observed, false).number();
    observed.looks_held.store(true, Ordering::Release);
    observed.stops_held.store(true, Ordering::Release);
    waiting_until("a stalled look", || {
        observed.stalled.load(Ordering::Acquire) > 0
    });

    // Every question the thread that draws asks between frames, and the key
    // that asks for the command to stop, while everything the process is
    // asked stalls.
    let (answered, heard) = std::sync::mpsc::channel();
    let drawing = {
        let left = left.clone();
        thread::spawn(move || {
            let _ = answered.send((
                left.count(),
                left.running().len(),
                left.reap().len(),
                left.wrote(number).is_some(),
                left.reported().len(),
                left.stop(number).is_ok(),
            ));
        })
    };
    let drawn = heard.recv_timeout(DRAWN);

    observed.release();
    drawing.join().expect("the drawing thread");
    drop(left);
    runtime.shutdown_timeout(Duration::from_secs(5));

    assert_eq!(
        drawn.expect("the thread that draws waited on a process that stalled"),
        (1, 1, 0, true, 0, true)
    );
}

/// Waits, up to `patience`, until `until` holds, and says whether it did.
fn within(patience: Duration, until: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + patience;
    while !until() {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
    true
}

#[test]
fn the_runtime_goes_on_while_every_command_left_running_stalls() {
    // Fewer workers than there may be commands left running, and room for
    // each command's owner on the blocking threads with one to spare: an
    // owner that asked its process anything on a worker would hold it, and
    // with it every timer the runtime drives — a sandbox's limit kill among
    // them.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(MOST + 1)
        .enable_time()
        .build()
        .expect("a runtime to own commands on");
    let left = registry(&runtime);
    let observed: Vec<Arc<Observed>> = (0..MOST).map(|_| Arc::new(Observed::default())).collect();
    for one in &observed {
        drop(keep(&left, one, false));
        one.looks_held.store(true, Ordering::Release);
    }
    let every = within(Duration::from_secs(5), || {
        observed
            .iter()
            .all(|one| one.stalled.load(Ordering::Acquire) > 0)
    });

    let (fired, heard) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        let _ = fired.send(());
    });
    let timely = heard.recv_timeout(Duration::from_secs(1));

    for one in &observed {
        one.release();
    }
    drop(left);
    runtime.shutdown_timeout(Duration::from_secs(5));

    timely.expect("a timer on the runtime waited on commands whose processes stalled");
    assert!(
        every,
        "not every command's owner was asking its process at once"
    );
}

#[test]
fn a_command_whose_owner_never_ran_is_ended_when_the_registry_goes() {
    // A runtime nothing drives: the owner is spawned and never polled, as one
    // queued behind workers that never come free.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("a runtime nothing drives");
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    observed.cleanup_allowed.store(true, Ordering::Relaxed);
    drop(keep(&left, &observed, false));

    drop(left);

    assert_eq!(
        observed.stops.load(Ordering::Relaxed),
        1,
        "a command whose owner never ran outlived the registry"
    );
    drop(runtime);
}

#[test]
fn an_owner_leaves_the_process_to_a_result_still_being_accepted() {
    // An acceptance borrows the process to bind its receipt. An owner that
    // took it out on its own looks meanwhile would make the acceptance fail,
    // and a failed acceptance ends the command.
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    let kept = keep(&left, &observed, true);
    let cell = left
        .standing
        .lock()
        .ok()
        .and_then(|standing| standing.left.first().map(|one| Arc::clone(&one.process)))
        .expect("the kept command's process");

    // Watched across many of its owner's looks.
    let until = Instant::now() + 20 * super::super::TICK;
    let mut borrowed = 0_usize;
    while Instant::now() < until {
        if cell.lock().is_ok_and(|held| held.is_none()) {
            borrowed += 1;
        }
    }

    drop(kept);
    drop(left);
    runtime.shutdown_timeout(Duration::from_secs(5));

    assert_eq!(
        borrowed, 0,
        "the owner took the process from under a result still being accepted"
    );
}

#[test]
fn letting_the_registry_go_is_bounded_while_its_stops_stall() {
    let runtime = runtime();
    let left = registry(&runtime);
    let observed = Arc::new(Observed::default());
    drop(keep(&left, &observed, false));
    observed.stops_held.store(true, Ordering::Release);

    let (went, heard) = std::sync::mpsc::channel();
    let letting_go = thread::spawn(move || {
        let began = Instant::now();
        drop(left);
        let _ = went.send(began.elapsed());
    });
    let took = heard.recv_timeout(LEAVING + Duration::from_secs(5));

    observed.release();
    letting_go.join().expect("the dropping thread");
    runtime.shutdown_timeout(Duration::from_secs(5));

    let took = took.expect("letting the registry go waited on a stop that stalled");
    assert!(took < LEAVING + Duration::from_secs(1), "{took:?}");
    assert!(
        observed.stalled.load(Ordering::Acquire) > 0,
        "nothing asked the process to end"
    );
}

/// The one ending the registry reports, waited for up to a ceiling no passing
/// run comes near: its owner finds it on the runtime, a tick at a time.
fn reaped(left: &Background) -> Ended {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(one) = left.reap().into_iter().next() {
            return one;
        }
        assert!(Instant::now() < deadline, "the command was never reported");
        thread::sleep(Duration::from_millis(10));
    }
}

/// Whether the last stop asked for the command running as `number` was
/// refused.
fn refused(left: &Background, number: usize) -> bool {
    left.running()
        .iter()
        .any(|standing| standing.number == number && standing.refused)
}

/// Whether a stop asked for the command running as `number` is still waiting
/// for its owner to take it up.
fn asked(left: &Background, number: usize) -> bool {
    left.standing.lock().is_ok_and(|standing| {
        standing
            .left
            .iter()
            .any(|entry| entry.number == number && entry.asks.stop.load(Ordering::Acquire))
    })
}
