//! The frames crucible exchanges with a program over a pipe.
//!
//! One document per line, which is the whole framing. There is no length prefix
//! because a length is a number the far end writes and crucible would have to
//! believe before it had read anything; a newline is a boundary the reader finds
//! for itself, having read no further than it was willing to read anyway.
//!
//! Two protocols run over this, an extension's and MCP's, and the framing is
//! the half neither of them owns: a line is a line whoever is on the other end.
//! The sentences here say "the program on the other end" for that reason —
//! which of the two it is belongs to the crate that started it, and it says so
//! around this.
//!
//! Everything here reads the far end as hostile. It is a program somebody
//! installed and crucible started with their privileges, and this is where its
//! bytes arrive — so a ceiling that holds here is a ceiling on what it can make
//! this process hold, whoever wrote it.
//!
//! Both halves read and write either kind of stream: a blocking one, through
//! [`Frames::next_frame`] and [`Written::send`], or an asynchronous one,
//! through [`Frames::next_frame_async`] and [`Written::send_async`], on
//! whatever runtime the caller is already on. The two kinds share one assembly
//! and one set of refusals, so what a program sends comes to the same frames
//! and the same errors whichever kind of stream it arrives on. The blocking
//! kind stays until the transport above it is asynchronous throughout.

use std::io::{self, BufRead, Write};
use std::ops::ControlFlow;
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt as _, AsyncWrite, AsyncWriteExt as _};

use crate::Said;

/// The most bytes one frame may carry, not counting the newline that ends it.
///
/// Past anything either protocol exchanges: a result somebody's program wanted
/// a person to read is already longer than a person reads at a megabyte. Held
/// all the same, because the number that matters is not what an honest program
/// sends but what a dishonest one can make crucible keep.
pub const FRAME_BYTES: usize = 1024 * 1024;

/// Why a frame did not arrive.
#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    /// The pipe would not read.
    #[error("the program on the other end could not be read: {source}")]
    Unreadable {
        /// What the operating system reported.
        #[from]
        source: io::Error,
    },

    /// A frame ran past its ceiling.
    ///
    /// How far past is deliberately not stated: crucible stopped reading at the
    /// ceiling, so it does not know, and a figure it guessed would be a figure
    /// the far end chose.
    #[error("the program on the other end sent more than {maximum} bytes without ending a frame")]
    TooLong {
        /// The ceiling it ran past.
        maximum: usize,
    },

    /// The stream ended partway through a frame.
    #[error("the program on the other end stopped {seen} bytes into an unfinished frame")]
    Truncated {
        /// How much of it had arrived.
        seen: usize,
    },

    /// A frame was not text.
    #[error("the program on the other end sent a frame that is not UTF-8")]
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

impl FrameError {
    /// Whether a frame crucible was sending can be proven not to have reached
    /// the far end.
    ///
    /// Asked of a sending failure; a reading one was never going anywhere. The
    /// answer is what a caller deciding whether to ask again depends on, so it
    /// is `true` only where nothing could have been read, and the doubtful
    /// cases are counted as sent.
    ///
    /// A ceiling and a boundary are settled before a byte is written, and a
    /// pipe nobody is left reading cannot have handed the bytes to anybody. A
    /// patience is the one that cannot be claimed: [`Said`] gives the frame to
    /// the task that owns the pipe and then waits, so a wait that ran out
    /// ended with those bytes already gone from here and possibly already read
    /// over there.
    ///
    /// [`Said`]: crate::Said
    #[must_use]
    pub fn never_left(&self) -> bool {
        match self {
            Self::TooLong { .. } | Self::Divided => true,
            Self::Unreadable { source } => source.kind() != io::ErrorKind::TimedOut,
            Self::Truncated { .. } | Self::NotText => false,
        }
    }
}

/// The frames arriving from one program, read one at a time.
///
/// Holds one frame's worth at most. A frame is handed over whole or not at all,
/// because half a document parses into something its author never wrote.
#[derive(Debug)]
pub struct Frames<R> {
    /// Where the bytes come from.
    from: R,
    /// The frame being put together from them.
    assembly: Assembly,
}

impl<R> Frames<R> {
    /// Reads frames from `from`.
    #[must_use]
    pub const fn new(from: R) -> Self {
        Self {
            from,
            assembly: Assembly {
                held: Vec::new(),
                done: false,
            },
        }
    }

    /// The stream underneath, for what only it can be asked.
    ///
    /// Framing is all this type does; how long the stream waits and what it
    /// has seen belong to the stream, and a caller that owns both should not
    /// have to keep a second handle on one of them.
    pub const fn stream_mut(&mut self) -> &mut R {
        &mut self.from
    }

    /// What one line comes to for whoever asked for a frame: an answer, or a
    /// blank line to read past.
    ///
    /// A blank line is not a frame and is skipped: it says nothing, and the
    /// alternative is refusing a program for a byte that means nothing in
    /// either direction. A refusal finishes the stream, through
    /// [`Assembly::finish`].
    fn answer(
        &mut self,
        line: Result<Option<String>, FrameError>,
    ) -> ControlFlow<Option<Result<String, FrameError>>> {
        match line {
            Ok(None) => ControlFlow::Break(None),
            // A line with nothing on it, skipped rather than handed up as an
            // empty document for the layer above to be confused by.
            Ok(Some(frame)) if frame.is_empty() => ControlFlow::Continue(()),
            Ok(Some(frame)) => ControlFlow::Break(Some(Ok(frame))),
            Err(err) => {
                self.assembly.finish();
                ControlFlow::Break(Some(Err(err)))
            }
        }
    }
}

impl<R: BufRead> Frames<R> {
    /// The next frame, or nothing once the stream has finished.
    ///
    /// A blank line is not a frame and is skipped.
    ///
    /// # Errors
    ///
    /// [`FrameError`] where the pipe fails, a frame runs past
    /// [`FRAME_BYTES`], the stream stops partway through one, or one
    /// arrives that is not text. Every one of those finishes the stream: the
    /// reader has lost its place in a boundary the far end was stating, and
    /// hunting for the next newline would mean reading whatever it sends until
    /// it decides to send one.
    pub fn next_frame(&mut self) -> Option<Result<String, FrameError>> {
        loop {
            let line = self.frame();
            if let ControlFlow::Break(answer) = self.answer(line) {
                return answer;
            }
        }
    }

    /// One line, however many reads it takes to arrive.
    ///
    /// `Ok(None)` is the stream ending where a frame was not in progress, which
    /// is the only clean way for it to end.
    fn frame(&mut self) -> Result<Option<String>, FrameError> {
        if self.assembly.done {
            return Ok(None);
        }
        loop {
            let arrived = match self.from.fill_buf() {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err.into()),
            };
            match self.assembly.take(arrived)? {
                Took::Ended => return Ok(None),
                Took::Part(consumed) => self.from.consume(consumed),
                Took::Whole(consumed, frame) => {
                    self.from.consume(consumed);
                    return frame.map(Some);
                }
            }
        }
    }
}

impl<R: AsyncBufRead + Unpin> Frames<R> {
    /// The next frame, or nothing once the stream has finished, from a stream
    /// read asynchronously.
    ///
    /// The same frames and the same refusals as [`next_frame`](Self::next_frame)
    /// over the same bytes: a blank line is skipped, and every refusal finishes
    /// the stream.
    ///
    /// # Errors
    ///
    /// [`FrameError`] where the pipe fails, a frame runs past
    /// [`FRAME_BYTES`], the stream stops partway through one, or one arrives
    /// that is not text.
    ///
    /// # Cancel safety
    ///
    /// Dropping this before it answers loses nothing, provided the stream's own
    /// `fill_buf` loses nothing when dropped, as Tokio's buffered reader does
    /// not. What had arrived of a frame stays with the reader, and the next
    /// call carries on from it, so a caller may give up waiting for a frame
    /// without giving up the stream.
    pub async fn next_frame_async(&mut self) -> Option<Result<String, FrameError>> {
        loop {
            let line = self.frame_async().await;
            if let ControlFlow::Break(answer) = self.answer(line) {
                return answer;
            }
        }
    }

    /// One line, however many reads it takes to arrive, read asynchronously.
    ///
    /// The one wait is for bytes to arrive. Bytes are consumed from the stream
    /// only together with their taking into the assembly, with no wait between
    /// the two, which is what lets the wait be abandoned.
    async fn frame_async(&mut self) -> Result<Option<String>, FrameError> {
        if self.assembly.done {
            return Ok(None);
        }
        loop {
            let arrived = match self.from.fill_buf().await {
                Ok(bytes) => bytes,
                Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
                Err(err) => return Err(err.into()),
            };
            match self.assembly.take(arrived)? {
                Took::Ended => return Ok(None),
                Took::Part(consumed) => self.from.consume(consumed),
                Took::Whole(consumed, frame) => {
                    self.from.consume(consumed);
                    return frame.map(Some);
                }
            }
        }
    }
}

/// A frame being put together from whatever reads it takes to arrive.
///
/// Both kinds of reader hand every read's bytes to this and decide nothing
/// about them themselves, which is what keeps a stream's outcome from depending
/// on which kind of stream it arrived on.
#[derive(Debug)]
struct Assembly {
    /// The frame being assembled, never its newline.
    held: Vec<u8>,
    /// Whether this stream has finished, cleanly or otherwise.
    done: bool,
}

/// What one read's bytes came to.
enum Took {
    /// The stream ended where a frame was not in progress.
    Ended,
    /// This many bytes, all of them part of a frame not yet ended.
    Part(usize),
    /// This many bytes, the last of them the newline that ended a frame, and
    /// that frame or why it is refused.
    Whole(usize, Result<String, FrameError>),
}

impl Assembly {
    /// Takes what `arrived` carries of the frame in progress.
    ///
    /// An empty read is the stream ending. A refusal that comes back as an
    /// error is settled before anything is taken, so nothing of `arrived` is to
    /// be consumed; one that comes back inside [`Took::Whole`] is a frame that
    /// ended and is not text, whose bytes were read to their newline.
    fn take(&mut self, arrived: &[u8]) -> Result<Took, FrameError> {
        if arrived.is_empty() {
            self.done = true;
            if self.held.is_empty() {
                return Ok(Took::Ended);
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
        if carried > FRAME_BYTES.saturating_sub(self.held.len()) {
            return Err(FrameError::TooLong {
                maximum: FRAME_BYTES,
            });
        }
        self.held.extend(arrived.iter().take(carried).copied());

        if ended.is_none() {
            return Ok(Took::Part(consumed));
        }
        let bytes = std::mem::take(&mut self.held);
        Ok(Took::Whole(
            consumed,
            String::from_utf8(bytes).map_err(|_| FrameError::NotText),
        ))
    }

    /// Ends the stream and lets go of whatever was being assembled.
    ///
    /// Every refusal comes through here, because each of them means the reader
    /// no longer knows where a frame starts: the boundary was the far end's
    /// to state, and reading on to find the next newline is reading whatever it
    /// sends until it chooses to send one. What was held goes with it — a
    /// refused frame is not evidence, and keeping it would leave a megabyte
    /// alive for a stream nobody will read again.
    fn finish(&mut self) {
        self.done = true;
        self.held = Vec::new();
    }
}

/// The frames going out to one program.
///
/// Nothing is buffered between calls. A program waiting on a request that is
/// sitting in crucible's buffer is a hang with no error and nothing on screen,
/// so a frame is on its way out by the time [`send`](Self::send) or
/// [`send_async`](Self::send_async) answers.
///
/// Those two are also the only way anything goes out. The stream is
/// not lent back, because whatever can borrow it can write to it, and a byte
/// written beside a frame is a line the far end reads as one nobody checked:
///
/// ```compile_fail,E0599
/// use crucible_transport::Written;
///
/// let mut written = Written::new(Vec::<u8>::new());
/// written.stream_mut();
/// ```
///
/// The same value sending a frame, which compiles. Any compile error passes
/// the example above, so this is the one that fails where a name in it moved:
///
/// ```
/// use crucible_transport::Written;
///
/// let mut written = Written::new(Vec::<u8>::new());
/// let _ = written.send("a frame");
/// ```
#[derive(Debug)]
pub struct Written<W> {
    /// Where the bytes go.
    to: W,
}

impl Written<Said> {
    /// Waits a different time out for one frame from here on.
    ///
    /// The one thing a caller has to ask the stream rather than the framing,
    /// asked by name so that asking it lends nothing that can be written to.
    pub const fn patient_for(&mut self, patience: Duration) {
        self.to.patient_for(patience);
    }
}

impl<W> Written<W> {
    /// Sends frames to `to`.
    #[must_use]
    pub const fn new(to: W) -> Self {
        Self { to }
    }

    /// Refuses a frame that may not go out, before a byte of it is written.
    ///
    /// A frame refused halfway would leave a fragment on the wire that the far
    /// end joins to whatever crucible sends next.
    fn admitted(frame: &str) -> Result<(), FrameError> {
        if frame.len() > FRAME_BYTES {
            return Err(FrameError::TooLong {
                maximum: FRAME_BYTES,
            });
        }
        if frame.as_bytes().contains(&b'\n') {
            return Err(FrameError::Divided);
        }
        Ok(())
    }
}

impl<W: Write> Written<W> {
    /// Sends one frame.
    ///
    /// # Errors
    ///
    /// [`FrameError`] where the frame carries a newline, runs past
    /// [`FRAME_BYTES`], or the pipe fails. The first two are settled
    /// before a byte is written: a frame refused halfway would leave a fragment
    /// on the wire that the far end joins to whatever crucible sends next.
    pub fn send(&mut self, frame: &str) -> Result<(), FrameError> {
        Self::admitted(frame)?;
        self.to.write_all(frame.as_bytes())?;
        self.to.write_all(b"\n")?;
        self.to.flush()?;
        Ok(())
    }
}

impl<W: AsyncWrite + Unpin> Written<W> {
    /// Sends one frame over a stream written asynchronously.
    ///
    /// The same refusals as [`send`](Self::send), settled the same way before
    /// a byte is written, and the same bytes on the wire.
    ///
    /// # Errors
    ///
    /// [`FrameError`] where the frame carries a newline, runs past
    /// [`FRAME_BYTES`], or the pipe fails.
    ///
    /// # Cancel safety
    ///
    /// None. Dropped before it answers, it may have written part of the frame,
    /// and whatever is sent after that arrives joined to the part: a caller
    /// that gives up on a send has given up on the stream with it.
    pub async fn send_async(&mut self, frame: &str) -> Result<(), FrameError> {
        Self::admitted(frame)?;
        self.to.write_all(frame.as_bytes()).await?;
        self.to.write_all(b"\n").await?;
        self.to.flush().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
