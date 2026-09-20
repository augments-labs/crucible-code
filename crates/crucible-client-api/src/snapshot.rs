//! What is true of a conversation between commands: authoritative, and whole.
//!
//! A [`Snapshot`] is read off the conversation itself, never assembled from
//! the [`Progress`](crate::Progress) that went by, so a client that missed
//! every report and one that saw them all read the same snapshot. It says
//! where the conversation stands — which session, who is being asked, under
//! what mode, how much has been said, what it is waiting on — and holds none
//! of what was said: the transcript is the runner's one growing value and a
//! snapshot is not a second copy of it.
//!
//! A snapshot frame says which [`Version`] it is written in, as a request and a
//! response do, and one in another is refused by name rather than read for
//! what it happens to share with this one.

use std::str::FromStr;

use crucible_types::SessionId;
use serde_json::Value;

use crate::bounds::{Name, Text};
use crate::command::{Mode, Rung};
use crate::error::{ErrorCode, Refusal};
use crate::pending::Pending;
use crate::request::Version;
use crate::wire::{Fields, Writing, frame, parsed, text, written};

/// A share of a whole in hundredths, and so never over a hundred.
///
/// A byte holds 101 to 255 as readily as a percentage, and a client drawing a
/// gauge from one would have to decide what those mean. Here there is no such
/// value to decide about: one is made from a number that is a percentage, or
/// not at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Percent(u8);

impl Percent {
    /// The whole of it.
    pub const WHOLE: Self = Self(100);

    /// `hundredths` as a percentage, where it is one.
    #[must_use]
    pub const fn new(hundredths: u8) -> Option<Self> {
        if hundredths > Self::WHOLE.0 {
            return None;
        }
        Some(Self(hundredths))
    }

    /// The number, from nothing to a hundred.
    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }
}

/// The words a model goes by, and so never none of them.
///
/// A conversation nothing has chosen a model for says so by having no model,
/// and empty words would be a second way to say it that a reader has to
/// remember to check for. One of these is made from words there are, or not at
/// all, so what is written is what is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model(Text);

impl Model {
    /// `words` as a model, cut as any [`Text`] is, where there are any.
    #[must_use]
    pub fn new(words: &str) -> Option<Self> {
        Self::of(Text::cut(words))
    }

    /// `text` as a model, where it holds any words.
    fn of(text: Text) -> Option<Self> {
        if text.as_str().is_empty() {
            return None;
        }
        Some(Self(text))
    }

    /// The words, and whether there were more of them.
    #[must_use]
    pub const fn text(&self) -> &Text {
        &self.0
    }
}

/// Where a conversation stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// The session being recorded into, where the run is being kept at all.
    pub session: Option<SessionId>,
    /// The provider being asked, by its registry name, where anybody is.
    pub provider: Option<Name>,
    /// The model in force, where one is.
    pub model: Option<Model>,
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
    pub left: Option<Percent>,
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
            .maybe(
                "model",
                self.model.as_ref().map(|model| written(model.text())),
            )
            .maybe("effort", self.effort.map(Rung::as_str))
            .with("mode", self.mode.as_str())
            .with("messages", self.messages)
            .with("turns", self.turns)
            .with("carrying", self.carrying)
            .maybe("left", self.left.map(Percent::get))
            .maybe("pending", self.pending.as_ref().map(Pending::written))
            .finish();

        Writing::new()
            .with("version", Version::CURRENT.number())
            .with("snapshot", standing)
            .finish()
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
    /// [`Refusal`] for anything but one whole, bounded snapshot in a version
    /// this build speaks.
    pub fn decode(bytes: &[u8]) -> Result<Self, Refusal> {
        let mut frame = Fields::of(parsed(bytes)?)?;
        if frame.number("version")? != u64::from(Version::CURRENT.number()) {
            return Err(ErrorCode::UnsupportedVersion.into());
        }
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
            model: fields
                .maybe("model")
                .map(text)
                .transpose()?
                .map(|model| Model::of(model).ok_or_else(|| Refusal::new(ErrorCode::Malformed)))
                .transpose()?,
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
                .map(|left| {
                    u8::try_from(left)
                        .ok()
                        .and_then(Percent::new)
                        .ok_or_else(|| Refusal::new(ErrorCode::Malformed))
                })
                .transpose()?,
            pending: fields.maybe("pending").map(Pending::read).transpose()?,
        };
        fields.done()?;
        Ok(snapshot)
    }
}
