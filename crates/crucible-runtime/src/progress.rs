//! What a task says while it is still running.
//!
//! A task that streams — a command printing, a provider answering a token at a
//! time — produces faster than anything reading it, and for longer than
//! anything reading it wants to keep. So this is bounded, and the bound is the
//! whole point: an unbounded buffer here is a session-sized second copy of the
//! transcript, and a full one that blocked its writer is a renderer that can
//! stop a shutdown.
//!
//! There are two ceilings because a line count is not a size. A thousand lines
//! of a build log and a thousand lines of a minified bundle are the same number
//! and not the same amount of memory, and it is the bytes that run a machine
//! out. So a buffer is given both, and whichever binds first is the one that
//! holds.
//!
//! It never blocks and never refuses. When it is full it drops the *oldest*
//! line, because what a reader wants from a stream they have fallen behind is
//! its end, not its beginning — and it counts every line it dropped, so
//! [`Progress::take`] hands back a number rather than a gap. Truncation that
//! does not say it truncated is the one outcome this must not have: the reader
//! is looking at less than happened and has no way to know it. That is also
//! why [`Progress::any`] answers yes to a buffer holding nothing but a count:
//! a loss nobody is shown is a loss nobody knows about.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A shared "here is how it is going" buffer, bounded.
///
/// Cloning shares the buffer rather than copying it, so a clone handed to the
/// task's thread fills the one the reader drains.
#[derive(Clone)]
pub struct Progress(Arc<Mutex<Bounded>>);

/// By hand, for the reason [`crate::Aside`]'s is: these are the lines a
/// command printed, and a command prints tokens and paths. How many are
/// waiting is the whole of what a reader of a `{:?}` needs.
impl std::fmt::Debug for Progress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let bounded = self.bounded();

        f.debug_struct("Progress")
            .field("held", &format_args!("{} redacted", bounded.held.len()))
            .field("bytes", &bounded.weight)
            .field("dropped", &bounded.dropped)
            .finish()
    }
}

/// The lines still waiting, the two bounds, and what they cost.
struct Bounded {
    held: VecDeque<String>,
    /// How many lines may wait.
    lines: usize,
    /// How many bytes those lines may come to.
    bytes: usize,
    /// What the waiting lines come to now.
    weight: usize,
    dropped: usize,
}

/// What was waiting, and what did not survive the wait.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Told {
    /// The lines, oldest first.
    pub lines: Vec<String>,
    /// How many lines were dropped to keep the bounds since the last take.
    ///
    /// Non-zero means the reader is looking at less than happened, and
    /// whatever shows it says so.
    pub dropped: usize,
}

impl Progress {
    /// A buffer that holds at most `lines` lines coming to at most `bytes`
    /// bytes.
    ///
    /// Whichever ceiling a new line would cross is the one that costs an old
    /// line, and a line that could not fit even in an empty buffer is dropped
    /// on arrival. Every one of those is counted.
    #[must_use]
    pub fn new(lines: usize, bytes: usize) -> Self {
        Self(Arc::new(Mutex::new(Bounded {
            held: VecDeque::new(),
            lines,
            bytes,
            weight: 0,
            dropped: 0,
        })))
    }

    /// Adds a line, dropping the oldest for as long as that is what the bounds
    /// cost.
    ///
    /// Never blocks and never refuses: the caller is a task in the middle of
    /// doing something, and a bound it had to handle would be a bound handed
    /// to the one place that cannot act on it.
    pub fn say(&self, line: String) {
        let mut bounded = self.bounded();

        // Nothing this buffer can be emptied to would make room for it.
        if bounded.lines == 0 || line.len() > bounded.bytes {
            bounded.dropped += 1;
            return;
        }

        while bounded.held.len() >= bounded.lines || bounded.weight + line.len() > bounded.bytes {
            // The bound says there is no room and the buffer says there is
            // nothing to give up. Neither can be acted on, and looping on it
            // would be a task that never returns to what it was doing.
            let Some(gone) = bounded.held.pop_front() else {
                break;
            };
            bounded.weight -= gone.len();
            bounded.dropped += 1;
        }

        bounded.weight += line.len();
        bounded.held.push_back(line);
    }

    /// Whether there is anything to show — a line, or a loss.
    ///
    /// A buffer that dropped everything it was given answers yes with no lines
    /// in it, because the count is the thing worth showing: a reader who is
    /// never told the stream outran the buffer reads what is left as all there
    /// was.
    #[must_use]
    pub fn any(&self) -> bool {
        let bounded = self.bounded();
        !bounded.held.is_empty() || bounded.dropped > 0
    }

    /// Takes every line waiting, and how many were dropped to fit them.
    ///
    /// Take-once, and the count is taken with them: it is the number of lines
    /// lost since the last take, so a reader shown two takes in a row is not
    /// told about the same loss twice.
    pub fn take(&self) -> Told {
        let mut bounded = self.bounded();
        bounded.weight = 0;

        Told {
            lines: bounded.held.drain(..).collect(),
            dropped: std::mem::take(&mut bounded.dropped),
        }
    }

    /// The buffer, whether or not a thread came apart while holding it.
    ///
    /// Poisoning says a panic happened somewhere; it says nothing about this
    /// buffer, whose invariant is restored by the end of every method that
    /// touches it. Refusing to read it would answer a panic in one task by
    /// silently throwing away what every other task had said, which is the
    /// loss this module exists to make impossible.
    fn bounded(&self) -> MutexGuard<'_, Bounded> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Room enough that a test about the line count is not accidentally a test
    /// about the byte ceiling.
    const ROOMY: usize = 1024;

    #[test]
    fn a_buffer_that_never_filled_reports_nothing_dropped() {
        let progress = Progress::new(4, ROOMY);
        progress.say("one".into());
        progress.say("two".into());

        assert_eq!(
            progress.take(),
            Told {
                lines: vec!["one".into(), "two".into()],
                dropped: 0,
            }
        );
    }

    #[test]
    fn a_full_buffer_keeps_the_end_and_says_what_it_cost() {
        let progress = Progress::new(2, ROOMY);
        for line in ["one", "two", "three", "four"] {
            progress.say(line.into());
        }

        assert_eq!(
            progress.take(),
            Told {
                lines: vec!["three".into(), "four".into()],
                dropped: 2,
            },
            "a reader who fell behind wants the end of the stream, and has to \
             be told the beginning is gone"
        );
    }

    #[test]
    fn the_byte_ceiling_binds_before_the_line_count_does() {
        // Room for ten lines, but only for six bytes of them.
        let progress = Progress::new(10, 6);
        for line in ["aaa", "bbb", "ccc"] {
            progress.say(line.into());
        }

        assert_eq!(
            progress.take(),
            Told {
                lines: vec!["bbb".into(), "ccc".into()],
                dropped: 1,
            },
            "three lines of three bytes were held under a six-byte ceiling"
        );
    }

    #[test]
    fn a_line_bigger_than_the_whole_buffer_is_dropped_and_counted() {
        let progress = Progress::new(10, 4);
        progress.say("kept".into());
        progress.say("far too long for this buffer".into());

        assert_eq!(
            progress.take(),
            Told {
                lines: vec!["kept".into()],
                dropped: 1,
            },
            "a line nothing could make room for took the buffer with it"
        );
    }

    #[test]
    fn a_buffer_reports_a_loss_even_when_it_is_holding_nothing() {
        let progress = Progress::new(0, ROOMY);
        progress.say("one".into());

        assert!(
            progress.any(),
            "a buffer that lost everything told the reader it had nothing to say"
        );
        assert_eq!(
            progress.take(),
            Told {
                lines: Vec::new(),
                dropped: 1
            }
        );
    }

    #[test]
    fn a_buffer_with_nothing_in_it_and_nothing_lost_says_so() {
        assert!(!Progress::new(4, ROOMY).any());
    }

    #[test]
    fn taking_twice_does_not_report_the_same_loss_twice() {
        let progress = Progress::new(1, ROOMY);
        progress.say("one".into());
        progress.say("two".into());

        assert_eq!(progress.take().dropped, 1);
        assert_eq!(
            progress.take().dropped,
            0,
            "the count is taken with the lines it explains"
        );
    }

    #[test]
    fn taking_the_lines_gives_back_the_room_they_held() {
        let progress = Progress::new(10, 6);
        progress.say("aaaaaa".into());
        assert_eq!(progress.take().lines, vec!["aaaaaa".to_owned()]);

        progress.say("bbbbbb".into());

        assert_eq!(
            progress.take(),
            Told {
                lines: vec!["bbbbbb".into()],
                dropped: 0,
            },
            "the buffer was still counting bytes a reader had already been given"
        );
    }

    #[test]
    fn a_clone_fills_the_buffer_the_original_drains() {
        let progress = Progress::new(2, ROOMY);
        let worker = progress.clone();

        worker.say("from the task".into());

        assert_eq!(progress.take().lines, vec!["from the task".to_owned()]);
    }

    #[test]
    fn a_task_coming_apart_does_not_take_what_the_others_said() {
        let progress = Progress::new(4, ROOMY);
        progress.say("before".into());

        let poisoner = progress.clone();
        let came_apart = std::thread::spawn(move || {
            let _held = poisoner.0.lock().unwrap();
            panic!("a task came apart holding the lock");
        })
        .join();
        assert!(came_apart.is_err(), "the thread was supposed to panic");

        progress.say("after".into());

        assert!(progress.any());
        assert_eq!(
            progress.take().lines,
            vec!["before".to_owned(), "after".to_owned()],
            "a panic in one task threw away what every other task had said"
        );
    }

    #[test]
    fn the_buffer_never_shows_what_a_command_printed() {
        let progress = Progress::new(2, ROOMY);
        progress.say("sk-live-0123456789".into());

        let shown = format!("{progress:?}");

        assert!(!shown.contains("sk-live"), "rendered as {shown}");
        assert!(shown.contains("1 redacted"), "rendered as {shown}");
    }
}
