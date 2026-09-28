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
//! Still no processes here. `R` and `W` are any reader and writer that can be
//! awaited, so every way a conversation can end is reachable from a test with
//! two byte buffers.
//!
//! Everything that waits here is awaited, and every wait can be given up on by
//! dropping it. Giving up on a read loses nothing: what had arrived of a frame
//! stays with the reader for the next turn. Giving up on a send is different,
//! because a frame dropped partway leaves its first half on the wire for the
//! next one to be joined to, so a conversation whose send was given up on is
//! over, and says so to whatever is asked of it next.

use serde_json::Value;
use tokio::io::{AsyncBufRead, AsyncWrite};

use crate::calls::{Call, CallError, Generation};
use crate::conversation::{Broken, Conversation, Next};
use crate::spoken::{Outcome, Spoken, SpokenError};
use crucible_transport::{FrameError, Frames, Written};

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

    /// This conversation ended already: the first ending said why, or it was
    /// a frame crucible was sending when the send was given up on.
    ///
    /// Every other ending leaves the reader without a boundary it trusts or
    /// the two ends disagreeing about which calls exist, and a send given up on
    /// leaves the far end reading a frame that never finished. None of those is
    /// something the bytes after it can settle, so asking for another turn gets
    /// this rather than whatever the extension went on to say.
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
        /// Which call to answer, in this conversation's generation.
        id: Call,
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
    /// Whether an ending has already been reported, or a send is unfinished.
    over: bool,
}

impl<R: AsyncBufRead + Unpin, W: AsyncWrite + Unpin, T> Speaking<R, W, T> {
    /// Speaks to `generation` of an extension that reads from `to` and writes
    /// to `from`.
    #[must_use]
    pub(crate) const fn new(from: R, to: W, generation: Generation) -> Self {
        Self {
            frames: Frames::new(from),
            to: Written::new(to),
            talk: Conversation::new(generation),
            over: false,
        }
    }

    /// The reader underneath, for what only it can be asked: how long a wait
    /// on it may be and where one exchange ends.
    pub(crate) const fn heard_mut(&mut self) -> &mut R {
        self.frames.stream_mut()
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
    ///
    /// # Cancel safety
    ///
    /// Dropped while it waits for the extension, it loses nothing, provided
    /// `R` loses nothing when a fill is dropped: the next turn carries on from
    /// whatever had arrived. Dropped while it sends a refusal, it leaves the
    /// conversation over, because part of that frame may already be on the
    /// wire.
    pub async fn turn(&mut self) -> Result<Turn<T>, Over> {
        loop {
            if self.over {
                return Err(Over::Finished);
            }
            match self.next().await {
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
    async fn next(&mut self) -> Result<Option<Turn<T>>, Over> {
        let frame = self
            .frames
            .next_frame_async()
            .await
            .ok_or(Over::Silent)?
            .map_err(|source| Over::Unreadable { source })?;
        let spoken = Spoken::read(&frame).map_err(|source| Over::Misspoken { source })?;
        match self.talk.heard(spoken) {
            Next::Answer { waiting, outcome } => Ok(Some(Turn::Answer { waiting, outcome })),
            Next::Asked { id, method, params } => Ok(Some(Turn::Asked { id, method, params })),
            Next::Told { method, params } => Ok(Some(Turn::Told { method, params })),
            Next::Late { .. } => Ok(None),
            Next::Refuse(refusal) => self.send(&refusal).await.map(|()| None),
            Next::Stop(source) => Err(Over::Broke { source }),
        }
    }

    /// Puts one thing crucible said on the wire.
    ///
    /// The conversation counts as over until the whole frame has gone, so a
    /// send dropped partway leaves it over rather than leaving the next frame
    /// to be read as the end of this one. A frame that could not go out leaves
    /// it over too: the far end is waiting on something it will never hear, so
    /// there is no state in which carrying on is honest.
    async fn send(&mut self, spoken: &Spoken) -> Result<(), Over> {
        self.over = true;
        self.to
            .send_async(&spoken.written())
            .await
            .map_err(|source| Over::Unanswerable { source })?;
        self.over = false;
        Ok(())
    }

    /// Starts a call of crucible's own and sends it.
    ///
    /// # Errors
    ///
    /// [`CallError`] where crucible is already waiting on as many calls as it
    /// allows, and [`Over::Unanswerable`] where the frame could not be sent.
    ///
    /// # Cancel safety
    ///
    /// None. The call is taken on before it is sent, so one dropped partway
    /// comes back from [`Speaking::ended`], and the conversation is over.
    pub async fn ask(
        &mut self,
        method: impl Into<Box<str>>,
        params: Value,
        about: T,
    ) -> Result<Call, Asking> {
        if self.over {
            return Err(Asking::Over(Over::Finished));
        }
        let (id, spoken) = self
            .talk
            .ask(method, params, about)
            .map_err(Asking::Refused)?;
        self.send(&spoken).await.map_err(Asking::Over)?;
        Ok(id)
    }

    /// Answers a call the extension made and sends it.
    ///
    /// # Errors
    ///
    /// [`CallError::Unknown`] where that is not a call crucible took on, and
    /// [`Over::Unanswerable`] where the frame could not be sent.
    ///
    /// # Cancel safety
    ///
    /// None. Dropped partway, the call counts as answered and the
    /// conversation is over.
    pub async fn answer(&mut self, call: Call, outcome: Outcome) -> Result<(), Asking> {
        if self.over {
            return Err(Asking::Over(Over::Finished));
        }
        let spoken = self.talk.answer(call, outcome).map_err(Asking::Refused)?;
        self.send(&spoken).await.map_err(Asking::Over)
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
    pub fn give_up(&mut self, call: Call) -> Result<T, Asking> {
        if self.over {
            return Err(Asking::Over(Over::Finished));
        }
        self.talk.give_up(call).map_err(Asking::Refused)
    }

    /// Everything crucible was still waiting on, now that nothing will answer.
    pub fn ended(&mut self) -> Vec<(Call, T)> {
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
