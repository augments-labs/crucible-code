//! The ceilings every value here is held to, and the bounded strings.
//!
//! A ceiling is checked where the bytes arrive and before they are kept: a
//! frame by its length before it is parsed, a field by its length before it is
//! boxed. Three strings carry that through the types, and which one a field is
//! says which way its words travel. A [`Name`] identifies something and is
//! refused whole when it does not fit, because a shortened name is another
//! name. A [`Text`] travels from the host to a client, is read by a person,
//! and is cut instead, and says that it was. A [`Said`] travels from a client
//! to the host, where a model or a tool will act on it, and is never cut:
//! words that do not fit are refused, because half of what a person answered
//! is an answer they did not give.

use std::fmt;

use crate::error::{ErrorCode, Refusal};

/// The most bytes one frame may be, whichever way it travels.
///
/// Room for the largest prompt the application takes, written out as JSON,
/// and no more: a decoder refuses a longer frame by its length alone.
pub const FRAME_BYTES: usize = 2 * 1024 * 1024;

/// The most bytes a prompt may be.
///
/// The transcript's own ceiling for one message, so that a prompt this
/// protocol lets through is never one the conversation then refuses for size.
pub const PROMPT_BYTES: usize = crucible_types::CONTINUATION_BYTES;

/// The most bytes a [`Name`] may be.
pub const NAME_BYTES: usize = 256;

/// The most bytes a [`Text`] may be.
pub const TEXT_BYTES: usize = 16 * 1024;

/// The most bytes a [`Said`] may be.
///
/// The ceiling a prompt has, which is also the most a terminal's editor lets a
/// person write, so that whatever can be typed as an answer can be carried.
pub const SAID_BYTES: usize = PROMPT_BYTES;

/// The most entries any list here may hold, and the most fields any object.
pub const ITEMS: usize = 128;

/// How many lists and objects deep a frame may nest.
///
/// At least twice what the deepest value here needs, so that a frame of
/// nothing but opening brackets is refused after this many rather than
/// recursed into.
pub const DEPTH: usize = 16;

/// The most values one frame may hold, counting every list, object, string,
/// number, flag and null in it.
///
/// [`ITEMS`] bounds each list and [`DEPTH`] how far they nest, and between
/// them that still allows a frame of lists of lists with a million entries in
/// all. The fullest value here is a snapshot whose pending action is [`ITEMS`]
/// questions of [`ITEMS`] choices: a choice is seven values — itself and two
/// texts of three — and eight apiece leaves room for the questions around them
/// and the snapshot around those.
pub const VALUES: usize = ITEMS * ITEMS * 8;

/// A short word that identifies something: a provider, a model, a theme.
///
/// Never empty, never over [`NAME_BYTES`], and free of control characters, so
/// one can be logged on a line and compared for identity.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Name(Box<str>);

impl Name {
    /// The name `word` spells.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] over [`NAME_BYTES`], checked first and before
    /// anything is copied; [`ErrorCode::InvalidArgument`] for an empty word or
    /// one holding a control character.
    pub fn new(word: &str) -> Result<Self, Refusal> {
        if word.len() > NAME_BYTES {
            return Err(ErrorCode::TooLarge.into());
        }
        if word.is_empty() || word.chars().any(char::is_control) {
            return Err(ErrorCode::InvalidArgument.into());
        }
        Ok(Self(word.into()))
    }

    /// The word.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Name({:?})", self.0)
    }
}

impl fmt::Display for Name {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Words for a person to read, cut to [`TEXT_BYTES`] and marked where cut.
///
/// What a model said, what a tool is asking to do and what an error's owner
/// would have shown all travel as this. It is redacted from `Debug`: the words
/// are a person's or a model's, and a log line is not where they belong.
#[derive(Clone, PartialEq, Eq)]
pub struct Text {
    text: Box<str>,
    truncated: bool,
}

impl Text {
    /// `words`, cut on a character boundary where they run over the ceiling.
    #[must_use]
    pub fn cut(words: &str) -> Self {
        if words.len() <= TEXT_BYTES {
            return Self {
                text: words.into(),
                truncated: false,
            };
        }

        let mut end = TEXT_BYTES;
        while !words.is_char_boundary(end) {
            end = end.saturating_sub(1);
        }
        Self {
            text: words.get(..end).unwrap_or_default().into(),
            truncated: true,
        }
    }

    /// `words` whose owner kept them under a ceiling of its own, and whether
    /// that cost any of them: cut again where they run over this one, and said
    /// to be cut where either ceiling was.
    ///
    /// A mark the owner wrote into the words is for a person. What a client
    /// acts on is this, and words can only claim a mark, not set it.
    #[must_use]
    pub fn cut_again(words: &str, already: bool) -> Self {
        let mut text = Self::cut(words);
        text.truncated |= already;
        text
    }

    /// Words that arrived already said to be whole or cut.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] over [`TEXT_BYTES`]: a sender that cut its words
    /// cut them to the ceiling, so longer ones were never cut by this protocol.
    pub(crate) fn arrived(words: &str, truncated: bool) -> Result<Self, Refusal> {
        if words.len() > TEXT_BYTES {
            return Err(ErrorCode::TooLarge.into());
        }
        Ok(Self {
            text: words.into(),
            truncated,
        })
    }

    /// The words that fitted.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// Whether there were more words than these.
    #[must_use]
    pub const fn truncated(&self) -> bool {
        self.truncated
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "Text([redacted; {} bytes, truncated: {}])",
            self.text.len(),
            self.truncated
        )
    }
}

/// Words a person gave a client for the host to act on: an answer of their own
/// to a question, or the note beside one.
///
/// Whole or not at all. Where a [`Text`] is cut for a reader, these are handed
/// on to whatever asked, so over [`SAID_BYTES`] they are refused and the
/// person is told, rather than shortened into something they did not say. May
/// be empty, which is what a note nobody wrote is. Redacted from `Debug`,
/// because they are a person's.
#[derive(Clone, PartialEq, Eq)]
pub struct Said(Box<str>);

impl Said {
    /// What was said, where it fits.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] over [`SAID_BYTES`], checked before the words
    /// are copied.
    pub fn new(words: &str) -> Result<Self, Refusal> {
        if words.len() > SAID_BYTES {
            return Err(ErrorCode::TooLarge.into());
        }
        Ok(Self(words.into()))
    }

    /// The words, all of them.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Said {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Said([redacted; {} bytes])", self.0.len())
    }
}
