//! The characters a component draws itself with.
//!
//! Two sets, and which one is in force is answered once rather than asked per
//! row. Box drawing and the half blocks have been in CP437 and in every font
//! shipped this century, so a terminal that shows a row of hollow squares has a
//! font problem rather than an encoding one — and a font is not something this
//! process can ask about. What it can do is take an answer, which is why this
//! is a setting and never a guess.

/// The wordmark, drawn from half blocks.
///
/// crucible's own, drawn for this program. Every row is the same width, which
/// [`super::tests`] is what keeps true — a row a column short leans the whole
/// mark, and a row a column long pushes it through the frame beside it.
const ART: [&str; 3] = [
    "▄▄▄ ▄▄▄ █  █ ▄▄▄ █ ▄▄▄ █   ▄▄▄",
    "█   █▄▄ █  █ █   █ █▄▄ █   █▄▄",
    "▀▄▄ █ █ ▀▄▄▀ ▀▄▄ █ █▄▄ █▄▄ █▄▄",
];

/// Which characters a component draws its frame and its marks with.
///
/// Closed, because a third set would be a third answer to a question the
/// configuration asks in one word.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Glyphs {
    /// Box drawing and the half blocks.
    #[default]
    Unicode,
    /// What is left when the font has neither.
    Ascii,
}

impl Glyphs {
    /// The corners a frame opens with: left, then right.
    pub(super) fn top(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("╭", "╮"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// The corners a frame closes with: left, then right.
    pub(super) fn bottom(self) -> (&'static str, &'static str) {
        match self {
            Self::Unicode => ("╰", "╯"),
            Self::Ascii => ("+", "+"),
        }
    }

    /// One column of an edge that runs across, and of a rule drawn inside one.
    pub(super) fn horizontal(self) -> &'static str {
        match self {
            Self::Unicode => "─",
            Self::Ascii => "-",
        }
    }

    /// One row of an edge that runs down.
    pub(super) fn vertical(self) -> &'static str {
        match self {
            Self::Unicode => "│",
            Self::Ascii => "|",
        }
    }

    /// The small mark that parts one thing on a row from the next, and that
    /// opens an item in a list.
    pub(super) fn dot(self) -> &'static str {
        match self {
            Self::Unicode => "·",
            Self::Ascii => "-",
        }
    }

    /// What stands where something did not fit.
    pub(super) fn ellipsis(self) -> &'static str {
        match self {
            Self::Unicode => "…",
            Self::Ascii => "...",
        }
    }

    /// The wordmark drawn from half blocks, where the font has them.
    ///
    /// `None` is not a failure to draw the name — it is the name drawn as
    /// letters instead, which is what every form narrower than two columns uses
    /// as well.
    pub(super) fn art(self) -> Option<&'static [&'static str]> {
        match self {
            Self::Unicode => Some(&ART),
            Self::Ascii => None,
        }
    }
}
