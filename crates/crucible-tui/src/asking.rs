//! The panel a tool call is answered in: what is about to run, and the answers.
//!
//! Drawn in the frame [`crate::Prompt`] uses, and standing where that box was,
//! so the box a line is typed into visibly becomes a box an answer is chosen
//! in. That is the whole of why it is bordered rather than ruled — the reader
//! has to notice that the keyboard now means something else, and a rule reads
//! as one more thing in the transcript.
//!
//! **Rows, not a screen.** Its subject, its sentences, its answers and its
//! footer are the caller's, and it never asks how tall the terminal is. The
//! bargain [`crate::Panel`] and [`crate::Ladder`] are already on.
//!
//! **What is about to run is never clipped.** Every other component here has a
//! ceiling and folds or shortens against it; this one does not, because a
//! report is read at a glance and a decision is consented to. A command cut at
//! the right edge is approved by its leading columns and then does whatever the
//! rest of it does. So the payload folds to the width, one row per simple
//! command, and where even that will not fit the panel returns nothing and the
//! caller asks the question in the scrollback instead, where nothing bounds it.
//!
//! **Three blanks, counted.** Under the subject, under the payload, under the
//! statement. They are what make the payload a block that is read rather than a
//! list skimmed past on the way to the answers, so they are the last thing
//! given up rather than the first — [`Spacing`] holds the order.
//!
//! **Where the colour goes.** The frame and the mark are [`Slot::Accent`], the
//! subject and the marked answer [`Slot::Strong`], the payload and the
//! sentences the reader's own foreground, and the footer [`Slot::Quiet`]. The
//! marked answer is marked as well as coloured, because the row a key is about
//! to act on is the last thing to leave to a hue.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::row::Row;
use crate::width::{clip, columns as wide, fold};

/// What a row spends on the frame: one column of edge on each side.
const AROUND: usize = 2;

/// Columns before the subject, the sentences and an answer's mark.
const SAID: usize = 2;

/// Columns before a command or a path.
///
/// Two further in than everything else, which is what makes the payload a block
/// rather than another sentence. It is also the one indent that costs width the
/// thing it indents may not be cut to fit — so it stays small.
const PAYLOAD: usize = 4;

/// One call, waiting for a verdict.
///
/// Every string here is the caller's. This crate depends on nothing and cannot
/// know what a `Sensitivity` is, which is what keeps the wording of a permission
/// question in the one place that has read the call.
#[derive(Debug, Clone, Copy)]
pub struct Question<'a> {
    /// The few words naming what is about to happen: `Bash command`.
    pub subject: &'a str,
    /// One row each, and the part that is never clipped: the simple commands a
    /// call decomposes into, or the path a change is about to be written to.
    pub payload: &'a [&'a str],
    /// The sentence under the payload, saying why this stopped.
    pub statement: &'a str,
    /// The question the answers answer.
    pub question: &'a str,
    /// The answers, in the order they are numbered.
    pub answers: &'a [&'a str],
    /// Which answer the mark stands on.
    pub marked: usize,
    /// The quiet row under the frame, naming the keys that are not on it.
    pub footer: &'a str,
}

impl Question<'_> {
    /// The panel in `room` rows, giving up spacing before it gives up sense,
    /// and empty where not even the floor fits.
    ///
    /// Empty is the right answer here rather than a degraded one: a panel drawn
    /// short is one whose payload was cut, and that is the single thing this
    /// component exists to refuse.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> Vec<Row> {
        for spacing in Spacing::LADDER {
            let rows = self.laid(columns, glyphs, spacing);
            if !rows.is_empty() && rows.len() <= room {
                return rows;
            }
        }

        Vec::new()
    }

    /// The panel at one rung of the ladder.
    fn laid(&self, columns: usize, glyphs: Glyphs, spacing: Spacing) -> Vec<Row> {
        let inner = columns.saturating_sub(AROUND);
        let across = inner.saturating_sub(SAID);
        let payload = inner.saturating_sub(PAYLOAD);

        // No room to fold what is about to run to is no panel. Everything else
        // here has somewhere to go at one column; the payload does not.
        if payload == 0 || self.payload.is_empty() || self.answers.is_empty() {
            return Vec::new();
        }

        let (open, opened) = glyphs.top();
        let (close, closed) = glyphs.bottom();
        let bar = glyphs.horizontal().repeat(inner);

        let mut rows = vec![Row::new().then(Slot::Accent, format!("{open}{bar}{opened}"))];
        rows.push(framed(
            said(SAID, Slot::Strong, clip(self.subject, across)),
            inner,
            glyphs,
        ));
        if spacing.opening {
            rows.push(framed(Row::new(), inner, glyphs));
        }

        for one in self.payload {
            for line in fold(one, payload) {
                rows.push(framed(said(PAYLOAD, Slot::Plain, line), inner, glyphs));
            }
        }
        rows.push(framed(Row::new(), inner, glyphs));

        if spacing.statement {
            for line in fold(self.statement, across) {
                rows.push(framed(said(SAID, Slot::Plain, line), inner, glyphs));
            }
            rows.push(framed(Row::new(), inner, glyphs));
        }

        for line in fold(self.question, across) {
            rows.push(framed(said(SAID, Slot::Plain, line), inner, glyphs));
        }
        for (at, answer) in self.answers.iter().enumerate() {
            rows.extend(self.answered(at, answer, inner, glyphs));
        }

        rows.push(Row::new().then(Slot::Accent, format!("{close}{bar}{closed}")));
        if spacing.footer {
            let room = columns.saturating_sub(SAID);
            rows.push(said(SAID, Slot::Quiet, clip(self.footer, room)));
        }

        rows
    }

    /// One answer's rows: its number, its words, and the mark where it is the
    /// one a key is about to act on.
    ///
    /// A continuation row opens under the answer's own first column rather than
    /// under its number, so a folded answer reads as one answer and the column
    /// of numbers stays a column.
    fn answered(&self, at: usize, answer: &str, inner: usize, glyphs: Glyphs) -> Vec<Row> {
        let marked = at == self.marked;
        let mark = if marked { glyphs.caret() } else { " " };
        let number = format!(" {}. ", at + 1);
        let front = SAID + wide(mark) + wide(&number);
        let slot = if marked { Slot::Strong } else { Slot::Plain };

        fold(answer, inner.saturating_sub(front))
            .into_iter()
            .enumerate()
            .map(|(row, line)| {
                let opening = if row == 0 {
                    Row::new()
                        .then(Slot::Plain, " ".repeat(SAID))
                        .then(Slot::Accent, mark)
                        .then(slot, format!("{number}{line}"))
                } else {
                    Row::new()
                        .then(Slot::Plain, " ".repeat(front))
                        .then(slot, line)
                };

                framed(opening, inner, glyphs)
            })
            .collect()
    }
}

/// Which blanks a rung of the ladder still draws.
///
/// The order is the argument: the footer names keys documented elsewhere, the
/// statement says what the subject and the answers between them already imply,
/// and the blank under the subject is the last thing to go. Below that there is
/// no rung — the payload and the blank under it are not on this list.
#[derive(Debug, Clone, Copy)]
struct Spacing {
    /// The quiet row of keys under the frame.
    footer: bool,
    /// The sentence saying why this stopped, and the blank under it.
    statement: bool,
    /// The blank between the subject and the payload.
    opening: bool,
}

impl Spacing {
    /// The rungs, in the order they are given up.
    const LADDER: [Self; 4] = [
        Self {
            footer: true,
            statement: true,
            opening: true,
        },
        Self {
            footer: false,
            statement: true,
            opening: true,
        },
        Self {
            footer: false,
            statement: false,
            opening: true,
        },
        Self {
            footer: false,
            statement: false,
            opening: false,
        },
    ];
}

/// One column of edge running down.
fn edge(glyphs: Glyphs) -> Row {
    Row::new().then(Slot::Accent, glyphs.vertical())
}

/// `row`, out to the full inner width, so that the edge on its right lands
/// where every other row put one.
fn framed(mut row: Row, inner: usize, glyphs: Glyphs) -> Row {
    row.pad(inner);
    edge(glyphs).join(row).join(edge(glyphs))
}

/// A row of `text` in `slot`, opening `indent` columns in.
fn said(indent: usize, slot: Slot, text: &str) -> Row {
    Row::new()
        .then(Slot::Plain, " ".repeat(indent))
        .then(slot, text)
}

#[cfg(test)]
mod tests;
