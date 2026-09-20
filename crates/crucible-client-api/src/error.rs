//! How a request is turned away.
//!
//! A refusal names a code and nothing the client sent. The code is the stable
//! part — a client branches on it — and the sentence beside it is fixed per
//! code, so what a hostile frame held can never be echoed back through the
//! refusal of it.

use std::fmt;

/// Why a request, a decision or an operation did not go through.
///
/// Closed, and spelled once: [`ErrorCode::as_str`] is the word that crosses,
/// and a word may be added but never reused for another meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The bytes were not one well-formed frame of this protocol.
    Malformed,
    /// The frame, or a field inside it, was over its ceiling.
    TooLarge,
    /// The frame named a protocol version this build does not speak.
    UnsupportedVersion,
    /// The frame claimed a capability this build has not heard of.
    UnknownCapability,
    /// The frame named a command this build does not ship.
    UnknownCommand,
    /// A field held a value its command does not take.
    InvalidArgument,
    /// The command cannot be taken now: a turn is running, or none is.
    Busy,
    /// A decision named an action that is not the one pending.
    StaleDecision,
    /// A decision named the pending action and answered a different question.
    WrongDecision,
    /// The command named a provider this build does not offer.
    UnknownProvider,
    /// A pending action was answered too many times with decisions that fit
    /// nothing, and has been settled as though nobody answered.
    Abandoned,
    /// The application tried and could not; the sentence beside the code says
    /// what its owner would have shown.
    Failed,
}

impl ErrorCode {
    /// Every code, in the order they are declared.
    pub const EVERY: [Self; 12] = [
        Self::Malformed,
        Self::TooLarge,
        Self::UnsupportedVersion,
        Self::UnknownCapability,
        Self::UnknownCommand,
        Self::InvalidArgument,
        Self::Busy,
        Self::StaleDecision,
        Self::WrongDecision,
        Self::UnknownProvider,
        Self::Abandoned,
        Self::Failed,
    ];

    /// The word this code crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::TooLarge => "too_large",
            Self::UnsupportedVersion => "unsupported_version",
            Self::UnknownCapability => "unknown_capability",
            Self::UnknownCommand => "unknown_command",
            Self::InvalidArgument => "invalid_argument",
            Self::Busy => "busy",
            Self::StaleDecision => "stale_decision",
            Self::WrongDecision => "wrong_decision",
            Self::UnknownProvider => "unknown_provider",
            Self::Abandoned => "abandoned",
            Self::Failed => "failed",
        }
    }

    /// The code `word` spells, where it spells one.
    #[must_use]
    pub fn named(word: &str) -> Option<Self> {
        Self::EVERY.into_iter().find(|code| code.as_str() == word)
    }

    /// The fixed sentence a refusal with this code reads as.
    const fn sentence(self) -> &'static str {
        match self {
            Self::Malformed => "the request is not a well-formed frame",
            Self::TooLarge => "the request is larger than this protocol carries",
            Self::UnsupportedVersion => "the request names a protocol version that is not spoken",
            Self::UnknownCapability => "the request claims a capability that is not known",
            Self::UnknownCommand => "the request names a command that is not shipped",
            Self::InvalidArgument => "the request holds a value its command does not take",
            Self::Busy => "the command cannot be taken now",
            Self::StaleDecision => "the decision names an action that is not pending",
            Self::WrongDecision => "the decision does not answer the action that is pending",
            Self::UnknownProvider => "the command names a provider that is not offered",
            Self::Abandoned => "the action was answered too often with nothing that fits it",
            Self::Failed => "the application could not do what was asked",
        }
    }
}

/// A request turned away, by code alone.
///
/// Holds nothing of what was sent, which is what lets it be logged, shown and
/// sent back without a second thought about what the frame contained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    code: ErrorCode,
}

impl Refusal {
    /// A refusal for `code`.
    #[must_use]
    pub const fn new(code: ErrorCode) -> Self {
        Self { code }
    }

    /// Why.
    #[must_use]
    pub const fn code(self) -> ErrorCode {
        self.code
    }
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code.sentence())
    }
}

impl std::error::Error for Refusal {}

impl From<ErrorCode> for Refusal {
    fn from(code: ErrorCode) -> Self {
        Self::new(code)
    }
}
