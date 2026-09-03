//! What draining a confined process's standard error has to guarantee.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crate::{SandboxOutput, SandboxRead};

use super::{KEPT, Muttered};

/// The pause between polls a test drains with. Short, because a test that waits
/// in real milliseconds should wait as few of them as it can.
const PAUSE: Duration = Duration::from_millis(1);

/// How long a test waits for a thread it does not control to get somewhere.
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

/// Waits for `settled` to hold, so a test never races a thread it started.
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
    let muttered = Muttered::with_pause(stream, PAUSE);

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
    let muttered = Muttered::with_pause(stream, PAUSE);

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
    let muttered = Muttered::with_pause(stream, PAUSE);

    thread::sleep(PAUSE * 8);
    assert_eq!(muttered.text(), "");
}

#[test]
fn dropping_it_stops_the_drain() {
    // Never closes, so only the drop can end the thread.
    let (stream, asked) = says([]);
    let muttered = Muttered::with_pause(stream, PAUSE);
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
