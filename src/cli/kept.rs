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
//! nothing: the rows of the old one are still in the terminal's scrollback,
//! still saying how many lines they could not fit and still naming the key that
//! gives them back. Emptying this there would leave those offers on screen with
//! nothing behind them, which is the one thing worse than not making them.
//!
//! One kind of cut is not here and cannot be. A call that changed a file is
//! shown as the change itself, and a change too long for the block is cut down
//! where the change is built rather than where it is drawn — those lines never
//! reach this process's drawing thread, so there is nothing held back from the
//! reader to hand over. The row says how many went, which is the whole of what
//! is still true about them.

use std::collections::VecDeque;

/// The most text held at once, in bytes.
///
/// Half a mebibyte, against a process budgeted 35. One result is bounded far
/// below this by the tools themselves, so the ceiling is really on how many
/// results stay reachable — enough that a reader who scrolled back through a
/// long turn finds what they are looking at still expandable, and small enough
/// that the answer to "what does this cost" is a fraction of one turn's
/// transcript.
const HELD: usize = 512 * 1024;

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
    /// The line of the call whose result has not arrived yet.
    ///
    /// A result says which call it answers by an identifier, and the reader
    /// knows the call by the line that was committed for it. The two meet here,
    /// one event apart, the same way they do on screen.
    calling: Option<String>,
}

impl Kept {
    /// Remembers the line of the call whose result comes next.
    pub(crate) fn calling(&mut self, called: String) {
        self.calling = Some(called);
    }

    /// Keeps what a row could not say.
    ///
    /// Called only where the row said less than the result did — a result that
    /// fitted is on screen already, and offering to expand it would be offering
    /// the reader what they are looking at.
    pub(crate) fn finished(&mut self, text: Box<str>) {
        let called = self.calling.take().unwrap_or_default();

        self.held = self.held.saturating_add(text.len());
        self.whole.push_back(Whole { called, text });

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

    /// Everything still reachable, newest first.
    ///
    /// Newest first because that is the order a reader is looking for them in:
    /// the result somebody wants to see is almost always the one that just went
    /// past.
    pub(crate) fn newest(&self) -> impl Iterator<Item = &Whole> {
        self.whole.iter().rev()
    }

    /// Whether nothing has been cut.
    ///
    /// Which is the whole of what the key asking for this has to know: with
    /// nothing held there was no offer on screen to have prompted it, so the
    /// answer is no frame rather than an empty one.
    pub(crate) fn is_empty(&self) -> bool {
        self.whole.is_empty()
    }
}

#[cfg(test)]
mod tests;
