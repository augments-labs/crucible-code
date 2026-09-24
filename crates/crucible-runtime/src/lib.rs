//! Work in flight, and the controls that reach it.
//!
//! Three of the things here are what somebody outside a turn says to it while
//! it runs: [`Cancel`] to stop, [`Steer`] to add a line to what it is doing,
//! and [`Aside`] to tell it something that happened. All three are shared
//! cells filled by whoever is outside the turn — in a session, the thread that
//! draws — and none of them interrupts anything: the turn looks, at a boundary
//! it chose.
//!
//! They are here rather than beside the domain because what they are about is
//! a turn being in flight, not what the turn is about. A provider trait that
//! takes a [`Cancel`] is naming a control, and a crate that owns controls can
//! be named by the provider, the tools and the loop alike without any of them
//! reaching the others.
//!
//! The rest is how work is owned. [`Group`] is a set of tasks with one owner
//! that accounts for every one of them: admission is bounded and refused
//! rather than queued, shutdown asks and then aborts rather than waiting on
//! whatever does not cooperate, and every task is accounted for — including
//! the ones that panicked and the ones that had not come back when the group
//! stopped waiting, which the owner is told about rather than blocked on.
//! [`Progress`] is the bounded buffer that carries what a task is saying while
//! it runs, and it drops the oldest rather than the newest and says how many it
//! dropped, because a reader shown a truncated stream that does not say it was
//! truncated has been told something false.
//!
//! Where the service contracts hand back a future and the caller is still
//! synchronous, [`Bridge`] is how it crosses: once, without waiting, and with
//! [`Unready`] where the future would have had to wait; or, for the entries
//! that say they wait, by waiting on a runtime handed to it until the future
//! answers or the turn is cancelled, and with [`Unwaited`] where it cannot. Its
//! variants are the ledger of every such caller, and [`BoxFuture`] is the one
//! shape every contract hands back.
//!
//! Nothing here starts a runtime. A library that built its own would decide
//! for the application how many threads it gets; [`Group`] runs on the runtime
//! of whoever called it, a crossing that polls once enters none, and one that
//! waits enters only the runtime its caller hands it.
//!
//! [`not_worker`] answers one narrower question: whether the code calling it
//! is polled as a spawned task right now, which [`Handle::try_current`] cannot
//! tell apart from the turn thread's own [`Bridge::wait`]. A step not yet
//! built to run on a worker checks it and refuses itself with [`OnWorker`]
//! rather than running where it is not safe to.
//!
//! [`Handle::try_current`]: tokio::runtime::Handle::try_current

mod aside;
mod bridge;
mod cancel;
mod group;
mod progress;
mod steer;
mod worker;

pub use aside::Aside;
#[cfg(feature = "proof")]
#[doc(hidden)]
pub use bridge::__answered;
pub use bridge::{BoxFuture, Bridge, NOTICED, Unready, Unwaited};
pub use cancel::Cancel;
pub use group::{Ended, Full, Group};
pub use progress::{Progress, Told};
pub use steer::Steer;
pub use worker::{OnWorker, not_worker};
