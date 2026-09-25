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
//! Most crossings poll a future they own once, on the caller's own stack, with
//! a waker that wakes nothing. A future that answers when it is first asked is
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
//! # Waiting
//!
//! A caller that has to let a future wait crosses with [`Bridge::wait`]
//! instead, the one other kind of crossing there is. It polls on the caller's
//! own thread too, entered into the application's runtime, whose workers run
//! the drivers the future waits on — a timer and an I/O driver; it asks the
//! future again each time the future wakes it, and it looks at the turn's
//! [`Cancel`] before each poll and at least every [`NOTICED`] between them.
//! The future answers, or the cancel is raised and the future is dropped, and
//! nothing of it is spawned or outlives the call. It refuses, with
//! [`Unwaited`], where there is no runtime to wait on and where the caller is
//! already on a thread a runtime runs: waiting there would hold a thread the
//! wait itself may need.
//!
//! What bounds a wait is therefore the cancel and whatever the future bounds
//! itself by — a deadline, a quiet tick — and never this crossing, which is why
//! each waiting entry says what bounds its wait in words.
//!
//! # The ledger
//!
//! [`Bridge`] is the whole ledger. Each variant is one crossing, and its
//! documentation says whether it polls once or waits, what bounds it and, for
//! one that waits, what bounds its wait, which crate owns it and what retires
//! it. A crate crosses only the bridges it owns, by the kind its entry says,
//! every bridge is crossed somewhere, and a bridge nothing crosses any more is
//! deleted rather than kept; the repository checks hold all of that against
//! the code.

use std::fmt;
use std::future::{Future, IntoFuture};
use std::pin::{Pin, pin};
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};
use std::time::Duration;

use tokio::runtime::Handle;

use crate::Cancel;

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
/// Each entry says which kind of crossing it is. One that polls once is
/// bounded the same way as every other such — one poll of a future it owns,
/// nothing queued, nothing kept — and the bound it states is what that one
/// poll covers. One that waits states what bounds its wait as well, since
/// nothing in the crossing does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bridge {
    /// The application waiting, on the thread that takes it, for a turn or
    /// a compaction it was asked for: the whole of the runner's asynchronous
    /// turn or compaction, polled on that thread and never spawned. And,
    /// after picking a session up or changing vendor, for the session to take
    /// the lines clearing what a vendor may not be sent owes it.
    ///
    /// - Crossing: waits.
    /// - Bound: one wait for each turn, each compaction, and each pick-up or
    ///   change of vendor.
    /// - Wait bounded by: for the owed lines, the session taking each of them,
    ///   as long as its store's own writes take. For a turn or a compaction,
    ///   the turn's own cancel, as far as the steps the turn awaits heed it.
    ///   The turn looks at that cancel between steps and hands it to every
    ///   step it awaits — the provider's stream and each read of it, every
    ///   call's run, and the toolset's preparation, listing, refreshing and
    ///   disposal — and how soon a step still waiting heeds it is that step's
    ///   own contract. A call's tool
    ///   deadline makes that call's cancel read as raised, which a run learns
    ///   at its next look or through a race on that cancel, so the deadline
    ///   is cooperative; no run is dropped part way, so what bounds a run
    ///   past its deadline or a stop is how soon it heeds its cancel, and a
    ///   run that never does is waited for until it answers. The turn's
    ///   session writes take no cancel: each waits for the session's writer
    ///   to take its line, as long as the log's own writes take, and answers
    ///   once that writer has gone, however it went. What the run keeps
    ///   bounds how much the turn does rather than how long a step waits: its
    ///   retry attempts, whose pauses heed the cancel, and its response,
    ///   tool-output and spend ceilings. The crossing itself waits under a
    ///   cancel nothing raises, so a stop ends the turn through its own
    ///   ending rather than by dropping it at a step, which could leave a
    ///   recorded call without its result. The calls' runs are spawned onto
    ///   the runtime's workers, a bounded few at once; the turn itself is
    ///   still polled here.
    /// - Owner: `crucible-app`
    /// - Retired: when the application awaits a turn directly.
    AppTurn,
    /// The runner's prompt-cache resources: a provider's lifecycle calls and
    /// the store their records are kept in. They are reached by a turn
    /// preparing its request, and by a compaction preparing its recap request
    /// inside a turn or between turns; by the cleanup pass, both when a user
    /// asks for it and when it runs as the retirement before a change of model
    /// or provider, a login or a logout; and by the listing a user's cache
    /// inspection reads.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each lifecycle call and each store operation, the
    ///   inspection's listing among them.
    /// - Owner: `crucible-runner`
    /// - Retired: when the turn loop, a compaction, the runner's cache
    ///   inspection and its cleanup pass are asynchronous.
    TurnCache,
    /// The bash tool's confined process: beginning a background result's
    /// acceptance, completing it, and stopping the process.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each acceptance begun, each acceptance completed
    ///   and each stop.
    /// - Owner: `crucible-builtins`
    /// - Retired: when the bash tool runs asynchronously and so does the
    ///   registry that takes its background commands over.
    BashSandbox,
    /// What `--sandbox` prints and what `/sandbox enable` checks: probing the
    /// local backend and preparing a session only to read it.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each probe and each preparation.
    /// - Owner: `crucible-app`
    /// - Retired: when the application runs on one runtime.
    SandboxReport,
    /// The `/sandbox` panel asking the local backend whether it is available.
    ///
    /// - Crossing: polls once.
    /// - Bound: one poll for each probe.
    /// - Owner: `crucible-code`
    /// - Retired: when the application runs on one runtime.
    SandboxPanel,
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

    /// Waits for `future` to answer, on `on`'s runtime, until it does or
    /// `cancel` is raised.
    ///
    /// The future is polled on the caller's own thread, entered into the
    /// runtime `on` names so that what it builds against a runtime — a timer,
    /// or a socket where that runtime was built with an I/O driver — registers
    /// with that runtime's drivers, and it is asked again each time it wakes
    /// the caller. It is never spawned, and nothing of it outlives this call.
    /// Between two polls the caller's thread sleeps, and it looks at `cancel`
    /// before every poll and at least every [`NOTICED`], so a cancel is heeded
    /// within that of being raised — as long as the future itself returns from
    /// each poll, since its own code runs on the caller's thread inside it.
    ///
    /// What drives those drivers is the runtime's own workers, never this
    /// thread: `on` must name a runtime whose workers run its drivers, which a
    /// current-thread runtime's do not. The application's
    /// runtime is built multi-thread for that reason; its module says where
    /// Tokio says so.
    ///
    /// # Errors
    ///
    /// - [`Unwaited::Cancelled`] where `cancel` was raised before the future
    ///   answered. The future is dropped before this returns, inside the
    ///   runtime still, and what that leaves behind is its own contract's to
    ///   say.
    /// - [`Unwaited::InsideRuntime`] where the caller is on a thread a runtime
    ///   runs or has entered — a worker, a blocking thread, or a thread inside
    ///   another wait. Waiting there would hold a thread the wait may need,
    ///   which is how a runtime deadlocks on itself.
    /// - [`Unwaited::NoRuntime`] where `on` is `None`.
    ///
    /// A refused future is dropped without being polled, so an `async` step
    /// has begun nothing; a future made by a call that did its work before
    /// handing the future back has begun that much.
    ///
    /// # Panics
    ///
    /// Where the future itself panics, which unwinds into the caller exactly
    /// as a panic inside [`Bridge::cross`]'s one poll does.
    pub fn wait<F: IntoFuture>(
        self,
        on: Option<&Handle>,
        cancel: &Cancel,
        future: F,
    ) -> Result<F::Output, Unwaited> {
        if Handle::try_current().is_ok() {
            return Err(Unwaited::InsideRuntime(self));
        }
        let Some(runtime) = on else {
            return Err(Unwaited::NoRuntime(self));
        };

        // Entered before the future is made or polled, so whatever it builds
        // is built against this runtime, and left only after the future has
        // been dropped: it is declared first, and so is dropped last.
        let _entered = runtime.enter();
        let waker = Waker::from(Arc::new(Unparks(thread::current())));
        let mut asked = Context::from_waker(&waker);
        let mut future = pin!(future.into_future());
        loop {
            if cancel.requested() {
                return Err(Unwaited::Cancelled(self));
            }
            if let Poll::Ready(answer) = future.as_mut().poll(&mut asked) {
                return Ok(answer);
            }
            thread::park_timeout(NOTICED);
        }
    }

    /// What is crossed, in words a reader of an error can follow.
    const fn crossing(self) -> &'static str {
        match self {
            Self::AppTurn => "a turn or a compaction",
            Self::TurnCache => "a prompt-cache step",
            Self::BashSandbox => "the bash tool's sandbox",
            Self::SandboxReport => "asking the sandbox what it can enforce",
            Self::SandboxPanel => "asking the sandbox whether it is available",
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

/// How long a waiting crossing may go without looking at the turn's
/// [`Cancel`], which is also the longest it sleeps between two polls of a
/// future that has not woken it.
///
/// Short against a person pressing a key and waiting to see the turn stop, and
/// long against a thread waking only to find nothing to do.
pub const NOTICED: Duration = Duration::from_millis(20);

/// Wakes the thread a waiting crossing is polling on.
struct Unparks(Thread);

impl Wake for Unparks {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Why a waiting crossing handed back no answer, naming the bridge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unwaited {
    /// The turn's [`Cancel`] was raised before the future answered, and the
    /// future was dropped, leaving behind what its contract says dropping it
    /// leaves.
    Cancelled(Bridge),
    /// The caller is on a thread a runtime runs or has entered, where waiting
    /// would hold a thread the wait may need. The future was dropped without
    /// being polled.
    InsideRuntime(Bridge),
    /// There was no runtime to wait on. The future was dropped without being
    /// polled.
    NoRuntime(Bridge),
}

impl Unwaited {
    /// The bridge the caller was crossing.
    #[must_use]
    pub fn bridge(&self) -> Bridge {
        match *self {
            Self::Cancelled(bridge) | Self::InsideRuntime(bridge) | Self::NoRuntime(bridge) => {
                bridge
            }
        }
    }
}

impl fmt::Display for Unwaited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let crossing = self.bridge().crossing();
        match self {
            Self::Cancelled(_) => write!(
                f,
                "{crossing} was stopped before it answered; the waiting step was dropped, so \
                 whatever that step began is unconfirmed"
            ),
            Self::InsideRuntime(_) => write!(
                f,
                "{crossing} would have had to wait on a thread the runtime runs, where waiting \
                 holds a thread the wait may need; the step was dropped before it was asked \
                 anything"
            ),
            Self::NoRuntime(_) => write!(
                f,
                "{crossing} would have had to wait, and there is no runtime to wait on; the step \
                 was dropped before it was asked anything"
            ),
        }
    }
}

impl std::error::Error for Unwaited {}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::task::{Context, Poll};

    use super::{BoxFuture, Bridge, Unready, Unwaited};
    use crate::Cancel;

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

        assert_eq!(Bridge::TurnCache.cross(answered), Ok(7));
    }

    #[test]
    fn a_future_that_would_wait_is_refused_naming_the_bridge() {
        let refused = Bridge::TurnCache.cross(std::future::pending::<()>());

        assert_eq!(
            refused,
            Err(Unready {
                bridge: Bridge::TurnCache
            })
        );
        assert_eq!(
            refused.map_err(|unready| unready.to_string()),
            Err(
                "a prompt-cache step would have had to wait, and the caller cannot; the waiting \
                 step was dropped before it answered, so whatever that step began is \
                 unconfirmed"
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

        let crossed = Bridge::TurnCache.cross(waiting);

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

    /// A runtime whose one worker drives its timer, as the application's does.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_time()
            .build()
            .unwrap()
    }

    #[test]
    fn a_waiting_crossing_is_handed_back_what_a_future_that_had_to_wait_answered() {
        let runtime = runtime();

        let slept = Bridge::AppTurn.wait(Some(runtime.handle()), &Cancel::new(), async {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            7
        });
        let woken = Bridge::AppTurn.wait(
            Some(runtime.handle()),
            &Cancel::new(),
            WakesItself { asked: false },
        );

        assert_eq!(
            (slept, woken),
            (Ok(7), Ok(())),
            "a waiting crossing gave up on a future that would have answered"
        );
    }

    /// Pending the first `left` times it is asked, waking whoever asked each
    /// time before it says so, and answering after.
    struct WakesEachTime {
        left: u32,
    }

    impl Future for WakesEachTime {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.left == 0 {
                return Poll::Ready(());
            }
            self.left -= 1;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    }

    /// A wake is what asks the future again, and the fallback look at the
    /// cancel is not: a future woken a hundred times in a row is asked a
    /// hundred times at once, where a crossing that ignored its waker would
    /// sleep [`super::NOTICED`] before each, two seconds in all. Half of that
    /// is the bound, which a crossing that answers wakes is under by orders of
    /// magnitude.
    #[test]
    fn a_waiting_crossing_asks_again_as_soon_as_it_is_woken() {
        const WAKES: u32 = 100;
        let runtime = runtime();
        let begun = std::time::Instant::now();

        let waited = Bridge::AppTurn.wait(
            Some(runtime.handle()),
            &Cancel::new(),
            WakesEachTime { left: WAKES },
        );
        let took = begun.elapsed();

        assert_eq!(waited, Ok(()));
        assert!(
            took < super::NOTICED * WAKES / 2,
            "a future that woke the crossing {WAKES} times took {took:?}, as though each wake \
             were ignored until the next look at the cancel"
        );
    }

    /// Says, when it is dropped, whether the turn had been cancelled by then.
    struct DroppedAfter {
        cancel: Cancel,
        cancelled: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl Drop for DroppedAfter {
        fn drop(&mut self) {
            self.cancelled
                .store(self.cancel.requested(), Ordering::Release);
            self.dropped.store(true, Ordering::Release);
        }
    }

    #[test]
    fn a_waiting_crossing_drops_its_future_once_the_turn_is_cancelled() {
        let runtime = runtime();
        let cancel = Cancel::new();
        let cancelled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        let held = DroppedAfter {
            cancel: cancel.clone(),
            cancelled: Arc::clone(&cancelled),
            dropped: Arc::clone(&dropped),
        };
        let raising = cancel.clone();
        let raiser = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            raising.request();
        });

        let waited = Bridge::AppTurn.wait(Some(runtime.handle()), &cancel, async move {
            let _held = held;
            std::future::pending::<()>().await;
        });
        raiser.join().unwrap();

        assert_eq!(waited, Err(Unwaited::Cancelled(Bridge::AppTurn)));
        assert!(
            dropped.load(Ordering::Acquire),
            "the cancelled future was still alive after the crossing returned"
        );
        assert!(
            cancelled.load(Ordering::Acquire),
            "the future was dropped before the turn was cancelled, so the crossing never waited"
        );
    }

    #[test]
    fn a_waiting_crossing_asks_nothing_of_a_turn_already_cancelled() {
        let runtime = runtime();
        let cancel = Cancel::new();
        cancel.request();
        let polled = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&polled);

        let waited = Bridge::AppTurn.wait(Some(runtime.handle()), &cancel, async move {
            seen.store(true, Ordering::Release);
        });

        assert_eq!(waited, Err(Unwaited::Cancelled(Bridge::AppTurn)));
        assert!(
            !polled.load(Ordering::Acquire),
            "a turn already cancelled had its step started anyway"
        );
    }

    /// A worker is where waiting would hold the thread the wait needs, so the
    /// crossing is made in a task spawned onto one, and refused there without
    /// the future being asked anything.
    #[test]
    fn a_waiting_crossing_on_a_runtime_worker_refuses_without_polling() {
        let runtime = runtime();
        let handle = runtime.handle().clone();
        let polled = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&polled);

        let refused = runtime
            .block_on(async move {
                tokio::spawn(async move {
                    Bridge::SandboxReport.wait(Some(&handle), &Cancel::new(), async move {
                        seen.store(true, Ordering::Release);
                    })
                })
                .await
            })
            .map_err(|failed| failed.to_string());

        assert_eq!(
            refused,
            Ok(Err(Unwaited::InsideRuntime(Bridge::SandboxReport)))
        );
        assert!(
            !polled.load(Ordering::Acquire),
            "a refused future was polled on the worker it was refused on"
        );
    }

    #[test]
    fn a_waiting_crossing_with_no_runtime_refuses_without_polling() {
        let polled = Arc::new(AtomicBool::new(false));
        let seen = Arc::clone(&polled);

        let refused = Bridge::AppTurn.wait(None, &Cancel::new(), async move {
            seen.store(true, Ordering::Release);
        });

        assert_eq!(refused, Err(Unwaited::NoRuntime(Bridge::AppTurn)));
        assert!(
            !polled.load(Ordering::Acquire),
            "a future with no runtime to wait on was polled anyway"
        );
    }

    #[test]
    fn a_waiting_crossing_says_why_it_handed_nothing_back() {
        assert_eq!(
            [
                Unwaited::Cancelled(Bridge::AppTurn),
                Unwaited::InsideRuntime(Bridge::AppTurn),
                Unwaited::NoRuntime(Bridge::AppTurn),
            ]
            .map(|unwaited| (unwaited.bridge(), unwaited.to_string())),
            [
                (
                    Bridge::AppTurn,
                    "a turn or a compaction was stopped before it answered; the waiting step was \
                     dropped, so whatever that step began is unconfirmed"
                        .to_owned()
                ),
                (
                    Bridge::AppTurn,
                    "a turn or a compaction would have had to wait on a thread the runtime runs, \
                     where waiting holds a thread the wait may need; the step was dropped \
                     before it was asked anything"
                        .to_owned()
                ),
                (
                    Bridge::AppTurn,
                    "a turn or a compaction would have had to wait, and there is no runtime to \
                     wait on; the step was dropped before it was asked anything"
                        .to_owned()
                ),
            ]
        );
    }

    fn comes_apart() -> u8 {
        panic!("came apart")
    }

    fn payload(unwound: std::thread::Result<Result<u8, impl Sized>>) -> Option<&'static str> {
        unwound
            .err()
            .and_then(|payload| payload.downcast_ref::<&'static str>().copied())
    }

    /// A crossing has always polled on the caller's own stack, so a future
    /// coming apart unwinds into the caller with what it came apart with. A
    /// waiting crossing polls there too, and leaves the runtime it waited on
    /// able to answer the next one.
    #[test]
    fn a_future_coming_apart_unwinds_into_the_caller_as_a_crossing_always_has() {
        let runtime = runtime();

        let crossed = std::panic::catch_unwind(|| Bridge::AppTurn.cross(async { comes_apart() }));
        let waited = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Bridge::AppTurn.wait(Some(runtime.handle()), &Cancel::new(), async {
                comes_apart()
            })
        }));

        assert_eq!(payload(crossed), Some("came apart"));
        assert_eq!(payload(waited), Some("came apart"));
        assert_eq!(
            Bridge::AppTurn.wait(Some(runtime.handle()), &Cancel::new(), async { 3 }),
            Ok(3),
            "the runtime a future came apart on would not answer the next crossing"
        );
    }
}
