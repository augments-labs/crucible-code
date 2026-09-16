//! How much an answer explains itself, as the reader chose it.
//!
//! The choice is a shared value because it is read in one place and spoken in
//! another: configuration parses the word a person wrote, and prompt assembly
//! turns the parsed choice into the words the model is given. Neither needs the
//! other's crate to agree on which tones exist.

/// How much the answer explains itself.
///
/// The reader's choice rather than the model's. All three describe the same
/// work done to the same standard; what changes is how much of the reasoning
/// comes back with it, which is a fact about who is reading and not about what
/// was asked. A rung here does not buy a better answer, so these are not
/// ordered and there is no ladder to climb.
///
/// A tone says how to answer and never what is answering. What crucible is,
/// is the prompt's role; what model is behind it, is the identity the prompt
/// states. A tone that opened by naming itself would be a third answer to a
/// question already answered twice, and the reader's choice of register would
/// quietly be a choice of who the model thinks it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    /// The conclusion, and what it cost to reach it.
    #[default]
    Concise,
    /// The conclusion, and why it is that one.
    Explanatory,
    /// The conclusion, and what to know before touching it again.
    Learning,
}

impl Tone {
    /// Every tone, in the order a picker offers them.
    pub const TONES: [Self; 3] = [Self::Concise, Self::Explanatory, Self::Learning];

    /// The tone as a document spells it.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Concise => "concise",
            Self::Explanatory => "explanatory",
            Self::Learning => "learning",
        }
    }
}

/// The word given for a tone was not one.
///
/// Names what was written and then every tone there is, because this is reached
/// with nothing on screen to look at: a key in a file somebody is reading with
/// an editor open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("no tone called {named}; crucible takes {}", Tone::TONES.map(Tone::as_str).join(", "))]
pub struct ToneError {
    /// What was asked for.
    pub named: Box<str>,
}

impl std::str::FromStr for Tone {
    type Err = ToneError;

    /// Trimmed and lowercased first, for the same reason a rung is: a word this
    /// short is typed in whatever case the person was already in.
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let named = text.trim().to_ascii_lowercase();

        Self::TONES
            .into_iter()
            .find(|tone| tone.as_str() == named)
            .ok_or(ToneError {
                named: named.into(),
            })
    }
}
