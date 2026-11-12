//! The questions the model asked without waiting, as the terminal keeps them:
//! the dock above the box, its drafts, and the era they belong to.
//!
//! **Kept on the drawing thread, for the life of the conversation.** What
//! waits is learned from the application alone — a report that something
//! changed, read through the handle the terminal supplied, and the outcome of
//! the terminal's own submissions — and a batch is settled only by sending
//! [`Command::Answer`] with the era it was shown under. Nothing here reaches
//! the registry, and `crucible-tui` names none of the domain values: a batch
//! is mapped onto the words [`Dock`] draws on the way in, and a draft back
//! onto [`Replied`] on the way out.
//!
//! **Nothing moves by itself.** A new batch is the closed row; a report
//! arriving changes neither the focus nor a draft, and a batch it lists again
//! under the same era and number keeps the draft it had. A report makes two
//! moves only. Under another era every batch goes with its draft and its
//! scroll, and the dock closes. Under the same era, when the batch in front is
//! gone, what is in front now opens at its start, and with nothing left the
//! dock closes.
//!
//! **Escape never answers.** Out of an editor it leaves the editor with the
//! line kept; while a decline waits for its second press it keeps the
//! question and closes nothing; anywhere else it closes the dock with every
//! draft kept. A decline is its own row, asked about once, and a declined
//! question is sent as declined with nothing beside it.
//!
//! **Drafts are bounded like notes.** Every line is written through an
//! [`Editor`], which holds what the box holds and no more, and a line longer
//! than a reply may carry is refused when it is sent, with the draft kept.

use std::cell::Cell;
use std::collections::BTreeSet;

use crucible_client_api::{Command, Era, Picked, Refusal, Replied, Said};
use crucible_tui::{
    Action, Batched, Caret, Chosen, Dock, Docked, Drafted, Editing, Editor, Focus, Glyphs, Key,
    Member, Offer, Phase, Pressed, Row, Scrolled, Slot, Typed,
};
use crucible_types::{Batch, QuestionId};

/// The quiet row under the open dock while a question is open.
const CHOOSING: &str = "enter choose · n note · tab composer · esc close";

/// The same, on a question that takes several answers.
const CHOOSING_SEVERAL: &str = "enter next · space toggle · n note · tab composer · esc close";

/// While a line is being written.
const WRITING: &str = "enter or esc done · tab composer";

/// While a decline waits for its second press.
const CONFIRMING: &str = "enter to decline this question · esc keep it";

/// While the answers are read back, or what became of them is shown.
const ACTING: &str = "enter select · tab composer · esc close";

/// While the dock is open and the composer has the keys.
const ELSEWHERE: &str = "tab questions";

/// The dock, and what the terminal keeps beside it.
///
/// Not `Debug`: the editor holds the line being written, which is the
/// person's.
pub(super) struct Docking {
    /// What is drawn.
    dock: Dock<QuestionId>,
    /// The era every batch here was shown under, once one has been.
    era: Option<Era>,
    /// What the dock was doing when it was last closed, shown on reopening.
    shown: Phase,
    /// Whether the dock has the keys rather than the composer.
    focus: bool,
    /// The line being written, loaded from the draft as writing starts.
    editor: Editor,
    /// Whether `Decline question` was pressed once and waits for a second.
    confirming: bool,
    /// The width, room and glyphs the dock was last drawn in, which a page
    /// key and a moved mark are measured against.
    laid: Cell<Option<(usize, usize, Glyphs)>>,
}

impl Default for Docking {
    fn default() -> Self {
        Self {
            dock: Dock {
                batches: Vec::new(),
                current: 0,
                focus: Focus::default(),
                scrolled: Scrolled::default(),
                phase: Phase::Collapsed,
            },
            era: None,
            shown: Phase::Answering,
            focus: false,
            editor: Editor::new(),
            confirming: false,
            laid: Cell::new(None),
        }
    }
}

/// What `/questions` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Toggled {
    /// The dock opened, with the keys.
    Opened,
    /// It closed, every draft kept.
    Closed,
    /// Nothing waits, so there was nothing to open.
    Empty,
}

/// What a key pressed while the dock has the keys did.
#[derive(Debug, PartialEq)]
pub(super) enum Keyed {
    /// It is not the dock's: the loop reading keys does with it what it would.
    Passed,
    /// Taken, and nothing on screen changes.
    Still,
    /// Taken, and the dock is drawn again.
    Moved,
    /// The composer has the keys now; the dock stays open.
    Composer,
    /// The dock closed, every draft kept.
    Closed,
    /// Every answer in the batch in front, to be sent through the client.
    Submit(Submission),
}

/// Every answer of one batch, for the era and id it was shown under: the
/// one thing the dock sends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Submission {
    pub(super) era: Era,
    pub(super) id: QuestionId,
    pub(super) replies: Vec<Replied>,
}

impl Submission {
    /// The request that settles it.
    pub(super) fn command(self) -> Command {
        Command::Answer {
            era: self.era,
            id: self.id,
            replies: self.replies,
        }
    }
}

impl Docking {
    /// Shows what waits as `era` and `batches` stood together: the view is
    /// replaced, a batch listed again under the same era and number keeps its
    /// draft, and one of another era is dropped with its draft and scroll.
    pub(super) fn waiting(&mut self, era: &Era, batches: &[Batch]) {
        self.moved(era);
        let front = self.front();
        let mut before = std::mem::take(&mut self.dock.batches);
        self.dock.batches = batches
            .iter()
            .map(
                |batch| match before.iter().position(|kept| kept.key == batch.id()) {
                    Some(at) => before.swap_remove(at),
                    None => batched(batch),
                },
            )
            .collect();

        let still =
            front.and_then(|key| self.dock.batches.iter().position(|batch| batch.key == key));
        match still {
            Some(at) => self.dock.current = at,
            // The batch in front is gone, settled or dropped: what is in
            // front now opens at its start.
            None if front.is_some() => self.restart(),
            None => {}
        }
        if self.dock.batches.is_empty() {
            self.close();
            self.shown = Phase::Answering;
        }
    }

    /// The conversation's questions moved to `era`, by `/clear`, a resume or
    /// a move of route: a batch of any other era goes, with its draft and
    /// its scroll. A batch of the same era stays as it was.
    pub(super) fn moved(&mut self, era: &Era) {
        if self.era.as_ref() == Some(era) {
            return;
        }
        self.era = Some(era.clone());
        self.dock.batches.clear();
        self.restart();
        self.close();
        self.shown = Phase::Answering;
    }

    /// `/questions`: opens the dock with the keys, closes it, or says nothing
    /// waits.
    pub(super) fn toggled(&mut self) -> Toggled {
        if self.dock.batches.is_empty() {
            return Toggled::Empty;
        }
        if self.is_open() {
            self.close();
            return Toggled::Closed;
        }
        self.dock.phase = std::mem::replace(&mut self.shown, Phase::Answering);
        self.focus = true;
        self.anchor();
        Toggled::Opened
    }

    /// Whether the dock is open, with the keys or without them.
    pub(super) fn is_open(&self) -> bool {
        self.dock.phase != Phase::Collapsed
    }

    /// Whether the dock has the keys.
    pub(super) fn has_focus(&self) -> bool {
        self.focus
    }

    /// `Tab` from the composer: the keys go to the dock where it is open.
    pub(super) fn focused(&mut self) -> bool {
        self.focus = self.is_open();
        self.focus
    }

    /// What became of the submission of the batch numbered `id`: taken, or
    /// refused for the reason given with the draft kept. Nothing where that
    /// batch is no longer in front.
    pub(super) fn answered(&mut self, id: QuestionId, outcome: Result<(), String>) {
        if self.front() != Some(id) {
            return;
        }
        let phase = match outcome {
            Ok(()) => Phase::Accepted,
            Err(reason) => Phase::Refused(reason),
        };
        if self.is_open() {
            self.dock.phase = phase;
        } else {
            self.shown = phase;
        }
        self.dock.focus.marked = 0;
        self.dock.focus.editing = None;
        self.anchor();
    }

    /// What `pressed` does while the dock has the keys.
    pub(super) fn key(&mut self, pressed: &Pressed) -> Keyed {
        let keyed = if let Some(editing) = self.dock.focus.editing {
            self.writing(editing, pressed)
        } else if self.confirming {
            self.confirm(pressed)
        } else if self.dock.phase == Phase::Answering {
            self.answering(pressed)
        } else {
            self.acting(pressed)
        };
        if matches!(keyed, Keyed::Moved) {
            self.anchor();
        }
        keyed
    }

    /// The dock in `room` rows of `columns`, and the caret of a line being
    /// written in it. The size is kept for the page keys, which is why a
    /// shared borrow can draw it: the frame that stands the dock holds the
    /// rest of the session lent too.
    pub(super) fn rows(
        &self,
        columns: usize,
        room: usize,
        glyphs: Glyphs,
    ) -> (Vec<Row>, Option<Caret>) {
        // The second press a decline waits for is asked in a row of its own,
        // under the dock, rather than in its row of keys, which a short window
        // takes away once the dock's blank rows are gone: asked there, a
        // decline in a short window would look like a key that did nothing.
        let asking = usize::from(self.confirming);
        let room = room.saturating_sub(asking);
        self.laid.set(Some((columns, room, glyphs)));
        let (mut rows, caret) = Docked {
            dock: &self.dock,
            footer: self.footer(),
        }
        .within(columns, room, glyphs);
        if self.confirming && !rows.is_empty() {
            rows.push(
                Row::new()
                    .then(Slot::Plain, "  ")
                    .then(Slot::Quiet, CONFIRMING)
                    .clipped(columns),
            );
        }
        (rows, caret)
    }

    /// The quiet row under the open dock, naming the keys that work now.
    fn footer(&self) -> &'static str {
        if !self.focus {
            return ELSEWHERE;
        }
        if self.dock.focus.editing.is_some() {
            return WRITING;
        }
        // Asked under the dock instead, by [`Docking::rows`].
        if self.confirming {
            return "";
        }
        match self.dock.phase {
            Phase::Answering
                if self
                    .member()
                    .is_some_and(|member| member.draft.chosen.several()) =>
            {
                CHOOSING_SEVERAL
            }
            Phase::Answering => CHOOSING,
            Phase::Collapsed
            | Phase::Reviewing
            | Phase::Submitting
            | Phase::Refused(_)
            | Phase::Accepted => ACTING,
        }
    }

    /// A key while a line is being written: the line's, but for the keys
    /// that stop writing, which keep what was written.
    fn writing(&mut self, editing: Editing, pressed: &Pressed) -> Keyed {
        let typed = match pressed {
            Pressed::Key(Key::Enter) | Pressed::Escape => {
                self.dock.focus.editing = None;
                return Keyed::Moved;
            }
            Pressed::Tab => {
                self.dock.focus.editing = None;
                self.focus = false;
                return Keyed::Composer;
            }
            Pressed::Key(Key::Interrupt | Key::Eof) => return Keyed::Passed,
            Pressed::Key(key) => self.editor.press(*key),
            Pressed::Pasted(pasted) => self.editor.paste(pasted),
            other => return passed(other),
        };
        if !matches!(typed, Typed::Changed) {
            return Keyed::Still;
        }
        let text = self.editor.text().to_owned();
        let column = self.editor.column();
        let Some(member) = self.member_mut() else {
            return Keyed::Still;
        };
        match editing {
            Editing::Written { .. } => {
                // An answer written in replaces a decline, as one chosen does.
                if !text.is_empty() {
                    member.draft.declined = false;
                    if !member.draft.chosen.several() {
                        member.draft.chosen = Chosen::One(None);
                    }
                }
                member.draft.written = text;
                self.dock.focus.editing = Some(Editing::Written { column });
            }
            Editing::Note { .. } => {
                member.draft.note = text;
                self.dock.focus.editing = Some(Editing::Note { column });
            }
        }
        Keyed::Moved
    }

    /// A key while a decline waits for its second press.
    fn confirm(&mut self, pressed: &Pressed) -> Keyed {
        self.confirming = false;
        match pressed {
            Pressed::Key(Key::Enter) => {
                if let Some(member) = self.member_mut() {
                    member.draft.declined = true;
                }
                self.next();
                Keyed::Moved
            }
            Pressed::Escape => Keyed::Moved,
            other => match self.answering(other) {
                Keyed::Still | Keyed::Passed => Keyed::Moved,
                keyed => keyed,
            },
        }
    }

    /// A key while a question is open.
    fn answering(&mut self, pressed: &Pressed) -> Keyed {
        let Some((at, member)) = self.dock.open() else {
            return Keyed::Still;
        };
        let (writes, declines) = (member.writes_at(), member.declines_at());
        let several = member.draft.chosen.several();
        let members = self
            .dock
            .front()
            .map_or(0, |(_, batch)| batch.members.len());
        let marked = self.dock.focus.marked.min(declines);

        match pressed {
            Pressed::Up => self.mark(marked.checked_sub(1)),
            Pressed::Down => self.mark(Some(marked + 1).filter(|next| *next <= declines)),
            Pressed::Key(Key::Left) => self.question(at.checked_sub(1)),
            Pressed::Key(Key::Right) => self.question(Some(at + 1).filter(|next| *next < members)),
            Pressed::Key(Key::Char(' ')) if several && marked < writes => {
                if let Some(member) = self.member_mut()
                    && let Chosen::Several(taken) = &mut member.draft.chosen
                {
                    if !taken.remove(&marked) {
                        taken.insert(marked);
                    }
                    member.draft.declined = false;
                }
                Keyed::Moved
            }
            Pressed::Key(Key::Char('n')) => self.write(Editing::Note { column: 0 }),
            Pressed::Key(Key::Enter) if marked < writes => {
                if let Some(member) = self.member_mut() {
                    if !several {
                        member.draft.chosen = Chosen::One(Some(marked));
                        member.draft.written.clear();
                    }
                    member.draft.declined = false;
                }
                self.next();
                Keyed::Moved
            }
            Pressed::Key(Key::Enter) if marked == writes => {
                if self
                    .member()
                    .is_some_and(|member| member.draft.written.is_empty())
                {
                    return self.write(Editing::Written { column: 0 });
                }
                if let Some(member) = self.member_mut() {
                    member.draft.declined = false;
                }
                self.next();
                Keyed::Moved
            }
            Pressed::Key(Key::Enter) => {
                if let Some(member) = self.member_mut()
                    && member.draft.declined
                {
                    member.draft.declined = false;
                    return Keyed::Moved;
                }
                self.confirming = true;
                Keyed::Moved
            }
            Pressed::Escape => self.closed(),
            Pressed::Tab => {
                self.focus = false;
                Keyed::Composer
            }
            Pressed::PageUp => self.page(false),
            Pressed::PageDown => self.page(true),
            other => passed(other),
        }
    }

    /// A key while the answers are read back, on their way, refused or
    /// taken: the mark walks the actions the phase offers.
    fn acting(&mut self, pressed: &Pressed) -> Keyed {
        let actions = self.dock.phase.actions().len();
        let marked = self.dock.focus.marked.min(actions.saturating_sub(1));
        match pressed {
            Pressed::Up => self.mark(marked.checked_sub(1)),
            Pressed::Down => self.mark(Some(marked + 1).filter(|next| *next < actions)),
            Pressed::Key(Key::Enter) => match self.dock.marked_action() {
                Some(Action::Submit | Action::Retry) => self.submit(),
                Some(Action::KeepEditing) => {
                    self.dock.phase = Phase::Answering;
                    self.dock.focus = Focus::default();
                    Keyed::Moved
                }
                Some(Action::Close) => self.closed(),
                None => Keyed::Still,
            },
            Pressed::Escape => self.closed(),
            Pressed::Tab => {
                self.focus = false;
                Keyed::Composer
            }
            Pressed::PageUp => self.page(false),
            Pressed::PageDown => self.page(true),
            other => passed(other),
        }
    }

    /// Every answer of the batch in front, as the command that sends them;
    /// a line too long to send refuses the batch here, with the draft kept.
    fn submit(&mut self) -> Keyed {
        let Some((era, (_, batch))) = self.era.clone().zip(self.dock.front()) else {
            return Keyed::Still;
        };
        let id = batch.key;
        match replies(batch) {
            Ok(replies) => {
                self.dock.phase = Phase::Submitting;
                self.dock.focus.marked = 0;
                Keyed::Submit(Submission { era, id, replies })
            }
            Err(refusal) => {
                self.dock.phase = Phase::Refused(refusal.to_string());
                self.dock.focus.marked = 0;
                Keyed::Moved
            }
        }
    }

    /// Starts writing the line `editing` names, from what the draft holds.
    fn write(&mut self, editing: Editing) -> Keyed {
        let Some(member) = self.member() else {
            return Keyed::Still;
        };
        let line = match editing {
            Editing::Written { .. } => member.draft.written.clone(),
            Editing::Note { .. } => member.draft.note.clone(),
        };
        self.editor.clear();
        // The line came out of this editor, so it is within the ceiling an
        // edit is refused past and nothing here can refuse it.
        let _ = self.editor.put(&line);
        let column = self.editor.column();
        self.dock.focus.editing = Some(match editing {
            Editing::Written { .. } => Editing::Written { column },
            Editing::Note { .. } => Editing::Note { column },
        });
        Keyed::Moved
    }

    /// Moves the mark to `to`, where there is a row there.
    fn mark(&mut self, to: Option<usize>) -> Keyed {
        match to {
            Some(to) => {
                self.dock.focus.marked = to;
                Keyed::Moved
            }
            None => Keyed::Still,
        }
    }

    /// Opens the question at `to`, at its top, where there is one there.
    fn question(&mut self, to: Option<usize>) -> Keyed {
        match to {
            Some(to) => {
                self.dock.focus = Focus {
                    member: to,
                    ..Focus::default()
                };
                Keyed::Moved
            }
            None => Keyed::Still,
        }
    }

    /// On to the next question, or to the review after the last.
    fn next(&mut self) {
        let members = self
            .dock
            .front()
            .map_or(0, |(_, batch)| batch.members.len());
        let at = self.dock.open().map_or(0, |(at, _)| at);
        if at + 1 < members {
            self.dock.focus = Focus {
                member: at + 1,
                ..Focus::default()
            };
        } else {
            self.dock.phase = Phase::Reviewing;
            self.dock.focus.marked = 0;
            self.dock.focus.editing = None;
        }
    }

    /// The window a page further up or down, from where it opens now and no
    /// further than it may go at this size.
    fn page(&mut self, down: bool) -> Keyed {
        let Some((columns, room, glyphs)) = self.laid.get() else {
            return Keyed::Still;
        };
        let docked = Docked {
            dock: &self.dock,
            footer: self.footer(),
        };
        let (at, end) = (
            docked.opens_at(columns, room, glyphs),
            docked.end(columns, room, glyphs),
        );
        let page = (room / 2).max(1);
        let to = if down {
            (at + page).min(end)
        } else {
            at.saturating_sub(page)
        };
        self.dock.scroll_to(to);
        Keyed::Moved
    }

    /// Holds the window where it opens now, after the mark or the cursor
    /// moved, so a page key moves on from what is on screen.
    fn anchor(&mut self) {
        let Some((columns, room, glyphs)) = self.laid.get() else {
            return;
        };
        let at = Docked {
            dock: &self.dock,
            footer: self.footer(),
        }
        .opens_at(columns, room, glyphs);
        self.dock.scroll_to(at);
    }

    /// Escape: the dock closes with every draft kept.
    fn closed(&mut self) -> Keyed {
        self.close();
        Keyed::Closed
    }

    /// Closes the dock, keeping what it was doing for when it opens again.
    fn close(&mut self) {
        if self.is_open() {
            self.shown = std::mem::replace(&mut self.dock.phase, Phase::Collapsed);
        }
        self.focus = false;
        self.confirming = false;
        self.dock.focus.editing = None;
    }

    /// The batch in front shown from its start: its first question, no
    /// scroll, nothing being written.
    fn restart(&mut self) {
        self.dock.current = 0;
        self.dock.focus = Focus::default();
        self.dock.scrolled = Scrolled::default();
        self.confirming = false;
        if self.is_open() {
            self.dock.phase = Phase::Answering;
        } else {
            self.shown = Phase::Answering;
        }
    }

    /// The number of the batch in front.
    fn front(&self) -> Option<QuestionId> {
        self.dock.front().map(|(_, batch)| batch.key)
    }

    /// The question open in the batch in front.
    fn member(&self) -> Option<&Member> {
        self.dock.open().map(|(_, member)| member)
    }

    /// The same, to change its draft.
    fn member_mut(&mut self) -> Option<&mut Member> {
        let (at, _) = self.dock.open()?;
        let (batch, _) = self.dock.front()?;
        self.dock.batches.get_mut(batch)?.members.get_mut(at)
    }
}

/// What a key the dock has no use for does: passed on where the loop reading
/// keys has something to do with it, taken and ignored where it would reach
/// the composer behind the dock.
fn passed(pressed: &Pressed) -> Keyed {
    match pressed {
        Pressed::Key(Key::Interrupt | Key::Eof)
        | Pressed::Resized
        | Pressed::Expand
        | Pressed::Background
        | Pressed::Cycle
        | Pressed::Explain
        | Pressed::Plan
        | Pressed::Scrolled { .. }
        | Pressed::Clicked { .. }
        | Pressed::Dragged { .. }
        | Pressed::Hovered { .. }
        | Pressed::Released { .. }
        | Pressed::Ignored => Keyed::Passed,
        Pressed::Key(_)
        | Pressed::Pasted(_)
        | Pressed::Tab
        | Pressed::Remove
        | Pressed::Copy
        | Pressed::PasteImage
        | Pressed::Rename
        | Pressed::All
        | Pressed::SendAll
        | Pressed::SendNow
        | Pressed::Escape
        | Pressed::Up
        | Pressed::Down
        | Pressed::PageUp
        | Pressed::PageDown => Keyed::Still,
    }
}

/// `batch` as the dock draws it: its words, and a draft with nothing in it.
fn batched(batch: &Batch) -> Batched<QuestionId> {
    Batched {
        key: batch.id(),
        members: batch
            .questions()
            .iter()
            .map(|question| Member {
                heading: question.heading().to_owned(),
                question: question.question().to_owned(),
                answers: question
                    .answers()
                    .map(|answer| Offer {
                        answer: answer.answer().to_owned(),
                        says: answer.says().to_owned(),
                        shows: answer.shows().map(str::to_owned).collect(),
                        recommended: answer.is_recommended(),
                    })
                    .collect(),
                draft: Drafted::new(if question.takes_several() {
                    Chosen::Several(BTreeSet::new())
                } else {
                    Chosen::One(None)
                }),
            })
            .collect(),
    }
}

/// One reply per question of `batch`, in order: declined alone where it was
/// declined, and otherwise the answers chosen by name, the reader's own last,
/// with the note beside them.
///
/// # Errors
///
/// The [`Refusal`] of a line longer than a reply may carry.
fn replies(batch: &Batched<QuestionId>) -> Result<Vec<Replied>, Refusal> {
    batch
        .members
        .iter()
        .map(|member| {
            let draft = &member.draft;
            if draft.declined {
                return Ok(Replied::Declined);
            }
            let mut chosen = draft
                .chosen
                .taken()
                .filter_map(|at| member.answers.get(at))
                .map(|offer| Said::new(&offer.answer))
                .collect::<Result<Vec<_>, _>>()?;
            if !draft.written.is_empty() {
                chosen.push(Said::new(&draft.written)?);
            }
            Ok(Replied::Answered(Picked {
                chosen,
                note: Said::new(&draft.note)?,
            }))
        })
        .collect()
}

#[cfg(test)]
mod tests;
