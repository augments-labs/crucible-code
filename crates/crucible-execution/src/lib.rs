//! The contract every agent a conversation can run on implements.
//!
//! A [`Backend`] is an agent behind crucible's interface: crucible's own
//! runner is one, and another program that runs turns of its own is another.
//! This crate says what any of them must answer — what it is and what it
//! claims to do ([`Description`], [`Capabilities`]), how a session is opened
//! and a turn run, what a turn reports while it runs ([`Event`]), how a
//! question it puts to the user is answered ([`PendingRef`], [`Decision`]),
//! and the facts about an instance that decide whether it may run at all
//! ([`Lifecycle`]) — and nothing about how any one of them does it. It names
//! no provider, runner, vendor, protocol or process, and depends on
//! `crucible-runtime` and `crucible-types` alone, so a backend added later
//! changes nothing here.
//!
//! A claim is a promise the contract holds a backend to. An operation whose
//! capability a backend does not claim as supported is refused with
//! [`Unsupported`], never answered `Ok`, and [`conformance`] checks exactly
//! that against any backend handed to it. A claim read from a cached
//! observation is never support: see [`Capabilities::observed`].
//!
//! Everything a backend reports is bounded before it is kept: text arrives as
//! a [`Text`] that clips itself and says so, and pending actions sit in a table
//! of at most [`PENDINGS`].

mod backend;
mod capability;
mod event;
mod lifecycle;
mod pending;

pub use backend::{
    Backend, Description, Error, Failure, NAME_BYTES, Nonconformance, Outcome, SessionRef, Sink,
    TurnRequest, Unsupported, conformance,
};
pub use capability::{
    Capabilities, Capability, Claim, Confined, Confinement, Freshness, Observation, Scope,
};
pub use event::{Cost, Currency, Effects, Event, Fault, TEXT_BYTES, Text, Usage};
pub use lifecycle::{
    Admission, Authentication, Ending, Lifecycle, Operation, Refused, Running, Step,
};
pub use pending::{
    Decision, Full, Generation, Instance, Lasting, PENDINGS, Pending, PendingRef, Pendings,
    RequestId, Stale,
};

#[cfg(test)]
mod tests;
