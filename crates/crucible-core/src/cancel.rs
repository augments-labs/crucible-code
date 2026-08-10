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
//! What raises it is the half that is missing. `crucible` leaves standard input
//! in cooked mode, so nothing in the binary calls [`Cancel::request`] and the
//! only stop a user has is ending the process; the callers it has are the tests
//! that pin each consumer. This is a seam waiting for a producer rather than a
//! mechanism that works and goes unused, and a reader adding one should expect
//! to find no keystroke handling to hang it off.
//!
//! It lives in core because [`crate::provider::Provider`] takes one, and core
//! owns every type its own traits name.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// A shared "stop what you are doing" flag.
///
/// Cloning shares the flag rather than copying it, so a clone handed to a
/// worker thread sees the cancellation the input thread requested.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    /// A flag that has not been raised.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Asks every holder to stop at its next check.
    pub fn request(&self) {
        // Release: the work a thread does after observing this must not be
        // reordered before it observes the request.
        self.0.store(true, Ordering::Release);
    }

    /// Whether a stop has been asked for.
    #[must_use]
    pub fn requested(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Clears the flag, ready for the next turn.
    ///
    /// Called by the runner between turns, on the one thread that owns the
    /// loop, so there is no window where a worker sees a stale request.
    pub fn reset(&self) {
        self.0.store(false, Ordering::Release);
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
}
