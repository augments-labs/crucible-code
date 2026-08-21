//! What a result said that its row in the transcript had no room for.
//!
//! A tool answers in as much text as it likes and the transcript gives it one
//! row, so most of what came back is on screen only as a count of the lines
//! that are not. This is where those lines wait for a reader who asks to see
//! them, and asking is [`Pressed::Expand`](crucible_tui::Pressed::Expand).
//!
//! Nothing is copied to put it here. The drawing thread is handed its own
//! `ToolOutput` and drops it once the row is drawn; what this holds is that
//! value's text, moved rather than cloned, which is why keeping it costs the
//! process nothing it was not already spending for the length of one event.
//!
//! It is bounded, because a session is not. [`HELD`] is the ceiling
//! on how much is held at once and the oldest result is dropped to stay under
//! it, so what this costs is the same after four hundred turns as after four —
//! the rule the whole renderer is built to keep.
//!
//! It outlives a session, and has to. `/clear` opens a new one and deletes
//! nothing: the rows of the old one are still in the transcript, still saying
//! how many lines they could not fit and still naming the key that gives them
//! back. Emptying this there would leave those offers on screen with nothing
//! behind them, which is the one thing worse than not making them.
//!
//! One kind of cut is not here and cannot be. A call that changed a file is
//! shown as the change itself, and a change too long for the block is cut down
//! where the change is built rather than where it is drawn — those lines never
//! reach this process's drawing thread, so there is nothing held back from the
//! reader to hand over. The row says how many went, which is the whole of what
//! is still true about them.

use std::collections::VecDeque;

use crucible_core::ToolId;

/// The most text held at once, in bytes.
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
}

/// Every result still reachable, oldest first.
#[derive(Debug, Default)]
pub(crate) struct Kept {
    /// What is held, oldest at the front because that is the end that gives.
    whole: VecDeque<Whole>,
    /// How many bytes of text `whole` holds, added up.
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
}

/// One call that has not answered yet.
#[derive(Debug)]
struct Pending {
    id: ToolId,
    called: String,
    writing: Option<Whole>,
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
            });
        }
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
        let called = self.take(call).map_or_else(String::new, |one| one.called);

        self.cut = self.cut.saturating_add(1);
        self.held = self.held.saturating_add(text.len());
        self.whole.push_back(Whole {
            called,
            text,
            at: Some(at),
        });

        // After the push rather than before it, so that the newest result is
        // held whatever it costs. One longer than the ceiling on its own would
        // otherwise be the one thing a reader could never see, and it is the
        // one they are most likely to be asking about.
        while self.held > HELD && self.whole.len() > 1 {
            if let Some(gone) = self.whole.pop_front() {
                self.held = self.held.saturating_sub(gone.text.len());
            }
        }
    }

    /// Forgets a call whose complete result fitted on screen.
    pub(crate) fn answered(&mut self, call: &ToolId) {
        self.take(call);
    }

    /// Forgets a call that reached a terminal turn event without a result.
    pub(crate) fn abandoned(&mut self, call: &ToolId) {
        self.take(call);
    }

    /// Takes one pending call by identity without disturbing its neighbours.
    fn take(&mut self, call: &ToolId) -> Option<Pending> {
        self.pending
            .iter()
            .position(|one| one.id == *call)
            .and_then(|at| self.pending.remove(at))
    }

    /// Everything still reachable, newest first.
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
    /// nothing held there was no offer on screen to have prompted it, so the
    /// answer is no frame rather than an empty one.
    pub(crate) fn is_empty(&self) -> bool {
        self.whole.is_empty() && self.writing().next().is_none()
    }
}

#[cfg(test)]
mod tests;
