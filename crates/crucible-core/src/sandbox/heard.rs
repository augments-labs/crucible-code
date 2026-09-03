//! What a confined process has said, as something a frame reader can read.
//!
//! A sandbox hands out its streams as [`SandboxOutput`], which answers with
//! whatever is available and never waits, and the frame reader wants a
//! [`BufRead`], which waits. Something has to hold the difference, and this is
//! it: the waiting, and the two answers a non-blocking stream can give that a
//! blocking one has no way to express.
//!
//! Neither of those two is an ordinary read. *Nothing yet* is what a program
//! thinking looks like, so it cannot be an ending — but it is also what a
//! program that has wedged looks like, and nothing here can tell them apart,
//! so crucible spends a patience on it and then says so. *Bytes were dropped*
//! is worse than a short read: the bytes that went past the output ceiling
//! included the newline somebody was going to use as a boundary, so the stream
//! is not shorter, it is unframeable.

use std::fmt;
use std::io::{self, BufRead, Read};
use std::thread;
use std::time::{Duration, Instant};

use crate::{SandboxOutput, SandboxRead};

/// How long crucible waits before asking a quiet stream again.
///
/// A trade with nothing clever in it: a wasted wake-up on one side, and a frame
/// sitting in a pipe nobody has asked about on the other.
const PAUSE: Duration = Duration::from_millis(5);

/// How much of one read is taken at a time.
///
/// Not a ceiling on anything. A frame's ceiling is the frame reader's, which
/// holds however many of these it takes; this is only how much of a pipe is
/// moved per call.
const CHUNK: usize = 8 * 1024;

/// A confined stream, read as bytes that arrive rather than bytes that are
/// ready.
pub struct Heard<O> {
    /// The stream, which answers with what is available and never waits.
    output: O,
    /// How long a silence crucible sits through before giving up.
    patience: Duration,
    /// How long it waits between asking.
    pause: Duration,
    /// The most recent bytes, held until they have been consumed.
    held: Vec<u8>,
    /// How much of them has been.
    at: usize,
}

impl<O> Heard<O> {
    /// Reads what `output` says, giving up after `patience` of silence.
    ///
    /// The patience is spent on one silence and handed back whenever anything
    /// arrives. A budget for the whole conversation would end a program for
    /// having been useful for longer than crucible guessed it would be.
    #[must_use]
    pub const fn new(output: O, patience: Duration) -> Self {
        Self::with_pause(output, patience, PAUSE)
    }

    /// The same, with the pause between polls chosen rather than inherited.
    ///
    /// A test that waits in real milliseconds should wait as few of them as it
    /// can, and nothing outside this crate has a reason to pick a different
    /// number from the one above.
    pub(crate) const fn with_pause(output: O, patience: Duration, pause: Duration) -> Self {
        Self {
            output,
            patience,
            pause,
            held: Vec::new(),
            at: 0,
        }
    }
}

impl<O> fmt::Debug for Heard<O> {
    /// Without the stream, which is a pipe and has nothing to show.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Heard")
            .field("patience", &self.patience)
            .field("pause", &self.pause)
            .field("held", &self.held.len())
            .field("at", &self.at)
            .finish_non_exhaustive()
    }
}

impl<O: SandboxOutput> Heard<O> {
    /// Waits until something arrives, the stream ends, or the patience is out.
    ///
    /// Leaving [`Self::held`] empty is how the end of the stream is said, which
    /// is what [`BufRead`] means by an empty fill.
    fn hear(&mut self) -> io::Result<()> {
        self.held.clear();
        self.at = 0;
        let mut buffer = [0_u8; CHUNK];
        let began = Instant::now();
        loop {
            match self.output.read_ready(&mut buffer)? {
                // Zero bytes is not an ending. A stream saying it has nothing
                // is the same thing whether it says so with a count or a word.
                SandboxRead::Bytes(0) | SandboxRead::Pending => {
                    if began.elapsed() >= self.patience {
                        return Err(silent(self.patience));
                    }
                    thread::sleep(self.pause);
                }
                SandboxRead::Bytes(count) => {
                    if let Some(bytes) = buffer.get(..count) {
                        self.held.extend_from_slice(bytes);
                    }
                    return Ok(());
                }
                // The retained prefix is dropped along with everything else.
                // It is bytes crucible could still read, but the conversation
                // ends on this error either way, so handing them up would only
                // mean a caller finding one more frame on the way out.
                SandboxRead::Limited { discarded, .. } => return Err(lost(discarded)),
                SandboxRead::End => return Ok(()),
            }
        }
    }
}

impl<O: SandboxOutput> Read for Heard<O> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let arrived = self.fill_buf()?;
        let taken = arrived.len().min(buffer.len());
        if let Some((into, from)) = buffer.get_mut(..taken).zip(arrived.get(..taken)) {
            into.copy_from_slice(from);
        }
        self.consume(taken);
        Ok(taken)
    }
}

impl<O: SandboxOutput> BufRead for Heard<O> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.at >= self.held.len() {
            self.hear()?;
        }
        Ok(self.held.get(self.at..).unwrap_or_default())
    }

    fn consume(&mut self, amount: usize) {
        self.at = self.at.saturating_add(amount).min(self.held.len());
    }
}

/// A peer that has stopped saying anything at all.
fn silent(patience: Duration) -> io::Error {
    io::Error::new(
        io::ErrorKind::TimedOut,
        format!("the confined program said nothing for {patience:?}"),
    )
}

/// A stream that lost bytes to the output ceiling, and a boundary with them.
fn lost(discarded: usize) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "the confined program's output passed its ceiling, so {discarded} bytes \
             are missing from the middle of what it said"
        ),
    )
}

#[cfg(test)]
mod tests;
