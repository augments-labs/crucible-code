//! The live tail: the only part of the transcript this process still owns.
//!
//! Streamed output is wrapped into display rows as it arrives and held here.
//! Once there are more rows than the bound, the oldest ones *overflow*: they are
//! handed back to be written once and then forgotten. Everything above the tail
//! belongs to the terminal's scrollback, so what this file holds is bounded by
//! the tail's own size rather than by how long the session has run. The
//! transcript is held whole by the runner, and is the one thing anywhere in
//! this program that grows with the session.
//!
//! Wrapping happens here rather than being left to the terminal because the
//! renderer has to know how many rows it drew in order to move back over them.
//! A terminal that wrapped a row on its own would leave the cursor somewhere
//! this process did not predict, and the next frame would erase the wrong lines.

use std::collections::VecDeque;

use crate::width::{self, EMOJI_PRESENTATION};

/// The wrapped, bounded live region.
#[derive(Debug)]
pub(crate) struct Tail {
    /// Display rows, oldest first. The last one is still being appended to.
    rows: VecDeque<Row>,
    /// Columns available for text.
    width: usize,
    /// The most rows that may stay live. Beyond this, the oldest overflow.
    bound: usize,
}

/// One display row, with its width kept alongside so appending stays O(1).
#[derive(Debug, Default, Clone)]
struct Row {
    text: String,
    width: usize,
}

impl Tail {
    /// A tail wrapping at `width` columns and holding at most `bound` rows.
    ///
    /// Both are clamped to at least one: a zero width would wrap forever, and a
    /// zero bound would overflow the row that is still being written.
    #[must_use]
    pub(crate) fn new(width: usize, bound: usize) -> Self {
        Self {
            rows: VecDeque::from([Row::default()]),
            width: width.max(1),
            bound: bound.max(1),
        }
    }

    /// Appends streamed text, moving any rows past the bound into `overflow`.
    ///
    /// The caller owns `overflow` and reuses it across frames, so a delta that
    /// pushes nothing out of the tail allocates nothing.
    pub(crate) fn push(&mut self, delta: &str, overflow: &mut Vec<String>) {
        for character in delta.chars() {
            match character {
                // A newline ends the row wherever it is.
                '\n' => self.rows.push_back(Row::default()),
                // Bare carriage returns arrive as half of a CRLF from a tool's
                // output. Dropping them is what stops a blank row per line.
                '\r' => {}
                '\t' => self.advance_to_tab_stop(),
                EMOJI_PRESENTATION => self.place_emoji_presentation(),
                _ => self.place(character),
            }
        }

        while self.rows.len() > self.bound {
            // The row that leaves is complete: only the last row is still being
            // appended to, and the bound is at least one.
            if let Some(row) = self.rows.pop_front() {
                overflow.push(row.text);
            }
        }
    }

    /// The rows currently drawn, oldest first.
    pub(crate) fn rows(&self) -> impl ExactSizeIterator<Item = &str> {
        self.rows.iter().map(|row| row.text.as_str())
    }

    /// The rows that are content, for the end of a turn.
    ///
    /// The last row is where the next character would have gone, so an empty
    /// one is the newline that ended the row above rather than a blank line
    /// somebody wrote. Settling [`Self::rows`] instead would leave a blank line
    /// in the record after every answer that ended with a newline -- which is
    /// most of them.
    pub(crate) fn content(&self) -> impl ExactSizeIterator<Item = &str> {
        let rows = match self.rows.back() {
            Some(row) if row.text.is_empty() => self.rows.len().saturating_sub(1),
            _ => self.rows.len(),
        };

        self.rows.range(..rows).map(|row| row.text.as_str())
    }

    /// How many rows are live. This is what the renderer moves back over.
    #[must_use]
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Whether the tail holds nothing but empty rows.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.iter().all(|row| row.text.is_empty())
    }

    /// Drops everything, ready for the next turn.
    ///
    /// The rows are gone rather than committed: the caller decides whether
    /// what was live deserved to reach scrollback.
    pub(crate) fn clear(&mut self) {
        self.rows.clear();
        self.rows.push_back(Row::default());
    }

    /// Puts one character on the current row, wrapping first if it will not
    /// fit, and dropping it if no row of this width could hold it.
    fn place(&mut self, character: char) {
        // Zero for combining marks, two for the wide scripts, and nothing at
        // all for a control character, which is dropped rather than drawn.
        let Some(advance) = width::advance(character) else {
            return;
        };

        // Wider than the whole row is nowhere to put it: wrapping would only
        // move the overflow onto a fresh row and count that row short, which is
        // the row the terminal wraps itself. Dropped, so the count stays honest.
        if advance > self.width {
            return;
        }

        if self.current_width() + advance > self.width {
            self.rows.push_back(Row::default());
        }

        if let Some(row) = self.rows.back_mut() {
            row.text.push(character);
            row.width += advance;
        }
    }

    /// Puts the emoji presentation selector down, taking the column it makes
    /// the character before it worth. A row counted a column short is a row the
    /// terminal wraps itself, leaving the cursor a row below where the next
    /// frame rewinds to.
    fn place_emoji_presentation(&mut self) {
        let current = self.rows.back();
        let base = current.and_then(|row| row.text.chars().next_back());

        if !width::widens(base) {
            self.place(EMOJI_PRESENTATION);
            return;
        }

        // A row one column wide has no room for the pair anywhere. The base is
        // already down and drawn as text; the selector only asked for a
        // presentation, so it goes rather than the character it applies to.
        if self.width < 2 {
            return;
        }

        if self.current_width() + 1 > self.width {
            // The pair moves down together: a selector parted from its base
            // stops asking for anything, and the base left behind would draw
            // narrow on a row this tail had already counted wide.
            if let Some(row) = self.rows.back_mut() {
                row.text.pop();
                row.width = row.width.saturating_sub(1);
            }
            self.rows.push_back(Row::default());
            if let Some(base) = base {
                self.place(base);
            }
        }

        if let Some(row) = self.rows.back_mut() {
            row.text.push(EMOJI_PRESENTATION);
            row.width += 1;
        }
    }

    /// Pads with spaces to the next tab stop, wrapping if the stop is past the
    /// edge. Expanded here rather than passed through so the width this tail
    /// reports is the width the terminal will show.
    fn advance_to_tab_stop(&mut self) {
        let current = self.current_width();
        let target = width::tab_stop(current);

        if target > self.width {
            self.rows.push_back(Row::default());
            return;
        }

        for _ in current..target {
            self.place(' ');
        }
    }

    /// The width of the row being appended to.
    fn current_width(&self) -> usize {
        self.rows.back().map_or(0, |row| row.width)
    }
}

#[cfg(test)]
mod tests;
