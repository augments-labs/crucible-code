//! The frames an extension and crucible exchange over a pipe.
//!
//! One document per line, which is the whole framing. There is no length prefix
//! because a length is a number the extension writes and crucible would have to
//! believe before it had read anything; a newline is a boundary the reader finds
//! for itself, having read no further than it was willing to read anyway.
//!
//! Everything here reads the far end as hostile. An extension is a program
//! somebody installed and crucible started with their privileges, and this is
//! where its bytes arrive — so a ceiling that holds here is a ceiling on what an
//! extension can make this process hold, whoever wrote it.

use std::io::{self, BufRead, Write};

/// The most bytes one frame may carry, not counting the newline that ends it.
///
/// Past anything the protocol exchanges: a result an extension wanted a person
/// to read is already longer than a person reads at a megabyte. Held all the
/// same, because the number that matters is not what an honest extension sends
/// but what a dishonest one can make crucible keep.
pub const EXTENSION_FRAME_BYTES: usize = 1024 * 1024;

/// Why a frame did not arrive.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The pipe would not read.
    #[error("the extension's output could not be read: {source}")]
    Unreadable {
        /// What the operating system reported.
        #[from]
        source: io::Error,
    },

    /// A frame ran past its ceiling.
    ///
    /// How far past is deliberately not stated: crucible stopped reading at the
    /// ceiling, so it does not know, and a figure it guessed would be a figure
    /// the extension chose.
    #[error("the extension sent more than {maximum} bytes without ending a frame")]
    TooLong {
        /// The ceiling it ran past.
        maximum: usize,
    },

    /// The stream ended partway through a frame.
    #[error("the extension's output ended {seen} bytes into an unfinished frame")]
    Truncated {
        /// How much of it had arrived.
        seen: usize,
    },

    /// A frame was not text.
    #[error("the extension sent a frame that is not UTF-8")]
    NotText,

    /// A frame crucible was about to send carried a boundary of its own.
    ///
    /// Refused rather than escaped. A newline already means one thing here, and
    /// crucible rewriting a byte on its way out would be crucible deciding what
    /// the sender meant by it — while sending it as it stands would let whatever
    /// composed the text choose where crucible's frames end.
    #[error("a frame crucible was about to send contains a newline")]
    Divided,
}

/// The frames arriving from one extension, read one at a time.
///
/// Holds one frame's worth at most. A frame is handed over whole or not at all,
/// because half a document parses into something its author never wrote.
#[derive(Debug)]
pub struct Frames<R> {
    /// Where the bytes come from.
    from: R,
    /// The frame being assembled, never its newline.
    held: Vec<u8>,
    /// Whether this stream has finished, cleanly or otherwise.
    done: bool,
}

impl<R: BufRead> Frames<R> {
    /// Reads frames from `from`.
    #[must_use]
    pub const fn new(from: R) -> Self {
        Self {
            from,
            held: Vec::new(),
            done: false,
        }
    }

    /// The next frame, or nothing once the stream has finished.
    ///
    /// A blank line is not a frame and is skipped: it says nothing, and the
    /// alternative is refusing an extension for a byte that means nothing in
    /// either direction.
    ///
    /// # Errors
    ///
    /// [`FrameError`] where the pipe fails, a frame runs past
    /// [`EXTENSION_FRAME_BYTES`], the stream stops partway through one, or one
    /// arrives that is not text. Every one of those finishes the stream: the
    /// reader has lost its place in a boundary the extension was stating, and
    /// hunting for the next newline would mean reading whatever it sends until
    /// it decides to send one.
    pub fn next_frame(&mut self) -> Option<Result<String, FrameError>> {
        loop {
            match self.frame() {
                Ok(None) => return None,
                // A line with nothing on it, skipped rather than handed up as
                // an empty document for the layer above to be confused by.
                Ok(Some(frame)) if frame.is_empty() => {}
                Ok(Some(frame)) => return Some(Ok(frame)),
                Err(err) => {
                    self.finish();
                    return Some(Err(err));
                }
            }
        }
    }

    /// One line, however many reads it takes to arrive.
    ///
    /// `Ok(None)` is the stream ending where a frame was not in progress, which
    /// is the only clean way for it to end.
    fn frame(&mut self) -> Result<Option<String>, FrameError> {
        if self.done {
            return Ok(None);
        }
        loop {
            let arrived = match self.from.fill_buf() {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err.into()),
            };
            if arrived.is_empty() {
                self.done = true;
                if self.held.is_empty() {
                    return Ok(None);
                }
                let seen = self.held.len();
                self.held = Vec::new();
                return Err(FrameError::Truncated { seen });
            }

            let ended = arrived.iter().position(|byte| *byte == b'\n');
            // The newline is the boundary and never part of what it delimits,
            // so the ceiling is counted over the frame's own bytes.
            let carried = ended.unwrap_or(arrived.len());
            let consumed = ended.map_or(arrived.len(), |at| at.saturating_add(1));
            if carried > EXTENSION_FRAME_BYTES.saturating_sub(self.held.len()) {
                return Err(FrameError::TooLong {
                    maximum: EXTENSION_FRAME_BYTES,
                });
            }
            self.held.extend(arrived.iter().take(carried).copied());
            self.from.consume(consumed);

            if ended.is_some() {
                let bytes = std::mem::take(&mut self.held);
                return String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| FrameError::NotText);
            }
        }
    }

    /// Ends the stream and lets go of whatever was being assembled.
    ///
    /// Every refusal comes through here, because each of them means the reader
    /// no longer knows where a frame starts: the boundary was the extension's
    /// to state, and reading on to find the next newline is reading whatever it
    /// sends until it chooses to send one. What was held goes with it — a
    /// refused frame is not evidence, and keeping it would leave a megabyte
    /// alive for a stream nobody will read again.
    fn finish(&mut self) {
        self.done = true;
        self.held = Vec::new();
    }
}

/// The frames going out to one extension.
///
/// Nothing is buffered between calls. An extension waiting on a request that is
/// sitting in crucible's buffer is a hang with no error and nothing on screen,
/// so a frame is on its way out by the time [`send`](Self::send) returns.
#[derive(Debug)]
pub struct Written<W> {
    /// Where the bytes go.
    to: W,
}

impl<W: Write> Written<W> {
    /// Sends frames to `to`.
    #[must_use]
    pub const fn new(to: W) -> Self {
        Self { to }
    }

    /// Sends one frame.
    ///
    /// # Errors
    ///
    /// [`FrameError`] where the frame carries a newline, runs past
    /// [`EXTENSION_FRAME_BYTES`], or the pipe fails. The first two are settled
    /// before a byte is written: a frame refused halfway would leave a fragment
    /// on the wire that the far end joins to whatever crucible sends next.
    pub fn send(&mut self, frame: &str) -> Result<(), FrameError> {
        if frame.len() > EXTENSION_FRAME_BYTES {
            return Err(FrameError::TooLong {
                maximum: EXTENSION_FRAME_BYTES,
            });
        }
        if frame.as_bytes().contains(&b'\n') {
            return Err(FrameError::Divided);
        }
        self.to.write_all(frame.as_bytes())?;
        self.to.write_all(b"\n")?;
        self.to.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
