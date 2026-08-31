//! Cancellation.
//!
//! One flag, shared with whichever threads are working. The provider checks it
//! between socket reads and a tool checks it between the steps of whatever it
//! is doing.
//!
//! Nothing is killed. Each thread notices and returns, which is why a
//! half-written file cannot happen: the write either did not start or ran to
//! completion.
//!
//! What raises it is Esc during a turn — the key that backs out of whatever is
//! standing in front of the reader, which while a turn runs is the turn. Raw
//! mode is held for the whole session, so it arrives at the loop reading the
//! keyboard rather than being swallowed as the start of an escape sequence, and
//! [`Cancel::request`] is what the loop does with it. One producer, on the
//! thread that draws; the consumers are all on the thread the turn runs on.
//!
//! The producer clears it too, and that is what keeps a press from being lost
//! rather than merely tidy — see [`Cancel::reset`]. One thread raises the flag
//! and clears it, so there is no moment at which a press can be overwritten by
//! a clearing that was decided before it happened.
//!
//! It lives in core because [`crate::provider::Provider`] takes one, and core
//! owns every type its own traits name.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

/// A shared "stop what you are doing" flag.
///
/// Cloning shares the flag rather than copying it, so a clone handed to a
/// worker thread sees the cancellation the input thread requested.
#[derive(Clone)]
pub struct Cancel(Arc<State>);

struct State {
    requested: AtomicBool,
    parent: Option<Cancel>,
    deadline: Option<Instant>,
}

impl Default for Cancel {
    fn default() -> Self {
        Self(Arc::new(State {
            requested: AtomicBool::new(false),
            parent: None,
            deadline: None,
        }))
    }
}

impl std::fmt::Debug for Cancel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cancel")
            .field("requested", &self.requested())
            .field("has_parent", &self.0.parent.is_some())
            .field("deadline", &self.0.deadline)
            .finish()
    }
}

impl Cancel {
    /// A flag that has not been raised.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A local child that also observes every request made of this token.
    #[must_use]
    pub fn child(&self) -> Self {
        self.child_until(None)
    }

    /// A child that stops with this token or at `deadline`.
    ///
    /// Requesting the child never raises its parent, which lets one timed-out
    /// call stop without ending its run. A parent request still reaches every
    /// descendant.
    #[must_use]
    pub fn child_until(&self, deadline: Option<Instant>) -> Self {
        Self(Arc::new(State {
            requested: AtomicBool::new(false),
            parent: Some(self.clone()),
            deadline,
        }))
    }

    /// Asks every holder to stop at its next check.
    pub fn request(&self) {
        // Release: the work a thread does after observing this must not be
        // reordered before it observes the request.
        self.0.requested.store(true, Ordering::Release);
    }

    /// Whether a stop has been asked for.
    #[must_use]
    pub fn requested(&self) -> bool {
        self.0.requested.load(Ordering::Acquire)
            || self
                .0
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
            || self.0.parent.as_ref().is_some_and(Self::requested)
    }

    /// Clears the flag, ready for the turn about to run.
    ///
    /// Called on the thread that reads the keyboard, before the thread the turn
    /// runs on exists. Both halves of that are load-bearing: whatever stopped
    /// the last turn is spent, and the only hand that can raise the flag is the
    /// one making this call, so nothing can be raised in the moment this call
    /// then clears.
    ///
    /// Cleared inside the turn instead — by the turn, on the turn's own thread
    /// — it would leave a window as wide as a thread takes to start, in which an
    /// Esc is raised by the loop and then wiped by the very turn it was pressed
    /// to stop. A turn that finds the flag raised is a turn somebody stopped,
    /// and it stops.
    pub fn reset(&self) {
        self.0.requested.store(false, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_flag_is_not_raised() {
        assert!(!Cancel::new().requested());
    }

    #[test]
    fn a_clone_sees_the_request() {
        let cancel = Cancel::new();
        let worker = cancel.clone();

        cancel.request();

        assert!(
            worker.requested(),
            "a clone must share the flag, not copy it"
        );
    }

    #[test]
    fn a_request_crosses_a_thread() {
        let cancel = Cancel::new();
        let worker = cancel.clone();

        let handle = std::thread::spawn(move || {
            while !worker.requested() {
                std::hint::spin_loop();
            }
            "noticed"
        });

        cancel.request();

        assert_eq!(handle.join().unwrap(), "noticed");
    }

    #[test]
    fn reset_clears_it_for_the_next_turn() {
        let cancel = Cancel::new();
        cancel.request();
        cancel.reset();
        assert!(!cancel.requested());
    }

    #[test]
    fn a_child_stops_with_its_parent_without_stopping_its_siblings() {
        let parent = Cancel::new();
        let one = parent.child();
        let two = parent.child();

        one.request();
        assert!(one.requested());
        assert!(!parent.requested());
        assert!(!two.requested());

        parent.request();
        assert!(two.requested());
    }

    #[test]
    fn a_child_deadline_is_a_cancellation_only_for_that_child() {
        let parent = Cancel::new();
        let child = parent.child_until(Some(std::time::Instant::now()));

        assert!(child.requested());
        assert!(!parent.requested());
    }
}
