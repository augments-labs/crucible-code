//! The live tail: the only part of the transcript this process still owns.
//!
//! Streamed output is wrapped into display rows as it arrives and held here.
//! Once there are more rows than the bound, the oldest ones *overflow*: they are
//! handed back to be written once and then forgotten -- all but whether the
//! last of them drew anything, which is [`Tail::parted`] and is the only thing
//! about a departed row that survives it. Everything above the tail belongs to
//! the terminal's scrollback, so what this file holds is bounded by the tail's
//! own size rather than by how long the session has run. The transcript is held
//! whole by the runner, and is the one thing anywhere in this program that grows
//! with the session.
//!
//! Wrapping happens here rather than being left to the terminal because the
//! renderer has to know how many rows it drew in order to move back over them.
//! A terminal that wrapped a row on its own would leave the cursor somewhere
//! this process did not predict, and the next frame would erase the wrong lines.

use std::collections::VecDeque;

use crate::color::{Palette, Slot, Worn};
use crate::escape::Escapes;
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
    /// How far into an escape sequence the stream is.
    ///
    /// Kept here rather than made fresh per delta: a delta is a piece of the
    /// wire, not a piece of the output, so a sequence arrives split as often as
    /// not. Started again each time, the half after the split would be drawn as
    /// text and counted as columns — which is the bug this exists to stop,
    /// reappearing only under streaming.
    escapes: Escapes,
    /// The sequence text is written under, and the one that ends it.
    ///
    /// Both come from the palette, which answers in `&'static str`, so wearing
    /// a slot allocates nothing and a row that wraps can open the same slot
    /// again without asking a second time. Empty for [`Slot::Plain`] and for
    /// every slot where colour is off, which is what keeps a redirected run
    /// free of escape bytes.
    worn: Worn,
    /// What closes [`Self::worn`].
    close: &'static str,
    /// Where the row being appended to could be cut, if it has to be.
    ///
    /// `None` until a space lands on the row, and again as soon as the row
    /// ends. See [`Broken`] for what a cut has to put back.
    broken: Option<Broken>,
    /// Whether the row being appended to has [`Self::worn`] open on it.
    ///
    /// The invariant this keeps is that every row is closed once [`Self::push`]
    /// returns. A row read with a sequence still open on it would set that
    /// attribute on everything drawn after it, and the row settled into the
    /// record would carry it into the reader's scrollback for good.
    open: bool,
    /// Whether the row that left most recently drew nothing.
    ///
    /// A row that has overflowed belongs to the terminal, and this tail is the
    /// only party that ever measured it. [`Self::parted`] is what the
    /// measurement is kept for. True to start with, because a tail that has let
    /// nothing go has put no row under anything.
    let_blank: bool,
}

/// The last place on a row a wrap could cut it, and what the cut has to mend.
///
/// A row is wrapped at the last space on it rather than at the column the text
/// happened to reach: a word split across two rows is one the reader has to put
/// back together, and an answer is prose far more often than it is a column of
/// anything. Where no space landed on the row -- a word wider than the window,
/// a line of code -- the cut is where the row ends, exactly as it always was.
///
/// The slot is here because a cut lands in the middle of one as often as not,
/// and every row this tail hands out is balanced on its own: see
/// [`Tail::break_row`] for why that is not negotiable.
#[derive(Debug, Clone, Copy)]
struct Broken {
    /// Where the carried text starts, in bytes into the row.
    at: usize,
    /// What the row measured up to there.
    width: usize,
    /// The slot open across the cut, and what closes it.
    worn: Worn,
    close: &'static str,
    /// Whether that slot was open at all.
    open: bool,
}

/// The spaces a wrapped row is indented with, taken as a slice rather than
/// built: a hang is a handful of columns and a wrap is on the render path.
const SPACES: &str = "                                ";

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
            escapes: Escapes::default(),
            worn: Worn::Chosen(""),
            close: "",
            open: false,
            broken: None,
            let_blank: true,
        }
    }

    /// Writes everything after this in `slot`, until the next call says
    /// otherwise.
    ///
    /// The slot is crucible's own reading of what arrived — which run of the
    /// answer is a heading, which is code — so it is applied here rather than
    /// travelling as bytes inside the text. Bytes inside the text are the one
    /// thing this tail refuses: they arrive from the model, they are dropped
    /// undrawn and uncounted, and the reason is that an instruction from there
    /// could move the cursor out of the region this renderer believes it owns.
    ///
    /// The sequence costs no column. Width is counted as characters are placed
    /// and a sequence is never placed, so a row wearing a slot wraps in exactly
    /// the same column as the same row plain.
    pub(crate) fn wear(&mut self, slot: Slot, palette: &Palette) {
        let worn = palette.open(slot);
        if worn == self.worn {
            return;
        }

        // Closed here and opened on the next character rather than now: a slot
        // worn at the end of a delta and changed at the start of the next one
        // would otherwise leave an opening sequence around nothing.
        self.close_worn();
        self.worn = worn;
        self.close = palette.close();
    }

    /// Appends streamed text, moving any rows past the bound into `overflow`.
    ///
    /// The caller owns `overflow` and reuses it across frames, so a delta that
    /// pushes nothing out of the tail allocates nothing.
    pub(crate) fn push(&mut self, delta: &str, overflow: &mut Vec<String>) {
        for character in delta.chars() {
            // Instruction rather than text: neither drawn nor counted. What
            // reaches a tail is output this process did not write, and a
            // sequence from there could move the cursor out of the region this
            // renderer believes it owns.
            if self.escapes.holds(character) {
                continue;
            }

            match character {
                // A newline ends the row wherever it is.
                '\n' => self.break_row(),
                // Bare carriage returns arrive as half of a CRLF from a tool's
                // output. Dropping them is what stops a blank row per line.
                '\r' => {}
                '\t' => self.advance_to_tab_stop(),
                EMOJI_PRESENTATION => self.place_emoji_presentation(),
                _ => self.place(character),
            }
        }

        // Every row this tail hands out is balanced, including the one still
        // being appended to. The next character reopens the slot where it left
        // off, so the pair costs a few bytes per delta and nothing per frame.
        self.close_worn();

        while self.rows.len() > self.bound {
            // The row that leaves is complete: only the last row is still being
            // appended to, and the bound is at least one.
            if let Some(row) = self.rows.pop_front() {
                self.let_blank = row.width == 0;
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
            Some(row) if row.width == 0 => self.rows.len().saturating_sub(1),
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
    ///
    /// Measured in columns rather than bytes, like everything else here: a row
    /// holding a slot's opening and closing sequence and no text draws nothing
    /// and is nothing.
    #[must_use]
    pub(crate) fn is_empty(&self) -> bool {
        self.rows.iter().all(|row| row.width == 0)
    }

    /// Whether what this tail has written ends in a blank row.
    ///
    /// Nothing live, and the row that left before it drew nothing — which is
    /// what an answer that has just ended a paragraph looks like from here, and
    /// the one thing the renderer cannot read off the tail itself, since the
    /// row it is asking about has already gone to the terminal.
    #[must_use]
    pub(crate) fn parted(&self) -> bool {
        self.is_empty() && self.let_blank
    }

    /// How many columns along the row being appended to the next character
    /// goes.
    ///
    /// Where the cursor belongs whenever a frame wrote something after the
    /// tail. It is the one column this crate cannot recover by counting bytes:
    /// a row holds escape sequences that cost nothing and characters that cost
    /// two, and the tail is what has already counted both.
    #[must_use]
    pub(crate) fn column(&self) -> usize {
        self.rows.back().map_or(0, |row| row.width)
    }

    /// Drops everything, ready for the next turn.
    ///
    /// The rows are gone rather than committed: the caller decides whether
    /// what was live deserved to reach scrollback.
    pub(crate) fn clear(&mut self) {
        self.rows.clear();
        self.rows.push_back(Row::default());
        // A turn that ended mid-sequence ended mid-sequence. Carrying that into
        // the next one would swallow the start of the next answer.
        self.escapes = Escapes::default();
        // The slot belongs to the answer that was being written, and that
        // answer is over. An unfinished code block would otherwise paint the
        // whole of the next one.
        self.worn = Worn::Chosen("");
        self.close = "";
        self.open = false;
        self.broken = None;
    }

    /// Ends the row being appended to and starts a fresh one, carrying the slot
    /// in force across the break.
    ///
    /// Every row is closed where it ends and opened where it begins, so a
    /// paragraph wrapped over four rows is four self-contained rows rather than
    /// one attribute set on the first and unset on the last. That matters
    /// because the rows do not stay together: the oldest overflow into
    /// scrollback one at a time, and a row that reached the terminal holding
    /// only half of a pair would leave the other half unwritten for good.
    fn break_row(&mut self) {
        self.close_worn();
        self.broken = None;
        self.rows.push_back(Row::default());
    }

    /// Ends the row being appended to at the last space on it, carrying what
    /// followed onto the next.
    ///
    /// Falls back to [`Tail::break_row`] where there is nothing to carry: a row
    /// with no space on it, and one whose space is the last thing on it.
    fn wrap_row(&mut self) {
        let hang = self.hang();

        let Some(broken) = self.broken.take() else {
            self.break_row();
            self.indent(hang);
            return;
        };

        let Some(row) = self.rows.back_mut() else {
            self.break_row();
            return;
        };

        if broken.at >= row.text.len() {
            self.break_row();
            self.indent(hang);
            return;
        }

        let mut carried = row.text.split_off(broken.at);
        let width = row.width.saturating_sub(broken.width);
        row.width = broken.width;

        // The cut fell inside a slot, so the half left behind is closed and the
        // half carried over opens the same one again. Whatever was opened or
        // closed after the cut travelled with the text it covers, so `open` is
        // still what it was: this mends the seam and nothing else.
        if broken.open {
            row.text.push_str(broken.close);
            carried.insert_str(0, broken.worn.as_str());
        }

        self.rows.push_back(Row {
            text: carried,
            width,
        });
        self.indent(hang);
    }

    /// How far a row started by a wrap is indented, so what continues a line
    /// is drawn under the line's own text rather than under its mark.
    ///
    /// Read off the row being cut rather than remembered, which is what makes
    /// it the same answer on the second wrap as on the first: an item's mark
    /// gives way to the spaces standing in for it, and those spaces are the
    /// leading indent the next wrap reads. A line break asks nothing of this,
    /// so a fresh line starts flush wherever the line above it hung.
    ///
    /// A mark here is what a mark looks like -- one narrow character that is
    /// not a letter, or a number and its dot, with a space after it. This is
    /// the wrapper's own reading of the row in front of it: the tail is handed
    /// text and never told what any of it meant, and a run of it can be an
    /// answer the model wrote with its markers still in.
    fn hang(&self) -> usize {
        let Some(row) = self.rows.back() else {
            return 0;
        };

        let mut escapes = Escapes::default();
        let mut drawn = row
            .text
            .chars()
            .filter(move |character| !escapes.holds(*character))
            .peekable();

        let mut hang = 0;
        while drawn.next_if_eq(&' ').is_some() {
            hang += 1;
        }

        let Some(mark) = drawn.next() else {
            return 0;
        };

        let mut marked = 1;
        if mark.is_ascii_digit() {
            while drawn.next_if(char::is_ascii_digit).is_some() {
                marked += 1;
            }

            if drawn.next_if(|next| matches!(next, '.' | ')')).is_none() {
                return hang;
            }

            marked += 1;
        } else if mark.is_alphanumeric() || width::advance(mark) != Some(1) {
            return hang;
        }

        if drawn.next() == Some(' ') {
            hang + marked + 1
        } else {
            hang
        }
    }

    /// Pads the row a wrap has just started, where there is room to.
    ///
    /// A hang with no columns left behind it is no hang: the row would be
    /// full before anything was written on it, and the character that would
    /// not fit would push a fresh row under it every time.
    fn indent(&mut self, hang: usize) {
        let hang = hang.min(SPACES.len());
        if hang == 0 {
            return;
        }

        if let Some(row) = self.rows.back_mut() {
            if hang + row.width >= self.width {
                return;
            }

            // Before the sequence a wrap carried over rather than after it: a
            // slot can paint what it covers, and what these stand in for is
            // the mark on the row above, not the words beside it.
            row.text.insert_str(0, &SPACES[..hang]);
            row.width += hang;
        }
    }

    /// Opens the worn slot on the current row, if it is not open already.
    fn open_worn(&mut self) {
        if self.open || self.worn.is_empty() {
            return;
        }

        if let Some(row) = self.rows.back_mut() {
            row.text.push_str(self.worn.as_str());
            self.open = true;
        }
    }

    /// Closes the worn slot on the current row, if it is open.
    fn close_worn(&mut self) {
        if !self.open {
            return;
        }

        if let Some(row) = self.rows.back_mut() {
            row.text.push_str(self.close);
        }
        self.open = false;
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

        if self.column() + advance > self.width {
            self.wrap_row();

            // What a wrap carried over can be nearly a whole row wide, and a
            // row counted short is a row the terminal wraps itself.
            if self.column() + advance > self.width {
                self.break_row();
            }
        }

        self.open_worn();
        if let Some(row) = self.rows.back_mut() {
            row.text.push(character);
            row.width += advance;

            // Recorded after the space rather than before it, so the space
            // stays on the row it ended and the carried half opens on a word.
            if character == ' ' {
                self.broken = Some(Broken {
                    at: row.text.len(),
                    width: row.width,
                    worn: self.worn,
                    close: self.close,
                    open: self.open,
                });
            }
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

        if self.column() + 1 > self.width {
            // The pair moves down together: a selector parted from its base
            // stops asking for anything, and the base left behind would draw
            // narrow on a row this tail had already counted wide.
            if let Some(row) = self.rows.back_mut() {
                row.text.pop();
                row.width = row.width.saturating_sub(1);
            }
            self.break_row();
            if let Some(base) = base {
                self.place(base);
            }
        }

        self.open_worn();
        if let Some(row) = self.rows.back_mut() {
            row.text.push(EMOJI_PRESENTATION);
            row.width += 1;
        }
    }

    /// Pads with spaces to the next tab stop, wrapping if the stop is past the
    /// edge. Expanded here rather than passed through so the width this tail
    /// reports is the width the terminal will show.
    fn advance_to_tab_stop(&mut self) {
        let current = self.column();
        let target = width::tab_stop(current);

        if target > self.width {
            self.break_row();
            return;
        }

        for _ in current..target {
            self.place(' ');
        }
    }
}

#[cfg(test)]
mod tests;
