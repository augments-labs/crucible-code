//! Steering: a line handed to a turn already running.
//!
//! One queue, shared between the thread reading the keyboard and the thread the
//! turn runs on. A prompt typed while a turn is going does not wait for that
//! turn to end: it is pushed here, and the turn takes it at the next boundary
//! it can — between one pass of asking and running tools and the next — so the
//! agent adjusts course mid-work rather than finishing a plan the reader has
//! already moved past.
//!
//! That is the difference from the queue a turn leaves behind. Both are prompts
//! typed too late for the turn in front of them; what they are for is not the
//! same. A line that would change what the running turn does next belongs in
//! the turn; one that is a whole new question belongs behind it. The reader
//! cannot always tell, and does not have to: steering is the offer, and a turn
//! that was already finishing ignores it and lets the line be answered as its
//! own prompt.
//!
//! It lives in core beside [`crate::Cancel`] because the runner's exchange loop
//! takes one, and core owns every type its own loop names. The shape is the one
//! `Cancel` sets: a shared cell, one producer on the thread that draws, the
//! consumer on the thread the turn runs on.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// A shared "here is more, while you are at it" queue.
///
/// Cloning shares the queue rather than copying it, so a clone handed to the
/// turn's thread sees the lines the input thread pushed.
#[derive(Clone, Default)]
pub struct Steer(Arc<Mutex<Waiting>>);

/// By hand: the lines are the reader's own words waiting to join the turn, and
/// [`crate::Event::Steered`] redacts the same words on their way out.
impl std::fmt::Debug for Steer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (lines, held) = self
            .0
            .lock()
            .map(|waiting| (waiting.lines.len(), waiting.held))
            .unwrap_or_default();

        f.debug_struct("Steer")
            .field("lines", &format_args!("{lines} redacted"))
            .field("held", &held)
            .finish()
    }
}

/// The lines, and whether the turn may have them yet.
///
/// One lock over both, because the two are one fact: a turn that read the lines
/// and the hold separately could take a line the reader had just opened the
/// queue to edit.
#[derive(Default)]
struct Waiting {
    /// The lines, oldest first.
    lines: VecDeque<String>,
    /// Whether the reader has the queue open in front of them.
    held: bool,
}

impl Steer {
    /// An empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Pushes a line the turn should work in, after the one it is on.
    ///
    /// Called on the thread that reads the keyboard. The line is taken whole:
    /// trimming and the empty case are the caller's, which is the editor that
    /// already decided the line was finished.
    ///
    /// A poisoned lock is not a reason to lose the line: the other side is gone
    /// and the turn it was steering is over, so the push is simply not seen.
    pub fn say(&self, line: String) {
        if let Ok(mut waiting) = self.0.lock() {
            waiting.lines.push_back(line);
        }
    }

    /// Holds every line where it is, until [`Steer::release`].
    ///
    /// Called on the thread that draws, when the reader opens the queue to go
    /// over it. A line still being edited is not a line the agent should be
    /// reading, and one taken mid-edit is in the transcript, where the reader
    /// cannot take it back.
    ///
    /// The turn goes on running while it is held. A held queue answers
    /// [`Steer::take`] the way an empty one does, which is what the exchange
    /// loop already meets at almost every pass.
    pub fn hold(&self) {
        if let Ok(mut waiting) = self.0.lock() {
            waiting.held = true;
        }
    }

    /// Lets the turn have them again, at its next pass boundary.
    ///
    /// All of them at once, the ones that were edited and the ones that were
    /// not, because that is what the queue was for: a burst typed and then gone
    /// over is still one course-correction.
    pub fn release(&self) {
        if let Ok(mut waiting) = self.0.lock() {
            waiting.held = false;
        }
    }

    /// Drops the oldest line that says `line`, and answers whether there was
    /// one.
    ///
    /// What the reader taking a line back out of the queue leaves behind. The
    /// same line sits in two places — here, where the turn reads it, and the
    /// panel that names it — and one dropped from the panel alone is a prompt
    /// the reader deleted that the turn works in anyway.
    ///
    /// Matched on what the line says rather than on where it sat, for the
    /// reason the panel's own drop is: the two are not indexed alike once a
    /// turn has taken from the front of this one.
    pub fn forget(&self, line: &str) -> bool {
        let Ok(mut waiting) = self.0.lock() else {
            return false;
        };

        let Some(at) = waiting.lines.iter().position(|said| said == line) else {
            return false;
        };

        waiting.lines.remove(at).is_some()
    }

    /// Whether a line is waiting to be worked in, and may be.
    ///
    /// Cheap and lock-light, so the exchange loop can ask it every pass without
    /// taking the lock when the answer is no — which is almost every pass. A
    /// held queue answers no however many lines are in it: they are the
    /// reader's until they say otherwise.
    #[must_use]
    pub fn any(&self) -> bool {
        self.0
            .lock()
            .is_ok_and(|waiting| !waiting.held && !waiting.lines.is_empty())
    }

    /// Takes every line waiting, oldest first.
    ///
    /// Called on the turn's thread, at the boundary between one pass and the
    /// next. Drained rather than popped one at a time, because a burst of lines
    /// typed in a pass are one course-correction, and the next request carries
    /// them together.
    ///
    /// Nothing while the queue is held: see [`Steer::hold`]. That is the same
    /// answer an empty queue gives, so a turn meeting it is a turn carrying on.
    ///
    /// A poisoned lock yields nothing, for the reason [`Steer::say`] drops one.
    pub fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut waiting| {
                if waiting.held {
                    Vec::new()
                } else {
                    waiting.lines.drain(..).collect()
                }
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_queue_never_shows_what_the_reader_typed() {
        // The lines are the reader's own words waiting to join the turn, and
        // [`crate::Event::Steered`] redacts the same words on their way out.
        let steer = Steer::new();
        steer.say("steer-debug-canary".to_owned());

        let shown = format!("{steer:?}");
        assert!(!shown.contains("steer-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }

    #[test]
    fn a_new_queue_has_nothing_to_say() {
        assert!(!Steer::new().any());
        assert!(Steer::new().take().is_empty());
    }

    #[test]
    fn a_clone_shares_the_lines_rather_than_copying_them() {
        let steer = Steer::new();
        let turn = steer.clone();

        steer.say("use the mock".to_owned());
        steer.say("and the fake clock".to_owned());

        assert!(turn.any());
        assert_eq!(
            turn.take(),
            vec!["use the mock".to_owned(), "and the fake clock".to_owned()]
        );
        assert!(!steer.any(), "the drain emptied the shared queue");
    }

    #[test]
    fn taking_what_is_not_there_takes_nothing() {
        assert!(Steer::new().take().is_empty());
    }

    #[test]
    fn a_held_queue_says_it_has_nothing_and_gives_nothing_up() {
        // The reader has it open in front of them, so a turn asking is told the
        // same thing an empty queue tells it — which is what lets the turn go on
        // running rather than waiting on a reader who may be a minute.
        let steer = Steer::new();
        steer.say("use the mock".to_owned());
        steer.hold();

        assert!(!steer.any());
        assert!(steer.take().is_empty());
        assert!(!steer.any(), "the line was still there to be released");
    }

    #[test]
    fn releasing_gives_up_every_line_at_once() {
        // Including the ones typed while it was held: what the reader closes the
        // queue on is one course-correction, and it arrives as one.
        let steer = Steer::new();
        steer.say("use the mock".to_owned());
        steer.hold();
        steer.say("and the fake clock".to_owned());
        steer.release();

        assert!(steer.any());
        assert_eq!(
            steer.take(),
            vec!["use the mock".to_owned(), "and the fake clock".to_owned()]
        );
    }

    #[test]
    fn a_line_taken_back_is_not_worked_in_when_the_rest_are() {
        // The reader dropped it while the queue was held. A line forgotten here
        // and left in the panel — or the other way about — is the one thing the
        // hold exists to make impossible.
        let steer = Steer::new();
        steer.say("first".to_owned());
        steer.say("second".to_owned());
        steer.hold();

        assert!(steer.forget("first"));
        assert!(!steer.forget("first"), "there was only ever one of it");

        steer.release();
        assert_eq!(steer.take(), vec!["second".to_owned()]);
    }
}
