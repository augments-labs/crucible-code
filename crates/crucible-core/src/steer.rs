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
#[derive(Debug, Clone, Default)]
pub struct Steer(Arc<Mutex<VecDeque<String>>>);

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
            waiting.push_back(line);
        }
    }

    /// Whether a line is waiting to be worked in.
    ///
    /// Cheap and lock-light, so the exchange loop can ask it every pass without
    /// taking the lock when the answer is no — which is almost every pass.
    #[must_use]
    pub fn any(&self) -> bool {
        self.0.lock().is_ok_and(|waiting| !waiting.is_empty())
    }

    /// Takes every line waiting, oldest first.
    ///
    /// Called on the turn's thread, at the boundary between one pass and the
    /// next. Drained rather than popped one at a time, because a burst of lines
    /// typed in a pass are one course-correction, and the next request carries
    /// them together.
    ///
    /// A poisoned lock yields nothing, for the reason [`Steer::say`] drops one.
    pub fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut waiting| waiting.drain(..).collect())
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
