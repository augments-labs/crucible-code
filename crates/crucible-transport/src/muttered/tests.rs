//! What draining a confined process's standard error has to guarantee.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crucible_sandbox::{SandboxOutput, SandboxRead};

use super::{KEPT, Muttered};

/// How long a test sleeps between looks at something it is waiting for. Short,
/// because a test that waits in real milliseconds should wait as few of them as
/// it can.
const PAUSE: Duration = Duration::from_millis(1);

/// How long a test waits for a task it does not control to get somewhere.
const LATEST: Duration = Duration::from_secs(2);

/// One thing a stream does when it is asked what it has.
enum Step {
    /// It has these bytes.
    Says(Vec<u8>),
    /// It has nothing yet.
    Waits,
    /// Its writer has gone.
    Closes,
}

/// A stream that does what it was told to, then goes quiet forever.
struct Says {
    /// What is left to do.
    steps: VecDeque<Step>,
    /// How many times it has been asked.
    asked: Arc<AtomicUsize>,
}

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        match self.steps.pop_front() {
            None | Some(Step::Waits) => Ok(SandboxRead::Pending),
            Some(Step::Closes) => Ok(SandboxRead::End),
            Some(Step::Says(mut bytes)) => {
                let taken = bytes.len().min(buffer.len());
                if let Some((into, from)) = buffer.get_mut(..taken).zip(bytes.get(..taken)) {
                    into.copy_from_slice(from);
                }
                // A pipe hands over what fits and keeps the rest, so this does
                // too; a fake that dropped the remainder would make a bound
                // this test is about look like it had never been reached.
                bytes.drain(..taken);
                if !bytes.is_empty() {
                    self.steps.push_front(Step::Says(bytes));
                }
                Ok(SandboxRead::Bytes(taken))
            }
        }
    }
}

fn says(steps: impl IntoIterator<Item = Step>) -> (Says, Arc<AtomicUsize>) {
    let asked = Arc::new(AtomicUsize::new(0));
    (
        Says {
            steps: steps.into_iter().collect(),
            asked: Arc::clone(&asked),
        },
        asked,
    )
}

/// Waits for `settled` to hold, so a test never races a task it started.
fn until(mut settled: impl FnMut() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < LATEST {
        if settled() {
            return true;
        }
        thread::sleep(PAUSE);
    }
    settled()
}

#[test]
fn what_a_process_writes_to_standard_error_is_kept() {
    let (stream, _) = says([
        Step::Says(b"loader: libfoo.so not found\n".to_vec()),
        Step::Says(b"giving up\n".to_vec()),
        Step::Closes,
    ]);
    let muttered = Muttered::draining(stream, crate::testing::runtime());

    assert!(
        until(|| muttered.text().contains("giving up")),
        "both lines should arrive: {:?}",
        muttered.text()
    );
    assert_eq!(muttered.text(), "loader: libfoo.so not found\ngiving up\n");
}

#[test]
fn a_talkative_process_is_bounded_and_told_on() {
    let over = KEPT + 500;
    let (stream, _) = says([Step::Says(vec![b'x'; over]), Step::Closes]);
    let muttered = Muttered::draining(stream, crate::testing::runtime());

    assert!(
        until(|| muttered.text().contains("dropped")),
        "the bound should be reported: {:?}",
        muttered.text()
    );
    let said = muttered.text();
    let kept = said.split('\n').next().expect("first line");
    assert_eq!(kept.len(), KEPT, "no more than the bound is kept");
    assert!(
        said.contains("500 further bytes were dropped"),
        "the exact overflow is named: {said:?}"
    );
}

#[test]
fn a_quiet_process_leaves_nothing_behind() {
    let (stream, _) = says([Step::Waits, Step::Waits, Step::Closes]);
    let muttered = Muttered::draining(stream, crate::testing::runtime());

    thread::sleep(PAUSE * 8);
    assert_eq!(muttered.text(), "");
}

#[test]
fn dropping_it_stops_the_drain() {
    // Never closes, so only the drop can end the task.
    let (stream, asked) = says([]);
    let muttered = Muttered::draining(stream, crate::testing::runtime());
    assert!(
        until(|| asked.load(Ordering::Relaxed) > 2),
        "the drain should be polling"
    );

    drop(muttered);
    // One more poll may already be under way; after that there are none.
    assert!(
        until(|| {
            let seen = asked.load(Ordering::Relaxed);
            thread::sleep(PAUSE * 8);
            asked.load(Ordering::Relaxed) == seen
        }),
        "the drain should have stopped asking"
    );
}

/// What a flooding standard error saw of the drain reading it.
#[derive(Default)]
struct Seen {
    /// Reads made on a thread that is not the runtime's.
    elsewhere: AtomicUsize,
    /// Whether the stream has been let go of.
    released: std::sync::atomic::AtomicBool,
}

/// A standard error that never stops talking and never ends.
///
/// Always ready, so nothing about the stream itself ever pauses the drain: the
/// only things that can end it are the drain's own bound and its owner.
struct Floods(Arc<Seen>);

impl SandboxOutput for Floods {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if !crate::testing::on_runtime() {
            self.0.elsewhere.fetch_add(1, Ordering::Relaxed);
        }
        buffer.fill(b'x');
        Ok(SandboxRead::Bytes(buffer.len()))
    }
}

impl Drop for Floods {
    fn drop(&mut self) {
        self.0.released.store(true, Ordering::Relaxed);
    }
}

/// The flood a talkative program makes is drained so that it cannot fill its
/// pipe, bounded so that it cannot fill crucible, and ended with its owner so
/// that it does not outlive the program it belongs to. All three by work on
/// the runtime the transport was handed: a thread of the transport's own is
/// one nobody owns once its value is gone.
#[test]
fn a_flood_on_standard_error_is_drained_by_owned_work_and_ends_with_it() {
    let seen = Arc::new(Seen::default());
    let muttered = Muttered::draining(Floods(Arc::clone(&seen)), crate::testing::runtime());

    assert!(
        until(|| muttered.text().contains("further bytes were dropped")),
        "a flood has to be drained past the bound, and the bound reported: {:?}",
        muttered.text().len()
    );
    let said = muttered.text();
    assert_eq!(
        said.split('\n').next().map(str::len),
        Some(KEPT),
        "no more than the bound is kept however long the flood goes on"
    );

    drop(muttered);

    assert!(
        until(|| seen.released.load(Ordering::Relaxed)),
        "dropping it has to end the drain and let the stream go, flood or not"
    );
    assert_eq!(
        seen.elsewhere.load(Ordering::Relaxed),
        0,
        "every read has to be made by work on the runtime the transport was handed, \
         never by a thread of the transport's own"
    );
}
