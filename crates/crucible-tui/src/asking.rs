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
//! rest of it does. So the payload folds to the width, word for word and
//! operators and all, and where even that will not fit the panel returns
//! nothing and the caller asks the question in the scrollback instead, where
//! nothing bounds it.
//!
//! **Three blanks, counted.** Under the subject, under the payload, under the
//! statement. They are what make the payload a block that is read rather than a
//! list skimmed past on the way to the answers, so they are the last thing
//! given up rather than the first — [`Spacing`] holds the order.
//!
//! **The description is a caption, and that is why it has no blank above it.**
//! A blank is what separates one block here from the next, so withholding one
//! is the whole of what joins that row to the command it describes.
//!
//! **The prose says whose words it is before it says anything else.** The
//! paragraphs are the model's and everything drawn around them is not, and the
//! person deciding whether to allow a command is exactly the one who has to
//! know which is which. So the block opens with a row of the panel's own,
//! drawn quiet like the row that counts what is below it. That row scrolls with
//! the prose rather than being pinned above it: a row held out of the window is
//! one the arithmetic on either side of the window would each have to subtract,
//! and those two disagreeing by one is an arrow that stops a row from where the
//! picture stops.
//!
//! **The prose is the first thing to give way, and it says so.** A description
//! and an explanation are written by whatever asked for the permission, so they
//! are drawn quiet, they sit below the thing they are about, and they are what
//! the height is taken out of: the explanation is cut to the rows left over and
//! the row where it stops says how many are still below it. What is about to
//! run and the answers under it are never what gives way — and unlike a rung of
//! [`Spacing`], the reader gets this back by pressing the key again. To keep
//! the same argument on one row, a description is clipped rather than folded:
//! a caption that folded would let whatever wrote it decide how tall this is.
//!
//! **The count is of what is below, and it runs out.** A reader looking at a
//! cut explanation is pressing down, so down is what the row counts: the number
//! falls as the window moves, and at the end of the prose there is nothing left
//! to say and the row is not drawn. A constant count would be a true sentence
//! about the whole explanation read as a false one about the direction being
//! pressed. The row it gives back goes to the prose, so the panel is the same
//! height throughout and the last press uncovers exactly what it promised.
//!
//! **Clamping happens twice, and the second one is the caller's.** The offset
//! into the prose arrives on the panel, because this crate reads no keys — so
//! whatever does read them holds a copy of it across frames. The panel clamps
//! what it is given, which is what stops a key held down scrolling the prose off
//! the top; [`Question::end`] is what stops the caller's copy running on past
//! the end, where every press of the key that went too far is a press coming
//! back that moves nothing.
//!
//! **A key is named only where it does something.** What the cut leaves off
//! screen is reachable — the window over the prose moves by a row at a time —
//! and the footer says so only where the prose did not fit. Where it did not,
//! it says so at the end of the prose too, because up is still a direction
//! there. The panel is the only party that knows any of this, because it is the
//! one that folded the paragraphs and found out how tall they were.
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

/// The fewest rows worth opening the prose in: the blank that opens it, and the
/// row saying how much of it is still below.
///
/// Under that there is nothing true to draw, so the panel stays the one the
/// reader was already looking at and the window is what has to change.
///
/// It did not move when the prose gained a row naming whose words it is, and the
/// reason is that the row is worth the first glimpse. A window this short opens
/// on who wrote what is under it and a count of how much that is, which is two
/// true things about a page nobody can see yet; the alternative is a first row of
/// somebody's argument with nothing saying it is theirs.
const AT_LEAST: usize = 2;

/// One call, waiting for a verdict.
///
/// Every string here is the caller's. This crate depends on nothing and cannot
/// know what a `Sensitivity` is, which is what keeps the wording of a permission
/// question in the one place that has read the call.
#[derive(Debug, Clone, Copy)]
pub struct Question<'a> {
    /// The few words naming what is about to happen: `Bash command`.
    pub subject: &'a str,
    /// What is about to happen, word for word: the command as the call carried
    /// it, or the path a change is about to be written to.
    ///
    /// Folded to the width and never clipped, reworded or joined. A compound
    /// command keeps the operators that make it compound, because `&&` and `;`
    /// are the difference between two commands and two commands *if the first
    /// one worked*, and that difference is part of what is being consented to.
    pub payload: &'a [&'a str],
    /// One row under the payload saying what the call is for, or empty.
    pub description: &'a str,
    /// The row that opens the prose, naming whose words the rest of it is.
    ///
    /// Drawn quiet like the row that counts what is below, because both are the
    /// panel talking about the paragraphs rather than a paragraph. Empty draws
    /// nothing, and the prose then opens on the first paragraph.
    pub attribution: &'a str,
    /// The paragraphs a reader asked to see, or empty until they ask.
    pub explanation: &'a [&'a str],
    /// Which row of the explanation the window over it opens at.
    ///
    /// Clamped here rather than by the caller, so a key held down runs to the
    /// end of the prose and stops instead of scrolling it off the top. A caller
    /// keeping its own copy of this across frames clamps that with
    /// [`Self::end`], which is where the clamp here lands.
    pub from: usize,
    /// The extra footer item, drawn only where the prose did not fit — at the
    /// end of it as much as at the start, because up is still a direction there.
    pub more: &'a str,
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
        // The prose gives way before the spacing does, and before anything the
        // ladder touches. It is the one thing here a reader can have back for
        // the price of the key they pressed to see it.
        let spare = self.spare(columns, room, glyphs);
        if spare > 0 {
            return self.laid(columns, glyphs, Spacing::WHOLE, spare);
        }

        for spacing in Spacing::LADDER {
            let rows = self.laid(columns, glyphs, spacing, 0);
            if !rows.is_empty() && rows.len() <= room {
                return rows;
            }
        }

        Vec::new()
    }

    /// The furthest down the prose the window can open, at this size.
    ///
    /// Zero where there is nowhere to scroll to, which is both the prose that
    /// fitted and the window with no room to open any in — so a caller clamps
    /// with this rather than asking first whether there was anything to clamp.
    ///
    /// [`Self::from`] is clamped here too, and that is not this. Clamping what
    /// arrives keeps a held key from scrolling the prose off the top; what this
    /// answers is the caller holding its own copy of the offset across frames,
    /// which is anything reading the arrows, since this crate reads no keys.
    /// Let that copy run on past the end and every press of the key that went
    /// too far is a press coming back that moves nothing.
    #[must_use]
    pub fn end(&self, columns: usize, room: usize, glyphs: Glyphs) -> usize {
        let spare = self.spare(columns, room, glyphs);
        if spare == 0 {
            return 0;
        }

        let inner = columns.saturating_sub(AROUND);
        self.prose(inner, inner.saturating_sub(PAYLOAD), glyphs)
            .len()
            .saturating_sub(spare)
    }

    /// How many rows are left over for the prose, and zero where none are owed.
    ///
    /// Both callers ask the same question of the same layout, and the answer is
    /// what the two of them have to agree about: one draws the window and the
    /// other says how far it can move, so a difference here would be an arrow
    /// that stops one row from where the picture stops.
    fn spare(&self, columns: usize, room: usize, glyphs: Glyphs) -> usize {
        if self.explanation.is_empty() {
            return 0;
        }

        let bare = self.laid(columns, glyphs, Spacing::WHOLE, 0);
        let spare = room.saturating_sub(bare.len());
        if bare.is_empty() || spare < AT_LEAST {
            return 0;
        }

        spare
    }

    /// The panel at one rung of the ladder, with `spare` rows for the prose.
    fn laid(&self, columns: usize, glyphs: Glyphs, spacing: Spacing, spare: usize) -> Vec<Row> {
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
        if !self.description.is_empty() {
            let line = clip(self.description, payload);
            rows.push(framed(said(PAYLOAD, Slot::Quiet, line), inner, glyphs));
        }
        let (prose, cut) = self.told(inner, payload, glyphs, spare);
        rows.extend(prose);
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
            let keys = if cut && !self.more.is_empty() {
                format!("{} {} {}", self.footer, glyphs.dot(), self.more)
            } else {
                self.footer.to_owned()
            };
            rows.push(said(SAID, Slot::Quiet, clip(&keys, room)));
        }

        rows
    }

    /// The window over the prose a reader asked for, and whether the prose was
    /// too tall for it.
    ///
    /// Empty where nothing was asked for, and empty where `spare` is zero,
    /// which is what leaves the unexplained panel exactly the panel it was.
    ///
    /// The second half of the answer is *was it cut at all*, not *is anything
    /// below the window now*, because the footer is what reads it: at the end
    /// of the prose ↑ still does something, so the arrows are still worth
    /// naming there.
    fn told(&self, inner: usize, payload: usize, glyphs: Glyphs, spare: usize) -> (Vec<Row>, bool) {
        if spare == 0 {
            return (Vec::new(), false);
        }

        let rows = self.prose(inner, payload, glyphs);
        if rows.len() <= spare {
            return (rows, false);
        }

        // The furthest down the prose the window can open: from here the whole
        // tail fits, so there is nothing below to count and the row that
        // counted it goes back to the prose. The panel is the same height
        // either way — what changes is whether its last row is a count or a
        // sentence.
        let last = rows.len() - spare;
        let from = self.from.min(last);
        let ended = from == last;

        let keep = if ended { spare } else { spare - 1 };
        let mut window: Vec<Row> = rows.into_iter().skip(from).take(keep).collect();

        // Counted in rows rather than paragraphs, because a row is what ran out
        // and a row is what the arrows move by. It counts what is *under* the
        // window rather than what is off screen in both directions, because
        // down is the direction somebody reading a cut explanation is pressing,
        // and a number that did not fall as they pressed would be answering a
        // question they had stopped asking.
        //
        // Which is also why it never reads one: the row the count sits on is one
        // of the two the end of the prose is waiting for, so the number goes
        // from two to gone. Anything that changes `keep` has to read that
        // sentence again.
        if !ended {
            let below = last - from + 1;
            window.push(framed(
                said(
                    PAYLOAD,
                    Slot::Quiet,
                    &format!("{} {below} more rows of explanation", glyphs.dot()),
                ),
                inner,
                glyphs,
            ));
        }

        (window, true)
    }

    /// Every row the whole explanation folds to, blanks and all.
    ///
    /// A paragraph opens with a blank rather than being separated from the next
    /// one by it, so the first paragraph is parted from the command above it the
    /// same way the second is parted from the first. The attribution opens the
    /// same way and for the same reason, and it is counted here rather than
    /// pinned above the window because a row held out of the window is a row
    /// [`Self::spare`] and [`Self::end`] would each have to subtract — and the
    /// two of them disagreeing by one is the arrow that stops a row from where
    /// the picture stops.
    fn prose(&self, inner: usize, payload: usize, glyphs: Glyphs) -> Vec<Row> {
        let mut rows = Vec::new();

        if !self.attribution.is_empty() {
            rows.push(framed(Row::new(), inner, glyphs));
            for line in fold(self.attribution, payload) {
                rows.push(framed(said(PAYLOAD, Slot::Quiet, line), inner, glyphs));
            }
        }

        for paragraph in self.explanation {
            rows.push(framed(Row::new(), inner, glyphs));
            for line in fold(paragraph, payload) {
                rows.push(framed(said(PAYLOAD, Slot::Plain, line), inner, glyphs));
            }
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
    /// Every blank drawn, which is the rung the prose is measured against.
    const WHOLE: Self = Self {
        footer: true,
        statement: true,
        opening: true,
    };

    /// The rungs, in the order they are given up.
    const LADDER: [Self; 4] = [
        Self::WHOLE,
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
