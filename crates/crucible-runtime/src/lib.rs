//! The controls that reach a turn while it runs.
//!
//! Each of these is what somebody outside a turn says to it while it is in
//! flight: [`Cancel`] to stop, [`Steer`] to add a line to what it is doing,
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

mod aside;
mod cancel;
mod steer;

pub use aside::Aside;
pub use cancel::Cancel;
pub use steer::Steer;
