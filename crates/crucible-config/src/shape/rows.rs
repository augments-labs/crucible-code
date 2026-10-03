//! The settings a menu lists, in the order it lists them.
//!
//! A row is a key whose value a menu can step through without a box to type
//! in: a boolean, a closed choice, a whole number between two bounds, or a
//! name out of a set the host holds — the syntax themes it reads code in.
//! Every other key is left out, and the list of what is left out sits beside
//! the test in `rows/tests.rs`, each entry with its reason. That test walks
//! every key [`DOCUMENT`] declares, so a key added to the document without a
//! row or a reason fails the build rather than going missing from the menu in
//! silence.
//!
//! What a row may be set to is the same constant its declaration names, and
//! the test holds the two to one answer: the choices a menu offers and the
//! choices the parser accepts are one list.
//!
//! No key that loosens what crucible does unasked is a row. A menu is not a
//! permission decision, and the test refuses a row over a key declared as
//! widening.

use super::{
    COLOR, COMPACTION_WHEN, DOCUMENT, Field, GLYPHS, PIN_AFTER, PROMPT_CACHE_ISOLATION,
    PROMPT_CACHE_MODE, PROMPT_CACHE_PERSISTENT, PROMPT_CACHE_RETENTION, SCREEN, SCROLL_SPEED, SEND,
    THEME, TONE, TOOL_DETAIL, TRANSCRIPT_COLOURS, UPDATE_CHECK,
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

/// Which setting a row is, as a value a host matches on.
///
/// The key says where a row is written and this says which it is, so a host
/// deciding what a change does matches on this, exhaustively, and a row added
/// here is a case every such match has to answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowId {
    /// `output.theme`.
    Theme,
    /// `output.syntaxTheme`.
    SyntaxTheme,
    /// `output.glyphs`.
    Glyphs,
    /// `output.color`.
    Colour,
    /// `output.transcriptColours`.
    TranscriptColours,
    /// `output.toolDetail`.
    ToolDetail,
    /// `output.scrollRail`.
    ScrollRail,
    /// `output.screen`.
    ScreenMode,
    /// `output.pinAfterSeconds`.
    PinAfter,
    /// `env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED`.
    ScrollSpeed,
    /// `input.send`.
    Send,
    /// `systemPrompt.tone`.
    Tone,
    /// `compaction.when`.
    Compaction,
    /// `updates.check`.
    UpdateCheck,
    /// `promptCaching.mode`.
    CacheMode,
    /// `promptCaching.isolationScope`.
    CacheIsolation,
    /// `promptCaching.requestedRetention.class`.
    CacheRetention,
    /// `promptCaching.persistentResources.mode`.
    CachePersistent,
}

/// One setting a menu lists.
#[derive(Debug)]
pub struct Row {
    id: RowId,
    key: &'static str,
    label: &'static str,
    values: Values,
}

impl Row {
    const fn new(id: RowId, key: &'static str, label: &'static str, values: Values) -> Self {
        Self {
            id,
            key,
            label,
            values,
        }
    }

    /// Which setting it is.
    #[must_use]
    pub const fn id(&self) -> RowId {
        self.id
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

    /// Whether `word` is a value it may be set to, spelled as a document
    /// spells it: `true`, `dark`, `12`.
    ///
    /// A [`Values::Named`] row takes any name, since which names there are is
    /// the host's to say and the host asks itself before anything is written.
    #[must_use]
    pub fn takes(&self, word: &str) -> bool {
        match self.values {
            Values::Flag => matches!(word, "true" | "false"),
            Values::Choice(words) => words.contains(&word),
            // Spelled the one way a number is: no sign, no leading zero.
            Values::Whole { least, most } => word
                .parse::<u16>()
                .is_ok_and(|number| (least..=most).contains(&number) && number.to_string() == word),
            Values::Named => !word.is_empty(),
        }
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

/// How long a running call is held back, as the row offers it.
const PINNING: Values = Values::Whole {
    least: PIN_AFTER.least,
    most: PIN_AFTER.most,
};

/// Every row, in the order a menu lists them.
///
/// What draws first, then the keyboard, then what the model is asked under,
/// and the prompt cache last: its four rows are the ones a reader is least
/// likely to have come for.
const ROWS: &[Row] = &[
    Row::new(RowId::Theme, "output.theme", "Theme", Values::Choice(THEME)),
    Row::new(
        RowId::SyntaxTheme,
        "output.syntaxTheme",
        "Syntax theme",
        Values::Named,
    ),
    Row::new(
        RowId::Glyphs,
        "output.glyphs",
        "Glyphs",
        Values::Choice(GLYPHS),
    ),
    Row::new(
        RowId::Colour,
        "output.color",
        "Colour",
        Values::Choice(COLOR),
    ),
    Row::new(
        RowId::TranscriptColours,
        "output.transcriptColours",
        "Transcript colours",
        Values::Choice(TRANSCRIPT_COLOURS),
    ),
    Row::new(
        RowId::ToolDetail,
        "output.toolDetail",
        "Tool detail",
        Values::Choice(TOOL_DETAIL),
    ),
    Row::new(
        RowId::ScrollRail,
        "output.scrollRail",
        "Scroll rail",
        Values::Flag,
    ),
    Row::new(
        RowId::ScreenMode,
        "output.screen",
        "Screen mode",
        Values::Choice(SCREEN),
    ),
    Row::new(
        RowId::PinAfter,
        "output.pinAfterSeconds",
        "Seconds before a call pins",
        PINNING,
    ),
    Row::new(
        RowId::ScrollSpeed,
        "env.CRUCIBLE_CODE_MOUSE_SCROLL_SPEED",
        "Mouse scroll speed",
        SPEED,
    ),
    Row::new(RowId::Send, "input.send", "Send with", Values::Choice(SEND)),
    Row::new(
        RowId::Tone,
        "systemPrompt.tone",
        "Tone",
        Values::Choice(TONE),
    ),
    Row::new(
        RowId::Compaction,
        "compaction.when",
        "Compaction",
        Values::Choice(COMPACTION_WHEN),
    ),
    Row::new(
        RowId::UpdateCheck,
        "updates.check",
        "Check for updates",
        Values::Choice(UPDATE_CHECK),
    ),
    Row::new(
        RowId::CacheMode,
        "promptCaching.mode",
        "Prompt caching",
        Values::Choice(PROMPT_CACHE_MODE),
    ),
    Row::new(
        RowId::CacheIsolation,
        "promptCaching.isolationScope",
        "Cache isolation",
        Values::Choice(PROMPT_CACHE_ISOLATION),
    ),
    Row::new(
        RowId::CacheRetention,
        "promptCaching.requestedRetention.class",
        "Cache retention",
        Values::Choice(PROMPT_CACHE_RETENTION),
    ),
    Row::new(
        RowId::CachePersistent,
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
