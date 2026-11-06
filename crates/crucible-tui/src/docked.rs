//! The dock questions wait in: one row above the box while it is closed, and
//! the questions themselves, in the frame [`crate::Asked`] is drawn in, once
//! it is open — while the turn that asked them keeps going.
//!
//! **A sibling of the blocking panel, not a second copy of it.** That panel
//! stands where the box was and takes the keyboard until it is answered; this
//! one stands above the box, which stays where it is, and the reader moves
//! between the two. So it shares that panel's frame, slots, glyphs, spacing
//! ladder and specimen box, and none of its orchestration: it holds no channel,
//! reads no key and decides nothing. [`Dock`] is the whole of what it draws and
//! the caller is the only one that changes it, and [`Phase::actions`] is the
//! one list both this and the caller read the marked action from.
//!
//! **One column at every width.** The specimen of the marked answer stands in
//! its box under the answers and never beside them, and every row the dock
//! draws is one row of that column: the panel is read top to bottom while the
//! box under it is still where the reader left it.
//!
//! **Text, not domain values.** This crate depends on no other in the
//! workspace, so a batch reaches it as the words the caller read off it and a
//! key the caller compares it by. Which answer a draft holds is a number into
//! the answers this was handed, for the same reason: the words of an answer
//! are the model's, and two answers may well say the same thing.
//!
//! **Every state says what it is in words.** A mark beside each one, and a
//! colour behind some, but never only those: a reader on a theme whose colours
//! they cannot tell apart, or on a terminal with none, reads the same states
//! as everyone else.
//!
//! **The model's words never speak in the dock's voice.** Headings, questions
//! and answers are the model's, and a model reads files other people wrote, so
//! one of them may say *Submit batch* as easily as anything else. What the dock
//! says for itself — its count, its states, its actions — is drawn in
//! [`Slot::Strong`], and nothing the caller handed over is: a marked answer is
//! raised in [`Slot::Bold`], weight without the hue. And every one of those
//! strings goes through [`spoken`] before it is measured or drawn, for the
//! reason the blocking panel gives.
//!
//! **Height gives way, and the words never do.** Nothing the dock shows is cut
//! to fit: every answer, description, written answer and note is folded in
//! full, because what the reader wrote is what is sent, and a draft read back
//! in part would be read as the whole. The rungs of [`Spacing::LADDER`] that
//! keep every description go first, giving up the row of keys and then the
//! air between blocks. Where the open question or the review is still taller
//! than the window, the dock becomes a window onto it, as the transcript's
//! [`crate::Expanded`] does: the row saying what state it is in and the actions
//! stay where they are, the rest scrolls, and the row under the frame says
//! `pgup pgdn to see more`, which it says nowhere else. [`Dock::scrolled`] is
//! how far down the window opens, and [`Docked::end`] how far it may go at this
//! size, which the caller clamps its offset to. Only where the window cannot
//! hold even the frame, one row and the actions is the dock the closed row.
//! Width gives way the way the blocking panel's does: the row of question
//! names becomes *question 2 of 3 · 1 answered* where it will not fit whole,
//! and every sentence folds.

use std::collections::BTreeSet;
use std::fmt;

use crate::asked::{AROUND, CHOSEN, GAP, NOTED, PAYLOAD, SAID, Spacing};
use crate::asked::{framed, last, said, slid, under};
use crate::color::Slot;
use crate::glyphs::Glyphs;
use crate::render::Caret;
use crate::row::Row;
use crate::width::{clip, columns as wide, fold, spoken};

/// Every batch of questions waiting, which one is in front, where the reader
/// is in it, and what the dock is doing.
///
/// `K` is the caller's name for a batch: compared, carried, never read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dock<K> {
    /// The batches, oldest first.
    ///
    /// Every question listed here is counted as waiting, so a batch belongs
    /// here only while it is still to be settled.
    pub batches: Vec<Batched<K>>,
    /// Which batch is in front: see [`Dock::front`].
    pub current: usize,
    /// Where the reader is in it.
    pub focus: Focus,
    /// How many rows down the open question or review the window opens, where
    /// it is taller than the room: see [`Docked::end`].
    pub scrolled: usize,
    /// What the dock is doing.
    pub phase: Phase,
}

impl<K> Dock<K> {
    /// The batch in front, and where it stands: the one [`Dock::current`]
    /// names, or the last where it names none, so a batch taken away under it
    /// leaves the dock showing what is still waiting rather than nothing.
    #[must_use]
    pub fn front(&self) -> Option<(usize, &Batched<K>)> {
        let at = self.current.min(self.batches.len().checked_sub(1)?);
        self.batches.get(at).map(|batch| (at, batch))
    }

    /// The question open in the batch in front, and where it stands: the one
    /// [`Focus::member`] names, or the last where it names none.
    ///
    /// What the dock draws and what the caller changes are both read through
    /// this, so the question on screen is the one a key goes to.
    #[must_use]
    pub fn open(&self) -> Option<(usize, &Member)> {
        let (_, batch) = self.front()?;
        let at = self.focus.member.min(batch.members.len().checked_sub(1)?);
        batch.members.get(at).map(|member| (at, member))
    }

    /// The action the mark stands on: the one [`Focus::marked`] counts to among
    /// [`Phase::actions`], or the last where it counts past them, and `None`
    /// where the phase offers none.
    #[must_use]
    pub fn marked_action(&self) -> Option<Action> {
        let actions = self.phase.actions();
        let at = self.focus.marked.min(actions.len().checked_sub(1)?);
        actions.get(at).copied()
    }

    /// How many questions wait, in every batch.
    fn waiting(&self) -> usize {
        self.batches.iter().map(|batch| batch.members.len()).sum()
    }
}

/// The questions of one call that went on without waiting for its answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Batched<K> {
    /// The caller's name for the batch.
    pub key: K,
    /// Its questions, in the order they were asked.
    pub members: Vec<Member>,
}

/// One question of a batch, and the reader's draft of its answer.
///
/// Its `Debug` counts what it holds and prints none of it: every string here
/// is somebody's own words, which the domain they came from redacts too.
#[derive(Clone, PartialEq, Eq)]
pub struct Member {
    /// The few words it is called by.
    pub heading: String,
    /// The question, in the words it was asked in.
    pub question: String,
    /// The answers it offers, in the order they are numbered.
    pub answers: Vec<Offer>,
    /// What the reader has made of it so far, which also says whether it
    /// takes several answers.
    pub draft: Drafted,
}

impl fmt::Debug for Member {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Member")
            .field("answers", &self.answers)
            .field("draft", &self.draft)
            .finish_non_exhaustive()
    }
}

/// One answer a question offers.
#[derive(Clone, PartialEq, Eq)]
pub struct Offer {
    /// What the answer is called.
    pub answer: String,
    /// The quiet rows under it saying what it means, or empty.
    pub says: String,
    /// The rows of what this answer would look like, or none.
    pub shows: Vec<String>,
    /// Whether whoever asked recommends it.
    ///
    /// Drawn as a word beside the answer and never as a choice: a
    /// recommendation the reader has not taken is not an answer.
    pub recommended: bool,
}

impl fmt::Debug for Offer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Offer")
            .field("shows", &self.shows.len())
            .field("recommended", &self.recommended)
            .finish_non_exhaustive()
    }
}

/// The reader's draft of one answer.
#[derive(Clone, PartialEq, Eq)]
pub struct Drafted {
    /// The offered answers taken.
    pub chosen: Chosen,
    /// An answer of the reader's own, or empty.
    pub written: String,
    /// The reader's own line about the question, or empty.
    pub note: String,
    /// Whether the reader declines to answer it.
    pub declined: bool,
}

impl Drafted {
    /// A draft with nothing in it yet, of a question taking what `chosen`
    /// says it takes.
    #[must_use]
    pub fn new(chosen: Chosen) -> Self {
        Self {
            chosen,
            written: String::new(),
            note: String::new(),
            declined: false,
        }
    }
}

impl fmt::Debug for Drafted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Drafted")
            .field("chosen", &self.chosen)
            .field("written", &!self.written.is_empty())
            .field("note", &!self.note.is_empty())
            .field("declined", &self.declined)
            .finish()
    }
}

/// Which offered answers a draft takes, as places among them, and so whether
/// its question takes one or several.
///
/// The two cannot disagree: a question taking one answer holds one place or
/// none, and one taking several holds each place once, in the order they were
/// offered. A place no answer stands at is held but never drawn or counted,
/// as [`Member::settles`] says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Chosen {
    /// A question taking one answer, and the one taken, if any.
    One(Option<usize>),
    /// A question taking several, and every one taken.
    Several(BTreeSet<usize>),
}

impl Chosen {
    /// Whether the question takes several answers rather than one.
    #[must_use]
    pub fn several(&self) -> bool {
        matches!(self, Self::Several(_))
    }

    /// Every place taken, in the order the answers were offered.
    pub fn taken(&self) -> impl Iterator<Item = usize> + '_ {
        let (one, several) = match self {
            Self::One(one) => (*one, None),
            Self::Several(several) => (None, Some(several.iter().copied())),
        };
        one.into_iter().chain(several.into_iter().flatten())
    }

    /// Whether the place `at` is taken.
    #[must_use]
    pub fn takes(&self, at: usize) -> bool {
        match self {
            Self::One(one) => *one == Some(at),
            Self::Several(several) => several.contains(&at),
        }
    }
}

/// Where the reader is in the batch in front.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Focus {
    /// Which question is open.
    pub member: usize,
    /// Which row of the open list the mark stands on: an answer, the answer of
    /// the reader's own after them, and `Decline question` after that while a
    /// question is open; an action everywhere else.
    pub marked: usize,
    /// The line being written, or `None` where nothing is.
    pub editing: Option<Editing>,
}

/// A line being written in the dock, and how many columns into it the cursor
/// stands.
///
/// A display column rather than a count of characters, for the reason
/// [`crate::Writing`] gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editing {
    /// The answer of the reader's own.
    Written {
        /// Columns from the start of the line.
        column: usize,
    },
    /// The reader's note about the question.
    Note {
        /// Columns from the start of the line.
        column: usize,
    },
}

/// What the dock is doing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Closed: one row above the box.
    Collapsed,
    /// Open on a question.
    Answering,
    /// Open on every answer, read back before they go.
    Reviewing,
    /// The answers are on their way and cannot be sent again.
    Submitting,
    /// The answers were refused, for this reason, and the draft is kept.
    Refused(String),
    /// The answers were taken, and wait for the model's next step.
    Accepted,
}

/// One of the dock's own actions, offered under the answers once they are read
/// back.
///
/// The list a phase offers is [`Phase::actions`], read both by what draws the
/// dock and by whatever carries out the one the mark is on, so the two can
/// never disagree about which row is which.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Send every answer in the batch.
    Submit,
    /// Send them again after a refusal.
    Retry,
    /// Go back to the questions with the draft as it is.
    KeepEditing,
    /// Close the dock, keeping the draft.
    Close,
}

impl Action {
    /// What the row offering it says.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Submit => "Submit batch",
            Self::Retry => "Retry submission",
            Self::KeepEditing => "Keep editing",
            Self::Close => "Close",
        }
    }
}

impl Phase {
    /// The actions offered in this phase, in the order they are drawn, which
    /// is the order [`Focus::marked`] counts them in.
    ///
    /// None while the dock is closed or open on a question, where the marked
    /// row is an answer instead. Nothing offers to send the answers while they
    /// are on their way or after they were taken.
    #[must_use]
    pub fn actions(&self) -> &'static [Action] {
        match self {
            Self::Collapsed | Self::Answering => &[],
            Self::Reviewing => &[Action::Submit, Action::KeepEditing, Action::Close],
            Self::Refused(_) => &[Action::Retry, Action::KeepEditing, Action::Close],
            Self::Submitting | Self::Accepted => &[Action::Close],
        }
    }
}

impl Member {
    /// Whether the draft gives an answer at all: an offered one this question
    /// has, one of the reader's own, or a decline.
    #[must_use]
    pub fn settles(&self) -> bool {
        let draft = &self.draft;
        draft.declined
            || !draft.written.is_empty()
            || draft.chosen.taken().any(|at| at < self.answers.len())
    }

    /// Where the answer of the reader's own stands among the rows
    /// [`Focus::marked`] counts: after every offered answer.
    #[must_use]
    pub fn writes_at(&self) -> usize {
        self.answers.len()
    }

    /// Where `Decline question` stands among them: the last row.
    #[must_use]
    pub fn declines_at(&self) -> usize {
        self.answers.len() + 1
    }
}

/// What the row offering the reader's own answer says until there is one.
const OWN: &str = "Write your own answer";

/// What the last row of an open question offers.
const DECLINE: &str = "Decline question";

/// Where the dock is opened from, named on the closed row.
const HINT: &str = "/questions";

/// The row under the frame while the open dock is a window onto more than it
/// shows, naming the keys that move it.
const MORE: &str = "pgup pgdn to see more";

/// The fewest columns a detail on the closed row is kept for. Narrower than
/// this and the row drops it, rather than showing a word and a half of it.
const DETAIL: usize = 8;

/// The dock, drawn.
#[derive(Debug)]
pub struct Docked<'a, K> {
    /// What it draws.
    pub dock: &'a Dock<K>,
    /// The quiet row under the frame, naming the keys, or empty.
    pub footer: &'a str,
}

impl<K> Clone for Docked<'_, K> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K> Copy for Docked<'_, K> {}

impl<K> Docked<'_, K> {
    /// The dock in `room` rows: the open dock where it fits, a window of it
    /// where it is taller than the room, the closed row where not even the
    /// window fits, and nothing where nothing waits or no row is left.
    ///
    /// The batch in front is [`Dock::front`] and the question open in it
    /// [`Dock::open`]; the window opens [`Dock::scrolled`] rows down, or at
    /// [`Docked::end`] where that is further. The caret is where a line is
    /// being written, and `None` everywhere else, including where that line is
    /// outside the window.
    #[must_use]
    pub fn within(&self, columns: usize, room: usize, glyphs: Glyphs) -> (Vec<Row>, Option<Caret>) {
        match self.drawn(columns, room, glyphs) {
            Drawn::Nothing => (Vec::new(), None),
            Drawn::Closed => (vec![self.closed(columns, glyphs)], None),
            Drawn::Whole(opened) => {
                let caret = opened.caret.map(|caret| Caret {
                    row: opened.head.len() + caret.row,
                    ..caret
                });
                (opened.whole(), caret)
            }
            Drawn::Window { opened, held } => {
                let end = opened.body.len().saturating_sub(held);
                let from = self.dock.scrolled.min(end);
                let caret = opened
                    .caret
                    .filter(|caret| caret.row >= from && caret.row < from + held)
                    .map(|caret| Caret {
                        row: opened.head.len() + caret.row - from,
                        ..caret
                    });
                let Opened {
                    head,
                    body,
                    pinned,
                    bottom,
                    ..
                } = opened;
                let mut rows = head;
                rows.extend(body.into_iter().skip(from).take(held));
                rows.extend(pinned);
                rows.push(bottom);
                let room = columns.saturating_sub(SAID);
                rows.push(said(SAID, Slot::Quiet, clip(MORE, room)));
                (rows, caret)
            }
        }
    }

    /// How far down [`Dock::scrolled`] may go at this size: zero wherever the
    /// open dock fits whole or is not drawn open, so a caller clamping its
    /// offset to this one is left with none to move.
    ///
    /// Read off the same layout [`Docked::within`] draws, at the same width
    /// and room, so the last window the caller can scroll to is the last one
    /// drawn.
    #[must_use]
    pub fn end(&self, columns: usize, room: usize, glyphs: Glyphs) -> usize {
        match self.drawn(columns, room, glyphs) {
            Drawn::Window { opened, held } => opened.body.len().saturating_sub(held),
            Drawn::Nothing | Drawn::Closed | Drawn::Whole(_) => 0,
        }
    }

    /// How the dock is drawn in `room` rows: the one choice both
    /// [`Docked::within`] and [`Docked::end`] are read from.
    fn drawn(&self, columns: usize, room: usize, glyphs: Glyphs) -> Drawn {
        let Some((_, batch)) = self.dock.front() else {
            return Drawn::Nothing;
        };
        if room == 0 || columns == 0 || self.dock.waiting() == 0 {
            return Drawn::Nothing;
        }
        if self.dock.phase == Phase::Collapsed {
            return Drawn::Closed;
        }

        let mut tightest = None;
        for spacing in Spacing::LADDER.into_iter().filter(|spacing| spacing.says) {
            let Some(opened) = self.open(batch, columns, glyphs, spacing) else {
                return Drawn::Closed;
            };
            if opened.height() <= room {
                return Drawn::Whole(opened);
            }
            tightest = Some(opened);
        }

        // A window is cut from the tightest rung that still says everything,
        // the one just found taller than the room, with the row under the frame
        // given to the keys that move it. Being taller than the room is what
        // leaves its body more rows than the window holds, so a window drawn is
        // always one that can move.
        let Some(opened) = tightest else {
            return Drawn::Closed;
        };
        let chrome = opened.head.len() + opened.pinned.len() + 2;
        match room.checked_sub(chrome).filter(|held| *held > 0) {
            Some(held) => Drawn::Window { opened, held },
            None => Drawn::Closed,
        }
    }

    /// How many questions wait, in every batch, in words.
    fn waiting(&self) -> String {
        let count = self.dock.waiting();
        if count == 1 {
            "1 question waiting".to_owned()
        } else {
            format!("{count} questions waiting")
        }
    }

    /// The mark, its slot, and the words of the state the dock is in.
    fn status(&self, glyphs: Glyphs) -> (Slot, &'static str, String) {
        match self.dock.phase {
            Phase::Collapsed | Phase::Answering => (Slot::Quiet, glyphs.open(), self.waiting()),
            Phase::Reviewing => (
                Slot::Quiet,
                glyphs.bullet(),
                "Review before you submit".to_owned(),
            ),
            Phase::Submitting => (
                Slot::DoingMark,
                glyphs.doing(),
                format!("Submitting answers{}", glyphs.ellipsis()),
            ),
            Phase::Refused(_) => (
                Slot::Trouble,
                glyphs.failed(),
                "Answer not accepted".to_owned(),
            ),
            Phase::Accepted => (Slot::DoneMark, glyphs.done(), "Answers accepted".to_owned()),
        }
    }

    /// The state's mark and words, as the first thing on a row.
    fn stated(&self, glyphs: Glyphs) -> Row {
        let (slot, mark, label) = self.status(glyphs);
        Row::new()
            .then(slot, mark)
            .then(Slot::Plain, " ")
            .then(Slot::Strong, label)
    }

    /// The one row the dock is while it is closed, or while the window cannot
    /// hold it open: the state, what it is about, and where to open it.
    ///
    /// What it is about goes first where the row is short, then the hint, and
    /// the state is cut only where nothing else is left.
    fn closed(&self, columns: usize, glyphs: Glyphs) -> Row {
        let detail = match &self.dock.phase {
            Phase::Refused(reason) => spoken(reason),
            Phase::Submitting | Phase::Accepted => String::new(),
            Phase::Collapsed | Phase::Answering | Phase::Reviewing => self
                .dock
                .open()
                .map(|(_, member)| spoken(&member.heading))
                .unwrap_or_default(),
        };
        let apart = format!(" {} ", glyphs.dot());
        let stated = self.stated(glyphs);
        let hinted = stated
            .clone()
            .then(Slot::Quiet, &apart)
            .then(Slot::Quiet, HINT);

        let room = columns
            .saturating_sub(hinted.columns())
            .saturating_sub(wide(&apart));
        if !detail.is_empty() && room >= DETAIL {
            let shown = if wide(&detail) <= room {
                detail.clone()
            } else {
                let ellipsis = glyphs.ellipsis();
                format!(
                    "{}{ellipsis}",
                    clip(&detail, room.saturating_sub(wide(ellipsis)))
                )
            };
            return stated
                .then(Slot::Quiet, &apart)
                .then(Slot::Plain, shown)
                .then(Slot::Quiet, &apart)
                .then(Slot::Quiet, HINT);
        }

        if hinted.columns() <= columns {
            hinted
        } else {
            stated.clipped(columns)
        }
    }

    /// The open dock at one rung of the ladder, in its parts, or `None` where
    /// the window is too narrow for the column its answers open in.
    fn open(
        &self,
        batch: &Batched<K>,
        columns: usize,
        glyphs: Glyphs,
        spacing: Spacing,
    ) -> Option<Opened> {
        let inner = columns.saturating_sub(AROUND);
        let across = inner.saturating_sub(SAID);
        let (_, member) = self.dock.open()?;
        let least = front(member).max(PAYLOAD + wide(NOTED) + SAID) + 1;
        if inner < least {
            return None;
        }

        let laid = Laid {
            inner,
            glyphs,
            spacing,
        };
        let (open, opened) = glyphs.top();
        let (close, closed) = glyphs.bottom();
        let bar = glyphs.horizontal().repeat(inner);

        let mut head = vec![Row::new().then(Slot::Accent, format!("{open}{bar}{opened}"))];
        let mut header = self.stated(glyphs);
        if let Some((at, _)) = self.dock.front().filter(|_| self.dock.batches.len() > 1) {
            header.push(Slot::Quiet, format!(" {} ", glyphs.dot()));
            header.push(
                Slot::Quiet,
                format!("batch {} of {}", at + 1, self.dock.batches.len()),
            );
        }
        head.push(framed(
            Row::new()
                .then(Slot::Plain, " ".repeat(SAID))
                .join(header.clipped(across)),
            inner,
            glyphs,
        ));

        let mut body = Vec::new();
        let (caret, pinned) = if self.dock.phase == Phase::Answering {
            (self.answering(batch, member, laid, &mut body), Vec::new())
        } else {
            (None, self.reviewed(batch, laid, &mut body))
        };

        let footer = (spacing.footer && !self.footer.is_empty()).then(|| {
            let room = columns.saturating_sub(SAID);
            said(SAID, Slot::Quiet, clip(&spoken(self.footer), room))
        });

        Some(Opened {
            head,
            body,
            pinned,
            bottom: Row::new().then(Slot::Accent, format!("{close}{bar}{closed}")),
            footer,
            caret,
        })
    }

    /// The open question's rows: which question of the batch it is, what it
    /// asks, every answer it offers, the reader's own, the note, the specimen,
    /// and the decline. Returns where the cursor stands, where a line is being
    /// written.
    fn answering(
        &self,
        batch: &Batched<K>,
        member: &Member,
        laid: Laid,
        rows: &mut Vec<Row>,
    ) -> Option<Caret> {
        let Laid {
            inner,
            glyphs,
            spacing,
        } = laid;
        let across = inner.saturating_sub(SAID);
        let focus = self.dock.focus;
        let draft = &member.draft;
        let blank = |rows: &mut Vec<Row>| {
            if spacing.opening {
                rows.push(framed(Row::new(), inner, glyphs));
            }
        };

        blank(rows);
        rows.push(framed(self.stepped(batch, across, glyphs), inner, glyphs));
        blank(rows);
        let question = spoken(&member.question);
        for line in fold(&question, across) {
            rows.push(framed(said(SAID, Slot::Plain, line), inner, glyphs));
        }
        blank(rows);

        let mut caret = None;
        for (at, offer) in member.answers.iter().enumerate() {
            for row in self.offered(member, at, offer, laid) {
                rows.push(framed(row, inner, glyphs));
            }
        }

        let own = member.writes_at();
        let front = front(member);
        let room = inner.saturating_sub(front);
        let marked = focus.marked == own;
        let mut row = opening(member, own, marked, glyphs);
        boxed_mark(
            &mut row,
            draft.chosen.several(),
            !draft.declined && !draft.written.is_empty(),
            glyphs,
        );
        if let Some(Editing::Written { column }) = focus.editing {
            caret = Some(Caret {
                row: rows.len(),
                column: 1 + front + column.min(last(room)),
            });
            let written = spoken(&draft.written);
            if written.is_empty() {
                rows.push(framed(
                    row.then(Slot::Quiet, clip(OWN, room)),
                    inner,
                    glyphs,
                ));
            } else {
                let slid = slid(&written, column, room).to_owned();
                rows.push(framed(row.then(Slot::Plain, slid), inner, glyphs));
            }
        } else if draft.written.is_empty() {
            rows.push(framed(
                row.then(Slot::Quiet, clip(OWN, room)),
                inner,
                glyphs,
            ));
        } else {
            let slot = if marked { Slot::Bold } else { Slot::Plain };
            let written = spoken(&draft.written);
            for (at, line) in fold(&written, room).into_iter().enumerate() {
                let row = if at == 0 {
                    row.clone().then(slot, line)
                } else {
                    said(front, slot, line)
                };
                rows.push(framed(row, inner, glyphs));
            }
        }

        let noting = match focus.editing {
            Some(Editing::Note { column }) => Some(column),
            _ => None,
        };
        if !draft.note.is_empty() || noting.is_some() {
            let at = PAYLOAD + wide(NOTED);
            let room = inner.saturating_sub(at).saturating_sub(SAID);
            let note = spoken(&draft.note);
            let opened = Row::new()
                .then(Slot::Plain, " ".repeat(PAYLOAD))
                .then(Slot::Quiet, NOTED);
            if let Some(column) = noting {
                caret = Some(Caret {
                    row: rows.len(),
                    column: 1 + at + column.min(last(room)),
                });
                let slid = slid(&note, column, room).to_owned();
                rows.push(framed(opened.then(Slot::Plain, slid), inner, glyphs));
            } else {
                for (line, text) in fold(&note, room).into_iter().enumerate() {
                    let row = if line == 0 {
                        opened.clone().then(Slot::Plain, text)
                    } else {
                        said(at, Slot::Plain, text)
                    };
                    rows.push(framed(row, inner, glyphs));
                }
            }
        }

        let shows: Vec<&[String]> = member
            .answers
            .iter()
            .map(|offer| offer.shows.as_slice())
            .collect();
        rows.extend(under(&shows, focus.marked, inner, glyphs, spacing.opening));

        blank(rows);
        let ruled = glyphs.horizontal().repeat(across.saturating_sub(SAID));
        rows.push(framed(said(SAID, Slot::Quiet, &ruled), inner, glyphs));
        let declines = member.declines_at();
        let mut row = opening(member, declines, focus.marked == declines, glyphs);
        boxed_mark(&mut row, false, draft.declined, glyphs);
        rows.push(framed(
            row.then(Slot::Strong, clip(DECLINE, room)),
            inner,
            glyphs,
        ));

        caret
    }

    /// The row naming every question of the batch, with the open one marked,
    /// or counting them where the names will not fit whole.
    fn stepped(&self, batch: &Batched<K>, across: usize, glyphs: Glyphs) -> Row {
        let here = self.dock.open().map_or(0, |(at, _)| at);
        let mut named = Row::new().then(Slot::Plain, " ".repeat(SAID));
        for (at, member) in batch.members.iter().enumerate() {
            if at > 0 {
                named.push(Slot::Plain, " ".repeat(GAP));
            }
            let open = at == here;
            named.push(Slot::Accent, if open { glyphs.caret() } else { " " });
            named.push(Slot::Plain, " ");
            if member.settles() {
                named.push(Slot::DoneMark, glyphs.done());
            } else {
                named.push(Slot::Quiet, glyphs.open());
            }
            named.push(Slot::Plain, " ");
            named.push(
                if open { Slot::Bold } else { Slot::Quiet },
                spoken(&member.heading),
            );
        }
        if named.columns() <= across + SAID {
            return named;
        }

        let answered = batch
            .members
            .iter()
            .filter(|member| member.settles())
            .count();
        let counted = format!(
            "question {} of {} {} {answered} answered",
            here + 1,
            batch.members.len(),
            glyphs.dot()
        );
        said(SAID, Slot::Quiet, clip(&counted, across))
    }

    /// One offered answer's rows: the caret, the number, the mark saying
    /// whether the draft takes it, the words, whether it is recommended, and
    /// the quiet rows under it. Unframed.
    fn offered(&self, member: &Member, at: usize, offer: &Offer, laid: Laid) -> Vec<Row> {
        let Laid {
            inner,
            glyphs,
            spacing,
        } = laid;
        let marked = self.dock.focus.marked == at;
        let front = front(member);
        let room = inner.saturating_sub(front);
        let slot = if marked { Slot::Bold } else { Slot::Plain };
        let draft = &member.draft;
        let chosen = !draft.declined && draft.chosen.takes(at);

        let mut first = opening(member, at, marked, glyphs);
        boxed_mark(&mut first, draft.chosen.several(), chosen, glyphs);

        let name = spoken(&offer.answer);
        let mut rows: Vec<Row> = fold(&name, room)
            .into_iter()
            .enumerate()
            .map(|(row, line)| {
                if row == 0 {
                    first.clone().then(slot, line)
                } else {
                    said(front, slot, line)
                }
            })
            .collect();

        if offer.recommended {
            let recommended = format!(" {} recommended", glyphs.dot());
            match rows.last_mut() {
                Some(row) if row.columns() + wide(&recommended) <= inner => {
                    row.push(Slot::Quiet, recommended);
                }
                _ => rows.push(said(front, Slot::Quiet, clip("recommended", room))),
            }
        }

        if spacing.says && !offer.says.is_empty() {
            let says = spoken(&offer.says);
            rows.extend(
                fold(&says, room)
                    .into_iter()
                    .map(|line| said(front, Slot::Quiet, line)),
            );
        }

        rows
    }

    /// Every answer of the batch read back, under what the state says about
    /// them; returns the rule and the actions the state offers, which stay in
    /// view where the rest of it scrolls.
    fn reviewed(&self, batch: &Batched<K>, laid: Laid, rows: &mut Vec<Row>) -> Vec<Row> {
        let Laid {
            inner,
            glyphs,
            spacing,
        } = laid;
        let across = inner.saturating_sub(SAID);
        let under = inner.saturating_sub(PAYLOAD).saturating_sub(SAID);
        let blank = |rows: &mut Vec<Row>| {
            if spacing.opening {
                rows.push(framed(Row::new(), inner, glyphs));
            }
        };

        match &self.dock.phase {
            Phase::Refused(reason) => {
                let reason = spoken(reason);
                for line in fold(&reason, across) {
                    rows.push(framed(said(SAID, Slot::Plain, line), inner, glyphs));
                }
                rows.push(framed(
                    said(SAID, Slot::Quiet, clip("Your draft is still here.", across)),
                    inner,
                    glyphs,
                ));
            }
            Phase::Accepted => {
                for line in fold("Waiting for the model's next step.", across) {
                    rows.push(framed(said(SAID, Slot::Quiet, line), inner, glyphs));
                }
            }
            Phase::Collapsed | Phase::Answering | Phase::Reviewing | Phase::Submitting => {}
        }

        for member in &batch.members {
            blank(rows);
            let draft = &member.draft;
            let (slot, mark) = if member.settles() {
                (Slot::DoneMark, glyphs.done())
            } else {
                (Slot::Quiet, glyphs.open())
            };
            let heading = spoken(&member.heading);
            let named = Row::new()
                .then(Slot::Plain, " ".repeat(SAID))
                .then(slot, mark)
                .then(Slot::Plain, " ")
                .then(Slot::Quiet, heading);
            rows.push(framed(named.clipped(inner), inner, glyphs));

            if draft.declined {
                rows.push(framed(
                    said(PAYLOAD, Slot::Strong, "Declined"),
                    inner,
                    glyphs,
                ));
            } else if !member.settles() {
                rows.push(framed(
                    said(PAYLOAD, Slot::Quiet, clip("No answer yet", under)),
                    inner,
                    glyphs,
                ));
            } else {
                let chosen = draft
                    .chosen
                    .taken()
                    .filter_map(|at| member.answers.get(at))
                    .map(|offer| spoken(&offer.answer));
                let written = Some(spoken(&draft.written)).filter(|text| !text.is_empty());
                for answer in chosen.chain(written) {
                    for line in fold(&answer, under) {
                        rows.push(framed(said(PAYLOAD, Slot::Plain, line), inner, glyphs));
                    }
                }
            }

            // A declined question is sent as declined and nothing else, so its
            // note stays in the draft for an undone decline and is not read
            // back beside it.
            if !draft.declined && !draft.note.is_empty() {
                let at = PAYLOAD + wide(NOTED);
                let room = inner.saturating_sub(at).saturating_sub(SAID);
                let note = spoken(&draft.note);
                for (line, text) in fold(&note, room).into_iter().enumerate() {
                    let row = if line == 0 {
                        Row::new()
                            .then(Slot::Plain, " ".repeat(PAYLOAD))
                            .then(Slot::Quiet, NOTED)
                            .then(Slot::Plain, text)
                    } else {
                        said(at, Slot::Plain, text)
                    };
                    rows.push(framed(row, inner, glyphs));
                }
            }
        }

        let actions = self.dock.phase.actions();
        if actions.is_empty() {
            return Vec::new();
        }
        blank(rows);
        let ruled = glyphs.horizontal().repeat(across.saturating_sub(SAID));
        let mut pinned = vec![framed(said(SAID, Slot::Quiet, &ruled), inner, glyphs)];
        let on = self.dock.marked_action();
        for action in actions {
            let marked = on == Some(*action);
            let row = Row::new()
                .then(Slot::Plain, " ".repeat(SAID))
                .then(Slot::Accent, if marked { glyphs.caret() } else { " " })
                .then(Slot::Plain, " ")
                .then(
                    Slot::Strong,
                    clip(action.label(), across.saturating_sub(AROUND)),
                );
            pinned.push(framed(row, inner, glyphs));
        }
        pinned
    }
}

/// The open dock in the parts a window is cut from: the rows that stay at the
/// top, the rows that scroll, the rows that stay under them, and the frame's
/// foot.
struct Opened {
    /// The frame's top and the row saying what state the dock is in.
    head: Vec<Row>,
    /// The question, or the answers read back: what scrolls.
    body: Vec<Row>,
    /// The rule and the actions the state offers, or nothing.
    pinned: Vec<Row>,
    /// The frame's foot.
    bottom: Row,
    /// The caller's row of keys under the frame, where this rung keeps it.
    footer: Option<Row>,
    /// Where the cursor stands, counted in rows of `body`.
    caret: Option<Caret>,
}

impl Opened {
    /// How many rows it is drawn whole.
    fn height(&self) -> usize {
        self.head.len()
            + self.body.len()
            + self.pinned.len()
            + 1
            + usize::from(self.footer.is_some())
    }

    /// Every row of it, top to bottom.
    fn whole(self) -> Vec<Row> {
        let mut rows = self.head;
        rows.extend(self.body);
        rows.extend(self.pinned);
        rows.push(self.bottom);
        rows.extend(self.footer);
        rows
    }
}

/// How the dock is drawn at one size.
enum Drawn {
    /// Nothing waits, or there is no room at all.
    Nothing,
    /// The one row.
    Closed,
    /// Open, every row of it.
    Whole(Opened),
    /// Open, `held` rows of its body at a time.
    Window {
        /// The dock, laid out at the rung a window is cut from.
        opened: Opened,
        /// How many rows of its body the window shows.
        held: usize,
    },
}

/// How a row is being drawn: the width inside the frame, the characters, and
/// which blanks this rung still has.
#[derive(Debug, Clone, Copy)]
struct Laid {
    inner: usize,
    glyphs: Glyphs,
    spacing: Spacing,
}

/// How many columns every number in `member`'s list is given, so the column
/// of numbers stays a column past nine.
fn figures(member: &Member) -> usize {
    member.declines_at().saturating_add(1).to_string().len()
}

/// The column an answer's words open in: the caret, the number, and the mark.
fn front(member: &Member) -> usize {
    SAID + 1 + figures(member) + 3 + CHOSEN
}

/// The front of a row of the open question: the caret where the mark is, and
/// the number.
fn opening(member: &Member, at: usize, marked: bool, glyphs: Glyphs) -> Row {
    let figures = figures(member);
    Row::new()
        .then(Slot::Plain, " ".repeat(SAID))
        .then(Slot::Accent, if marked { glyphs.caret() } else { " " })
        .then(
            if marked { Slot::Strong } else { Slot::Plain },
            format!(" {:>figures$}. ", at + 1),
        )
}

/// The mark saying whether the draft takes a row: square for a question that
/// takes several, round for one that takes one.
fn boxed_mark(row: &mut Row, several: bool, chosen: bool, glyphs: Glyphs) {
    let (open, close) = if several { ("[", "]") } else { ("(", ")") };
    row.push(Slot::Quiet, open);
    if chosen {
        row.push(Slot::DoneMark, glyphs.done());
    } else {
        row.push(Slot::Plain, " ");
    }
    row.push(Slot::Quiet, close);
    row.push(Slot::Plain, " ");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dump::dump;

    /// An offered answer called `answer`, saying nothing more.
    fn offer(answer: &str) -> Offer {
        Offer {
            answer: answer.to_owned(),
            says: String::new(),
            shows: Vec::new(),
            recommended: false,
        }
    }

    /// The question about the wire format: one answer, the first recommended.
    fn format() -> Member {
        Member {
            heading: "Format".to_owned(),
            question: "Which wire format should the exporter use first?".to_owned(),
            answers: vec![
                Offer {
                    says: "one object a line".to_owned(),
                    recommended: true,
                    ..offer("JSON lines")
                },
                offer("CSV"),
                offer("Parquet"),
            ],
            draft: Drafted::new(Chosen::One(None)),
        }
    }

    /// The question about the tables: several answers.
    fn scope() -> Member {
        Member {
            heading: "Scope".to_owned(),
            question: "Which tables go in the first export?".to_owned(),
            answers: vec![offer("Orders"), offer("Customers"), offer("Invoices")],
            draft: Drafted::new(Chosen::Several(BTreeSet::new())),
        }
    }

    /// A dock in `phase` holding one batch of `members`.
    fn dock(members: Vec<Member>, phase: Phase) -> Dock<u32> {
        Dock {
            batches: vec![Batched { key: 7, members }],
            current: 0,
            focus: Focus::default(),
            scrolled: 0,
            phase,
        }
    }

    /// Both questions, the first answered `CSV` and the second declined after
    /// `Orders` had been taken.
    fn drafted(phase: Phase) -> Dock<u32> {
        let mut format = format();
        format.draft.chosen = Chosen::One(Some(1));
        let mut scope = scope();
        scope.draft.chosen = Chosen::Several(BTreeSet::from([0]));
        scope.draft.declined = true;
        dock(vec![format, scope], phase)
    }

    /// What the dock draws, row by row.
    fn art(dock: &Dock<u32>, columns: usize, room: usize) -> Vec<String> {
        Docked { dock, footer: "" }
            .within(columns, room, Glyphs::Unicode)
            .0
            .iter()
            .map(Row::text)
            .collect()
    }

    /// The same, as one block.
    fn said(dock: &Dock<u32>, columns: usize, room: usize) -> String {
        art(dock, columns, room).join("\n")
    }

    /// The one row that says `what`.
    fn row<'a>(rows: &'a [String], what: &str) -> &'a str {
        let found: Vec<&String> = rows.iter().filter(|row| row.contains(what)).collect();
        assert_eq!(found.len(), 1, "one row says {what:?}: {rows:#?}");
        found.first().map_or("", |row| row.as_str())
    }

    /// Every phase there is, with a reason for the refusal.
    fn phases() -> [Phase; 6] {
        [
            Phase::Collapsed,
            Phase::Answering,
            Phase::Reviewing,
            Phase::Submitting,
            Phase::Refused("the session could not save it".to_owned()),
            Phase::Accepted,
        ]
    }

    #[test]
    fn nothing_waiting_draws_nothing_in_any_phase() {
        for phase in phases() {
            let empty: Dock<u32> = Dock {
                batches: Vec::new(),
                current: 0,
                focus: Focus::default(),
                scrolled: 0,
                phase: phase.clone(),
            };
            assert!(art(&empty, 80, 40).is_empty(), "{phase:?}");
        }
    }

    #[test]
    fn the_closed_dock_is_one_row_with_the_count_the_heading_and_the_hint() {
        let rows = art(&dock(vec![format(), scope()], Phase::Collapsed), 80, 40);

        assert_eq!(rows.len(), 1, "{rows:#?}");
        let only = rows.join("");
        assert!(only.contains(Glyphs::Unicode.open()), "{only}");
        assert!(only.contains("2 questions waiting"), "{only}");
        assert!(only.contains("Format"), "{only}");
        assert!(only.contains("/questions"), "{only}");
    }

    #[test]
    fn one_question_is_counted_as_one() {
        let only = said(&dock(vec![format()], Phase::Collapsed), 80, 40);
        assert!(only.contains("1 question waiting"), "{only}");
    }

    #[test]
    fn an_open_question_shows_its_answers_and_which_one_is_marked() {
        let mut open = dock(vec![format(), scope()], Phase::Answering);
        open.focus.marked = 1;
        let rows = art(&open, 80, 40);
        let caret = Glyphs::Unicode.caret();

        assert!(
            rows.iter().any(|row| row.contains("2 questions waiting")),
            "{rows:#?}"
        );
        assert!(
            rows.iter()
                .any(|row| row.contains("Which wire format should the exporter use first?")),
            "{rows:#?}"
        );
        assert!(row(&rows, "CSV").contains(caret), "{rows:#?}");
        for unmarked in [
            "JSON lines",
            "Parquet",
            "Write your own answer",
            "Decline question",
        ] {
            assert!(
                !row(&rows, unmarked).contains(caret),
                "{unmarked}: {rows:#?}"
            );
        }
    }

    #[test]
    fn a_question_taking_several_shows_each_choice_the_written_answer_and_the_note() {
        let mut several = scope();
        several.draft = Drafted {
            chosen: Chosen::Several(BTreeSet::from([0, 2])),
            written: "Payments".to_owned(),
            note: "only the last quarter".to_owned(),
            declined: false,
        };
        let mut open = dock(vec![format(), several], Phase::Answering);
        open.focus.member = 1;
        let rows = art(&open, 80, 40);
        let done = Glyphs::Unicode.done();

        assert!(
            row(&rows, "Orders").contains(&format!("[{done}]")),
            "{rows:#?}"
        );
        assert!(
            row(&rows, "Invoices").contains(&format!("[{done}]")),
            "{rows:#?}"
        );
        assert!(row(&rows, "Customers").contains("[ ]"), "{rows:#?}");
        assert!(
            row(&rows, "Payments").contains(&format!("[{done}]")),
            "{rows:#?}"
        );
        assert!(
            row(&rows, "Note: only the last quarter").contains("Note: "),
            "{rows:#?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains("Write your own answer")),
            "{rows:#?}"
        );
    }

    #[test]
    fn writing_an_answer_or_a_note_parks_the_cursor_in_the_line_being_written() {
        for (editing, line) in [
            (Editing::Written { column: 3 }, "Payments"),
            (Editing::Note { column: 3 }, "only the last quarter"),
        ] {
            let mut several = scope();
            several.draft.written = "Payments".to_owned();
            several.draft.note = "only the last quarter".to_owned();
            let mut open = dock(vec![several], Phase::Answering);
            open.focus.marked = 3;
            open.focus.editing = Some(editing);

            let (rows, caret) = Docked {
                dock: &open,
                footer: "",
            }
            .within(80, 40, Glyphs::Unicode);
            let texts: Vec<String> = rows.iter().map(Row::text).collect();
            let at = texts
                .iter()
                .position(|row| row.contains(line))
                .unwrap_or(usize::MAX);
            let start = texts
                .get(at)
                .and_then(|row| row.find(line).map(|offset| wide(&row[..offset])))
                .unwrap_or(usize::MAX);

            assert_eq!(
                caret,
                Some(Caret {
                    row: at,
                    column: start + 3
                }),
                "{editing:?}: {texts:#?}"
            );
        }
    }

    #[test]
    fn nothing_is_being_written_so_there_is_no_cursor() {
        let open = dock(vec![format()], Phase::Answering);
        let (rows, caret) = Docked {
            dock: &open,
            footer: "",
        }
        .within(80, 40, Glyphs::Unicode);
        assert!(!rows.is_empty());
        assert_eq!(caret, None);
    }

    #[test]
    fn a_recommended_answer_is_marked_but_never_shown_as_chosen_without_a_draft() {
        let rows = art(&dock(vec![format()], Phase::Answering), 80, 40);
        let recommended = row(&rows, "JSON lines");
        assert!(recommended.contains("recommended"), "{rows:#?}");
        assert!(recommended.contains("( )"), "{rows:#?}");
        assert!(!recommended.contains(Glyphs::Unicode.done()), "{rows:#?}");

        let review = art(&dock(vec![format()], Phase::Reviewing), 80, 40);
        assert!(
            review.iter().any(|row| row.contains("No answer yet")),
            "{review:#?}"
        );
        assert!(
            !review.iter().any(|row| row.contains("JSON lines")),
            "{review:#?}"
        );
    }

    #[test]
    fn the_review_reads_every_answer_back_and_offers_submit_batch() {
        let rows = art(&drafted(Phase::Reviewing), 80, 40);

        for said in [
            "Review before you submit",
            "Format",
            "CSV",
            "Scope",
            "Declined",
        ] {
            assert!(
                rows.iter().any(|row| row.contains(said)),
                "{said}: {rows:#?}"
            );
        }
        assert!(
            row(&rows, "Submit batch").contains(Glyphs::Unicode.caret()),
            "{rows:#?}"
        );
        row(&rows, "Keep editing");
        row(&rows, "Close");
    }

    #[test]
    fn a_declined_question_reads_back_as_declined_and_nothing_it_offered() {
        let review = said(&drafted(Phase::Reviewing), 80, 40);
        assert!(review.contains("Declined"), "{review}");
        for offered in ["Orders", "Customers", "Invoices"] {
            assert!(!review.contains(offered), "{offered}: {review}");
        }
    }

    #[test]
    fn a_refused_submission_says_why_and_keeps_the_draft_on_screen() {
        let rows = art(
            &drafted(Phase::Refused("the session could not save it".to_owned())),
            80,
            40,
        );
        let all = rows.join("\n");

        for said in [
            "Answer not accepted",
            "the session could not save it",
            "Your draft is still here",
            "CSV",
            "Declined",
            "Retry submission",
            "Keep editing",
            "Close",
        ] {
            assert!(all.contains(said), "{said}: {all}");
        }
        assert!(!all.contains("Submit batch"), "{all}");
        assert!(
            row(&rows, "Answer not accepted").contains(Glyphs::Unicode.failed()),
            "{all}"
        );
    }

    #[test]
    fn while_the_answers_are_on_their_way_nothing_offers_to_send_them_again() {
        let all = said(&drafted(Phase::Submitting), 80, 40);
        assert!(all.contains("Submitting"), "{all}");
        assert!(all.contains("CSV"), "{all}");
        assert!(!all.contains("Submit batch"), "{all}");
        assert!(!all.contains("Retry submission"), "{all}");
    }

    #[test]
    fn accepted_answers_say_they_were_taken_and_not_that_they_were_saved_or_read() {
        let rows = art(&drafted(Phase::Accepted), 80, 40);
        let all = rows.join("\n");
        assert!(
            row(&rows, "Answers accepted").contains(Glyphs::Unicode.done()),
            "{all}"
        );
        assert!(all.contains("next step"), "{all}");
        for claim in ["saved", "understood", "Submit batch"] {
            assert!(!all.contains(claim), "{claim}: {all}");
        }
    }

    #[test]
    fn every_state_has_a_label_in_words_and_a_mark() {
        let glyphs = Glyphs::Unicode;
        for (phase, label, mark) in [
            (Phase::Collapsed, "2 questions waiting", glyphs.open()),
            (Phase::Answering, "2 questions waiting", glyphs.open()),
            (
                Phase::Reviewing,
                "Review before you submit",
                glyphs.bullet(),
            ),
            (Phase::Submitting, "Submitting", glyphs.doing()),
            (
                Phase::Refused("the session could not save it".to_owned()),
                "Answer not accepted",
                glyphs.failed(),
            ),
            (Phase::Accepted, "Answers accepted", glyphs.done()),
        ] {
            let rows = art(&drafted(phase.clone()), 80, 40);
            assert!(row(&rows, label).contains(mark), "{phase:?}: {rows:#?}");
        }
    }

    #[test]
    fn a_narrow_window_counts_the_questions_instead_of_naming_them() {
        let mut first = format();
        first.heading = "Wire format for the exporter".to_owned();
        let mut second = scope();
        second.heading = "Tables in the first export".to_owned();
        let open = dock(vec![first.clone(), second.clone()], Phase::Answering);

        let wide = said(&open, 100, 40);
        assert!(wide.contains("Wire format for the exporter"), "{wide}");

        let narrow = said(&open, 40, 40);
        assert!(narrow.contains("question 1 of 2"), "{narrow}");
        assert!(!narrow.contains("Tables in the first export"), "{narrow}");
    }

    #[test]
    fn a_room_too_short_for_the_open_dock_gets_the_closed_row_instead() {
        let open = art(&dock(vec![format(), scope()], Phase::Answering), 80, 3);
        assert_eq!(open.len(), 1, "{open:#?}");
        assert!(open.join("").contains("2 questions waiting"), "{open:#?}");

        let refused = art(
            &drafted(Phase::Refused("the session could not save it".to_owned())),
            80,
            2,
        );
        assert_eq!(refused.len(), 1, "{refused:#?}");
        assert!(
            refused.join("").contains("Answer not accepted"),
            "{refused:#?}"
        );

        assert!(art(&drafted(Phase::Answering), 80, 0).is_empty());
    }

    #[test]
    fn a_focus_past_the_end_names_the_last_question_at_every_height() {
        let mut open = dock(vec![format(), scope()], Phase::Answering);
        open.focus.member = 5;

        let tall = said(&open, 80, 40);
        assert!(
            tall.contains("Which tables go in the first export?"),
            "{tall}"
        );

        let short = art(&open, 80, 3);
        assert_eq!(short.len(), 1, "{short:#?}");
        let only = short.join("");
        assert!(only.contains("Scope"), "{only}");
        assert!(!only.contains("Format"), "{only}");
    }

    #[test]
    fn a_mark_past_the_actions_stands_on_the_last_one() {
        let caret = Glyphs::Unicode.caret();
        for phase in phases() {
            if phase.actions().is_empty() {
                continue;
            }
            let mut past = drafted(phase.clone());
            past.focus.marked = 4;
            let rows = art(&past, 80, 40);
            assert!(row(&rows, "Close").contains(caret), "{phase:?}: {rows:#?}");
        }
    }

    #[test]
    fn a_choice_the_question_does_not_offer_is_no_answer() {
        let mut dangling = format();
        dangling.draft.chosen = Chosen::One(Some(99));
        assert!(!dangling.settles());

        let rows = art(&dock(vec![dangling], Phase::Reviewing), 80, 40);
        assert!(
            rows.iter().any(|row| row.contains("No answer yet")),
            "{rows:#?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains(Glyphs::Unicode.done())),
            "{rows:#?}"
        );
    }

    #[test]
    fn choices_are_read_in_the_order_they_were_offered_and_each_once() {
        let one = Chosen::One(Some(2));
        assert_eq!(one.taken().collect::<Vec<_>>(), [2]);
        assert_eq!(Chosen::One(None).taken().count(), 0);

        let several = Chosen::Several([2, 0, 2, 1].into_iter().collect::<BTreeSet<_>>());
        assert_eq!(several.taken().collect::<Vec<_>>(), [0, 1, 2]);
        assert!(several.several() && !one.several());
    }

    #[test]
    fn the_dock_resolves_what_is_in_front_the_way_it_draws_it() {
        let mut past = dock(vec![format(), scope()], Phase::Answering);
        past.batches.push(Batched {
            key: 8,
            members: vec![format()],
        });
        past.current = 3;
        past.focus.member = 5;
        assert_eq!(
            past.front().map(|(at, batch)| (at, batch.key)),
            Some((1, 8))
        );
        assert_eq!(
            past.open()
                .map(|(at, member)| (at, member.heading.as_str())),
            Some((0, "Format"))
        );

        past.current = 0;
        assert_eq!(
            past.open()
                .map(|(at, member)| (at, member.heading.as_str())),
            Some((1, "Scope"))
        );
        assert_eq!(past.marked_action(), None);

        past.phase = Phase::Reviewing;
        past.focus.marked = 9;
        assert_eq!(past.marked_action(), Some(Action::Close));
        past.focus.marked = 0;
        assert_eq!(past.marked_action(), Some(Action::Submit));
    }

    #[test]
    fn a_batch_with_no_questions_draws_nothing() {
        for phase in phases() {
            let empty = dock(Vec::new(), phase.clone());
            assert!(art(&empty, 80, 40).is_empty(), "{phase:?}");
        }
    }

    #[test]
    fn debugging_the_dock_prints_nobody_s_words() {
        let mut member = format();
        if let Some(first) = member.answers.first_mut() {
            first.shows = vec!["{\"id\": 1}".to_owned()];
        }
        member.draft.written = "my own answer".to_owned();
        member.draft.note = "a note of mine".to_owned();
        let printed = format!("{:?}", dock(vec![member], Phase::Answering));

        for words in [
            "Format",
            "Which wire format",
            "JSON lines",
            "one object a line",
            "\"id\"",
            "my own answer",
            "a note of mine",
        ] {
            assert!(!printed.contains(words), "{words:?} in {printed}");
        }
    }

    #[test]
    fn the_models_words_are_cleaned_and_never_drawn_in_the_docks_own_voice() {
        let mut hostile = format();
        hostile.heading = "Submit batch".to_owned();
        hostile.question = "Pick \u{1b}[31mone\u{1b}[0m\u{7} now".to_owned();
        hostile.draft.chosen = Chosen::One(Some(0));
        let mut review = dock(vec![hostile.clone()], Phase::Reviewing);
        review.focus.marked = 0;
        let open = dock(vec![hostile], Phase::Answering);

        for (dock, phase) in [(&review, "review"), (&open, "open")] {
            let rows = Docked { dock, footer: "" }
                .within(80, 40, Glyphs::Unicode)
                .0;
            for row in &rows {
                assert!(
                    !row.text().chars().any(char::is_control),
                    "{phase}: {:?}",
                    row.text()
                );
                for (slot, text) in row.spans() {
                    for model in ["JSON lines", "Pick one now", "one object a line"] {
                        if text.contains(model) {
                            assert_ne!(slot, Slot::Strong, "{phase}: {text:?}");
                        }
                    }
                }
            }
            let strong: usize = rows
                .iter()
                .flat_map(Row::spans)
                .filter(|(slot, text)| *slot == Slot::Strong && text.contains("Submit batch"))
                .count();
            let expected = usize::from(phase == "review");
            assert_eq!(strong, expected, "{phase}: the action, and not the heading");
        }
    }

    /// Four questions of eight answers each, every string as hostile as the
    /// sweep's: long, unbroken, wide, combining, and carrying escapes.
    fn hostile() -> Dock<u32> {
        const LONG: &str =
            "crucible-code/crates/crucible-tui/src/docked/a/name/nothing/can/shorten.rs";
        const WIDE: &str = "\u{691c}\u{7d22}\u{304c} \u{9014}\u{4e2d}\u{3067} \u{2600}\u{fe0f} \
                            \u{30c7}\u{30a3}\u{30ec}\u{30af}\u{30c8}\u{30ea}";
        const MARKED: &str = "a re\u{301}sume\u{301} of a nai\u{308}ve cafe\u{301} that stops";
        const ESCAPED: &str =
            "\u{1b}[2J\u{1b}[31mred\u{1b}[0m \u{7}\u{8}\r\nand \u{1b}]0;title\u{7}";
        const PROSE: &str = "a search that stops partway through a directory nobody meant to \
                             open, and says so on the row it stopped on";

        let texts = [LONG, WIDE, MARKED, ESCAPED];
        let members = (0..4)
            .map(|at| {
                let text = texts.get(at).copied().unwrap_or(LONG);
                Member {
                    heading: format!("{text} {at}"),
                    question: format!("{PROSE} {text} {ESCAPED}"),
                    answers: (0..8)
                        .map(|answer| Offer {
                            answer: format!("{text} {answer}"),
                            says: format!("{PROSE} {WIDE}"),
                            shows: (0..=answer * 2)
                                .map(|line| format!("{line} {LONG} {ESCAPED} {MARKED}"))
                                .collect(),
                            recommended: answer == 0,
                        })
                        .collect(),
                    draft: Drafted {
                        chosen: if at % 2 == 1 {
                            Chosen::Several(BTreeSet::from([0, 3, 7]))
                        } else {
                            Chosen::One(Some(7))
                        },
                        written: format!("{WIDE} {LONG} {ESCAPED}"),
                        note: format!("{PROSE} {MARKED} {WIDE} {ESCAPED}").repeat(8),
                        declined: at == 3,
                    },
                }
            })
            .collect();

        Dock {
            batches: vec![
                Batched { key: 1, members },
                Batched {
                    key: 2,
                    members: vec![format()],
                },
            ],
            current: 0,
            focus: Focus {
                member: 1,
                marked: 7,
                editing: None,
            },
            scrolled: 0,
            phase: Phase::Answering,
        }
    }

    /// Every way the hostile batch can be drawn: each phase, each place the
    /// mark can stand on the open question, and each line being written.
    fn hostile_docks() -> Vec<Dock<u32>> {
        let mut docks = Vec::new();
        for phase in phases() {
            for (marked, editing) in [
                (0, None),
                (7, None),
                (8, Some(Editing::Written { column: 400 })),
                (9, Some(Editing::Note { column: 7 })),
                (2, None),
            ] {
                let mut dock = hostile();
                dock.phase = phase.clone();
                dock.focus.marked = marked;
                dock.focus.editing = editing;
                docks.push(dock);
            }
        }
        docks
    }

    #[test]
    fn the_hostile_batch_draws_no_control_byte_and_no_row_past_the_window() {
        for dock in hostile_docks() {
            for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
                for columns in [1, 2, 5, 9, 14, 20, 33, 40, 61, 80, 120, 200] {
                    for room in [0, 1, 2, 5, 12, 40, 200] {
                        let (rows, caret) = Docked {
                            dock: &dock,
                            footer: "tab composer \u{1b}[1m· esc close",
                        }
                        .within(columns, room, glyphs);
                        assert!(rows.len() <= room, "{:?} {columns}x{room}", dock.phase);
                        for row in &rows {
                            assert!(row.columns() <= columns, "{columns}: {:?}", row.text());
                            assert!(
                                !row.text().chars().any(char::is_control),
                                "{:?}",
                                row.text()
                            );
                        }
                        if let Some(caret) = caret {
                            assert!(caret.row < rows.len(), "{caret:?}");
                            assert!(caret.column < columns, "{caret:?} at {columns}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_dock_keeps_the_colour_rule_in_every_phase() {
        for dock in hostile_docks() {
            let rows = Docked {
                dock: &dock,
                footer: "esc close",
            }
            .within(80, 200, Glyphs::Unicode)
            .0;
            assert!(!rows.is_empty(), "{:?}", dock.phase);
            crate::colour_rule::holds_framed("the dock", &rows, |_| false);
        }
    }

    #[test]
    fn the_review_ready_to_submit_at_eighty_columns() {
        let mut ready = drafted(Phase::Reviewing);
        if let Some(first) = ready
            .batches
            .first_mut()
            .and_then(|batch| batch.members.first_mut())
        {
            first.draft.note = "start with the one the warehouse reads".to_owned();
        }
        let rows = Docked {
            dock: &ready,
            footer: "enter choose · tab composer · esc close",
        }
        .within(80, 40, Glyphs::Unicode)
        .0;
        insta::assert_snapshot!(dump(&rows, 80));
    }

    #[test]
    fn a_refused_submission_at_eighty_columns() {
        let refused = drafted(Phase::Refused("the session could not save it".to_owned()));
        let rows = Docked {
            dock: &refused,
            footer: "enter choose · tab composer · esc close",
        }
        .within(80, 40, Glyphs::Unicode)
        .0;
        insta::assert_snapshot!(dump(&rows, 80));
    }

    #[test]
    fn an_open_question_in_a_narrow_window() {
        let mut first = format();
        first.heading = "Wire format for the exporter".to_owned();
        let mut open = dock(vec![first, scope()], Phase::Answering);
        open.focus.marked = 0;
        let rows = Docked {
            dock: &open,
            footer: "enter review · esc close",
        }
        .within(36, 40, Glyphs::Unicode)
        .0;
        insta::assert_snapshot!(dump(&rows, 36));
    }

    /// Words numbered from `tag`, each five bytes with the space after it, to
    /// `bytes` bytes: every one of them a word nothing else in the dock says.
    fn numbered(tag: char, bytes: usize) -> (String, Vec<String>) {
        let words: Vec<String> = (0..bytes / 5).map(|at| format!("{tag}{at:03}")).collect();
        (words.join(" "), words)
    }

    /// Every window the dock can be scrolled to at this size, top to bottom.
    fn windows(dock: &Dock<u32>, columns: usize, room: usize) -> Vec<Vec<String>> {
        let end = Docked { dock, footer: "" }.end(columns, room, Glyphs::Unicode);
        (0..=end)
            .map(|scrolled| {
                let at = Dock {
                    scrolled,
                    ..dock.clone()
                };
                art(&at, columns, room)
            })
            .collect()
    }

    /// Whether every one of `words` is on a row of some window, whole.
    fn reachable(windows: &[Vec<String>], words: &[String]) -> Vec<String> {
        words
            .iter()
            .filter(|word| {
                !windows.iter().flatten().any(|row| {
                    row.split(|c: char| !c.is_alphanumeric())
                        .any(|said| said == word.as_str())
                })
            })
            .cloned()
            .collect()
    }

    #[test]
    fn a_long_note_leaves_the_question_on_screen_and_every_row_of_it_in_reach() {
        let (note, words) = numbered('n', 4096);
        let mut format = format();
        format.draft.note = note;
        let open = dock(vec![format, scope()], Phase::Answering);

        let rows = art(&open, 80, 60);
        assert!(rows.len() <= 60, "{rows:#?}");
        row(&rows, "Which wire format should the exporter use first?");
        let windows = windows(&open, 80, 60);
        for window in &windows {
            assert!(window.len() <= 60, "{window:#?}");
        }
        assert_eq!(reachable(&windows, &words), Vec::<String>::new());
    }

    #[test]
    fn four_long_notes_scroll_the_review_down_to_submit_batch() {
        let mut members = Vec::new();
        let mut every = Vec::new();
        for tag in ['p', 'q', 'r', 's'] {
            let (note, words) = numbered(tag, 600);
            let mut format = format();
            format.draft.chosen = Chosen::One(Some(1));
            format.draft.note = note;
            members.push(format);
            every.extend(words);
        }
        let review = dock(members, Phase::Reviewing);

        // Sixty rows is the room the case was set in; the review's own height
        // at eighty columns is about that, so the rooms the round-one probe
        // found it closed in are tried too.
        for room in [60, 40, 30, 24] {
            let windows = windows(&review, 80, room);
            for window in &windows {
                assert!(window.len() <= room, "{window:#?}");
            }
            assert!(
                windows
                    .iter()
                    .flatten()
                    .any(|row| row.contains("Submit batch")),
                "{room}: {windows:#?}"
            );
            assert_eq!(reachable(&windows, &every), Vec::<String>::new(), "{room}");
        }
    }

    #[test]
    fn the_scroll_keys_are_drawn_exactly_where_the_window_can_move() {
        for bytes in (0..=2400).step_by(150) {
            let (note, _) = numbered('n', bytes);
            for phase in [Phase::Answering, Phase::Reviewing] {
                let mut sized = dock(vec![format(), scope()], phase);
                if let Some(member) = sized
                    .batches
                    .first_mut()
                    .and_then(|b| b.members.first_mut())
                {
                    member.draft.note.clone_from(&note);
                }
                for columns in [24, 40, 80, 120] {
                    for room in 1..=70 {
                        let rows = art(&sized, columns, room);
                        let keys = rows.last().is_some_and(|row| row.contains("pgup"));
                        let end = Docked {
                            dock: &sized,
                            footer: "",
                        }
                        .end(columns, room, Glyphs::Unicode);
                        assert_eq!(
                            keys,
                            end > 0,
                            "{bytes} bytes of note, {columns}x{room}, end {end}: {rows:#?}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_scroll_keys_are_named_only_where_there_is_more_to_see() {
        let short = dock(vec![format(), scope()], Phase::Answering);
        let (note, _) = numbered('n', 4096);
        let mut long = short.clone();
        if let Some(member) = long.batches.first_mut().and_then(|b| b.members.first_mut()) {
            member.draft.note = note;
        }

        assert!(
            !said(&short, 80, 60).contains("pgup"),
            "{}",
            said(&short, 80, 60)
        );
        assert_eq!(
            Docked {
                dock: &short,
                footer: ""
            }
            .end(80, 60, Glyphs::Unicode),
            0
        );
        assert!(
            said(&long, 80, 60).contains("pgup pgdn"),
            "{}",
            said(&long, 80, 60)
        );
        assert!(
            Docked {
                dock: &long,
                footer: ""
            }
            .end(80, 60, Glyphs::Unicode)
                > 0
        );
    }

    #[test]
    fn a_scroll_past_the_end_draws_the_last_window() {
        let (note, _) = numbered('n', 4096);
        let mut long = dock(vec![format(), scope()], Phase::Answering);
        if let Some(member) = long.batches.first_mut().and_then(|b| b.members.first_mut()) {
            member.draft.note = note;
        }
        let end = Docked {
            dock: &long,
            footer: "",
        }
        .end(80, 60, Glyphs::Unicode);
        let last = Dock {
            scrolled: end,
            ..long.clone()
        };
        let past = Dock {
            scrolled: usize::MAX,
            ..long
        };

        assert_ne!(end, 0);
        assert_eq!(art(&past, 80, 60), art(&last, 80, 60));
    }

    #[test]
    fn a_declined_question_keeps_its_draft_for_when_the_decline_is_undone() {
        let mut declined = drafted(Phase::Reviewing);
        if let Some(scope) = declined
            .batches
            .first_mut()
            .and_then(|b| b.members.get_mut(1))
        {
            scope.draft.note = "send the orders table only".to_owned();
        }
        let rows = art(&declined, 80, 40);
        row(&rows, "Declined");
        assert!(!rows.iter().any(|row| row.contains("Orders")), "{rows:#?}");
        assert!(
            !rows.iter().any(|row| row.contains("orders table only")),
            "a declined question is sent with no note, so none is read back: {rows:#?}"
        );

        let mut undone = declined;
        if let Some(scope) = undone
            .batches
            .first_mut()
            .and_then(|b| b.members.get_mut(1))
        {
            scope.draft.declined = false;
        }
        let rows = art(&undone, 80, 40);
        assert!(
            !rows.iter().any(|row| row.contains("Declined")),
            "{rows:#?}"
        );
        row(&rows, "Orders");
        row(&rows, "orders table only");
    }

    #[test]
    fn a_question_taking_several_with_none_taken_reads_back_as_no_answer_yet() {
        let mut format = format();
        format.draft.chosen = Chosen::One(Some(0));
        let review = dock(vec![format, scope()], Phase::Reviewing);
        let rows = art(&review, 80, 40);
        row(&rows, "No answer yet");
    }
}
