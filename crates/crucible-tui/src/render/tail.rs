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

    /// Puts one character on the current row, wrapping first if it will not fit.
    fn place(&mut self, character: char) {
        // Zero for combining marks, two for the wide scripts, and nothing at
        // all for a control character, which is dropped rather than drawn.
        let Some(advance) = width::advance(character) else {
            return;
        };

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
mod tests {
    use super::*;

    /// One column as text, two once the selector follows it. Spelled out
    /// because a selector is invisible in a source file.
    const WARNING: &str = "\u{26A0}\u{FE0F}";

    /// Pushes and returns what overflowed, so a test reads as one call.
    fn push(tail: &mut Tail, delta: &str) -> Vec<String> {
        let mut overflow = Vec::new();
        tail.push(delta, &mut overflow);
        overflow
    }

    fn rows(tail: &Tail) -> Vec<&str> {
        tail.rows().collect()
    }

    #[test]
    fn text_shorter_than_the_width_stays_on_one_row() {
        let mut tail = Tail::new(20, 5);
        assert!(push(&mut tail, "hello").is_empty());
        assert_eq!(rows(&tail), ["hello"]);
    }

    #[test]
    fn a_delta_split_mid_word_still_lands_on_one_row() {
        // Providers split wherever the socket did, so the tail must not treat a
        // delta boundary as anything at all.
        let mut tail = Tail::new(20, 5);
        push(&mut tail, "hel");
        push(&mut tail, "lo wor");
        push(&mut tail, "ld");
        assert_eq!(rows(&tail), ["hello world"]);
    }

    #[test]
    fn a_newline_starts_a_row() {
        let mut tail = Tail::new(20, 5);
        push(&mut tail, "one\ntwo");
        assert_eq!(rows(&tail), ["one", "two"]);
    }

    #[test]
    fn crlf_does_not_leave_a_blank_row() {
        let mut tail = Tail::new(20, 5);
        push(&mut tail, "one\r\ntwo");
        assert_eq!(rows(&tail), ["one", "two"]);
    }

    #[test]
    fn text_wider_than_the_width_wraps() {
        let mut tail = Tail::new(4, 5);
        push(&mut tail, "abcdefghij");
        assert_eq!(rows(&tail), ["abcd", "efgh", "ij"]);
    }

    #[test]
    fn a_wide_character_takes_two_columns_when_wrapping() {
        // The bug this catches is counting characters instead of columns: five
        // of these fit in a five-column row only if each is one wide, and they
        // are not.
        let mut tail = Tail::new(5, 5);
        push(&mut tail, "日本語です");
        assert_eq!(rows(&tail), ["日本", "語で", "す"]);
    }

    #[test]
    fn a_combining_mark_does_not_take_a_column() {
        // "e" plus a combining acute is two chars and one column, so it must
        // not push the row over the edge.
        let mut tail = Tail::new(2, 5);
        push(&mut tail, "e\u{301}x");
        assert_eq!(rows(&tail), ["e\u{301}x"]);
    }

    #[test]
    fn an_emoji_presentation_selector_takes_the_column_it_asks_for() {
        // The base is one column alone and two once the selector follows, so
        // three do not fit on a four-column row. Counted per character it is 45
        // of them on an 80-column row the terminal lays out at 90.
        let mut tail = Tail::new(4, 5);
        push(&mut tail, &WARNING.repeat(3));

        assert_eq!(rows(&tail), [WARNING.repeat(2), WARNING.to_owned()]);
    }

    #[test]
    fn a_selector_arriving_in_the_next_delta_still_widens_the_pair() {
        // Providers split wherever the socket did, so the row was measured
        // before the selector turned up. The pair moves down together: one
        // parted from its base stops being a selector at all.
        let mut tail = Tail::new(3, 5);
        push(&mut tail, "ab\u{26A0}");
        push(&mut tail, "\u{FE0F}");

        assert_eq!(rows(&tail), ["ab", WARNING]);
    }

    #[test]
    fn an_escape_sequence_from_a_tool_cannot_move_the_cursor() {
        // Tool output is not trusted to be plain text. A control character kept
        // verbatim would move a cursor this renderer believes it is tracking,
        // and the next frame would erase the wrong lines.
        let mut tail = Tail::new(20, 5);
        push(&mut tail, "a\x1b[2Jb");
        assert_eq!(rows(&tail), ["a[2Jb"]);
    }

    #[test]
    fn a_tab_advances_to_the_next_stop() {
        let mut tail = Tail::new(40, 5);
        push(&mut tail, "ab\tc");
        assert_eq!(rows(&tail), ["ab      c"]);
    }

    #[test]
    fn a_tab_past_the_edge_wraps_instead_of_overflowing_the_row() {
        let mut tail = Tail::new(6, 5);
        push(&mut tail, "abcde\tx");
        assert_eq!(rows(&tail), ["abcde", "x"]);
    }

    #[test]
    fn rows_past_the_bound_overflow_oldest_first() {
        let mut tail = Tail::new(20, 2);
        let out = push(&mut tail, "one\ntwo\nthree\nfour");

        assert_eq!(out, ["one", "two"]);
        assert_eq!(rows(&tail), ["three", "four"]);
    }

    #[test]
    fn the_tail_never_grows_past_its_bound() {
        // This is the property the whole type exists for: memory must not grow
        // with how long the session has run.
        let mut tail = Tail::new(20, 3);
        let mut overflow = Vec::new();

        for turn in 0..10_000 {
            tail.push(&format!("line {turn}\n"), &mut overflow);
            overflow.clear();
            assert!(tail.len() <= 3, "tail grew to {} rows", tail.len());
        }
    }

    #[test]
    fn wrapped_rows_count_against_the_bound_too() {
        // One logical line can be many display rows, so the bound has to be
        // about rows or a single long paragraph would blow past it.
        let mut tail = Tail::new(4, 2);
        let out = push(&mut tail, "abcdefghijklmnop");

        assert_eq!(out, ["abcd", "efgh"]);
        assert_eq!(rows(&tail), ["ijkl", "mnop"]);
    }

    #[test]
    fn the_row_the_cursor_sits_on_is_not_content() {
        // Otherwise every answer ending in a newline settles a blank line.
        let mut tail = Tail::new(20, 5);
        push(&mut tail, "one\ntwo\n");

        assert_eq!(rows(&tail), ["one", "two", ""]);
        assert_eq!(tail.content().collect::<Vec<_>>(), ["one", "two"]);
    }

    #[test]
    fn a_blank_line_somebody_wrote_is_content() {
        // Only the *last* empty row is the cursor's. The rest are output.
        let mut tail = Tail::new(20, 5);
        push(&mut tail, "one\n\ntwo");

        assert_eq!(tail.content().collect::<Vec<_>>(), ["one", "", "two"]);
    }

    #[test]
    fn clearing_leaves_one_empty_row() {
        let mut tail = Tail::new(20, 5);
        push(&mut tail, "one\ntwo");
        tail.clear();

        assert_eq!(rows(&tail), [""]);
        assert!(tail.is_empty());
    }

    #[test]
    fn a_zero_width_does_not_wrap_forever() {
        // A terminal reports zero columns while a pane is being dragged, and a
        // literal zero here would loop pushing empty rows.
        let mut tail = Tail::new(0, 3);
        push(&mut tail, "abc");
        assert_eq!(rows(&tail), ["a", "b", "c"]);
    }
}
