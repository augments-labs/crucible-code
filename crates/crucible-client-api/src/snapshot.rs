//! What is true of a conversation between commands: authoritative, and whole.
//!
//! A [`Snapshot`] is read off the conversation itself, never assembled from
//! the [`Progress`](crate::Progress) that went by, so a client that missed
//! every report and one that saw them all read the same snapshot. It says
//! where the conversation stands — which session, who is being asked, under
//! what mode, how much has been said, what it is waiting on — and holds none
//! of what was said: the transcript is the runner's one growing value and a
//! snapshot is not a second copy of it.

use std::str::FromStr;

use crucible_types::SessionId;
use serde_json::Value;

use crate::bounds::{Name, Text};
use crate::command::{Mode, Rung};
use crate::error::{ErrorCode, Refusal};
use crate::pending::Pending;
use crate::wire::{Fields, Writing, frame, parsed};

/// Where a conversation stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The session being recorded into, where the run is being kept at all.
    pub session: Option<SessionId>,
    /// The provider being asked, by its registry name, where anybody is.
    pub provider: Option<Name>,
    /// The model in force; empty where none is.
    pub model: Text,
    /// The effort in force, where one was asked for.
    pub effort: Option<Rung>,
    /// The permission mode in force.
    pub mode: Mode,
    /// How many messages the conversation holds.
    pub messages: u64,
    /// How many of them are turns a person took.
    pub turns: u64,
    /// How many tokens the next request would carry.
    pub carrying: u64,
    /// What percentage of the model's window is left, where that is known.
    pub left: Option<u8>,
    /// What the conversation is waiting on, where it is waiting.
    pub pending: Option<Pending>,
}

impl Snapshot {
    /// The value this travels as.
    #[must_use]
    pub fn written(&self) -> Value {
        let standing = Writing::new()
            .maybe("session", self.session.as_ref().map(SessionId::as_str))
            .maybe("provider", self.provider.as_ref().map(Name::as_str))
            .text("model", &self.model)
            .maybe("effort", self.effort.map(Rung::as_str))
            .with("mode", self.mode.as_str())
            .with("messages", self.messages)
            .with("turns", self.turns)
            .with("carrying", self.carrying)
            .maybe("left", self.left)
            .maybe("pending", self.pending.as_ref().map(Pending::written))
            .finish();

        Writing::new().with("snapshot", standing).finish()
    }

    /// The frame this snapshot travels as.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] where the frame would be over the ceiling.
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        frame(&self.written())
    }

    /// The snapshot `bytes` spell.
    ///
    /// A frame that is progress is refused here: a snapshot is named by its
    /// own field, which progress does not have.
    ///
    /// # Errors
    ///
    /// [`Refusal`] for anything but one whole, bounded snapshot.
    pub fn decode(bytes: &[u8]) -> Result<Self, Refusal> {
        let mut frame = Fields::of(parsed(bytes)?)?;
        let mut fields = Fields::of(frame.take("snapshot")?)?;
        frame.done()?;

        let snapshot = Self {
            session: fields
                .maybe("session")
                .map(|value| SessionId::from_str(value.as_str().unwrap_or_default()))
                .transpose()
                .map_err(|_| Refusal::new(ErrorCode::InvalidArgument))?,
            provider: fields
                .maybe("provider")
                .map(|value| Name::new(value.as_str().unwrap_or_default()))
                .transpose()?,
            model: fields.text("model")?,
            effort: fields
                .maybe("effort")
                .map(|value| value.as_str().unwrap_or_default().parse())
                .transpose()?,
            mode: fields.string("mode")?.parse()?,
            messages: fields.number("messages")?,
            turns: fields.number("turns")?,
            carrying: fields.number("carrying")?,
            left: fields
                .maybe_number("left")?
                .map(|left| u8::try_from(left).map_err(|_| Refusal::new(ErrorCode::Malformed)))
                .transpose()?,
            pending: fields.maybe("pending").map(Pending::read).transpose()?,
        };
        fields.done()?;
        Ok(snapshot)
    }
}
