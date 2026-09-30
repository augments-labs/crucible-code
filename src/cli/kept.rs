//! What a result said that its row in the transcript had no room for.
//!
//! A tool answers in as much text as it likes and the transcript gives it one
//! row, so most of what came back is on screen only as a count of the lines
//! that are not. This is where those lines wait for a reader who asks to see
//! them, and asking is [`Pressed::Expand`](crucible_tui::Pressed::Expand).
//!
//! Live results move their event-owned text here after the row is drawn.
//! Replay copies the result from the current message batch into this bounded
//! store; that batch is then dropped as history advances.
//!
//! It is bounded, because a session is not. [`HELD`] is the ceiling
//! on how much is held at once and the oldest result is dropped to stay under
//! it, so what this costs is the same after four hundred turns as after four —
//! the rule the whole renderer is built to keep. A row whose text was dropped
//! and still offers counts against the same ceiling, and the places waiting
//! for a row are bounded by [`UNCLAIMED`].
//!
//! It does not outlive the rows that made the offers. `/clear` and `/resume`
//! both empty the transcript — what follows either is a different conversation
//! rather than the next thing that happened in this one — and this is emptied
//! with it. The rule is the same one both ways: what is held here is what a row
//! somebody can still see has offered.
//!
//! One row can be the door to many. A run of calls that only looked around is
//! counted into a single line, and every result in that run is held against
//! that line rather than against a row of its own — so opening it opens all of
//! them. The rule above is unchanged by that: what is held is still what a row
//! somebody can still see has offered.
//!
//! What is let go of under the ceiling is not always gone. Where the session
//! has a log, the result is still in it, so a row whose text was dropped keeps
//! where the log holds it — the call it answered and the position of its
//! record, and no text — and a view that reaches that row reads the result
//! back from there. The log is reached through [`Log`], which is set from the
//! session on the screen, so nothing here names the storage it is kept in.
//! Where there is no log, a row whose result was let go of can open nothing,
//! and it is handed to what draws it to stop offering. A row can also stop
//! offering with a log: a row whose text went counts its place against the
//! ceiling, so a short result, cheaper held than placed, stays held, and where
//! nothing held would make room the oldest row goes with every result it
//! offered.
//!
//! Where a result went is learned two ways. A replay says so before it draws
//! the result. A turn does not know: the log's writer says where each result
//! landed once it has written it, which can be before the screen has drawn
//! that result or long after. So each place is filed as a result arrives,
//! without waiting for the writer, against the oldest thing that has none of
//! its call: a row, a result that had no row, a call still out. A place that
//! arrives before anything of its call waits for its result, and a result
//! that arrives with no row to keep a place for takes its place when it comes
//! rather than leaving it for a later call of the same name.
//!
//! One kind of cut is not here and cannot be. A call that changed a file is
//! shown as the change itself, and a change too long for the block is cut down
//! where the change is built rather than where it is drawn — those lines never
//! reach this process's drawing thread, so there is nothing held back from the
//! reader to hand over. The row says how many went, which is the whole of what
//! is still true about them.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::fmt;

use crucible_types::ToolId;

/// The most held at once, in bytes: the text of the results still held, and
/// what a row whose text was let go of keeps instead.
///
/// Half a mebibyte, against a process budgeted 35. One result is bounded far
/// below this by the tools themselves, so the ceiling is really on how many
/// results stay reachable — enough that a reader who scrolled back through a
/// long turn finds what they are looking at still expandable, and small enough
/// that the answer to "what does this cost" is a fraction of one turn's
/// transcript.
const HELD: usize = 512 * 1024;

/// The most held of a call that has not answered yet, in bytes.
///
/// A running command's output arrives a piece at a time and there is no result to
/// bound it against yet, so this is the bound. It keeps the *end*: what a reader
/// opening this while a command runs is looking for is where it has got to, and
/// the beginning of a build is the part they have already watched go past.
///
/// Nothing here says how much went, and it does not have to. The row above the
/// box counts every line and every byte the command has printed, so a reader who
/// opens this has already been told the total by the row whose key they pressed —
/// and when the call answers, the result replaces this with the tool's own
/// bounded answer, which marks its own gap.
const WRITING: usize = 64 * 1024;

/// The most places waiting for a result, and the most results waiting for a
/// place, each.
///
/// As many as the log's writer keeps for this to take, so that a screen
/// behind by that much still files every place it is handed. The oldest goes
/// first; a row it belonged to then says its result could not be read back.
const UNCLAIMED: usize = 8 * 128;

/// One result the transcript had to cut down to a row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Whole {
    /// The call's line, in the words it was committed under.
    ///
    /// Kept rather than worked out again: the expansion names the call the same
    /// way the row above it does, and two spellings of one call read as two
    /// calls.
    called: String,
    /// The whole of what came back.
    text: Box<str>,
    /// Which row of the record the offer to expand it was written on, or `None`
    /// where the call has not answered yet — a live call has no committed row for
    /// a click to land on, and nothing but a key reaches it.
    ///
    /// What a click is answered from. The renderer counts the rows it has let
    /// go of and this is the count at the moment that row went, so a pointer
    /// landing somewhere on the screen becomes a row of the record and a row of
    /// the record becomes this — or becomes nothing, which is a click on a row
    /// that made no offer.
    at: Option<usize>,
    /// The call whose result this is.
    call: ToolId,
    /// Where the session log holds it, where that is known yet.
    position: Option<u64>,
    /// How many results had been cut before it: the order rows were drawn in,
    /// which the store keeps as rows move between held and let go of.
    drawn: usize,
}

impl Whole {
    /// The call's line.
    pub(crate) fn called(&self) -> &str {
        &self.called
    }

    /// The whole of what came back.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// Which row of the record offered it, where one did.
    pub(crate) fn at(&self) -> Option<usize> {
        self.at
    }

    /// The order it was drawn in among every row the store has.
    pub(crate) const fn drawn(&self) -> usize {
        self.drawn
    }
}

/// Where a session's log is read back from, for rows whose results the
/// ceiling let go of.
///
/// A trait here rather than the session itself, so that what holds results
/// and the view that opens one name no storage: what implements it is set for
/// the session on the screen, when a run starts, on `/resume` and on `/clear`.
pub(crate) trait Log: fmt::Debug {
    /// Takes where the results written since this was last asked went: each
    /// call, and where in the log the record holding its result begins.
    ///
    /// Waits for nothing, so a result still queued for the log is not among
    /// them yet; a later take hands it over.
    fn landed(&self) -> Vec<(ToolId, u64)>;

    /// Takes them as [`Log::landed`] does, once everything queued before this
    /// has been written. Empty where the writer has stopped, which
    /// [`Log::places`] tells apart from nothing new.
    fn settled(&self) -> Vec<(ToolId, u64)>;

    /// Whether what is written to the log from now on is still placed: not
    /// once its writer has stopped, or has stopped placing for good.
    fn places(&self) -> bool;

    /// What the result of `call`, in the record beginning at `position`,
    /// said, or `None` where the log does not hold it there.
    fn read(&self, call: &ToolId, position: u64) -> Option<Box<str>>;
}

/// What reading one let-go result back from the log came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Back {
    /// What it said.
    Said(Box<str>),
    /// The log cannot give it back: the file is gone or changed, the record
    /// is not the one placed there, or no place for it is ever coming.
    Unread,
    /// The log has not said where it is yet, and still may: the turn has not
    /// written its batch, and nothing written after it has been placed.
    Unplaced,
}

/// A row whose result the ceiling let go of, and where the log holds it.
///
/// No text: the call's line, the call, the row, and the position.
#[derive(Debug)]
pub(crate) struct Placed {
    called: String,
    call: ToolId,
    at: usize,
    position: Option<u64>,
    /// How many results had been cut before it, as [`Whole`] says.
    drawn: usize,
}

/// Which let-go result a view has read back: the row that offered it and the
/// call it answered, which together name one result however the store moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Mark {
    at: usize,
    call: ToolId,
}

impl Placed {
    /// The call's line.
    pub(crate) fn called(&self) -> &str {
        &self.called
    }

    /// Which row of the record offered it.
    pub(crate) fn at(&self) -> usize {
        self.at
    }

    /// What names it to a view that read it back.
    pub(crate) fn mark(&self) -> Mark {
        Mark {
            at: self.at,
            call: self.call.clone(),
        }
    }

    /// Whether it is the one `mark` names.
    pub(crate) fn is(&self, mark: &Mark) -> bool {
        self.at == mark.at && self.call == mark.call
    }

    /// What it costs against [`HELD`]: its call's line, its call and the
    /// place itself. Counted so that the rows still offering after their text
    /// went are bounded by the same ceiling the text is.
    fn weight(&self) -> usize {
        weighing(&self.called, &self.call)
    }

    /// The order it was drawn in among every row the store has.
    pub(crate) const fn drawn(&self) -> usize {
        self.drawn
    }
}

/// Every result still reachable, oldest first: held whole, or let go of with
/// where the log holds it.
#[derive(Debug, Default)]
pub(crate) struct Kept {
    /// What is held, oldest at the front because that is the end that gives.
    whole: VecDeque<Whole>,
    /// How many bytes of headings and result text `whole` holds, and what the
    /// rows in `placed` weigh, added up.
    ///
    /// Carried rather than counted per push: the number is wanted on the path a
    /// result arrives on, and a walk over the whole queue there would be work
    /// proportional to how long the session has run.
    held: usize,
    /// Calls waiting for results, in request order and carrying their identity.
    ///
    /// A pass is bounded to 128 calls, so a linear identity lookup keeps this
    /// representation bounded while preserving the order the model requested.
    pending: VecDeque<Pending>,
    /// How many results have been cut this session, counting the ones since
    /// dropped.
    ///
    /// Not `whole.len()`, and the difference is the point. A view standing over
    /// what was cut has to know what has arrived *since* it opened, and the
    /// queue's length answers that only until the ceiling starts dropping from
    /// the other end. This only ever goes up.
    cut: usize,
    /// Rows whose text was let go of while the log still holds it, oldest
    /// first.
    placed: VecDeque<Placed>,
    /// Where results are read back from, where the session has a log.
    log: Option<Box<dyn Log>>,
    /// Rows that stopped being able to open anything, for what draws them to
    /// take the offer off.
    gone: Vec<usize>,
    /// Places the log handed over while a result was being read back, which
    /// is when nothing here may be changed, kept to be filed the next time
    /// something may.
    unfiled: RefCell<VecDeque<(ToolId, u64)>>,
    /// Places that came before anything of their call, oldest first, waiting
    /// for the result they belong to.
    early: VecDeque<(ToolId, u64)>,
    /// Results that arrived with no row to keep a place for and whose place
    /// has not come, oldest first, so that it is not taken for a later call
    /// of the same name.
    rowless: VecDeque<ToolId>,
}

/// One call that has not answered yet.
#[derive(Debug)]
struct Pending {
    id: ToolId,
    called: String,
    writing: Option<Whole>,
    /// Where the log holds its result, where a replay said so first.
    position: Option<u64>,
}

impl Kept {
    /// Remembers the line of one call until the result naming it arrives.
    pub(crate) fn calling(&mut self, call: ToolId, called: String) {
        if let Some(pending) = self.pending.iter_mut().find(|one| one.id == call) {
            pending.called = called;
            if let Some(writing) = &mut pending.writing {
                writing.called.clone_from(&pending.called);
            }
        } else {
            self.pending.push_back(Pending {
                id: call,
                called,
                writing: None,
                position: None,
            });
        }
    }

    /// The original heading, before the transcript clips it to one row.
    pub(crate) fn heading(&self, call: &ToolId) -> Option<&str> {
        self.pending
            .iter()
            .find(|one| one.id == *call)
            .map(|one| one.called.as_str())
    }

    /// Keeps what the call still out has printed.
    ///
    /// The end of it: bounded by dropping from the front, and by whole lines
    /// where it can, so what is held opens on a line rather than half of one.
    pub(crate) fn wrote(&mut self, call: &ToolId, text: &str) {
        let Some(pending) = self.pending.iter_mut().find(|one| one.id == *call) else {
            // A mismatched event is not permission to create an anonymous live
            // call that can never finish. In particular it must not borrow the
            // heading of another call still waiting beside it.
            return;
        };
        let writing = pending.writing.get_or_insert_with(|| Whole {
            called: pending.called.clone(),
            text: String::new().into(),
            at: None,
            call: pending.id.clone(),
            position: None,
            drawn: 0,
        });

        let mut held = String::from(&*writing.text);
        held.push_str(text);

        if held.len() > WRITING {
            // From the front, and from the line boundary after the ceiling —
            // which is what keeps the first row of the view a row somebody wrote
            // rather than the tail of one. Where there is no newline left to cut
            // at, the next character boundary: a count of bytes into text is not
            // always one, and taking the front off at the wrong offset is the one
            // way this could end a session.
            let over = held.len().saturating_sub(WRITING);
            let from = held
                .get(over..)
                .and_then(|rest| rest.find('\n'))
                .map_or_else(
                    || {
                        (over..=held.len())
                            .find(|at| held.is_char_boundary(*at))
                            .unwrap_or(held.len())
                    },
                    |at| over.saturating_add(at + 1),
                );

            held.drain(..from);
        }

        writing.text = held.into();
    }

    /// Keeps what a row could not say.
    ///
    /// Called only where the row said less than the result did — a result that
    /// fitted is on screen already, and offering to expand it would be offering
    /// the reader what they are looking at.
    ///
    /// `at` is the row of the record the offer went onto, which is what a click
    /// on that row is looked up by.
    pub(crate) fn finished(&mut self, call: &ToolId, text: Box<str>, at: usize) {
        self.keep(call, text, Some(at));
    }

    /// Keeps the whole of what a call in a folded run came back with.
    ///
    /// No row of its own was written, so the row this answers to is the line
    /// counting the run. A turn hands `None`, because that line is written once
    /// the run ends and there is nothing to point at yet — [`Kept::onto`] is
    /// where those find it. A walk putting a session back on the screen knows
    /// the run before it draws it, so it has the row already and says so.
    ///
    /// The whole of it, however short. A result that fitted on a row is dropped
    /// by [`Kept::finished`] because the row said it all — but this call has no
    /// row, so dropping it would leave a reader opening the run to find one of
    /// the calls in it simply missing.
    pub(crate) fn gathered(&mut self, call: &ToolId, text: Box<str>, at: Option<usize>) {
        self.keep(call, text, at);
    }

    /// Points everything gathered but not yet offered at the row that offers
    /// it.
    ///
    /// One row for a run of calls, so several results answer to one line of the
    /// record — which is the whole of what folding a run means here. Only the
    /// ones still waiting are touched: a result already pointing somewhere is
    /// pointing at a row that is still on screen.
    pub(crate) fn onto(&mut self, at: usize) {
        for whole in &mut self.whole {
            if whole.at.is_none() {
                whole.at = Some(at);
            }
        }
    }

    /// Says where the log holds the result of `call`, just before it arrives.
    ///
    /// A replay knows where it read each result from, so it says so as it
    /// goes, and the result drawn next takes it; a turn does not, and those
    /// places are learned from the log.
    pub(crate) fn placing(&mut self, call: &ToolId, position: u64) {
        waiting(&mut self.early, (call.clone(), position));
    }

    /// Sets where results let go of are read back from: the log of the
    /// session now on the screen, or nowhere where it has none.
    ///
    /// Whatever was learned from the log before goes: a place is a position
    /// in one file and says nothing about another. A row already let go of
    /// was placed in that file, so it stops offering.
    pub(crate) fn logging(&mut self, log: Option<Box<dyn Log>>) {
        self.log = log;
        self.unfiled.get_mut().clear();
        self.early.clear();
        self.rowless.clear();
        for whole in &mut self.whole {
            whole.position = None;
        }
        for pending in &mut self.pending {
            pending.position = None;
        }
        for placed in std::mem::take(&mut self.placed) {
            self.held = self.held.saturating_sub(placed.weight());
            self.gone.push(placed.at);
        }
    }

    /// Keeps one result, pointing wherever the caller can point it.
    fn keep(&mut self, call: &ToolId, text: Box<str>, at: Option<usize>) {
        self.file();
        let (called, position) = self
            .take(call)
            .map_or_else(|| (String::new(), None), |one| (one.called, one.position));
        let position = position.or_else(|| self.claim(call));

        let drawn = self.cut;
        self.cut = self.cut.saturating_add(1);
        self.held = self
            .held
            .saturating_add(text.len())
            .saturating_add(called.len());
        self.whole.push_back(Whole {
            called,
            text,
            at,
            call: call.clone(),
            position,
            drawn,
        });

        // After the push rather than before it, so that the newest result is
        // held whatever it costs. One longer than the ceiling on its own would
        // otherwise be the one thing a reader could never see, and it is the
        // one they are most likely to be asking about.
        //
        // What goes first is the oldest result whose place weighs less than it
        // does, because letting it go is what makes room. A short result costs
        // less held than its place would, so it stays held; only where nothing
        // held would make room does a row stop offering, and then it is the
        // oldest row, held or let go of, with every result it offered.
        while self.held > HELD {
            let newest = self.whole.len().saturating_sub(1);
            let freeing = self
                .whole
                .iter()
                .take(newest)
                .position(|whole| self.frees(whole));
            if let Some(gone) = freeing.and_then(|at| self.whole.remove(at)) {
                self.let_go(gone);
            } else if !self.withdraw_oldest() {
                break;
            }
        }
    }

    /// Whether letting `whole` go leaves less held: where there is no log or
    /// no row, the whole of it goes; where there is, its place stays.
    fn frees(&self, whole: &Whole) -> bool {
        self.log.is_none()
            || whole.at.is_none()
            || whole.text.len() + whole.called.len() > weighing(&whole.called, &whole.call)
    }

    /// Takes the oldest row there is, held or let go of, off with every result
    /// it offered, and says whether there was one to take. The newest result
    /// stays whatever it costs.
    fn withdraw_oldest(&mut self) -> bool {
        let held = self.whole.front().filter(|_| self.whole.len() > 1);
        let oldest = match (self.placed.front(), held) {
            (Some(placed), Some(whole)) if whole.drawn < placed.drawn => whole.at.ok_or(()),
            (Some(placed), _) => Ok(placed.at),
            (None, Some(whole)) => whole.at.ok_or(()),
            (None, None) => return false,
        };
        match oldest {
            Ok(at) => self.withdraw_row(at),
            // The oldest held result, which no row offers yet, goes on its own,
            // its place still its own where it comes.
            Err(()) => {
                if let Some(gone) = self.whole.pop_front() {
                    self.held = self
                        .held
                        .saturating_sub(gone.text.len())
                        .saturating_sub(gone.called.len());
                    if gone.position.is_none() && self.log.is_some() {
                        waiting(&mut self.rowless, gone.call);
                    }
                }
            }
        }
        true
    }

    /// Takes row `at` off with every result it offered, held or let go of.
    fn withdraw_row(&mut self, at: usize) {
        let mut freed = 0_usize;
        self.whole.retain(|whole| {
            let offered = whole.at == Some(at);
            if offered {
                freed = freed
                    .saturating_add(whole.text.len())
                    .saturating_add(whole.called.len());
            }
            !offered
        });
        self.placed.retain(|placed| {
            let offered = placed.at == at;
            if offered {
                freed = freed.saturating_add(placed.weight());
            }
            !offered
        });
        self.held = self.held.saturating_sub(freed);
        self.gone.push(at);
    }

    /// What becomes of a result the ceiling dropped: a place where the log
    /// holds it, or a row that has to stop offering where there is no log.
    fn let_go(&mut self, gone: Whole) {
        self.held = self
            .held
            .saturating_sub(gone.text.len())
            .saturating_sub(gone.called.len());

        // A result no row offers yet — one of a run whose line is still to be
        // written — has no row to keep a place for. Its place may still be
        // coming, and it is this result's rather than a later call's.
        let Some(at) = gone.at else {
            if gone.position.is_none() && self.log.is_some() {
                waiting(&mut self.rowless, gone.call);
            }
            return;
        };
        if self.log.is_none() {
            self.gone.push(at);
            return;
        }

        let placed = Placed {
            called: gone.called,
            call: gone.call,
            at,
            position: gone.position,
            drawn: gone.drawn,
        };
        self.held = self.held.saturating_add(placed.weight());
        self.placed.push_back(placed);
    }

    /// Files where the log's writer put the results it has written since this
    /// was last asked, waiting for nothing.
    ///
    /// Asked as every result arrives, which is what keeps what the writer
    /// holds for this short however long a result stays held before it is
    /// let go of.
    fn file(&mut self) {
        let Some(log) = &self.log else { return };
        let mut landed = std::mem::take(self.unfiled.get_mut());
        landed.extend(log.landed());
        for (call, position) in landed {
            self.place(call, position);
        }
    }

    /// Gives one place to the oldest thing of its call that has none: a row
    /// let go of, a row still held, a result that had no row, a call still
    /// out. Where there is none yet, it waits for its result.
    fn place(&mut self, call: ToolId, position: u64) {
        if let Some(placed) = self
            .placed
            .iter_mut()
            .find(|placed| placed.call == call && placed.position.is_none())
        {
            placed.position = Some(position);
        } else if let Some(whole) = self
            .whole
            .iter_mut()
            .find(|whole| whole.call == call && whole.position.is_none())
        {
            whole.position = Some(position);
        } else if let Some(at) = self.rowless.iter().position(|rowless| *rowless == call) {
            self.rowless.remove(at);
        } else if let Some(pending) = self
            .pending
            .iter_mut()
            .find(|pending| pending.id == call && pending.position.is_none())
        {
            pending.position = Some(position);
        } else {
            waiting(&mut self.early, (call, position));
        }
    }

    /// Takes the place that came for `call` before its result did.
    fn claim(&mut self, call: &ToolId) -> Option<u64> {
        let at = self.early.iter().position(|(early, _)| early == call)?;
        self.early.remove(at).map(|(_, position)| position)
    }

    /// Forgets a call that answered with no row to keep a place for, and
    /// makes sure its place, where it has not come yet, goes nowhere else.
    fn unrowed(&mut self, call: &ToolId) {
        self.file();
        let placed = self.take(call).and_then(|pending| pending.position);
        // A place said for it before it arrived is its own, whether or not its
        // call was ever drawn.
        if placed.is_some() || self.log.is_none() || self.claim(call).is_some() {
            return;
        }
        waiting(&mut self.rowless, call.clone());
    }

    /// Whether a row drawn after `placed` has been placed already.
    fn passed(&self, placed: &Placed) -> bool {
        let unfiled = self.unfiled.borrow();
        let mut later = self
            .placed
            .iter()
            .map(|one| (&one.call, one.position, one.drawn))
            .chain(
                self.whole
                    .iter()
                    .map(|whole| (&whole.call, whole.position, whole.drawn)),
            )
            .filter(|(_, _, drawn)| *drawn > placed.drawn);
        later.any(|(call, position, _)| {
            position.is_some() || unfiled.iter().any(|(landed, _)| landed == call)
        })
    }

    /// Every row whose text was let go of and that still offers, newest
    /// first. A short result can stay held after a longer one drawn later was
    /// let go of, so [`Placed::drawn`] rather than this and [`Kept::newest`]
    /// says which of two rows came first.
    pub(crate) fn older(&self) -> impl Iterator<Item = &Placed> {
        self.placed.iter().rev()
    }

    /// Reads back from the log what the row `placed` offered.
    ///
    /// Asked of a store that is only being looked at, because a view reads
    /// what it is standing over. Where the place has not been filed yet, what
    /// the log has written by now is taken, waiting for it, and kept aside to
    /// be filed the next time a result arrives.
    pub(crate) fn read_back(&self, placed: &Placed) -> Back {
        let Some(log) = self.log.as_ref() else {
            return Back::Unread;
        };
        let position = placed.position.or_else(|| {
            let mut unfiled = self.unfiled.borrow_mut();
            for landed in log.settled() {
                waiting(&mut *unfiled, landed);
            }
            unfiled
                .iter()
                .find(|(call, _)| *call == placed.call)
                .map(|(_, position)| *position)
        });
        let Some(position) = position else {
            // No place is coming where the log has stopped placing, or where a
            // row drawn after this one has one already: the writer places what
            // it writes in the order it writes it.
            return if log.places() && !self.passed(placed) {
                Back::Unplaced
            } else {
                Back::Unread
            };
        };
        log.read(&placed.call, position)
            .map_or(Back::Unread, Back::Said)
    }

    /// The rows that can no longer open anything, handed over once.
    ///
    /// A row still offering another result — one line of a folded run, say — is
    /// not among them.
    pub(crate) fn withdrawn(&mut self) -> Vec<usize> {
        let mut gone = std::mem::take(&mut self.gone);
        gone.retain(|at| !self.offered(*at));
        gone.sort_unstable();
        gone.dedup();
        gone
    }

    /// Forgets a call whose complete result fitted on screen.
    pub(crate) fn answered(&mut self, call: &ToolId) {
        self.unrowed(call);
    }

    /// Forgets a call that reached a terminal turn event without a result.
    pub(crate) fn abandoned(&mut self, call: &ToolId) {
        self.unrowed(call);
    }

    /// Takes one pending call by identity without disturbing its neighbours.
    fn take(&mut self, call: &ToolId) -> Option<Pending> {
        self.pending
            .iter()
            .position(|one| one.id == *call)
            .and_then(|at| self.pending.remove(at))
    }

    /// Drops everything, held and pending alike.
    ///
    /// What a session picked up asks for, and only that: the rows that made
    /// these offers have gone from the screen with the transcript they were in,
    /// so what is behind them is unreachable and holding it would be holding it
    /// for nobody.
    ///
    /// The count of what has been cut goes back to nothing with them, because
    /// what it is for is a view stepping over results that arrived underneath
    /// it — and none of these can arrive again.
    ///
    /// The log is kept: it belongs to the session rather than to the rows, and
    /// what changes the session sets it again.
    pub(crate) fn forget(&mut self) {
        let log = self.log.take();
        *self = Self::default();
        self.log = log;
    }

    /// Every result still held, newest first. The rows whose text was let go
    /// of are [`Kept::older`].
    ///
    /// Newest first because that is the order a reader is looking for them in:
    /// the result somebody wants to see is almost always the one that just went
    /// past.
    pub(crate) fn newest(&self) -> impl Iterator<Item = &Whole> {
        self.whole.iter().rev()
    }

    /// How many results have been cut this session.
    ///
    /// Read by a view that is standing over them while a turn is still running:
    /// the difference between this and what it read when it opened is how many
    /// arrived underneath it, and those are the ones it steps over so that the
    /// rows being read stay where the reader left them.
    pub(crate) fn cut(&self) -> usize {
        self.cut
    }

    /// Whether the row `at` of the record is one that offered to expand.
    ///
    /// A walk rather than a lookup, and it stays one: what it walks is bounded
    /// by [`HELD`] however long the session has run, and it is walked once per
    /// click. A map keyed by row would be a second thing to drop from when the
    /// ceiling drops, which is a way for the two to disagree about what is
    /// still held.
    pub(crate) fn offered(&self, at: usize) -> bool {
        self.whole.iter().any(|whole| whole.at == Some(at))
            || self.placed.iter().any(|placed| placed.at == at)
    }

    /// What the call still out has printed, where it has printed anything.
    ///
    /// Kept apart from [`Kept::newest`] rather than folded into it, because the
    /// count of what has been cut is what a standing view steps over to keep its
    /// rows still — and a call that has not answered has not been cut.
    pub(crate) fn writing(&self) -> impl DoubleEndedIterator<Item = &Whole> {
        self.pending.iter().filter_map(|one| one.writing.as_ref())
    }

    /// Whether nothing has been cut.
    ///
    /// Which is the whole of what the key asking for this has to know: with
    /// nothing reachable there was no offer on screen to have prompted it, so
    /// the answer is no frame rather than an empty one.
    pub(crate) fn is_empty(&self) -> bool {
        self.whole.is_empty() && self.placed.is_empty() && self.writing().next().is_none()
    }
}

/// What a row let go of weighs: its call's line, its call, and the place.
fn weighing(called: &str, call: &ToolId) -> usize {
    called.len() + call.as_str().len() + size_of::<Placed>()
}

/// Adds `one` at the back of a queue bounded by [`UNCLAIMED`], letting the
/// oldest go.
fn waiting<T>(queue: &mut VecDeque<T>, one: T) {
    queue.push_back(one);
    while queue.len() > UNCLAIMED {
        queue.pop_front();
    }
}

#[cfg(test)]
mod tests;
