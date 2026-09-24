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
//! [`Cancel::request`] is what the loop does with it.
//!
//! Reading it is not confined to one thread. A token is cloned into every task
//! that has to notice, and a child of it narrows what a request reaches — see
//! [`Cancel::child`] — so a timed-out call or a task group closing stops what
//! it owns without ending the run around it.
//!
//! Who clears the session's own token is confined, and that is what keeps a
//! press from being lost rather than merely tidy — see [`Cancel::reset`].
//!
//! Waiting is the one place looking is not enough: a future that is waiting
//! is not running, so it cannot check. [`Cancel::race`] is how one stops
//! waiting instead, and it is woken by the request itself rather than by a
//! look on a timer: every token keeps the wakers of the races waiting on it or
//! on any of its descendants, and [`Cancel::request`] wakes them. A deadline
//! has nobody to raise it, so a race waiting on one sets the runtime's timer
//! for it. What a race gives up on it drops, and that is all it does to it;
//! nothing is killed here either.

use std::future::{Future, IntoFuture, poll_fn};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use std::time::Instant;

use tokio::sync::Notify;
use tokio::sync::futures::Notified;
use tokio::time::Sleep;

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
    /// Wakes the races waiting on this token or on a descendant of it.
    raised: Notify,
}

impl Default for Cancel {
    fn default() -> Self {
        Self(Arc::new(State {
            requested: AtomicBool::new(false),
            parent: None,
            deadline: None,
            raised: Notify::new(),
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
            raised: Notify::new(),
        }))
    }

    /// Asks every holder to stop at its next check, and wakes every race
    /// waiting on this token or on a descendant of it.
    pub fn request(&self) {
        // Release: the work a thread does after observing this must not be
        // reordered before it observes the request.
        self.0.requested.store(true, Ordering::Release);
        self.0.raised.notify_waiters();
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

    /// Awaits `work`, unless a stop is asked for first.
    ///
    /// `Some` with what `work` answered, or `None` once this token is
    /// requested while `work` is still waiting — by a request of it or of an
    /// ancestor, or by a deadline of either passing. `work` is asked first
    /// each time the race is, so an answer it has is handed back even when
    /// the stop was asked for by then, and work that answers when first
    /// asked is never refused. Work the stop won against has been dropped by
    /// the time the race ends, and what dropping it leaves behind is its own
    /// contract's to say.
    ///
    /// A request wakes the race as it is made, whichever thread makes it. A
    /// deadline is timed on the timer of the runtime the race is polled in;
    /// polled outside any runtime, a race notices a deadline only when
    /// something else wakes it.
    ///
    /// A deadline is a wall-clock instant, and the race ends only once the
    /// wall clock reaches it: the timer only says when to look. Under a paused
    /// Tokio test clock the timer runs ahead of the wall clock, fires, finds
    /// the deadline not yet passed and is set again at once, so the race is
    /// polled over and over, and ends no sooner, until real time reaches the
    /// deadline. A test does not pause the clock around a race with a deadline.
    ///
    /// # Panics
    ///
    /// Where a deadline has to be timed on a runtime built without a timer:
    /// setting any timer there panics.
    pub async fn race<F: IntoFuture>(&self, work: F) -> Option<F::Output> {
        let mut work = pin!(work.into_future());
        // Enabled before the first look at the flag, so a request made
        // between that look and the race's wait still wakes it.
        let mut heard: Vec<_> = self.lineage().map(Self::heard).collect();
        let mut clock = None;
        poll_fn(|context| {
            loop {
                if let Poll::Ready(answer) = work.as_mut().poll(context) {
                    return Poll::Ready(Some(answer));
                }
                if self.requested() {
                    return Poll::Ready(None);
                }

                // A request that has since been cleared woke the race without
                // leaving the flag raised, so it waits for the next one.
                let mut woken = false;
                for (listening, token) in heard.iter_mut().zip(self.lineage()) {
                    if listening.as_mut().poll(context).is_ready() {
                        *listening = token.heard();
                        woken = true;
                    }
                }
                if woken {
                    continue;
                }

                if clock.is_none() {
                    clock = self.clock();
                }
                match clock.as_mut().map(|timer| timer.as_mut().poll(context)) {
                    Some(Poll::Ready(())) => clock = None,
                    Some(Poll::Pending) | None => return Poll::Pending,
                }
            }
        })
        .await
    }

    /// This token, its parent, and so on up.
    fn lineage(&self) -> impl Iterator<Item = &Self> {
        std::iter::successors(Some(self), |token| token.0.parent.as_ref())
    }

    /// Waits for the next request of this token, counting from now.
    fn heard(&self) -> Pin<Box<Notified<'_>>> {
        let mut heard = Box::pin(self.0.raised.notified());
        heard.as_mut().enable();
        heard
    }

    /// A timer on the current runtime for the earliest deadline in this
    /// token's lineage, where there is a deadline and a runtime to time it.
    fn clock(&self) -> Option<Pin<Box<Sleep>>> {
        let deadline = self.lineage().filter_map(|token| token.0.deadline).min()?;
        tokio::runtime::Handle::try_current().ok()?;
        Some(Box::pin(tokio::time::sleep(
            deadline.saturating_duration_since(Instant::now()),
        )))
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

    /// Long against anything here that should happen at once, so a race that
    /// never ends fails its test rather than hanging it.
    const PROMPTLY: std::time::Duration = std::time::Duration::from_secs(2);

    /// Short against [`PROMPTLY`]. A race that ends only once the guard
    /// around it asks it again, that long after, is one nothing woke.
    const WOKEN: std::time::Duration = std::time::Duration::from_millis(500);

    /// What `race` answered, if it answered within [`PROMPTLY`], and how long
    /// it took to.
    async fn timed<T>(
        race: impl std::future::Future<Output = Option<T>>,
    ) -> (Option<Option<T>>, std::time::Duration) {
        let begun = Instant::now();
        let raced = tokio::time::timeout(PROMPTLY, race).await.ok();
        (raced, begun.elapsed())
    }

    /// Raises `cancel` from a thread of its own once `after` has passed, the
    /// way a key pressed on another thread does.
    fn raised_after(cancel: &Cancel, after: std::time::Duration) -> std::thread::JoinHandle<()> {
        let raising = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(after);
            raising.request();
        })
    }

    #[tokio::test]
    async fn a_race_hands_back_what_its_work_answered() {
        assert_eq!(Cancel::new().race(async { 7 }).await, Some(7));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_race_ends_as_its_cancel_is_requested() {
        let cancel = Cancel::new();
        let raiser = raised_after(&cancel, std::time::Duration::from_millis(50));

        let (raced, took) = timed(cancel.race(std::future::pending::<()>())).await;
        raiser.join().unwrap();

        assert_eq!(
            raced,
            Some(None),
            "a race whose cancel was requested went on waiting for its work"
        );
        assert!(
            took < WOKEN,
            "a race took {took:?} to end after its cancel was requested at 50 ms"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_race_ends_as_an_ancestor_of_its_cancel_is_requested() {
        let parent = Cancel::new();
        let child = parent.child().child();
        let raiser = raised_after(&parent, std::time::Duration::from_millis(50));

        let (raced, took) = timed(child.race(std::future::pending::<()>())).await;
        raiser.join().unwrap();

        assert_eq!(
            raced,
            Some(None),
            "a request of the parent never reached the race"
        );
        assert!(
            took < WOKEN,
            "a race took {took:?} to end after its grandparent was requested at 50 ms"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_race_ends_at_a_deadline_on_its_lineage_without_anyone_requesting_it() {
        let after = std::time::Duration::from_millis(50);
        let cancel = Cancel::new()
            .child_until(Some(Instant::now() + after))
            .child();

        let (raced, took) = timed(cancel.race(std::future::pending::<()>())).await;

        assert_eq!(
            raced,
            Some(None),
            "a race went on waiting past its deadline"
        );
        assert!(
            took >= after,
            "the race ended {took:?} in, before its deadline"
        );
        assert!(
            took < WOKEN,
            "a race took {took:?} to end at a deadline 50 ms away"
        );
    }

    #[tokio::test]
    async fn a_race_already_stopped_asks_its_work_once_and_keeps_only_an_answer() {
        let cancel = Cancel::new();
        cancel.request();

        assert_eq!(cancel.race(async { 7 }).await, Some(7));
        let (raced, took) = timed(cancel.race(std::future::pending::<()>())).await;
        assert_eq!(raced, Some(None));
        assert!(took < WOKEN, "a race already stopped took {took:?} to end");
    }

    /// Says, when it is dropped, that it was.
    struct Dropped(Arc<AtomicBool>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_race_its_cancel_won_has_dropped_its_work_by_the_time_it_ends() {
        let cancel = Cancel::new();
        let dropped = Arc::new(AtomicBool::new(false));
        let held = Dropped(Arc::clone(&dropped));
        let raiser = raised_after(&cancel, std::time::Duration::from_millis(50));

        let (raced, _) = timed(cancel.race(async move {
            let _held = held;
            std::future::pending::<()>().await;
        }))
        .await;
        raiser.join().unwrap();

        assert_eq!(raced, Some(None));
        assert!(
            dropped.load(Ordering::Acquire),
            "the work the cancel won against was still alive after the race ended"
        );
    }
}
