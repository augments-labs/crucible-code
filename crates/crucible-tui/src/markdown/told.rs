//! What the live foot says of a held block while it streams, where the
//! drawer that takes its fence's name says what the block is.
//!
//! Invariants:
//!
//! - The drawer is found as a closed fence's is, by the first word of the
//!   fence's info string, and is asked with the line after the fence's own
//!   only once that line has ended: a line still arriving could yet name
//!   another kind.
//! - What it says is its own words, never the source's, so nothing written in
//!   the block reaches the row.
//! - A drawer that names nothing, a fence no drawer takes, a fence of a kind
//!   a setting shows as written, a table and display math between markers of
//!   its own leave the row naming what is held, as it did before any drawer
//!   named a block.

use super::{Holding, Markdown};
use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;

impl Holding {
    /// The row, drawn for a window `columns` wide and never wider, naming the
    /// block as `label` does where its drawer said what it is ("a mermaid
    /// flowchart"), and by what is held where none did.
    ///
    /// Structural, as [`Holding::row`] is.
    #[must_use]
    pub fn named(&self, label: Option<&str>, columns: usize, glyphs: Glyphs) -> Row {
        let lines = match self.lines {
            1 => "1 line".to_owned(),
            lines => format!("{lines} lines"),
        };
        let what = label.map_or_else(|| format!("a {}", self.held.word()), str::to_owned);
        let said = format!(
            "{} drawing {what} {} {lines} so far",
            glyphs.summed(),
            glyphs.dot(),
        );
        Row::new()
            .then_structural(Slot::Quiet, said)
            .clipped(columns)
    }
}

impl Markdown {
    /// The name of the fence being gathered and the first line inside it,
    /// once that line has ended; `None` for a block that is no fence, and for
    /// one whose first line has not ended.
    pub(crate) fn opened(&self) -> Option<(&str, &str)> {
        let source = &self.gathering.as_ref()?.source;
        let (opening, rest) = source.split_once('\n')?;
        let mark = opening
            .chars()
            .next()
            .filter(|mark| matches!(mark, '`' | '~'))?;
        let name = opening.trim_start_matches(mark).split_whitespace().next()?;
        let (first, _) = rest.split_once('\n')?;
        Some((name, first))
    }
}
