//! Runs somebody else's program under confinement and keeps talking to it.
//!
//! Everything an extension *is* was decided before this crate is reached: a
//! manifest was read, a trust decision was made, and a capability grant was
//! written down. What is left is the part that has a process in it — starting
//! one under the sandbox, hearing what it says, saying what crucible owes back,
//! and deciding when a peer that has gone quiet is a peer that has stopped.
//!
//! Core alone, deliberately. A host that named a concrete sandbox backend or
//! read a settings document would be deciding what it is only supposed to run.

mod heard;

pub use heard::Heard;
