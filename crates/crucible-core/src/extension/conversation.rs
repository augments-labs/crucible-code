//! What one thing said means, and what crucible does about it.
//!
//! The two tables next door keep calls straight; this decides what arriving
//! frames mean against them. It is where the policy lives and where none of the
//! input is trusted, so it is deliberately free of processes, pipes and clocks:
//! everything hostile an extension can do to the conversation can be done to
//! this type from a test, in one line, without spawning anything.
//!
//! The distinction worth reading is between a frame crucible refuses and a
//! frame that ends the conversation. Backpressure is a refusal — the call is
//! named unambiguously, crucible says no, and both ends still agree about what
//! is in flight. Confusion is not. When the far end says something that only
//! makes sense if it disagrees with crucible about which calls exist, there is
//! nothing to reply to that would not compound the disagreement, so the
//! conversation ends instead of guessing.

use serde_json::Value;

use super::calls::{Asked, CallError, Serving};
use super::spoken::{CallId, Outcome, Spoken, Trouble};

/// Why a conversation cannot go on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Broken {
    /// An answer arrived for a call crucible is not waiting on.
    ///
    /// Either it was never made or it was already settled. Both mean the far
    /// end believes something about this conversation that is not so, and an
    /// answer crucible cannot place is one it must not act on.
    #[error("the extension answered call {id}, which crucible is not waiting on")]
    Unmatched {
        /// The identifier that arrived.
        id: CallId,
    },

    /// The extension started a second call under an identifier already in
    /// flight.
    ///
    /// This one cannot be refused politely: a refusal would have to name the
    /// identifier, and naming it would settle the call the extension already
    /// has open under it.
    #[error("the extension started call {id} while it was already in flight")]
    Doubled {
        /// The identifier both calls claim.
        id: CallId,
    },
}

/// What crucible does about one thing said.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next<T> {
    /// A call crucible made was answered; here is what was waiting on it.
    Answer {
        /// What was remembered when the call was made.
        waiting: T,
        /// How it went.
        outcome: Outcome,
    },

    /// The extension is asking for something, and is owed one answer.
    Asked {
        /// Which call to answer.
        id: CallId,
        /// What is being asked for.
        method: Box<str>,
        /// What rides with it, still unread.
        params: Value,
    },

    /// The extension said something that expects nothing back.
    Told {
        /// What happened.
        method: Box<str>,
        /// What rides with it, still unread.
        params: Value,
    },

    /// Send this and carry on.
    Refuse(Spoken),

    /// Stop.
    Stop(Broken),
}

/// One extension's side of a conversation.
///
/// `T` is whatever the host wants back when one of its own calls is answered.
#[derive(Debug)]
pub struct Conversation<T> {
    /// Calls crucible made.
    asked: Asked<T>,
    /// Calls the extension made.
    serving: Serving,
}

impl<T> Default for Conversation<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Conversation<T> {
    /// A conversation with nothing in flight.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            asked: Asked::new(),
            serving: Serving::new(),
        }
    }

    /// What to do about one thing the extension said.
    pub fn heard(&mut self, spoken: Spoken) -> Next<T> {
        match spoken {
            Spoken::Answer { id, outcome } => self
                .asked
                .answered(id)
                .map_or(Next::Stop(Broken::Unmatched { id }), |waiting| {
                    Next::Answer { waiting, outcome }
                }),
            Spoken::Request { id, method, params } => match self.serving.take(id) {
                Ok(()) => Next::Asked { id, method, params },
                Err(CallError::Repeated { .. }) => Next::Stop(Broken::Doubled { id }),
                Err(_) => Next::Refuse(Spoken::Answer {
                    id,
                    outcome: Outcome::Failed(Trouble::ours(CROWDED)),
                }),
            },
            Spoken::Told { method, params } => Next::Told { method, params },
        }
    }

    /// Starts a call of crucible's own, remembering `about` until it is
    /// answered, and returns the frame to send.
    ///
    /// # Errors
    ///
    /// [`CallError`] where crucible is already waiting on as many calls as it
    /// allows, or has no identifiers left.
    pub fn ask(
        &mut self,
        method: impl Into<Box<str>>,
        params: Value,
        about: T,
    ) -> Result<(CallId, Spoken), CallError> {
        let id = self.asked.ask(about)?;
        Ok((
            id,
            Spoken::Request {
                id,
                method: method.into(),
                params,
            },
        ))
    }

    /// Answers a call the extension made, and returns the frame to send.
    ///
    /// # Errors
    ///
    /// [`CallError::Unknown`] where that is not a call crucible took on, which
    /// covers answering one twice.
    pub fn answer(&mut self, id: CallId, outcome: Outcome) -> Result<Spoken, CallError> {
        self.serving.answered(id)?;
        Ok(Spoken::Answer { id, outcome })
    }

    /// Everything crucible was waiting on, once the conversation is over.
    ///
    /// Whatever was waiting has to become a failure the host reports. Nothing
    /// else will answer these.
    pub fn ended(&mut self) -> Vec<(CallId, T)> {
        self.asked.abandoned()
    }
}

/// What crucible tells an extension that is asking faster than it is answered.
///
/// It names the ceiling, because the extension's author is the one who has to
/// do something about it. The number is written out rather than interpolated,
/// so this stays a literal crucible can build without a step that might fail;
/// a test holds it to [`crate::EXTENSION_CALLS`] and to what one frame carries.
const CROWDED: &str = "crucible is already carrying 64 calls from this extension";

#[cfg(test)]
mod tests;
