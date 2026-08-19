//! The panel a model's questions are answered in: what is being asked, the
//! answers offered, and which of them a key is about to take.
//!
//! Drawn in the frame [`crate::Prompt`] uses and standing where that box was,
//! for the reason [`crate::Question`] is: the reader has to notice that the
//! keyboard now means something else, and a rule reads as one more thing in the
//! transcript. The two panels are siblings and not the same component — that
//! one reports a command and collects consent, this one asks a question and
//! collects a decision, and almost every row differs.
//!
//! **Rows, not a screen.** Its words are the caller's and it never asks how tall
//! the terminal is. The bargain [`crate::Panel`] and [`crate::Ladder`] are
//! already on.
//!
//! **The subject says whose the questions are to answer.** Not whose they were
//! to write: the row above this panel is the call that asked, so it already
//! names the model, and a second attribution would be the panel repeating the
//! transcript. What the reader cannot get from anywhere else is that these are
//! theirs to act on.
//!
//! **Two axes, and each pair of arrows is named under the thing it moves.** The
//! questions run across the top and the answers down the middle, so a reader
//! walks one with ↑↓ and steps the other with ←→. This component draws both and
//! reads neither; which key does what is the caller's, and the footer it hands
//! over is where any of that is said.
//!
//! **The questions row is the one thing that gives way to width rather than to
//! height.** Every other row folds or clips; that one is a row of names that
//! stops meaning anything once it is cut, so where it will not fit whole it is
//! replaced by a count — *question 2 of 3 · 1 answered* — which says the same
//! two facts in a fifth of the width. Height is given up separately, by
//! [`Spacing`], and in the order that costs the least sense.
//!
//! **Where the colour goes.** The frame and every caret are [`Slot::Accent`];
//! the subject, the question on screen and the marked answer are
//! [`Slot::Strong`]; the question and the answers the mark has not reached are
//! the reader's own foreground; descriptions, the questions not on screen and
//! the footer are [`Slot::Quiet`]. The one green is [`Slot::DoneMark`], on a
//! question already answered and on an answer already chosen, because in both
//! places it says the one thing: this one is settled.
//!
//! **Every word here is somebody else's.** The questions and answers are the
//! model's, and a model reads files other people wrote — so a terminal
//! instruction carried through in one of them would move the cursor out of the
//! live region this process is tracking, or leave an attribute set for every row
//! after it. Nothing a caller handed over reaches a row without going through
//! [`spoken`] first, and the one place that is easy to get wrong is a string
//! that is folded rather than clipped: it is made safe before it is folded, so
//! the widths are measured on what will actually be drawn.
//!
//! Nothing paints a ground. The question in view is said by a mark and a weight
//! rather than by a highlight, which is what keeps the ground behind every row
//! the reader's own.

use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::render::Caret;
use crate::row::Row;
use crate::width::{clip, columns as wide, fold, spoken};

/// What a row spends on the frame: one column of edge on each side.
const AROUND: usize = 2;

/// Columns before the subject, a sentence, and an answer's mark.
const SAID: usize = 2;

/// Columns before a block: the note, and the answer read back under the
/// question it answers.
const PAYLOAD: usize = 4;

/// Blank columns between one stop on the questions row and the next.
const GAP: usize = 3;

/// What a chosen mark costs an answer: the brackets, the mark, and the space
/// after it.
const CHOSEN: usize = 4;

/// The most rows of a specimen the block will draw, the row that counts what
/// was left out included.
///
/// A bound rather than a preference. Every tool in this program bounds what it
/// hands back and says when it cut it, and a block that could be any height
/// would be the one thing on screen deciding how tall this panel is — decided
/// by whatever wrote the call rather than by the panel drawing it.
const MOST: usize = 10;

/// What opens the reader's own line about a question.
const NOTED: &str = "Note: ";

/// What the box says where an answer has nothing to show.
///
/// The box is drawn anyway. One that vanished would move every row under it as
/// the mark walked past, which is the same defect as the panel changing height.
const NOTHING: &str = "nothing to show for this one";

/// One question, as the row across the top draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stop<'a> {
    /// The few words it is called there.
    pub name: &'a str,
    /// Whether it has been answered.
    pub done: bool,
    /// Whether it is a question at all.
    ///
    /// False for the stop that sends: it carries no mark, because a box beside
    /// it would be asking whether the sending had been answered. It is left out
    /// of the count as well, so *question 2 of 3* counts questions and not
    /// places the mark can stand.
    pub asks: bool,
}

/// One answer, as the list draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Choice<'a> {
    /// What the answer is called.
    pub answer: &'a str,
    /// The quiet row under it saying what it means, or empty.
    pub says: &'a str,
    /// Whether it is chosen, on a question taking several answers. `None` is a
    /// question taking one, where the mark alone says which — a box drawn
    /// beside a single answer would offer a choice that is not being made.
    ///
    /// Drawn bracketed, and the row of questions above it is not. The two say
    /// different things — *this one is chosen* against *this one is answered* —
    /// and on a question taking several they are on screen at once, one under
    /// the other. Marks that looked alike there would be one column of state
    /// read as another.
    pub chosen: Option<bool>,
    /// The rows of what this answer would look like, or empty.
    pub shows: &'a [&'a str],
}

/// A line somebody is writing, and where their cursor sits in it.
///
/// The column is a display column rather than a count of characters, because a
/// cursor placed by counting characters lands inside a glyph two columns wide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Writing<'a> {
    /// What has been typed so far.
    pub text: &'a str,
    /// How many columns from the start of the line the cursor is.
    pub column: usize,
    /// What the row says while nothing has been typed. Drawn quiet, because it
    /// is not what the answer is called — it is what to do with the row.
    pub placeholder: &'a str,
}

/// One question read back, with what was answered to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Given<'a> {
    /// The question, in the words it was asked in.
    pub question: &'a str,
    /// Every answer given to it, already joined by whoever knows how many.
    pub answer: &'a str,
}

/// One question standing, waiting to be answered.
///
/// Every string is the caller's. This crate depends on nothing and cannot know
/// what a question is about, which is what keeps the wording in the one place
/// that has read the call.
#[derive(Debug, Clone, Copy)]
pub struct Asked<'a> {
    /// The few words at the top: whose the questions are to answer.
    pub subject: &'a str,
    /// Every question, in the order they are asked, and then the stop that
    /// sends. Fewer than two draws no row at all — one question has no row to
    /// step along.
    pub stops: &'a [Stop<'a>],
    /// Which stop is on screen.
    pub at: usize,
    /// The sentence above the block, or empty. What the stop that sends uses to
    /// say what is below it.
    pub statement: &'a str,
    /// The answers read back, on the stop that sends. Empty everywhere else.
    pub given: &'a [Given<'a>],
    /// The question the answers answer.
    pub question: &'a str,
    /// The answers, in the order they are numbered.
    pub answers: &'a [Choice<'a>],
    /// Which answer the mark stands on.
    pub marked: usize,
    /// The reader's own line about this question, or empty.
    pub note: &'a str,
    /// The line being written, or `None` where nothing is.
    ///
    /// Which row it belongs to is [`Asked::at_note`]'s answer: the note, or the
    /// answer the mark is on.
    pub writing: Option<Writing<'a>>,
    /// Whether what is being written is the note rather than an answer.
    pub at_note: bool,
    /// The answer under the rule, which answers the whole ask rather than this
    /// question. Empty draws neither it nor the rule.
    pub leaves: &'a str,
    /// The quiet row under the frame, naming the keys that are not on it.
    pub footer: &'a str,
}

impl Asked<'_> {
    /// The panel in `room` rows, giving up spacing before it gives up sense,
    /// and empty where not even the floor fits.
    ///
    /// Empty is the right answer rather than a degraded one, for the reason
    /// [`crate::Question::within`] gives: a panel drawn at as-much-as-fits reads
    /// as the whole panel, and the caller with no room owes the reader the
    /// questions some other way.
    ///
    /// The caret is where a line is being written, and `None` everywhere else —
    /// which is every panel this task draws.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> (Vec<Row>, Option<Caret>) {
        for spacing in Spacing::LADDER {
            let (rows, caret) = self.laid(columns, glyphs, spacing);
            if !rows.is_empty() && rows.len() <= room {
                // A caret on a row that was given up would park the cursor
                // where the frame does not go, and the next frame would rewind
                // over the wrong rows.
                return (rows, caret.filter(|caret| caret.row < room));
            }
        }

        (Vec::new(), None)
    }

    /// The panel at one rung of the ladder.
    fn laid(&self, columns: usize, glyphs: Glyphs, spacing: Spacing) -> (Vec<Row>, Option<Caret>) {
        let inner = columns.saturating_sub(AROUND);
        let across = inner.saturating_sub(SAID);

        // Nothing to answer with, or no room for the column the answers open
        // in. Either way there is no panel to draw rather than a short one:
        // every other row here folds or clips, and the gutter in front of an
        // answer cannot do either — it is the mark, the number and the box, and
        // an answer drawn without them is not an answer anybody can take.
        if self.answers.is_empty() || inner <= self.gutter() {
            return (Vec::new(), None);
        }
        if !self.given.is_empty() && inner <= PAYLOAD + AROUND {
            return (Vec::new(), None);
        }

        let mut caret = None;

        let laid = Laid {
            inner,
            glyphs,
            spacing,
        };
        let (open, opened) = glyphs.top();
        let (close, closed) = glyphs.bottom();
        let bar = glyphs.horizontal().repeat(inner);

        let subject = spoken(self.subject);
        let mut rows = vec![Row::new().then(Slot::Accent, format!("{open}{bar}{opened}"))];
        rows.push(framed(
            said(SAID, Slot::Strong, clip(&subject, across)),
            inner,
            glyphs,
        ));

        if let Some(stepped) = self.stepped(across, glyphs) {
            if spacing.opening {
                rows.push(framed(Row::new(), inner, glyphs));
            }
            rows.push(framed(stepped, inner, glyphs));
        }

        rows.extend(self.told(across, laid));

        if spacing.opening {
            rows.push(framed(Row::new(), inner, glyphs));
        }
        let question = spoken(self.question);
        for line in fold(&question, across) {
            rows.push(framed(said(SAID, Slot::Plain, line), inner, glyphs));
        }
        if spacing.opening {
            rows.push(framed(Row::new(), inner, glyphs));
        }

        for (at, answer) in self.answers.iter().enumerate() {
            if at == self.marked && !self.at_note && self.writing.is_some() {
                caret = Some(Caret {
                    row: rows.len(),
                    column: 1 + self.written(at),
                });
            }
            rows.extend(self.answered(at, answer, laid));
        }

        if let Some(note) = self.noted(laid) {
            if self.at_note && self.writing.is_some() {
                caret = Some(Caret {
                    row: rows.len(),
                    column: 1 + PAYLOAD + wide(NOTED) + self.column(),
                });
            }
            rows.push(note);
        }

        rows.extend(self.shown(laid));

        if !self.leaves.is_empty() {
            if spacing.opening {
                rows.push(framed(Row::new(), inner, glyphs));
            }
            let ruled = glyphs.horizontal().repeat(across.saturating_sub(SAID));
            rows.push(framed(said(SAID, Slot::Quiet, &ruled), inner, glyphs));
            rows.push(framed(self.left(across), inner, glyphs));
        }

        rows.push(Row::new().then(Slot::Accent, format!("{close}{bar}{closed}")));

        if spacing.footer && !self.footer.is_empty() {
            let room = columns.saturating_sub(SAID);
            let footer = spoken(self.footer);
            rows.push(said(SAID, Slot::Quiet, clip(&footer, room)));
        }

        (rows, caret)
    }

    /// Where the cursor sits on the answer at `at`, counted from the first
    /// column inside the frame.
    fn written(&self, at: usize) -> usize {
        SAID + 1 + wide(&format!(" {}. ", at + 1)) + self.column()
    }

    /// How far into the line being written the cursor is, and zero where
    /// nothing is being written.
    fn column(&self) -> usize {
        self.writing.map_or(0, |writing| writing.column)
    }

    /// The reader's own line about this question, or nothing where there is
    /// none and none is being written.
    ///
    /// Drawn only once there is something to draw. The key that opens one is
    /// named in the footer instead, so the offer costs no row.
    fn noted(&self, laid: Laid) -> Option<Row> {
        let writing = self.writing.filter(|_| self.at_note);
        let text = writing.map_or(self.note, |writing| writing.text);
        if text.is_empty() && writing.is_none() {
            return None;
        }

        let room = laid
            .inner
            .saturating_sub(PAYLOAD + wide(NOTED))
            .saturating_sub(SAID);
        let row = Row::new()
            .then(Slot::Plain, " ".repeat(PAYLOAD))
            .then(Slot::Quiet, NOTED)
            .then(Slot::Plain, clip(&spoken(text), room));

        Some(framed(row, laid.inner, laid.glyphs))
    }

    /// The one box every answer in this question is drawn in, and `None` where
    /// none of them shows anything.
    ///
    /// Read off every answer rather than off the marked one, which is the whole
    /// of what keeps the panel one height as the mark walks down them.
    fn boxed(&self, room: usize) -> Option<(usize, usize)> {
        if self.answers.iter().all(|answer| answer.shows.is_empty()) {
            return None;
        }

        let wide = self
            .answers
            .iter()
            .flat_map(|answer| {
                if answer.shows.is_empty() {
                    vec![NOTHING]
                } else {
                    answer.shows.to_vec()
                }
            })
            .map(|line| wide(clip(line, room)))
            .max()
            .unwrap_or_default();

        let tall = self
            .answers
            .iter()
            .map(|answer| answer.shows.len().clamp(1, MOST))
            .max()
            .unwrap_or(1);

        Some((wide, tall))
    }

    /// The specimen of the marked answer, in the box the whole question shares.
    fn shown(&self, laid: Laid) -> Vec<Row> {
        let Laid {
            inner,
            glyphs,
            spacing,
        } = laid;

        // Two columns for the box's own edges, and one more for the space that
        // parts a specimen from the left one.
        let Some(room) = inner.checked_sub(PAYLOAD + 4) else {
            return Vec::new();
        };
        let Some((wide, tall)) = self.boxed(room) else {
            return Vec::new();
        };

        let bar = glyphs.horizontal();
        let mut rows = Vec::new();
        if spacing.opening {
            rows.push(framed(Row::new(), inner, glyphs));
        }
        rows.push(framed(
            said(PAYLOAD, Slot::Quiet, &format!("┌{}┐", bar.repeat(wide + 2))),
            inner,
            glyphs,
        ));

        for at in 0..tall {
            let (slot, line) = self.showing(at, room, glyphs);
            let mut row = Row::new()
                .then(Slot::Plain, " ".repeat(PAYLOAD))
                .then(Slot::Quiet, glyphs.vertical())
                .then(Slot::Plain, " ")
                .then(slot, line);
            // A column of air on each side, so the widest specimen in the
            // question does not read as one the box grew too tight for.
            row.pad(PAYLOAD + 3 + wide);
            rows.push(framed(
                row.then(Slot::Quiet, glyphs.vertical()),
                inner,
                glyphs,
            ));
        }

        rows.push(framed(
            said(PAYLOAD, Slot::Quiet, &format!("└{}┘", bar.repeat(wide + 2))),
            inner,
            glyphs,
        ));

        rows
    }

    /// What row `at` of the box says for the answer the mark is on.
    ///
    /// A specimen is clipped rather than folded: a folded specimen is a picture
    /// of something else, where a cut one is at least a picture of the first
    /// columns of the right thing.
    fn showing(&self, at: usize, room: usize, glyphs: Glyphs) -> (Slot, String) {
        let Some(answer) = self.answers.get(self.marked) else {
            return (Slot::Plain, String::new());
        };

        if answer.shows.is_empty() {
            return if at == 0 {
                // Clipped, because `boxed` measured this line clipped when it
                // decided how wide the box is. Drawn whole it would reach past
                // the edge that measurement drew, on the one window narrow
                // enough for the sentence not to fit.
                (Slot::Quiet, clip(NOTHING, room).to_owned())
            } else {
                (Slot::Plain, String::new())
            };
        }

        // The last row the bound allows is spent saying how much was left out,
        // because a block cut with nothing said reads as the whole specimen.
        if answer.shows.len() > MOST && at == MOST - 1 {
            let left = answer.shows.len() - (MOST - 1);
            let counted = format!("{} {left} more rows", glyphs.dot());
            return (Slot::Quiet, clip(&counted, room).to_owned());
        }

        answer.shows.get(at).map_or_else(
            || (Slot::Plain, String::new()),
            |line| (Slot::Plain, clip(&spoken(line), room).to_owned()),
        )
    }

    /// The widest gutter an answer opens after: the caret, the number, and the
    /// box where the question takes several answers.
    ///
    /// Read off the answers rather than assumed, because a list long enough to
    /// reach two figures spends a column more on every one of them.
    fn gutter(&self) -> usize {
        let widest = self
            .answers
            .iter()
            .enumerate()
            .map(|(at, answer)| {
                SAID + 1
                    + wide(&format!(" {}. ", at + 1))
                    + usize::from(answer.chosen.is_some()) * CHOSEN
            })
            .max()
            .unwrap_or_default();

        let left = if self.leaves.is_empty() {
            0
        } else {
            SAID + 1 + wide(&format!(" {}. ", self.answers.len() + 1))
        };

        widest.max(left)
    }

    /// The row across the top, or nothing where there is only one question.
    ///
    /// The names where they fit whole, and a count where they do not. A row of
    /// names cut at the right edge says less than the two facts a count says
    /// whole, and it is the only row here that gives way to width rather than
    /// to height.
    fn stepped(&self, across: usize, glyphs: Glyphs) -> Option<Row> {
        if self.stops.len() < 2 {
            return None;
        }

        let named = self.named(glyphs);
        if named.columns() <= across {
            return Some(named);
        }

        let asking = self.stops.iter().filter(|stop| stop.asks).count();
        let answered = self
            .stops
            .iter()
            .filter(|stop| stop.asks && stop.done)
            .count();
        let counted = format!(
            "question {} of {asking} {} {answered} answered",
            self.at + 1,
            glyphs.dot()
        );

        Some(said(SAID, Slot::Quiet, clip(&counted, across)))
    }

    /// Every stop, named, with the one on screen marked.
    ///
    /// Two columns are kept in front of every stop — the caret and the space
    /// after it — whether or not the caret is on that one, so the names stand
    /// still as it steps along them.
    fn named(&self, glyphs: Glyphs) -> Row {
        let mut row = Row::new().then(Slot::Plain, " ".repeat(SAID));

        for (at, stop) in self.stops.iter().enumerate() {
            if at > 0 {
                row.push(Slot::Plain, " ".repeat(GAP));
            }

            let here = at == self.at;
            row.push(Slot::Accent, if here { glyphs.caret() } else { " " });
            row.push(Slot::Plain, " ");

            if stop.asks {
                let (slot, mark) = if stop.done {
                    (Slot::DoneMark, glyphs.done())
                } else {
                    (Slot::Quiet, glyphs.open())
                };
                row.push(slot, mark);
                row.push(Slot::Plain, " ");
            }

            row.push(
                if here { Slot::Strong } else { Slot::Quiet },
                spoken(stop.name),
            );
        }

        row
    }

    /// The sentence above the block, and the answers read back under it.
    ///
    /// Both are the stop that sends, and both are empty everywhere else.
    fn told(&self, across: usize, laid: Laid) -> Vec<Row> {
        let Laid {
            inner,
            glyphs,
            spacing,
        } = laid;
        let mut rows = Vec::new();

        if !self.statement.is_empty() {
            if spacing.opening {
                rows.push(framed(Row::new(), inner, glyphs));
            }
            let statement = spoken(self.statement);
            for line in fold(&statement, across) {
                rows.push(framed(said(SAID, Slot::Plain, line), inner, glyphs));
            }
        }

        if !self.given.is_empty() && spacing.opening {
            rows.push(framed(Row::new(), inner, glyphs));
        }
        for given in self.given {
            let mark = Row::new()
                .then(Slot::Plain, " ".repeat(SAID))
                .then(Slot::DoneMark, glyphs.done())
                .then(Slot::Plain, " ")
                .then(
                    Slot::Quiet,
                    clip(&spoken(given.question), across.saturating_sub(AROUND)),
                );
            rows.push(framed(mark, inner, glyphs));

            let under = inner.saturating_sub(PAYLOAD + AROUND);
            let answer = spoken(given.answer);
            for line in fold(&answer, under) {
                rows.push(framed(
                    said(PAYLOAD + AROUND, Slot::Strong, line),
                    inner,
                    glyphs,
                ));
            }
        }

        rows
    }

    /// One answer's rows: the caret, the number, the box where there is one,
    /// the words, and the quiet row under them.
    ///
    /// A continuation row opens under the answer's own first column rather than
    /// under its number, so a folded answer reads as one answer and the column
    /// of numbers stays a column.
    fn answered(&self, at: usize, answer: &Choice<'_>, laid: Laid) -> Vec<Row> {
        let Laid {
            inner,
            glyphs,
            spacing,
        } = laid;
        let marked = at == self.marked;
        let number = format!(" {}. ", at + 1);
        let boxed = answer.chosen.is_some();
        let front = SAID + 1 + wide(&number) + usize::from(boxed) * CHOSEN;
        let slot = if marked { Slot::Strong } else { Slot::Plain };

        if marked
            && !self.at_note
            && let Some(writing) = self.writing
        {
            let room = inner.saturating_sub(front);
            let (slot, text) = if writing.text.is_empty() {
                (Slot::Quiet, writing.placeholder)
            } else {
                (Slot::Plain, writing.text)
            };

            let row = Row::new()
                .then(Slot::Plain, " ".repeat(SAID))
                .then(Slot::Accent, glyphs.caret())
                .then(Slot::Strong, &number)
                .then(slot, clip(&spoken(text), room));

            return vec![framed(row, inner, glyphs)];
        }

        let name = spoken(answer.answer);
        let mut rows: Vec<Row> = fold(&name, inner.saturating_sub(front))
            .into_iter()
            .enumerate()
            .map(|(row, line)| {
                let opening = if row == 0 {
                    let mut open = Row::new()
                        .then(Slot::Plain, " ".repeat(SAID))
                        .then(Slot::Accent, if marked { glyphs.caret() } else { " " })
                        .then(slot, &number);

                    if let Some(chosen) = answer.chosen {
                        open.push(Slot::Quiet, "[");
                        if chosen {
                            open.push(Slot::DoneMark, glyphs.done());
                        } else {
                            open.push(Slot::Plain, " ");
                        }
                        open.push(Slot::Quiet, "]");
                        open.push(Slot::Plain, " ");
                    }

                    open.then(slot, line)
                } else {
                    Row::new()
                        .then(Slot::Plain, " ".repeat(front))
                        .then(slot, line)
                };

                framed(opening, inner, glyphs)
            })
            .collect();

        if spacing.says && !answer.says.is_empty() {
            let says = spoken(answer.says);
            let line = clip(&says, inner.saturating_sub(front));
            rows.push(framed(said(front, Slot::Quiet, line), inner, glyphs));
        }

        rows
    }

    /// The answer under the rule, numbered after the question's own.
    fn left(&self, across: usize) -> Row {
        let number = format!(" {}. ", self.answers.len() + 1);
        let front = SAID + 1 + wide(&number);

        Row::new()
            .then(Slot::Plain, " ".repeat(SAID))
            .then(Slot::Plain, " ")
            .then(Slot::Plain, &number)
            .then(
                Slot::Plain,
                clip(&spoken(self.leaves), across.saturating_sub(front - SAID)),
            )
    }
}

/// How a row is being drawn: the width inside the frame, the characters, and
/// which blanks this rung still has.
///
/// One value rather than three arguments, because they never travel apart and a
/// call site passing them in a row says nothing about which is which.
#[derive(Debug, Clone, Copy)]
struct Laid {
    inner: usize,
    glyphs: Glyphs,
    spacing: Spacing,
}

/// Which blanks a rung of the ladder still draws.
///
/// The order is the argument: the footer names keys documented elsewhere, a
/// description says what the answer above it already implies, and the blanks
/// that make the panel readable are the last to go.
#[derive(Debug, Clone, Copy)]
struct Spacing {
    /// The quiet row of keys under the frame.
    footer: bool,
    /// The quiet row under each answer.
    says: bool,
    /// The blanks that part one block from the next.
    opening: bool,
}

impl Spacing {
    /// The rungs, in the order they are given up.
    const LADDER: [Self; 4] = [
        Self {
            footer: true,
            says: true,
            opening: true,
        },
        Self {
            footer: false,
            says: true,
            opening: true,
        },
        Self {
            footer: false,
            says: false,
            opening: true,
        },
        Self {
            footer: false,
            says: false,
            opening: false,
        },
    ];
}

/// One column of edge running down.
fn edge(glyphs: Glyphs) -> Row {
    Row::new().then(Slot::Accent, glyphs.vertical())
}

/// `row`, out to the full inner width, so the edge on its right lands where
/// every other row put one.
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
