//! Polling a call's future by hand, for the tests that need to see it wait.
//!
//! A test drives a call lent a worker from its own thread: once, to see that
//! the call is waiting rather than answering, and then until it answers. Only
//! a test does either — shipped code crosses to a future through a `Bridge` —
//! and this file's name is what says so to the checks that look for hand-made
//! polls.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};

use super::PATIENCE;

/// Waits for `future` to answer, polled on this thread each time it wakes
/// the thread — the tests' runtime runs the worker's jobs and fires its
/// timers meanwhile — or fails the test once [`PATIENCE`] has passed.
pub(crate) fn waited<F: Future>(future: F) -> F::Output {
    /// Wakes the waiting thread.
    struct Unpark(std::thread::Thread);

    impl std::task::Wake for Unpark {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }

    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut future = std::pin::pin!(future);
    let began = Instant::now();
    loop {
        if let Poll::Ready(answer) = future
            .as_mut()
            .poll(&mut std::task::Context::from_waker(&waker))
        {
            return answer;
        }
        assert!(
            began.elapsed() < PATIENCE,
            "the call did not answer in time"
        );
        std::thread::park_timeout(Duration::from_millis(10));
    }
}

/// Asks `future` once, on this thread, with a waker nobody listens to.
///
/// For a test that needs to know a call is waiting — for room on a worker, or
/// for a job it handed over — rather than to wait with it.
pub(crate) fn asked_once<F: Future + ?Sized>(future: Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut std::task::Context::from_waker(std::task::Waker::noop()))
}
