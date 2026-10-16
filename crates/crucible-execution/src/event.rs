//! What a backend reports while a turn runs.
//!
//! The family is closed, so a consumer that matches it decides about every
//! kind, and a new kind makes each of them decide again. Every field is
//! bounded and none is a wire payload: a backend parses what its program sent
//! and reports the meaning, clipped to [`TEXT_BYTES`] and marked incomplete
//! where it was clipped.
//!
//! What the other agent did is reported, never requested: a
//! [`Event::ToolActivity`] says a tool ran over there, a [`Event::ToolReport`]
//! under the same call says what it came back with, and nothing that reads
//! either runs anything here.

use std::fmt;

use crucible_types::{Compacting, TurnId};

use crate::backend::{Failure, Outcome, SessionRef};
use crate::capability::Observation;
use crate::pending::{Pending, PendingRef};

/// The most bytes one [`Text`] keeps.
pub const TEXT_BYTES: usize = 64 * 1024;

/// Text a backend reported, clipped to a ceiling and saying whether it was.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Text {
    /// What was kept: whole characters, at most the ceiling's bytes.
    text: Box<str>,
    /// Whether anything was left out.
    incomplete: bool,
}

impl Text {
    /// `text`, clipped to [`TEXT_BYTES`].
    #[must_use]
    pub fn new(text: &str) -> Self {
        Self::clipped(text, TEXT_BYTES)
    }

    /// `text`, clipped to at most `ceiling` bytes on a character boundary,
    /// and never to more than [`TEXT_BYTES`].
    #[must_use]
    pub fn clipped(text: &str, ceiling: usize) -> Self {
        let end = text.floor_char_boundary(ceiling.min(TEXT_BYTES));
        let kept = text.get(..end).unwrap_or_default();
        Self {
            text: kept.into(),
            incomplete: end < text.len(),
        }
    }

    /// What was kept.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether anything was left out.
    #[must_use]
    pub const fn incomplete(&self) -> bool {
        self.incomplete
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        form.debug_struct("Text")
            .field("bytes", &self.text.len())
            .field("incomplete", &self.incomplete)
            .finish()
    }
}

impl fmt::Display for Text {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        form.write_str(&self.text)
    }
}

/// A three-letter currency code, upper case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Currency([u8; 3]);

impl Currency {
    /// `code`, where it is three upper-case ASCII letters.
    #[must_use]
    pub fn new(code: &str) -> Option<Self> {
        let letters: [u8; 3] = code.as_bytes().try_into().ok()?;
        letters
            .iter()
            .all(u8::is_ascii_uppercase)
            .then_some(Self(letters))
    }

    /// The code.
    #[must_use]
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.0).unwrap_or_default()
    }
}

/// What a turn cost, as the agent reported it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cost {
    /// Millionths of one unit of the currency.
    millionths: u64,
    /// Which currency.
    currency: Currency,
}

impl Cost {
    /// `millionths` of one unit of `currency`.
    #[must_use]
    pub const fn new(millionths: u64, currency: Currency) -> Self {
        Self {
            millionths,
            currency,
        }
    }

    /// Millionths of one unit of the currency.
    #[must_use]
    pub const fn millionths(self) -> u64 {
        self.millionths
    }

    /// Which currency.
    #[must_use]
    pub const fn currency(self) -> Currency {
        self.currency
    }
}

/// What one turn used, as the agent reported it.
///
/// One turn's figures, never a session's total. A figure the agent did not give
/// is absent rather than zero, because a zero is a reading and an absence is
/// not one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Usage {
    /// Which turn.
    turn: TurnId,
    /// Tokens read, cache reads included.
    input: Option<u64>,
    /// Of those, tokens read from a cache.
    cache_read: Option<u64>,
    /// Tokens written to a cache.
    cache_write: Option<u64>,
    /// Tokens produced.
    output: Option<u64>,
    /// What it cost, where the agent said.
    cost: Option<Cost>,
}

impl Usage {
    /// No figures yet for `turn`.
    #[must_use]
    pub const fn of(turn: TurnId) -> Self {
        Self {
            turn,
            input: None,
            cache_read: None,
            cache_write: None,
            output: None,
            cost: None,
        }
    }

    /// With `tokens` read, cache reads included.
    #[must_use]
    pub const fn input(mut self, tokens: u64) -> Self {
        self.input = Some(tokens);
        self
    }

    /// With `tokens` of the input read from a cache.
    #[must_use]
    pub const fn cache_read(mut self, tokens: u64) -> Self {
        self.cache_read = Some(tokens);
        self
    }

    /// With `tokens` written to a cache.
    #[must_use]
    pub const fn cache_write(mut self, tokens: u64) -> Self {
        self.cache_write = Some(tokens);
        self
    }

    /// With `tokens` produced.
    #[must_use]
    pub const fn output(mut self, tokens: u64) -> Self {
        self.output = Some(tokens);
        self
    }

    /// With the turn costing `cost`.
    #[must_use]
    pub const fn cost(mut self, cost: Cost) -> Self {
        self.cost = Some(cost);
        self
    }

    /// Which turn.
    #[must_use]
    pub const fn turn(&self) -> TurnId {
        self.turn
    }

    /// Tokens read, cache reads included, where the agent said.
    #[must_use]
    pub const fn input_tokens(&self) -> Option<u64> {
        self.input
    }

    /// Tokens read from a cache, where the agent said.
    #[must_use]
    pub const fn cache_read_tokens(&self) -> Option<u64> {
        self.cache_read
    }

    /// Tokens written to a cache, where the agent said.
    #[must_use]
    pub const fn cache_write_tokens(&self) -> Option<u64> {
        self.cache_write
    }

    /// Tokens produced, where the agent said.
    #[must_use]
    pub const fn output_tokens(&self) -> Option<u64> {
        self.output
    }

    /// What it cost, where the agent said.
    #[must_use]
    pub const fn spent(&self) -> Option<Cost> {
        self.cost
    }
}

/// What a turn may have done that nobody can now confirm.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Effects {
    /// What was under way when the turn was lost.
    under_way: Text,
}

impl Effects {
    /// `under_way` was in progress when the turn was lost.
    #[must_use]
    pub const fn new(under_way: Text) -> Self {
        Self { under_way }
    }

    /// What was in progress.
    #[must_use]
    pub const fn under_way(&self) -> &Text {
        &self.under_way
    }
}

/// The agent behind a backend stopped answering.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Fault {
    /// What was seen.
    seen: Text,
}

impl Fault {
    /// It stopped, and `seen` is what was seen.
    #[must_use]
    pub const fn new(seen: Text) -> Self {
        Self { seen }
    }

    /// What was seen.
    #[must_use]
    pub const fn seen(&self) -> &Text {
        &self.seen
    }
}

/// What kind of thing the summary of the other agent's tool use names.
///
/// Read off the field the summary came from, by the backend that parsed the
/// call, so whatever draws the row never guesses it back out of the words.
/// Closed for the reason a native call's kind is: a row tells these four apart
/// and no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum About {
    /// A file or directory.
    Path,
    /// An address.
    Address,
    /// A command, or a pattern looked for.
    Command,
    /// Words that are none of those.
    Other,
}

/// One thing a backend reports while a turn runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A session was opened, under this reference.
    Opened(SessionRef),
    /// A turn began.
    Started {
        /// Which turn.
        turn: TurnId,
    },
    /// More of the answer.
    Delta(Text),
    /// The other agent used a tool. Something it did, never an instruction to
    /// run one here.
    ToolActivity {
        /// The agent's name for the call, which what the call reported once it
        /// ended carries too.
        call: Text,
        /// Which tool, as the agent named it.
        tool: Text,
        /// What it did with it.
        summary: Text,
        /// What kind of thing the summary names, as the field it was read
        /// from says.
        about: About,
    },
    /// What a tool the other agent used reported once the call ended.
    ToolReport {
        /// The agent's name for the call it ends.
        call: Text,
        /// Whether the agent said the call failed.
        failed: bool,
        /// What it reported, which may be nothing.
        report: Text,
    },
    /// A file changed.
    FileChanged {
        /// Which file, as the agent named it.
        path: Text,
    },
    /// The agent is waiting on a decision.
    Pending(PendingRef, Pending),
    /// What the turn used.
    Usage(Observation<Usage>),
    /// How many tokens the agent's context held at its latest request to its
    /// model, that request's answer included.
    Carried(u64),
    /// The agent made room in its own context.
    Compacted {
        /// What asked for it.
        why: Compacting,
        /// The tokens its context held before.
        before: u64,
        /// And after, where the agent said.
        after: Option<u64>,
    },
    /// Something the agent is doing outside the turn.
    Background(Text),
    /// The turn ended.
    Finished(Outcome),
    /// The turn failed.
    Failed(Failure),
    /// The turn was lost, and what it was doing may or may not have happened.
    Uncertain(Effects),
    /// The agent stopped answering.
    Process(Fault),
}
