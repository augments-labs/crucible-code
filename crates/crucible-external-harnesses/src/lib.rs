//! Another vendor's coding agent, run as a supervised process behind the
//! execution contract.

pub mod acp;
pub mod antigravity;
pub mod claude;
#[cfg(feature = "proof")]
pub mod conformance;
#[cfg(feature = "proof")]
pub mod fake_peer;
mod host;
mod journal;
mod launch;
mod lifecycle;
mod pending;

pub use antigravity::Ended;
pub use host::{
    Account, Act, CANCEL_GRACE, Control, Dialect, Host, PREPARING, RAN_BYTES, Ran, Reading,
    SILENCE, STOP_GRACE, Shape, Told, Undecided, Unstopped, WAITING, WITHHELD_BYTES, stripped,
};
pub use journal::{
    Entries, HISTORY, JOURNAL_BYTES, Journal, JournalError, REFERENCE_BYTES, Record, Stage, Work,
};
pub use launch::{DIGESTED, Discovery, Held, Launch, Program, RESUME_BYTES, Refusal, Request};
pub use lifecycle::{Contended, Coordinator, Slot, needs_recovery};
pub use pending::{Asked, NATIVE_BYTES};

#[cfg(test)]
mod tests;
