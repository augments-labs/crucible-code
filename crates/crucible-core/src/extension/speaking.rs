//! Driving one conversation over a pair of pipes.
//!
//! [`Conversation`] decides what a frame means once it has been read. This
//! reads them, sends what crucible owes back, and settles the question that
//! neither neighbour answers: what happens when a frame cannot be read or
//! understood at all.
//!
//! The answer is that it ends the conversation, and the reason is that there is
//! nobody to tell. Every refusal crucible can send is an answer, and an answer
//! names a call — but a frame that is not readable JSON has no identifier to
//! name, so a refusal would be crucible guessing which call the extension
//! thought it was making. A far end that cannot frame what it says is not a far
//! end crucible can keep talking to.
//!
//! Still no processes here. `R` and `W` are any reader and writer, so every way
//! a conversation can end is reachable from a test with two byte buffers.

use std::io::{BufRead, Write};

use serde_json::Value;

use super::calls::CallError;
use super::conversation::{Broken, Conversation, Next};
use super::spoken::{CallId, Outcome, Spoken, SpokenError};
use super::wire::{FrameError, Frames, Written};

/// Why a conversation is over.
#[derive(Debug, thiserror::Error)]
pub enum Over {
    /// The extension closed its output with nothing in progress.
    ///
    /// The ordinary ending, and the only one that is nobody's fault.
    #[error("the extension stopped speaking")]
    Silent,

    /// A frame could not be read.
    #[error("the extension's output could not be read: {source}")]
    Unreadable {
        /// What the wire said.
        #[source]
        source: FrameError,
    },

    /// A frame was read but could not be understood.
    #[error("the extension said something crucible cannot read: {source}")]
    Misspoken {
        /// What was wrong with it.
        #[source]
        source: SpokenError,
    },

    /// The extension and crucible disagree about which calls exist.
    #[error("the conversation broke: {source}")]
    Broke {
        /// Which disagreement.
        #[source]
        source: Broken,
    },

    /// Crucible could not send.
    #[error("crucible could not answer the extension: {source}")]
    Unanswerable {
        /// What the wire said.
        #[source]
        source: FrameError,
    },

    /// This conversation ended already, and the first ending said why.
    ///
    /// Every other ending leaves the reader without a boundary it trusts or
    /// the two ends disagreeing about which calls exist. Neither is something
    /// the bytes after it can settle, so asking for another turn gets this
    /// rather than whatever the extension went on to say.
    #[error("this conversation is already over")]
    Finished,
}

/// What one turn of the conversation produced.
#[derive(Debug, PartialEq, Eq)]
pub enum Turn<T> {
    /// A call crucible made was answered.
    Answer {
        /// What was remembered when the call was made.
        waiting: T,
        /// How it went.
        outcome: Outcome,
    },

    /// The extension is asking for something and is owed one answer.
    Asked {
        /// Which call to answer.
        id: CallId,
        /// What is being asked for.
        method: Box<str>,
        /// What rides with it, still unread.
        params: Value,
    },

    /// The extension said something that expects nothing back.
    Told {
        /// What happened.
        method: Box<str>,
        /// What rides with it, still unread.
        params: Value,
    },
}

/// One extension, spoken to over a reader and a writer.
#[derive(Debug)]
pub struct Speaking<R, W, T> {
    /// What the extension says.
    frames: Frames<R>,
    /// What crucible says.
    to: Written<W>,
    /// Which calls are in flight.
    talk: Conversation<T>,
    /// Whether an ending has already been reported.
    over: bool,
}

impl<R: BufRead, W: Write, T> Speaking<R, W, T> {
    /// Speaks to an extension that reads from `to` and writes to `from`.
    #[must_use]
    pub const fn new(from: R, to: W) -> Self {
        Self {
            frames: Frames::new(from),
            to: Written::new(to),
            talk: Conversation::new(),
            over: false,
        }
    }

    /// Everything crucible has sent, where the far end is a test's buffer.
    #[cfg(test)]
    pub(crate) const fn written(&self) -> &W {
        self.to.sent()
    }

    /// The next thing the extension said that the host has to act on.
    ///
    /// Refusals are sent from here rather than handed back, because a refusal
    /// is crucible's own word about a call it declined to take on and there is
    /// nothing for the host to decide about it.
    ///
    /// # Errors
    ///
    /// [`Over`] once there will be nothing further, whether because the
    /// extension stopped, because it said something unreadable, or because the
    /// two ends stopped agreeing about which calls exist. Whatever crucible was
    /// still waiting on is collected with [`Speaking::ended`].
    pub fn turn(&mut self) -> Result<Turn<T>, Over> {
        loop {
            if self.over {
                return Err(Over::Finished);
            }
            match self.next() {
                // A refusal is crucible's own word about a call it declined,
                // and a late answer is one it gave up on. Both go no further:
                // there is nothing about a call crucible did not take on, or
                // has already reported, for the host to decide.
                Ok(None) => {}
                Ok(Some(turn)) => return Ok(turn),
                Err(over) => {
                    self.over = true;
                    return Err(over);
                }
            }
        }
    }

    /// One frame, which is a turn, a refusal crucible has now sent, or an end.
    fn next(&mut self) -> Result<Option<Turn<T>>, Over> {
        let frame = self
            .frames
            .next_frame()
            .ok_or(Over::Silent)?
            .map_err(|source| Over::Unreadable { source })?;
        let spoken = Spoken::read(&frame).map_err(|source| Over::Misspoken { source })?;
        match self.talk.heard(spoken) {
            Next::Answer { waiting, outcome } => Ok(Some(Turn::Answer { waiting, outcome })),
            Next::Asked { id, method, params } => Ok(Some(Turn::Asked { id, method, params })),
            Next::Told { method, params } => Ok(Some(Turn::Told { method, params })),
            Next::Late { .. } => Ok(None),
            Next::Refuse(refusal) => self.send(&refusal).map(|()| None),
            Next::Stop(source) => Err(Over::Broke { source }),
        }
    }

    /// Puts one thing crucible said on the wire.
    fn send(&mut self, spoken: &Spoken) -> Result<(), Over> {
        self.to
            .send(&spoken.written())
            .map_err(|source| Over::Unanswerable { source })
    }

    /// Starts a call of crucible's own and sends it.
    ///
    /// # Errors
    ///
    /// [`CallError`] where crucible is already waiting on as many calls as it
    /// allows, and [`Over::Unanswerable`] where the frame could not be sent.
    pub fn ask(
        &mut self,
        method: impl Into<Box<str>>,
        params: Value,
        about: T,
    ) -> Result<CallId, Asking> {
        if self.over {
            return Err(Asking::Over(Over::Finished));
        }
        let (id, spoken) = self
            .talk
            .ask(method, params, about)
            .map_err(Asking::Refused)?;
        self.said(&spoken)?;
        Ok(id)
    }

    /// Answers a call the extension made and sends it.
    ///
    /// # Errors
    ///
    /// [`CallError::Unknown`] where that is not a call crucible took on, and
    /// [`Over::Unanswerable`] where the frame could not be sent.
    pub fn answer(&mut self, id: CallId, outcome: Outcome) -> Result<(), Asking> {
        if self.over {
            return Err(Asking::Over(Over::Finished));
        }
        let spoken = self.talk.answer(id, outcome).map_err(Asking::Refused)?;
        self.said(&spoken)
    }

    /// Stops waiting on a call crucible made, handing back what it remembered.
    ///
    /// Nothing goes on the wire: the extension is not told, and an answer that
    /// arrives afterwards is recognised and dropped. Once the conversation is
    /// over this is refused, because by then [`Speaking::ended`] has handed
    /// back everything that was outstanding and giving up again would be a
    /// second final answer for one call.
    ///
    /// # Errors
    ///
    /// [`Asking::Refused`] where that is not a call crucible is waiting on,
    /// and [`Asking::Over`] once the conversation has ended.
    pub fn give_up(&mut self, id: CallId) -> Result<T, Asking> {
        if self.over {
            return Err(Asking::Over(Over::Finished));
        }
        self.talk.give_up(id).map_err(Asking::Refused)
    }

    /// Sends what crucible owes, ending the conversation where it cannot.
    ///
    /// A frame that could not go out leaves the far end waiting on something it
    /// will never hear, so there is no state in which carrying on is honest.
    fn said(&mut self, spoken: &Spoken) -> Result<(), Asking> {
        self.send(spoken).map_err(|over| {
            self.over = true;
            Asking::Over(over)
        })
    }

    /// Everything crucible was still waiting on, now that nothing will answer.
    pub fn ended(&mut self) -> Vec<(CallId, T)> {
        self.talk.ended()
    }
}

/// Why crucible could not put a call of its own on the wire.
#[derive(Debug, thiserror::Error)]
pub enum Asking {
    /// The call could not be started.
    #[error("{0}")]
    Refused(#[source] CallError),

    /// The call was started but could not be sent, so the conversation is over.
    #[error("{0}")]
    Over(#[source] Over),
}

#[cfg(test)]
mod tests;
