//! How fast a model is asked to answer, how fast it said it answered, and
//! how one model is asked to answer fast at all.
//!
//! A speed is asked beside a request rather than inside it: a [`Request`] is
//! written out field by field in places that must not change with it, so
//! [`crate::Provider::stream_at`] carries the speed and
//! [`crate::Provider::stream`] is the same call at [`Speed::Standard`].
//!
//! [`Request`]: crate::Request

use std::fmt;

/// How fast a model is asked to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Speed {
    /// The vendor's ordinary service, and what every request is until
    /// somebody chooses otherwise.
    #[default]
    Standard,
    /// The vendor's fast form, where the model has one.
    Fast,
}

impl Speed {
    /// The word the setting and the label use.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Fast => "fast",
        }
    }
}

impl fmt::Display for Speed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an answer's response said about the speed it was served at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Served {
    /// The vendor said it answered fast.
    Fast,
    /// The vendor said it answered at its ordinary speed, which it may do for
    /// a request that asked for fast.
    Standard,
    /// The response said nothing about it, which counts as standard.
    #[default]
    Unsaid,
}

impl Served {
    /// Whether the answer was served fast: only where the vendor said so.
    #[must_use]
    pub const fn fast(self) -> bool {
        matches!(self, Self::Fast)
    }
}

/// What a fast form costs, and what its vendor says of it that somebody
/// choosing it should read first, each in the vendor's words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cost {
    /// What fast costs beside standard.
    pub price: &'static str,
    /// Who may use it, or when the vendor serves a fast request at standard
    /// speed, where the vendor says either.
    pub caveat: Option<&'static str>,
}

/// How one model is asked to answer fast.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FastForm {
    /// The vendor serves no fast form of this model.
    #[default]
    None,
    /// A field or a header of the request asks for it, at this cost.
    Field(Cost),
    /// The model is itself a fast id with no standard form, at this cost.
    Own(Cost),
}

impl FastForm {
    /// Whether a speed can be switched for this model: only a form the
    /// request carries can be turned on and off.
    #[must_use]
    pub const fn switched(self) -> bool {
        matches!(self, Self::Field(_))
    }
}
