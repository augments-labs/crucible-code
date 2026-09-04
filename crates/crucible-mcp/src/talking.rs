//! One conversation with a server, over the pipes something else opened.
//!
//! Sequential, deliberately. MCP allows a client to have several calls in
//! flight, and crucible never does: it asks a server for a catalogue, or for
//! one tool's result, and has nothing to do until the answer arrives. A queue
//! of outstanding calls would be state to keep, an ordering to reason about and
//! a way for a server to hold work open, in exchange for a concurrency crucible
//! has no caller for.
//!
//! Whether a call reached the server is the one thing a failure here has to be
//! precise about, because it is what decides whether asking again is safe.
//! A frame that never left crucible left the far end untouched; anything that
//! goes wrong after it has gone could be a tool that already ran. So the two
//! are separate answers rather than one error a caller has to guess at — see
//! [`Trouble::outstanding`].
//!
//! Everything the server says while crucible is waiting is dealt with here
//! rather than handed up. A notification is dropped, because crucible
//! subscribes to nothing; a question is refused, because crucible offers a
//! server nothing to call back into and a server left waiting on an answer is a
//! server not getting on with the request it was given. Both are counted: a
//! server that says anything else forever is a server crucible would wait on
//! forever, so the waiting is bounded by how much it will listen to and not by
//! a clock alone.

use std::io::{self, BufRead, Write};

use crucible_core::{FrameError, Frames, Written};
use serde_json::Value;

use crate::wire::{Call, Garbled, Heard, Reply, Sent};

/// The most frames crucible will read past while waiting on one answer.
///
/// A server has reason to send a few — progress on the call being waited for,
/// a log line, a question crucible then refuses. It has no reason to send
/// hundreds, and a bound is what turns "this server is chatty" into a sentence
/// rather than a session that never comes back.
pub const ASIDES: usize = 64;

/// Why a call did not come back.
#[derive(Debug, thiserror::Error)]
pub enum Trouble {
    /// The frame crucible was writing did not go.
    ///
    /// Its own variant because it is the only failure here that can leave the
    /// far end exactly as it was. Every other one happened after crucible let
    /// go of the request, and from this side a tool that never started, one
    /// that ran, and one that ran and lost its answer look the same.
    ///
    /// Can, not does: a write that ran out of patience left the bytes with the
    /// thread that owns the pipe, and they may yet be read. Which of the two
    /// this is, is [`FrameError::never_left`]'s answer and not the variant's.
    #[error("crucible could not send {method} to the server: {source}")]
    Unsent {
        /// What it was going to say.
        method: Box<str>,
        /// Why the frame did not go.
        source: FrameError,
    },

    /// The pipe broke, or a frame ran past its ceiling.
    #[error(transparent)]
    Frame(#[from] FrameError),

    /// The server sent something that is not a message crucible can act on.
    #[error(transparent)]
    Garbled(#[from] Garbled),

    /// The server stopped before answering.
    #[error("the server stopped without answering call {call}")]
    Stopped {
        /// What was still outstanding.
        call: Call,
    },

    /// The server answered, and the answer was a failure.
    ///
    /// Not an error of crucible's making, and kept whole: the code is what a
    /// caller decides on and the words are the server's own.
    #[error("the server refused call {call}: {said} ({code})")]
    Refused {
        /// Which call.
        call: Call,
        /// The code it gave.
        code: i64,
        /// The words it gave.
        said: Box<str>,
    },

    /// The server settled a call crucible was not waiting on.
    ///
    /// Refused rather than ignored. Crucible has one call outstanding at a
    /// time, so an answer to a different one means the two ends disagree about
    /// what has been asked, and reading on would be matching later answers to
    /// the wrong questions.
    #[error("the server answered call {found} while crucible was waiting on {call}")]
    Astray {
        /// What crucible asked.
        call: Call,
        /// What came back.
        found: Call,
    },

    /// The server said more than crucible will listen to while waiting.
    #[error("the server sent more than {most} frames without answering call {call}")]
    Talkative {
        /// Which call was outstanding.
        call: Call,
        /// How many frames were read past.
        most: usize,
    },
}

impl Trouble {
    /// Whether the server may have acted on the call.
    ///
    /// True for everything except a frame that can be proven never to have
    /// reached the far end, which is the honest shape of it: crucible can prove
    /// it did not ask, and can prove nothing about what happened once it did. A
    /// caller deciding whether to ask again reads this, and a `false` is the
    /// only answer that makes repeating the request a repeat rather than a
    /// second one.
    ///
    /// So a write that did not go is not enough on its own. A frame crucible
    /// stopped waiting for the pipe to take is a frame the pipe may still take,
    /// and counting that as unasked would be crucible asking twice for the one
    /// ending where it cannot tell.
    #[must_use]
    pub fn outstanding(&self) -> bool {
        match self {
            Self::Unsent { source, .. } => !source.never_left(),
            _ => true,
        }
    }

    /// Whether the conversation can carry another call.
    ///
    /// Only a server that answered leaves one that can: a refusal is the far
    /// end settling the call and going back to waiting for the next. Every
    /// other ending here left the two sides disagreeing about what has been
    /// asked, or left nothing on the other side at all, and a second call over
    /// the same pipes would be read against the wrong question.
    #[must_use]
    pub const fn settled(&self) -> bool {
        matches!(self, Self::Refused { .. })
    }

    /// Whether the waiting ended because crucible was asked to stop.
    ///
    /// Not a slow server and not a broken pipe: the far end was doing what it
    /// was told and is simply no longer wanted. It reads the kind rather than
    /// the words because the words come from the operating system and the kind
    /// is the part crucible set.
    #[must_use]
    pub fn interrupted(&self) -> bool {
        matches!(
            self,
            Self::Frame(FrameError::Unreadable { source })
                if source.kind() == io::ErrorKind::ConnectionAborted
        )
    }
}

/// A server, spoken to over a reader and a writer.
///
/// It takes pipes rather than starting a process. What to run, under what
/// confinement and with what environment is a lifecycle with its own owner, and
/// a conversation that reached into it would be deciding sandbox policy on the
/// way past.
#[derive(Debug)]
pub struct Talking<R, W> {
    /// What the server says.
    heard: Frames<R>,
    /// What crucible says.
    said: Written<W>,
    /// The number the next call gets.
    next: u64,
}

impl<R: BufRead, W: Write> Talking<R, W> {
    /// Speaks over `from` and `to`.
    #[must_use]
    pub const fn new(from: R, to: W) -> Self {
        Self {
            heard: Frames::new(from),
            said: Written::new(to),
            next: 1,
        }
    }

    /// The two streams this runs over, for what only they can be asked.
    pub const fn streams_mut(&mut self) -> (&mut R, &mut W) {
        (self.heard.stream_mut(), self.said.stream_mut())
    }

    /// Asks the server something and waits for its answer.
    ///
    /// # Errors
    ///
    /// [`Trouble`] where the frame will not go, the pipe fails, the server
    /// sends a frame crucible cannot act on, it answers with a failure, it
    /// answers a call crucible was not waiting on, it says more than [`ASIDES`]
    /// frames without answering, or it stops first.
    pub fn ask(&mut self, method: &str, params: &Value) -> Result<Value, Trouble> {
        let call = Call::new(self.next);
        self.next = self.next.saturating_add(1);
        self.said
            .send(&Sent::asking(call, method, params).frame())
            .map_err(|source| Trouble::Unsent {
                method: method.into(),
                source,
            })?;
        self.wait(call)
    }

    /// Tells the server something that expects nothing back.
    ///
    /// # Errors
    ///
    /// [`Trouble::Unsent`] where the frame could not be sent. There is nothing
    /// else it can be: a message that expects no answer has nothing to go
    /// wrong after it has gone.
    pub fn tell(&mut self, method: &str, params: &Value) -> Result<(), Trouble> {
        self.said
            .send(&Sent::telling(method, params).frame())
            .map_err(|source| Trouble::Unsent {
                method: method.into(),
                source,
            })?;
        Ok(())
    }

    /// Reads until `call` is settled, dealing with whatever else arrives.
    fn wait(&mut self, call: Call) -> Result<Value, Trouble> {
        for _ in 0..=ASIDES {
            let Some(frame) = self.heard.next_frame() else {
                return Err(Trouble::Stopped { call });
            };
            match Heard::read(&frame?)? {
                Heard::Answer { call: found, .. } if found != call => {
                    return Err(Trouble::Astray { call, found });
                }
                Heard::Answer {
                    reply: Reply::Worked(result),
                    ..
                } => return Ok(result),
                Heard::Answer {
                    reply: Reply::Failed { code, said },
                    ..
                } => return Err(Trouble::Refused { call, code, said }),
                Heard::Asked {
                    call: asked,
                    method,
                } => self.said.send(&Sent::refusing(asked, &method).frame())?,
                Heard::Told { .. } => {}
            }
        }
        Err(Trouble::Talkative { call, most: ASIDES })
    }
}

#[cfg(test)]
mod tests;
