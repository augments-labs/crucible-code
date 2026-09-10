//! Work in flight, and the controls that reach it.
//!
//! Three of the things here are what somebody outside a turn says to it while
//! it runs: [`Cancel`] to stop, [`Steer`] to add a line to what it is doing,
//! and [`Aside`] to tell it something that happened. All three are shared
//! cells with one producer on the thread that draws and one consumer on the
//! thread the turn runs on, and none of them interrupts anything: the turn
//! looks, at a boundary it chose.
//!
//! They are here rather than beside the domain because what they are about is
//! a turn being in flight, not what the turn is about. A provider trait that
//! takes a [`Cancel`] is naming a control, and a crate that owns controls can
//! be named by the provider, the tools and the loop alike without any of them
//! reaching the others.
//!
//! The rest is how work is owned. [`Group`] is a set of tasks with one owner
//! and no way to outlive it: admission stops, what was started is cancelled,
//! and every task's end is accounted for by name — including the ones that
//! panicked. [`Progress`] is the bounded buffer that carries what a task is
//! saying while it runs, and it drops the oldest rather than the newest and
//! says how many it dropped, because a reader shown a truncated stream that
//! does not say it was truncated has been told something false.
//!
//! Nothing here starts a runtime. A library that built its own would decide
//! for the application how many threads it gets and would deadlock the moment
//! two of them nested; [`Group`] runs on the runtime of whoever called it.

mod aside;
mod cancel;
mod group;
mod progress;
mod steer;

pub use aside::Aside;
pub use cancel::Cancel;
pub use group::{Ended, Full, Group};
pub use progress::Progress;
pub use steer::Steer;
