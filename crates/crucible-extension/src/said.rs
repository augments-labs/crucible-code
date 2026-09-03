//! What crucible says to a confined process, as something a frame writer can
//! write.
//!
//! The mirror of [`Heard`](crate::Heard), and it exists for the same reason
//! pointed the other way. A frame writer wants a [`Write`] whose failures are
//! errors, and a pipe into a confined process is a [`Write`] whose failure mode
//! is not an error at all: it stops taking bytes and the caller waits. A peer
//! that stopped reading and a peer that is thinking look identical from this
//! side, so crucible spends a patience on one frame and then says the peer is
//! gone — the same trade, and the same admission that nothing here can tell
//! them apart.
//!
//! The patience cannot be spent on the pipe directly. Standard input arrives as
//! an opaque writer whose blocking write is a syscall with no deadline in it, so
//! the writer moves onto a thread of its own and the deadline is spent waiting
//! for that thread to report back. A frame given up on is still in that thread's
//! hands, which is why nothing further is said afterwards: those bytes may yet
//! land, and a later frame would arrive joined to the one crucible already
//! called undelivered.
//!
//! Bytes are held until the frame is whole. A pipe takes whatever it is given,
//! so writing a frame in pieces puts a fragment in front of the far end that it
//! reads joined to whatever comes next; holding until the newline is what makes
//! a frame arrive as one thing or not at all.

use std::fmt;
use std::io::{self, ErrorKind, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use crucible_core::EXTENSION_FRAME_BYTES;

/// The most bytes one unfinished frame may hold before it is handed over.
///
/// A frame and the newline that ends it. A frame writer already refuses
/// anything longer before a byte is written; this is that same ceiling standing
/// where the bytes are actually retained, so a caller that never ends a frame
/// cannot grow this buffer instead.
const HELD: usize = EXTENSION_FRAME_BYTES + 1;

/// What crucible says to a confined process, bounded in bytes and in patience.
pub struct Said {
    /// The frame being written, held until it is whole.
    held: Vec<u8>,
    /// Where a whole frame is handed to the thread that owns the pipe.
    to: Sender<Vec<u8>>,
    /// What that thread says once the bytes have gone.
    done: Receiver<io::Result<()>>,
    /// How long crucible waits for one frame to be taken.
    patience: Duration,
    /// Whether an ending has been reached already.
    stopped: bool,
}

impl Said {
    /// Says what crucible owes to `to`, giving up on one frame after
    /// `patience`.
    ///
    /// The writer moves onto a thread that lives as long as this value or as
    /// long as one blocked write, whichever is longer. A thread still parked in
    /// a write when this is dropped goes away when the pipe does, which is what
    /// stopping the process it belongs to does.
    #[must_use]
    pub fn new<W: Write + Send + 'static>(mut to: W, patience: Duration) -> Self {
        let (frames, waiting) = mpsc::channel::<Vec<u8>>();
        let (spoke, done) = mpsc::channel::<io::Result<()>>();
        thread::spawn(move || {
            while let Ok(frame) = waiting.recv() {
                let gone = to.write_all(&frame).and_then(|()| to.flush());
                let failed = gone.is_err();
                // A pipe that has answered once answers the same way forever,
                // and nobody is left to tell if the report could not be sent.
                if spoke.send(gone).is_err() || failed {
                    break;
                }
            }
        });
        Self {
            held: Vec::new(),
            to: frames,
            done,
            patience,
            stopped: false,
        }
    }

    /// Hands the whole frame over and waits for the pipe to have taken it.
    fn hand_over(&mut self) -> io::Result<()> {
        let frame = std::mem::take(&mut self.held);
        if self.to.send(frame).is_err() {
            return Err(closed());
        }
        match self.done.recv_timeout(self.patience) {
            Ok(gone) => gone,
            Err(RecvTimeoutError::Timeout) => Err(deaf(self.patience)),
            Err(RecvTimeoutError::Disconnected) => Err(closed()),
        }
    }
}

impl fmt::Debug for Said {
    /// Without the pipe, which is on another thread and has nothing to show.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Said")
            .field("patience", &self.patience)
            .field("held", &self.held.len())
            .field("stopped", &self.stopped)
            .finish_non_exhaustive()
    }
}

impl Write for Said {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.stopped {
            return Err(over());
        }
        if self.held.len().saturating_add(bytes.len()) > HELD {
            // The part already held can never be finished now, and sending it
            // would put a fragment in front of the far end.
            self.stopped = true;
            return Err(unbounded());
        }
        self.held.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    /// Nothing reaches the pipe until here, which is what makes a frame arrive
    /// whole.
    fn flush(&mut self) -> io::Result<()> {
        if self.stopped {
            return Err(over());
        }
        if self.held.is_empty() {
            return Ok(());
        }
        self.hand_over().inspect_err(|_| {
            self.stopped = true;
        })
    }
}

/// A peer that has stopped taking what crucible says.
fn deaf(patience: Duration) -> io::Error {
    io::Error::new(
        ErrorKind::TimedOut,
        format!("the extension stopped reading for {patience:?}"),
    )
}

/// A pipe with nobody left on the other end of it.
fn closed() -> io::Error {
    io::Error::new(ErrorKind::BrokenPipe, "the extension's input is closed")
}

/// A conversation crucible has already stopped holding up its end of.
fn over() -> io::Error {
    io::Error::new(
        ErrorKind::BrokenPipe,
        "crucible already stopped speaking to this extension",
    )
}

/// A frame that grew past what one frame is allowed to be.
fn unbounded() -> io::Error {
    io::Error::new(
        ErrorKind::InvalidInput,
        format!("crucible tried to say more than {HELD} bytes without ending a frame"),
    )
}

#[cfg(test)]
mod tests;
