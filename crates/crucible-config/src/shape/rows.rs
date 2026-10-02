//! The settings a menu lists, in the order it lists them.
//!
//! A row is a key whose value a menu can step through without a box to type
//! in: a boolean, a closed choice, a whole number between two bounds, or a
//! name out of a set the host holds — the syntax themes it reads code in.
//! Every other key is left out, and the list of what is left out sits beside
//! the test in `rows/tests.rs`, each entry with its reason. That test walks
//! every key [`DOCUMENT`](super::DOCUMENT) declares, so a key added to the
//! document without a row or a reason fails the build rather than going
//! missing from the menu in silence.
//!
//! What a row may be set to is the same constant its declaration names, and
//! the test holds the two to one answer: the choices a menu offers and the
//! choices the parser accepts are one list.
//!
//! No key that loosens what crucible does unasked is a row. A menu is not a
//! permission decision, and the test refuses a row over a key declared as
//! widening.

use super::{
    COLOR, COMPACTION_WHEN, DOCUMENT, Field, GLYPHS, PROMPT_CACHE_ISOLATION, PROMPT_CACHE_MODE,
    PROMPT_CACHE_PERSISTENT, PROMPT_CACHE_RETENTION, SCROLL_SPEED, SEND, THEME, TONE, TOOL_DETAIL,
    UPDATE_CHECK,
};

#[cfg(test)]
mod tests;

/// What a row may be set to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Values {
    /// True or false.
    Flag,
    /// One of these words.
    Choice(&'static [&'static str]),
    /// A whole number from `least` to `most`, both included.
    Whole {
        /// The fewest.
        least: u16,
        /// The most.
        most: u16,
    },
    /// A name out of a set the host holds rather than this crate: the syntax
    /// themes whatever draws can read code in.
    Named,
}

/// One setting a menu lists.
#[derive(Debug)]
pub struct Row {
    key: &'static str,
    label: &'static str,
    values: Values,
}

impl Row {
    const fn new(key: &'static str, label: &'static str, values: Values) -> Self {
        Self { key, label, values }
    }

    /// The key, dotted the way a document nests it: `output.theme`.
    #[must_use]
    pub const fn key(&self) -> &'static str {
        self.key
    }

    /// What the menu calls it.
    #[must_use]
    pub const fn label(&self) -> &'static str {
        self.label
    }

    /// What it may be set to.
    #[must_use]
    pub const fn values(&self) -> Values {
        self.values
    }

    /// What it means where no layer set it, as a document would write it.
    #[must_use]
    pub fn usual(&self) -> Option<&'static str> {
        self.field().and_then(|field| field.usual)
    }

    /// The names a document nests it under, outermost first.
    pub(crate) fn path(&self) -> impl Iterator<Item = &'static str> {
        self.key.split('.')
    }

    /// Its declaration.
    pub(crate) fn field(&self) -> Option<&'static Field> {
        let mut path = self.path();
        let mut field = DOCUMENT.declared(path.next()?)?;
        for name in path {
            field = field.shape.declared(name)?;
        }
        Some(field)
    }
}

/// The scroll speed's bounds, as the row offers them.
const SPEED: Values = Values::Whole {
    least: SCROLL_SPEED.least,
    most: SCROLL_SPEED.most,
};

/// Every row, in the order a menu lists them.
///
/// What draws first, then the keyboard, then what the model is asked under,
/// and the prompt cache last: its four rows are the ones a reader is least
/// likely to have come for.
const ROWS: &[Row] = &[
    Row::new("output.theme", "Theme", Values::Choice(THEME)),
    Row::new("output.syntaxTheme", "Syntax theme", Values::Named),
    Row::new("output.glyphs", "Glyphs", Values::Choice(GLYPHS)),
    Row::new("output.color", "Colour", Values::Choice(COLOR)),
    Row::new(
        "output.toolDetail",
        "Tool detail",
        Values::Choice(TOOL_DETAIL),
    ),
    Row::new("output.scrollRail", "Scroll rail", Values::Flag),
    Row::new(
        "env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED",
        "Mouse scroll speed",
        SPEED,
    ),
    Row::new("input.send", "Send with", Values::Choice(SEND)),
    Row::new("systemPrompt.tone", "Tone", Values::Choice(TONE)),
    Row::new(
        "compaction.when",
        "Compaction",
        Values::Choice(COMPACTION_WHEN),
    ),
    Row::new(
        "updates.check",
        "Check for updates",
        Values::Choice(UPDATE_CHECK),
    ),
    Row::new(
        "promptCaching.mode",
        "Prompt caching",
        Values::Choice(PROMPT_CACHE_MODE),
    ),
    Row::new(
        "promptCaching.isolationScope",
        "Cache isolation",
        Values::Choice(PROMPT_CACHE_ISOLATION),
    ),
    Row::new(
        "promptCaching.requestedRetention.class",
        "Cache retention",
        Values::Choice(PROMPT_CACHE_RETENTION),
    ),
    Row::new(
        "promptCaching.persistentResources.mode",
        "Persistent cache",
        Values::Choice(PROMPT_CACHE_PERSISTENT),
    ),
];

/// Every row, in the order a menu lists them.
#[must_use]
pub fn rows() -> &'static [Row] {
    ROWS
}

/// The row for `key`, dotted, where there is one.
#[must_use]
pub fn row(key: &str) -> Option<&'static Row> {
    ROWS.iter().find(|row| row.key == key)
}
