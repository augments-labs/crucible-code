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
//! What raises it is Ctrl-C during a turn. Raw mode is held for the whole
//! session, so that key arrives at the loop reading the keyboard rather than
//! becoming a signal the terminal sends, and [`Cancel::request`] is what the
//! loop does with it. One producer, on the thread that draws; the consumers are
//! all on the thread the turn runs on.
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

    /// Clears the flag, ready for the turn about to run.
    ///
    /// Called by the runner at the top of a turn, so whatever stopped the last
    /// one is spent and no turn begins against a request that was not made of
    /// it.
    ///
    /// That leaves a window, and the window is open rather than closed. The
    /// binary runs a turn on a thread of its own and goes on reading the
    /// keyboard on the thread that spawned it, so a Ctrl-C pressed between the
    /// spawn and this call is raised and then cleared here: the turn carries on
    /// and the press is lost. It is as wide as a thread takes to start, and
    /// pressing again stops the turn. Closing it would mean clearing the flag
    /// before the worker is spawned rather than inside the turn it runs.
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
