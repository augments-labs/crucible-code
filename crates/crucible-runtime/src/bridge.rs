//! Where a synchronous caller meets an asynchronous contract.
//!
//! The service contracts — a provider's stream, a tool's run, a sandbox's
//! lifecycle, a session's writes — hand back a [`BoxFuture`]: boxed, so a
//! contract stays a trait object; `Send`, so the work can be moved to whichever
//! worker is free; and borrowing nothing past the arguments it was given, so a
//! context lent to one call cannot be kept by it. What only describes or
//! classifies stays synchronous, because it asks no source to do any work; an
//! implementation may still wait on its own state, as the MCP toolset's
//! `registered` can wait behind a dispose in progress.
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
//! That is enough today because every implementation behind these contracts
//! still does its work synchronously inside the future, and so answers the
//! first time it is polled. It stops being enough the moment one really waits,
//! and [`Unready`] is how that is found out: as an error the caller is handed
//! rather than a hang.
//!
//! # The ledger
//!
//! [`Bridge`] is the whole ledger. Each variant is one crossing, and its
//! documentation says what bounds it, which crate owns it and what retires it.
//! A crate crosses only the bridges it owns, every bridge is crossed somewhere,
//! and a bridge nothing crosses any more is deleted rather than kept; the
//! repository checks hold all three against the code.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::{Pin, pin};
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

/// One synchronous caller of an asynchronous contract, and the ledger of all of
/// them.
///
/// Every crossing is bounded the same way — one poll of a future it owns,
/// nothing queued, nothing kept — and the bound each entry states is what that
/// one poll covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bridge {
    /// The turn loop asking the model, and a compaction asking it, inside a
    /// turn or between turns: opening a provider's stream and reading each of
    /// its deltas.
    ///
    /// - Bound: one poll to open a stream, and one poll for each delta read.
    /// - Owner: `crucible-runner`
    /// - Retired: when the turn loop and a compaction are asynchronous.
    TurnProvider,
    /// The runner's prompt-cache resources: a provider's lifecycle calls and
    /// the store their records are kept in. They are reached by a turn
    /// preparing its request, and by a compaction preparing its recap request
    /// inside a turn or between turns; by the cleanup pass, both when a user
    /// asks for it and when it runs as the retirement before a change of model
    /// or provider, a login or a logout; and by the listing a user's cache
    /// inspection reads.
    ///
    /// - Bound: one poll for each lifecycle call and each store operation, the
    ///   inspection's listing among them.
    /// - Owner: `crucible-runner`
    /// - Retired: when the turn loop, a compaction, the runner's cache
    ///   inspection and its cleanup pass are asynchronous.
    TurnCache,
    /// The turn's tools: running an admitted call, accepting a background
    /// result, and preparing, listing, refreshing and disposing of toolsets.
    ///
    /// - Bound: one poll for each call run, result accepted and toolset
    ///   operation.
    /// - Owner: `crucible-runner`
    /// - Retired: when the turn loop is asynchronous.
    TurnTools,
    /// The runner's writes to its session: the turn's, and the ones a
    /// compaction, picking a session up or changing vendor makes between turns.
    ///
    /// - Bound: one poll for each write.
    /// - Owner: `crucible-runner`
    /// - Retired: when the turn loop, a compaction, picking a session up and
    ///   changing vendor are asynchronous.
    TurnSession,
    /// The bash tool's confined process: beginning a background result's
    /// acceptance, completing it, and stopping the process.
    ///
    /// - Bound: one poll for each acceptance begun, each acceptance completed
    ///   and each stop.
    /// - Owner: `crucible-builtins`
    /// - Retired: when the bash tool runs asynchronously and so does the
    ///   registry that takes its background commands over.
    BashSandbox,
    /// Starting an MCP server inside its sandbox.
    ///
    /// - Bound: one poll for each of preparing, materializing and starting.
    /// - Owner: `crucible-mcp`
    /// - Retired: when hosting an MCP server is asynchronous.
    McpHosting,
    /// Stopping a hosted program's process once talking to it is over, or
    /// once its pipes could not be taken.
    ///
    /// - Bound: one poll for each stop.
    /// - Owner: `crucible-transport`
    /// - Retired: when the transport to a hosted program is asynchronous.
    TransportProcess,
    /// The local backend's own synchronous paths through the contract: the
    /// Linux backend stopping the process it wraps when a launch is refused,
    /// rolled back, quarantined or stopped, and the conformance audit probing a
    /// backend and preparing the sessions it is asked to refuse or accept.
    ///
    /// - Bound: one poll for each stop, each probe and each preparation.
    /// - Owner: `crucible-sandbox-local`
    /// - Retired: when the local backend is supervised asynchronously and its
    ///   conformance audit runs asynchronously.
    LocalBackend,
    /// What `--sandbox` prints and what `/sandbox enable` checks: probing the
    /// local backend and preparing a session only to read it.
    ///
    /// - Bound: one poll for each probe and each preparation.
    /// - Owner: `crucible-app`
    /// - Retired: when the application runs on one runtime.
    SandboxReport,
    /// The `/sandbox` panel asking the local backend whether it is available.
    ///
    /// - Bound: one poll for each probe.
    /// - Owner: `crucible-code`
    /// - Retired: when the application runs on one runtime.
    SandboxPanel,
    /// The performance probes timing a tool's run the way a turn runs one.
    ///
    /// - Bound: one poll for each run timed.
    /// - Owner: `crucible-code`
    /// - Retired: when the turn loop is asynchronous.
    Probes,
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
            Self::TurnProvider => "asking the model",
            Self::TurnCache => "a prompt-cache step",
            Self::TurnTools => "the turn's tools",
            Self::TurnSession => "writing to the session",
            Self::BashSandbox => "the bash tool's sandbox",
            Self::McpHosting => "starting an MCP server",
            Self::TransportProcess => "stopping a hosted program",
            Self::LocalBackend => "the local sandbox backend",
            Self::SandboxReport => "asking the sandbox what it can enforce",
            Self::SandboxPanel => "asking the sandbox whether it is available",
            Self::Probes => "a probed tool",
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

        assert_eq!(Bridge::TurnTools.cross(answered), Ok(7));
    }

    #[test]
    fn a_future_that_would_wait_is_refused_naming_the_bridge() {
        let refused = Bridge::TurnProvider.cross(std::future::pending::<()>());

        assert_eq!(
            refused,
            Err(Unready {
                bridge: Bridge::TurnProvider
            })
        );
        assert_eq!(
            refused.map_err(|unready| unready.to_string()),
            Err(
                "asking the model would have had to wait, and the caller cannot; the waiting step \
                 was dropped before it answered, so whatever that step began is unconfirmed"
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

        let crossed = Bridge::TurnSession.cross(waiting);

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
                Bridge::SandboxReport.cross(WakesItself { asked: false }),
                Bridge::SandboxReport.cross(async { "ready" }),
            )
        })
        .await
        .map_err(|failed| failed.to_string());

        assert_eq!(
            crossed,
            Ok((
                Err(Unready {
                    bridge: Bridge::SandboxReport
                }),
                Ok("ready")
            )),
            "a crossing on a runtime worker did something other than refuse a \
             future that would wait and hand back one that answered"
        );
    }
}
