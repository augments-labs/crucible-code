//! What a front end may say to the application, and what it is told back.
//!
//! A terminal, a script and anything later that is neither all drive the same
//! application. This crate is the form their side of that takes once it has to
//! be written down: a [`Request`] carrying one [`Command`], the [`Response`]
//! carrying what came of it, the [`Snapshot`] of what is now true, the
//! [`Progress`] that is only true so far, and the [`Pending`] action a turn
//! stops on until a [`Decision`] naming it arrives.
//!
//! Three things are decided here and nowhere else.
//!
//! **What crosses.** Every value is a closed enum or a bounded struct with a
//! hand-written JSON form, and a field a decoder does not know refuses the
//! whole frame. No field here is a path, a credential, a workspace, a trait
//! object or an error's internals: a failure crosses as an [`ErrorCode`] and at
//! most the sentence its owner would have shown a person, cut to
//! [`bounds::TEXT_BYTES`]. Text a person or a model wrote is redacted from
//! `Debug`. Words for a reader are cut and say so ([`Text`]); words the host
//! will act on are whole or refused ([`Said`], [`Prompt`]).
//!
//! **What is refused before it is read.** A frame over
//! [`bounds::FRAME_BYTES`] is refused by its length, before a parser sees it,
//! and one with too many entries in a list, nested too deep, made of more
//! values than [`bounds::VALUES`] or saying a key twice is refused where the
//! parser meets that; a version this build does not speak — in a request, a
//! response, a snapshot or a progress report, each of which says its own — a
//! capability it has not heard of and a command it does not ship are each
//! refused with their own stable code rather than guessed at.
//!
//! **What is authoritative.** [`Snapshot`] and [`Outcome`] say what the
//! application settled; [`Progress`] says what a turn has produced so far and
//! may never be complete. They are different types with no conversion between
//! them, so a stream of deltas cannot be mistaken for a transcript.
//!
//! What stays in `crucible-app` is everything that acts: performing a command,
//! translating a domain outcome or a runner event into the values here, and
//! settling a [`Decision`] against the one action that is actually pending. A
//! decision is a client's word, never a permission: nothing in this crate can
//! name, build or stand in for the proof a tool runs under.
//!
//! No listener, socket or process is opened here. The crate is values and
//! their validation, and every client that exists runs in the host's process,
//! at the hands of the person the host belongs to. Three things are as they are
//! because of that, and an adapter that carried these values out of the process
//! would have to decide each of them first:
//!
//! - A [`Problem`]'s message and a permission question's subject are sentences
//!   the host wrote for that person. No field is a path, but either sentence
//!   can spell one — the file a tool wants to write, the file that could not be
//!   opened — and neither is scrubbed here.
//! - A [`PendingId`] is the next number from a counter. It is used once and
//!   names nothing afterwards, which is what settling relies on; it is not a
//!   secret and must not be treated as one. A decision is matched to a pending
//!   action by that number and by the kind of question it answers, and by
//!   nothing else, so an adapter discards every decision that reaches it
//!   before it has put the action the decision names: one sent ahead, naming
//!   the number that comes next, was written by somebody who was never shown
//!   the action and would otherwise settle it.
//! - Changing the permission mode, turning the sandbox requirement off, and
//!   logging in or out are plain commands behind no [`Capability`], because a
//!   person at the terminal can do all of them. Capabilities say what a client
//!   can take part in, not what it is allowed to change. Nothing that ships
//!   reads a [`Request`] from bytes, which is the only reason that is safe,
//!   and the repository's checks fail when something starts to without each
//!   of those commands having been decided about for it.

pub mod bounds;
pub mod command;
pub mod error;
pub mod outcome;
pub mod pending;
pub mod progress;
pub mod request;
pub mod snapshot;
mod wire;

pub use bounds::{Name, Said, Text};
pub use command::{Command, Mode, Palette, Prompt, Rung, Theme};
pub use error::{ErrorCode, Refusal};
pub use outcome::{
    CacheOutcome, CleanOutcome, ClearOutcome, EffortOutcome, LoginOutcome, LogoutOutcome,
    ModelOutcome, Outcome, Problem, Resource, Response, ResumeOutcome, Retained, RoomOutcome,
    SandboxOutcome, Standing, Stop, ThemeOutcome, TurnOutcome,
};
pub use pending::{Asked, Choice, Decision, Effect, Lasting, Pending, PendingId, Picked, Ruling};
pub use progress::Progress;
pub use request::{Capabilities, Capability, Correlation, Refused, Request, Version};
pub use snapshot::{Percent, Snapshot};

#[cfg(test)]
mod tests;
