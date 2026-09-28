//! Where a synchronous caller meets an asynchronous contract.
//!
//! The service contracts — a provider's stream, a tool's run, a sandbox's
//! lifecycle, a session's writes — hand back a [`BoxFuture`]: boxed, so a
//! contract stays a trait object; `Send`, so the work can be moved to whichever
//! worker is free; and borrowing nothing past the arguments it was given, so a
//! context lent to one call cannot be kept by it. What only describes or
//! classifies stays synchronous, because it asks no source to do any work,
//! though an implementation may still wait on its own state to answer.
//!
//! Most callers of those contracts are still synchronous. A [`Bridge`] is how
//! one of them crosses, and there is one for each caller that has not been made
//! asynchronous yet — not one per call site, and not one that anybody can reach
//! for.
//!
//! A crossing polls a future it owns once, on the caller's own stack, with a
//! waker that wakes nothing. A future that answers when it is first asked is
//! handed back its answer. One that would have to wait is dropped before the
//! crossing returns, and the caller is told [`Unready`], naming the bridge.
//! Dropping it is all the crossing does to it: what a step dropped part way
//! leaves behind is whatever the contract it came from says dropping it leaves,
//! and where that contract says nothing, the caller treats the step as
//! unconfirmed rather than as undone. The crossing itself blocks on nothing,
//! starts no thread and builds or enters no runtime, so one reached from inside
//! a runtime behaves exactly as one reached from outside: it cannot deadlock on
//! the worker it is on, and it cannot panic on finding a runtime already
//! running. One poll bounds how often the future is asked, not how long
//! answering takes: the implementation's own code runs on the caller's thread
//! inside that poll, and one that blocks there blocks the caller.
//!
//! Owning the future is what makes the drop final, and the type does not insist
//! on it: a lent future — a mutable borrow of one the caller keeps — crosses as
//! well, and then only the borrow is dropped. The future keeps its state for
//! the next crossing, and crossing it until it answers is a wait written as a
//! loop. The type cannot refuse a lent future, so the repository checks refuse
//! a borrow of one written as a crossing's argument, in the spellings they
//! know. That is not every lent future: one borrowed first and crossed by name
//! passes them.
//!
//! That is enough where every implementation behind a contract still does its
//! work synchronously inside the future, and so answers the first time it is
//! polled. It stops being enough the moment one really waits, and [`Unready`]
//! is how that is found out: as an error the caller is handed rather than a
//! hang.
//!
//! # The ledger
//!
//! [`Bridge`] is the whole ledger. Each variant is one crossing, and its
//! documentation says that it polls once, what bounds it, which crate owns it
//! and what retires it. A crate crosses only the bridges it owns, every bridge
//! is crossed somewhere, and a bridge nothing crosses any more is
//! deleted rather than kept; the repository checks hold all of that against
//! the code.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::{Pin, pin};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

/// What an asynchronous service contract hands back.
///
/// `'a` is the shortest of the borrows the call was given, so a future cannot
/// outlive a context lent to it. `Send` is what lets whichever worker is free
/// poll it, and it is also what keeps a value that must stay on the thread that
/// made it — a lock the platform requires to be released by the thread that
/// took it — from being held across a wait inside one:
///
/// ```compile_fail,E0277
/// use std::sync::{Mutex, PoisonError};
///
/// use crucible_runtime::BoxFuture;
///
/// fn counted<'a>(count: &'a Mutex<u32>, then: BoxFuture<'a, ()>) -> BoxFuture<'a, ()> {
///     Box::pin(async move {
///         {
///             let mut held = count.lock().unwrap_or_else(PoisonError::into_inner);
///             *held += 1;
///             then.await;
///         }
///     })
/// }
/// ```
///
/// The error code records which failure the example is about, an unmet `Send`
/// bound, rather than enforcing it: `compile_fail` accepts any compile error.
/// What shows the failure comes from the lock being held across the wait is its
/// twin, which differs only in waiting once the scope holding the lock has
/// closed, and compiles. The scope is the release that counts: a `drop` of the
/// guard before the wait still leaves it in scope across it, and is refused the
/// same way.
///
/// ```
/// use std::sync::{Mutex, PoisonError};
///
/// use crucible_runtime::BoxFuture;
///
/// fn counted<'a>(count: &'a Mutex<u32>, then: BoxFuture<'a, ()>) -> BoxFuture<'a, ()> {
///     Box::pin(async move {
///         {
///             let mut held = count.lock().unwrap_or_else(PoisonError::into_inner);
///             *held += 1;
///         }
///         then.await;
///     })
/// }
/// ```
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One synchronous caller of an asynchronous contract, and the ledger of all of
/// them.
///
/// Each entry says which kind of crossing it is, and every one polls once: it
/// is bounded the same way as every other — one poll of a future it owns,
/// nothing queued, nothing kept — and the bound it states is what that one
/// poll covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bridge {
    /// A kept command's stop, asked from the one blocking step its owner holds:
    /// a foreground guard ending the command it was left holding, and a kept
    /// command's owner ending one that was stopped, abandoned, left running, or
    /// whose cleanup it refused.
    ///
    /// The stop is a future, and the step that asks for it is a thread the
    /// runtime's blocking pool handed out for process work, so the ask happens
    /// there rather than on a runtime worker. The process contract keeps its
    /// bounded stop work inside the contract, so the one poll answers; a stop
    /// that would have had to wait is refused, and the step that asked is told
    /// only that. What happens next is the caller's own: a stop the caller
    /// already meant to ask again on its next attempt is asked again — a
    /// descendant's, a registry's own end's, and a release task's, whose own
    /// interval grows between asks — while a stop asked for by a key or by an
    /// abandoned result waits for that ask to be made again.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each stop asked for, one at a time for each
    ///   command, on the thread that step already holds.
    /// - Owner: `crucible-builtins`
    /// - Retired: when a kept command's stop is awaited on the runtime that
    ///   owns the command rather than asked from a blocking step.
    CommandStop,
    /// A kept command's result acceptance lifecycle: the call that begins it
    /// and the call that completes it, each asked for while the caller still
    /// holds the process.
    ///
    /// Asked from a caller that may or may not be on a runtime, so it is
    /// answered on the frame that asks or refused: a lifecycle future nobody
    /// can time is a call that is not made, and the process is handed straight
    /// back rather than left borrowed for it. On a runtime the wait is timed
    /// instead, which is the bound this crossing states where there is a clock.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each lifecycle call where no runtime is running;
    ///   elsewhere the acceptance bound the builtins pass to their own
    ///   `bounded`, which is `background::ACCEPTANCE` there.
    /// - Owner: `crucible-builtins`
    /// - Retired: when every acceptance lifecycle call is made where a runtime
    ///   is running.
    CommandAcceptance,
}

impl Bridge {
    /// Polls `future` once, and hands back what it answered.
    ///
    /// # Errors
    ///
    /// [`Unready`], naming this bridge, where the future would have had to
    /// wait. A future it owns has been dropped by then, and what that leaves
    /// behind is the future's own contract's to say. A lent one loses only the
    /// borrow and keeps its state. This signature cannot refuse one, so the
    /// repository checks refuse a borrow of a future written as the crossing's
    /// argument, in the spellings they know; one borrowed first and crossed by
    /// name passes them.
    pub fn cross<F: IntoFuture>(self, future: F) -> Result<F::Output, Unready> {
        answered_at_once(future.into_future()).ok_or(Unready { bridge: self })
    }

    /// What is crossed, in words a reader of an error can follow.
    const fn crossing(self) -> &'static str {
        match self {
            Self::CommandStop => "a kept command's stop",
            Self::CommandAcceptance => "a kept command's result acceptance",
        }
    }
}

/// What `answered!` expands to. Not API.
#[cfg(feature = "proof")]
#[doc(hidden)]
pub fn __answered<F: IntoFuture>(future: F) -> Option<F::Output> {
    answered_at_once(future.into_future())
}

/// Asks a future once and evaluates to its answer, failing the test where it
/// would have had to wait.
///
/// For tests that drive a fake service by hand: a fake whose future does not
/// answer when first asked is a test waiting on its own scheduling, and this
/// says so at the line that asked, naming the expression, rather than hanging.
#[cfg(feature = "proof")]
#[macro_export]
macro_rules! answered {
    ($future:expr $(,)?) => {
        match $crate::__answered($future) {
            ::core::option::Option::Some(answer) => answer,
            ::core::option::Option::None => ::core::panic!(
                "`{}` would have had to wait; a future a test drives by hand has to answer when it is first asked",
                ::core::stringify!($future)
            ),
        }
    };
}

/// The future's answer, if it has one the first time it is asked.
///
/// The future is pinned on this frame and dropped with it, so one that would
/// have waited is gone by the time this returns.
fn answered_at_once<F: Future>(future: F) -> Option<F::Output> {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(answer) => Some(answer),
        Poll::Pending => None,
    }
}

/// A future a synchronous caller crossed to would have had to wait. One the
/// crossing owned, which is every crossing in the tree, was dropped before it
/// answered, leaving behind what its contract says dropping it leaves; a lent
/// one lost only the borrow and keeps its state (see [`Bridge::cross`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unready {
    bridge: Bridge,
}

impl Unready {
    /// The bridge the caller was crossing.
    #[must_use]
    pub fn bridge(&self) -> Bridge {
        self.bridge
    }
}

impl fmt::Display for Unready {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} would have had to wait, and the caller cannot; the waiting step was dropped \
             before it answered, so whatever that step began is unconfirmed",
            self.bridge.crossing()
        )
    }
}

impl std::error::Error for Unready {}

/// How long a wait that a [`Cancel`](crate::Cancel) cannot wake may go without
/// looking at it: a raised cancel wakes nobody, so such a wait is also woken
/// this often to look.
///
/// Short against a person pressing a key and waiting to see the turn stop, and
/// long against a thread waking only to find nothing to do.
pub const NOTICED: Duration = Duration::from_millis(20);

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use super::{BoxFuture, Bridge, Unready};

    /// Says, when it is dropped, that it was.
    struct Dropped(Arc<AtomicBool>);

    impl Drop for Dropped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::Release);
        }
    }

    #[test]
    fn a_future_that_answers_at_once_is_handed_back_its_answer() {
        let answered: BoxFuture<'_, u32> = Box::pin(async { 7 });

        assert_eq!(Bridge::CommandStop.cross(answered), Ok(7));
    }

    #[test]
    fn a_future_that_would_wait_is_refused_naming_the_bridge() {
        let refused = Bridge::CommandStop.cross(std::future::pending::<()>());

        assert_eq!(
            refused,
            Err(Unready {
                bridge: Bridge::CommandStop
            })
        );
        assert_eq!(
            refused.map_err(|unready| unready.to_string()),
            Err(
                "a kept command's stop would have had to wait, and the caller cannot; the waiting \
                 step was dropped before it answered, so whatever that step began is unconfirmed"
                    .to_owned()
            )
        );
    }

    #[test]
    fn a_future_that_would_wait_is_dropped_before_the_crossing_returns() {
        let dropped = Arc::new(AtomicBool::new(false));
        let held = Dropped(Arc::clone(&dropped));
        let waiting: BoxFuture<'_, ()> = Box::pin(async move {
            let _held = held;
            std::future::pending::<()>().await;
        });

        let crossed = Bridge::CommandStop.cross(waiting);

        assert!(crossed.is_err(), "a future that waits was answered");
        assert!(
            dropped.load(Ordering::Acquire),
            "the future that would have waited was still alive after the crossing returned"
        );
    }

    /// Wakes whoever polls it the first time it is asked, and answers the
    /// second time.
    struct WakesItself {
        asked: bool,
    }

    impl Future for WakesItself {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.asked {
                Poll::Ready(())
            } else {
                self.asked = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }

    /// A worker thread is where a `block_on` would panic or deadlock, so both
    /// crossings are made in a task spawned onto one. The refused one wakes
    /// itself when first asked and answers when asked again, so a crossing
    /// that waited for the wake instead of refusing is handed that answer, and
    /// the test fails at once rather than hanging. The test's own body runs on
    /// the thread that started the runtime, which is not a worker.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_crossing_made_on_a_runtime_worker_answers_rather_than_blocking() {
        let crossed = tokio::spawn(async move {
            (
                Bridge::CommandStop.cross(WakesItself { asked: false }),
                Bridge::CommandStop.cross(async { "ready" }),
            )
        })
        .await
        .map_err(|failed| failed.to_string());

        assert_eq!(
            crossed,
            Ok((
                Err(Unready {
                    bridge: Bridge::CommandStop
                }),
                Ok("ready")
            )),
            "a crossing on a runtime worker did something other than refuse a \
             future that would wait and hand back one that answered"
        );
    }

    fn comes_apart() -> u8 {
        panic!("came apart")
    }

    /// A crossing polls on the caller's own stack, so a future coming apart
    /// unwinds into the caller with what it came apart with.
    #[test]
    fn a_future_coming_apart_unwinds_into_the_caller_as_a_crossing_always_has() {
        let crossed =
            std::panic::catch_unwind(|| Bridge::CommandStop.cross(async { comes_apart() }));

        assert_eq!(
            crossed
                .err()
                .and_then(|payload| payload.downcast_ref::<&'static str>().copied()),
            Some("came apart")
        );
    }
}
