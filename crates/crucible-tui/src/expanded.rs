//! What a transcript cut down to a row, shown whole: a rule, and under it each
//! result behind the line of the call it answers.
//!
//! **Rows, not a screen.** Like every component here this hands back rows and
//! draws nothing. What is on screen at the moment somebody asks is the caller's
//! to know, and so is what happens to these rows afterwards — which for this one
//! is that they are taken back rather than written down. A result is already in
//! the record as the row that could not say it; the same text committed a second
//! time would be a transcript that says everything twice.
//!
//! **What folds and what is left alone.** A result is a tool's own output and
//! its lines mean what their layout means, so a line too wide for the window is
//! cut rather than folded — a column of numbers rewrapped into a paragraph is
//! harder to read than one that runs off the edge, and the edge is where the
//! reader already knows to look. The call's line above it is a label and is cut
//! the same way. Nothing here folds.
//!
//! **The window.** Everything is laid out and then a window `from` rows down is
//! taken out of it, because how far down the reader may go is a fact about the
//! layout rather than about the keyboard: it depends on how many rows the
//! results came to at *this* width, which is not known until they are laid out.
//! [`Expanded::end`] is that number and the caller clamps its own offset to it
//! before asking for the picture.
//!
//! **The footer names a key only where it does something.** `↑↓ to see more` is
//! true when there are rows the window did not reach and false when there are
//! not, and a row that says it either way is one nobody can believe the rest of
//! the time.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::{clip, columns as wide};

/// Rows spent on everything that is not a result: the rule, the blank under it,
/// and the blank and the footer at the foot.
const CHROME: usize = 4;

/// The one key always worth naming.
const CLOSE: &str = "esc to close";

/// And the same row where the window did not reach the end.
///
/// Written out whole rather than joined, for the reason the panels here write
/// theirs out whole: what a footer costs is read off the row somebody sees, and
/// there is no joining two `&'static str` without allocating on the layout path.
const MORE: &str = "esc to close · ↑↓ to see more";

/// One result, under the line of the call it answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Shown<'a> {
    /// The call's line, in the words the transcript committed it under.
    pub called: &'a str,
    /// The whole of what came back.
    pub text: &'a str,
}

/// Results shown whole, with a window over them.
#[derive(Debug, Clone, Copy)]
pub struct Expanded<'a> {
    /// What to show, in the order it is read: newest first, because the result
    /// somebody is asking about is almost always the one that just went past.
    pub shown: &'a [Shown<'a>],
    /// How far down the whole of it the window opens.
    pub from: usize,
}

impl Expanded<'_> {
    /// The whole of it, at this width, before any window is taken.
    ///
    /// Never wider than `columns` anywhere: a row past the last column is one
    /// the terminal wraps itself, which leaves the cursor a row below where the
    /// next frame expects it.
    #[must_use]
    fn laid(&self, columns: usize) -> Vec<Row> {
        let mut rows = Vec::new();

        for (at, shown) in self.shown.iter().enumerate() {
            // Between results and not above the first, which already has the
            // rule and a blank above it. A blank leading the list would part it
            // from a heading that is not there.
            if at > 0 {
                rows.push(Row::new());
            }

            rows.push(Row::new().then(Slot::Strong, clip(shown.called, columns)));
            rows.push(Row::new());

            // On the reader's own ground rather than in the quieter colour the
            // transcript's row for it is drawn in. That row is a fragment beside
            // the call it hangs off; this is the thing somebody asked to read,
            // and a screen of dim text is a screen asking not to be.
            for line in shown.text.lines() {
                rows.push(Row::new().then(Slot::Plain, clip(line, columns)));
            }
        }

        rows
    }

    /// How far down [`Expanded::from`] may go at this size.
    ///
    /// Zero where the whole of it fits, which is what closes the window as well
    /// as bounding it: there is nothing to scroll, so the offset the caller
    /// clamps against this is zero too.
    #[must_use]
    pub fn end(&self, columns: usize, room: usize) -> usize {
        let held = room.saturating_sub(CHROME);

        self.laid(columns).len().saturating_sub(held)
    }

    /// The window as it fits in `room` rows, and empty where nothing does.
    ///
    /// Empty rather than as-much-as-fits, for the reason every component here
    /// gives nothing back rather than something short: a caller with no room
    /// draws no live region at all, and one drawn at as-much-as-fits is a frame
    /// whose next rewind reaches over rows the terminal has already taken.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        let Some(held) = room.checked_sub(CHROME).filter(|held| *held > 0) else {
            return Vec::new();
        };

        let laid = self.laid(columns);
        let from = self.from.min(laid.len().saturating_sub(held));
        let scrolls = laid.len() > held;

        let mut rows = vec![
            Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
            Row::new(),
        ];

        // As tall as what it holds and no taller. Padding out to the window
        // would put the footer at the foot of the screen with a block of
        // nothing above it, and most results are a few lines rather than a
        // screenful — the view is meant to be read and then closed, not lived
        // in.
        rows.extend(laid.into_iter().skip(from).take(held));

        rows.push(Row::new());
        rows.push(Row::new().then(Slot::Quiet, footer(scrolls, columns)));

        rows
    }
}

/// The row under the window, saying only what is true of it.
fn footer(scrolls: bool, columns: usize) -> String {
    let said = if scrolls { MORE } else { CLOSE };

    // A window this narrow has given the arrows up rather than the way out, so
    // what is cut is the end of the row rather than the start of it.
    if wide(said) > columns {
        return clip(CLOSE, columns).to_owned();
    }

    said.to_owned()
}

#[cfg(test)]
mod tests;
