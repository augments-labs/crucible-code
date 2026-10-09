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
//! **The complete text.** Explicit line breaks stay in place, and a line too
//! wide for the window continues on rows below it. The transcript already
//! clipped its preview: clipping again here would make the end of a command
//! or a single-line result unreachable even after asking for the whole thing.
//!
//! **The window.** Everything is measured and then a window `from` rows down is
//! taken out of it, because how far down the reader may go is a fact about the
//! layout rather than about the keyboard: it depends on how many rows the
//! results came to at *this* width, which is not known until they are laid out.
//! [`Expanded::end`] is that number and the caller clamps its own offset to it
//! before asking for the picture. Only the results the window reaches are laid
//! out for it, and a caller that kept how long each one was at this width
//! hands that to [`Expanded::within_measured`] rather than having it measured
//! again, so a frame over a long list costs what its window shows.
//!
//! **The footer names a key only where it does something.** `↑↓ pgup pgdn to
//! see more` is true when there are rows the window did not reach and false
//! when there are not, and a row that says it either way is one nobody can
//! believe the rest of the time. `←→ result 2 of 7` is said only where there is
//! both a window to move and another result to move it to, and counts the
//! result at the top of the window, newest first. Where the row is wider than
//! the window it loses whole segments from the right, so what is left still
//! says something true, and `esc to close` is the one it never loses. Its
//! arrows and the mark parting its segments are the session's glyphs, so with
//! `glyphs` set to `ascii` it reads `esc to close - ^v pgup pgdn to see more -
//! <> result 2 of 7`.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::{clip, columns as wide};

/// Rows spent on everything that is not a result: the rule, the blank under it,
/// and the blank and the footer at the foot.
const CHROME: usize = 4;

/// The one key always worth naming.
const CLOSE: &str = "esc to close";

/// The keys that move the window, named where it did not reach the end, after
/// the arrows that walk it.
const MORE: &str = "pgup pgdn to see more";

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
            lay(at, shown, columns, &mut rows);
        }
        rows
    }

    /// How many rows each result comes to at this width, counting the blank
    /// that parts it from the one above.
    fn lengths(&self, columns: usize) -> Vec<usize> {
        let mut rows = Vec::new();
        self.shown
            .iter()
            .enumerate()
            .map(|(at, shown)| {
                rows.clear();
                lay(at, shown, columns, &mut rows);
                rows.len()
            })
            .collect()
    }

    /// How many rows of results `room` rows show at once: what the rule, the
    /// blanks and the footer leave.
    #[must_use]
    pub const fn seen(room: usize) -> usize {
        room.saturating_sub(CHROME)
    }

    /// How many rows the whole of it comes to at this width, before any
    /// window is taken.
    ///
    /// Asked one result at a time by a caller that has to know where each
    /// begins. Each is laid out on its own, and one blank row parts it from
    /// the result above, so the rows of a list are those of its results and a
    /// blank between each two.
    #[must_use]
    pub fn length(&self, columns: usize) -> usize {
        self.laid(columns).len()
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
    /// stands nothing at all, and a window drawn at as-much-as-fits reads as the
    /// whole of what was cut short while hiding that it was cut twice.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        self.within_measured(&self.lengths(columns), columns, room, glyphs)
    }

    /// [`Expanded::within`], for a caller that already knows how many rows
    /// each result comes to at this width: `lengths` has one for each of
    /// [`Expanded::shown`], as [`Expanded::length`] counts that result laid
    /// out on its own, and the blank parting it from the one above.
    ///
    /// Only the results the window reaches are laid out, so a frame costs what
    /// the window shows rather than everything standing, which is what lets a
    /// caller walking a long list keep what it measured between frames. Where
    /// `lengths` does not have one for each result they are measured here.
    #[must_use]
    pub fn within_measured(
        &self,
        lengths: &[usize],
        columns: usize,
        room: usize,
        glyphs: Glyphs,
    ) -> Vec<Row> {
        let Some(held) = room.checked_sub(CHROME).filter(|held| *held > 0) else {
            return Vec::new();
        };
        let measured;
        let lengths = if lengths.len() == self.shown.len() {
            lengths
        } else {
            measured = self.lengths(columns);
            &measured
        };

        let total = lengths
            .iter()
            .fold(0_usize, |total, length| total.saturating_add(*length));
        let from = self.from.min(total.saturating_sub(held));
        let to = from.saturating_add(held);
        let scrolls = total > held;

        // Each result the window reaches, from the row the first of them
        // begins on; the rest are counted past rather than laid out. Counted
        // from one, which is the newest: the last result to begin at or above
        // the top of the window is the one the reader is in.
        let mut laid = Vec::new();
        let mut first = None;
        let mut top = 0;
        let mut start = 0_usize;
        for (at, (shown, length)) in self.shown.iter().zip(lengths).enumerate() {
            let end = start.saturating_add(*length);
            if start <= from {
                top += 1;
            }
            if end > from && start < to {
                first.get_or_insert(start);
                lay(at, shown, columns, &mut laid);
            }
            start = end;
        }

        let mut rows = vec![
            Row::new().then(Slot::Accent, glyphs.horizontal().repeat(columns)),
            Row::new(),
        ];

        // As tall as what it holds and no taller. Padding out to the window
        // would put the footer at the foot of the screen with a block of
        // nothing above it, and most results are a few lines rather than a
        // screenful — the view is meant to be read and then closed, not lived
        // in.
        let skipped = from.saturating_sub(first.unwrap_or(from));
        rows.extend(laid.into_iter().skip(skipped).take(held));

        rows.push(Row::new());
        let footer = footer(scrolls, top, self.shown.len(), columns, glyphs);
        rows.push(Row::new().then(Slot::Quiet, footer));

        rows
    }
}

/// Lays result `at` out under the rows already laid: the blank parting it from
/// the one above, the call's line, a blank, and what came back.
fn lay(at: usize, shown: &Shown<'_>, columns: usize, rows: &mut Vec<Row>) {
    // Between results and not above the first, which already has the rule and
    // a blank above it. A blank leading the list would part it from a heading
    // that is not there.
    if at > 0 {
        rows.push(Row::new());
    }

    for line in shown.called.lines() {
        rows.extend(Row::new().then(Slot::Strong, line).fold(columns));
    }
    rows.push(Row::new());

    // On the reader's own ground rather than in the quieter colour the
    // transcript's row for it is drawn in. That row is a fragment beside the
    // call it hangs off; this is the thing somebody asked to read, and a screen
    // of dim text is a screen asking not to be.
    for line in shown.text.lines() {
        if line.is_empty() {
            rows.push(Row::new());
        } else {
            rows.extend(Row::new().then(Slot::Plain, line).fold(columns));
        }
    }
}

/// The row under the window, saying only what is true of it: the way out, the
/// keys that move the window where there is somewhere to move it, and which of
/// `of` results is at its top where there is another to step to.
fn footer(scrolls: bool, top: usize, of: usize, columns: usize, glyphs: Glyphs) -> String {
    // A window too narrow even for the way out keeps as much of it as fits,
    // and nothing after it.
    let mut said = clip(CLOSE, columns).to_owned();
    if !scrolls {
        return said;
    }

    let parted = format!(" {} ", glyphs.dot());
    let (up, down) = glyphs.walking();
    let (newer, older) = glyphs.stepping();
    let more = format!("{up}{down} {MORE}");
    let counted = (of > 1).then(|| format!("{newer}{older} result {top} of {of}"));
    for segment in std::iter::once(more).chain(counted) {
        // Whole segments or none, from the right: half of a key's name is not
        // a key anybody can press, and a count cut short reads as a different
        // count.
        if wide(&said) + wide(&parted) + wide(&segment) > columns {
            break;
        }
        said.push_str(&parted);
        said.push_str(&segment);
    }
    said
}

#[cfg(test)]
mod tests;
