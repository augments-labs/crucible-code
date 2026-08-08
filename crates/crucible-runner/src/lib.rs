//! Drives turns to completion.
//!
//! The runner streams deltas from a provider, dispatches the tool calls the
//! model asks for, feeds the results back, and repeats until the model yields
//! or the user cancels.
//!
//! It depends on `crucible-core` alone. Every collaborator arrives as a trait
//! object chosen during wiring, so the loop never names Anthropic, `OpenAI`,
//! `grep`, or a renderer. Swapping any of them is a change in `main.rs`.
//!
//! Two things leave this crate, and they leave by different routes. *Progress*
//! — words arriving, a tool starting, a tool finishing — goes out as events,
//! because the thread that draws is not the thread that runs. The *outcome* of
//! a turn is the return value, because the caller is what decides whether the
//! session goes on.

mod answer;
#[cfg(test)]
mod fake;
mod runner;
#[cfg(test)]
mod sample;
mod session;
mod tools;
mod work;

use std::sync::mpsc::Sender;

use crucible_core::Event;

pub use runner::{Model, Runner};
pub use session::{Session, SessionError};
pub use tools::Tools;

/// Reports something that happened.
///
/// A closed channel means the terminal is gone, which happens only while the
/// process is shutting down — and that path raises [`crucible_core::Cancel`],
/// which is what actually stops the work. There is nothing useful to do here.
fn post(events: &Sender<Event>, event: Event) {
    drop(events.send(event));
}
