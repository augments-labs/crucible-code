//! The application as a front end drives it: requests in, outcomes out.
//!
//! `crucible-client-api` says what may be asked and what may be answered, in
//! values that can leave the process. This module is the other half: it takes
//! a [`Request`](crucible_client_api::Request) that has already been read and
//! checked, carries it out against a [`Conversation`](crate::Conversation),
//! and says what came of it. A terminal and a consumer with no terminal at all
//! come through the same doors, so neither can do something the other cannot,
//! and what either did is the same to the conversation.
//!
//! Several doors rather than one, because a conversation is somewhere else
//! while a turn runs. [`perform`] answers at once, on the thread that holds the
//! conversation and the host's files. [`turn`] is the long one: it runs where
//! the conversation has been lent for the length of a turn, and stops on every
//! [`Pending`](crucible_client_api::Pending) action until a [`Front`] answers
//! it. [`interrupt`] and [`keep`] need no conversation at all, which is the
//! point of them: a turn is stopped, and a look is written down, while the
//! conversation is away.
//!
//! What comes back is the application's own value — [`Performed`], [`Ended`] —
//! and not yet the contract's. A terminal draws from the first, because a
//! sentence on a screen is made from the whole error and not from the cut copy
//! a client is sent; a client is sent [`Performed::outcome`]. Both are read off
//! one value, so they cannot say different things happened.
//!
//! Nothing that arrives in a request is an authority. A provider is a name
//! looked up in the registry the host lent; a session is an identity looked up
//! under the workspace the host lent; a decision is a word about a pending
//! action, held against the one this module itself put, and the verdict the
//! permission engine acts on is made here from the engine's own type — see
//! [`deciding`].

pub mod deciding;
mod performing;
mod reading;
#[cfg(test)]
mod tests;
mod turning;

pub use deciding::{Deciding, Front, Shown, TRIES, questions};
pub use performing::{Cleared, Desk, Performed, Resumed, keep, perform};
pub use reading::{mode_out as mode, progress, rung, snapshot};
pub use turning::{Ended, interrupt, turn};
