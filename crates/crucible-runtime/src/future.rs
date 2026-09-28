//! What an asynchronous service contract hands back.
//!
//! The service contracts — a provider's stream, a tool's run, a sandbox's
//! lifecycle, a session's writes — hand back a [`BoxFuture`]: boxed, so a
//! contract stays a trait object; `Send`, so the work can be moved to whichever
//! worker is free; and borrowing nothing past the arguments it was given, so a
//! context lent to one call cannot be kept by it. What only describes or
//! classifies stays synchronous, because it asks no source to do any work,
//! though an implementation may still wait on its own state to answer.
//!
//! Every caller awaits what it is handed. Nothing shipped polls a future by
//! hand: the one poll written here is `answered!`'s, which a test uses to drive
//! a fake service and which exists only with the `proof` feature.

use std::future::Future;
#[cfg(feature = "proof")]
use std::future::IntoFuture;
use std::pin::Pin;
#[cfg(feature = "proof")]
use std::pin::pin;
#[cfg(feature = "proof")]
use std::task::{Context, Poll, Waker};

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
#[cfg(feature = "proof")]
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
