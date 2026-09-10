//! What a task says while it is still running.
//!
//! A task that streams — a command printing, a provider answering a token at a
//! time — produces faster than anything reading it, and for longer than
//! anything reading it wants to keep. So this is bounded, and the bound is the
//! whole point: an unbounded buffer here is a session-sized second copy of the
//! transcript, and a full one that blocked its writer is a renderer that can
//! stop a shutdown.
//!
//! It therefore never blocks and never refuses. When it is full it drops the
//! *oldest* line, because what a reader wants from a stream they have fallen
//! behind is its end, not its beginning — and it counts every line it dropped,
//! so [`Progress::take`] hands back a number rather than a gap. Truncation
//! that does not say it truncated is the one outcome this must not have: the
//! reader is looking at less than happened and has no way to know it.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

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
        let (held, dropped) = self
            .0
            .lock()
            .map(|bounded| (bounded.held.len(), bounded.dropped))
            .unwrap_or_default();

        f.debug_struct("Progress")
            .field("held", &format_args!("{held} redacted"))
            .field("dropped", &dropped)
            .finish()
    }
}

/// The lines still waiting, the bound, and what the bound cost.
struct Bounded {
    held: VecDeque<String>,
    limit: usize,
    dropped: usize,
}

/// What was waiting, and what did not survive the wait.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Told {
    /// The lines, oldest first.
    pub lines: Vec<String>,
    /// How many lines were dropped to keep the bound since the last take.
    ///
    /// Non-zero means the reader is looking at less than happened, and
    /// whatever shows it says so.
    pub dropped: usize,
}

impl Progress {
    /// A buffer that holds at most `limit` lines.
    #[must_use]
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(Mutex::new(Bounded {
            held: VecDeque::new(),
            limit,
            dropped: 0,
        })))
    }

    /// Adds a line, dropping the oldest if that is what the bound costs.
    ///
    /// Never blocks and never refuses: the caller is a task in the middle of
    /// doing something, and a bound it had to handle would be a bound handed
    /// to the one place that cannot act on it.
    ///
    /// A poisoned lock loses the line, for the reason [`crate::Aside::say`]
    /// drops one: the other side is gone, and there is nobody left to show it
    /// to.
    pub fn say(&self, line: String) {
        if let Ok(mut bounded) = self.0.lock() {
            if bounded.limit == 0 {
                bounded.dropped += 1;
                return;
            }

            while bounded.held.len() >= bounded.limit {
                bounded.held.pop_front();
                bounded.dropped += 1;
            }

            bounded.held.push_back(line);
        }
    }

    /// Whether anything is waiting to be shown.
    ///
    /// Cheap and lock-light, so a redraw can ask it every frame.
    #[must_use]
    pub fn any(&self) -> bool {
        self.0.lock().is_ok_and(|bounded| !bounded.held.is_empty())
    }

    /// Takes every line waiting, and how many were dropped to fit them.
    ///
    /// Take-once, and the count is taken with them: it is the number of lines
    /// lost since the last take, so a reader shown two takes in a row is not
    /// told about the same loss twice.
    ///
    /// A poisoned lock yields nothing, and reports nothing dropped, because
    /// what it could not read it also cannot count.
    pub fn take(&self) -> Told {
        self.0
            .lock()
            .map(|mut bounded| Told {
                lines: bounded.held.drain(..).collect(),
                dropped: std::mem::take(&mut bounded.dropped),
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_buffer_that_never_filled_reports_nothing_dropped() {
        let progress = Progress::new(4);
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
        let progress = Progress::new(2);
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
    fn taking_twice_does_not_report_the_same_loss_twice() {
        let progress = Progress::new(1);
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
    fn a_clone_fills_the_buffer_the_original_drains() {
        let progress = Progress::new(2);
        let worker = progress.clone();

        worker.say("from the task".into());

        assert_eq!(progress.take().lines, vec!["from the task".to_owned()]);
    }

    #[test]
    fn a_buffer_with_no_room_at_all_still_counts_what_it_lost() {
        let progress = Progress::new(0);
        progress.say("one".into());

        assert_eq!(
            progress.take(),
            Told {
                lines: Vec::new(),
                dropped: 1
            }
        );
    }

    #[test]
    fn the_buffer_never_shows_what_a_command_printed() {
        let progress = Progress::new(2);
        progress.say("sk-live-0123456789".into());

        let shown = format!("{progress:?}");

        assert!(!shown.contains("sk-live"), "rendered as {shown}");
        assert!(shown.contains("1 redacted"), "rendered as {shown}");
    }
}
