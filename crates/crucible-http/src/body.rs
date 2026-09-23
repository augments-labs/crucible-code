//! Reading a response body, within the bounds its reader asks for.
//!
//! A body comes from the network, so nothing here keeps more of it than its
//! reader said it would take, and nothing waits on it for longer than its
//! reader said it would wait. There are three ways to read one:
//!
//! - **As it arrives** ([`Chunks`]), for a stream such as a model's answer.
//!   It has no deadline, because a quiet model and a dead peer look the same
//!   from here, and keeps nothing: each chunk is handed on as hyper hands it
//!   over. A wait that finds nothing for [`QUIET`] comes back as
//!   [`Chunk::Quiet`], so a caller that is looking at something else, such
//!   as whether it has been cancelled, gets to look.
//! - **Whole, up to a limit** ([`read_limited`]), for an answer that is used
//!   whole or not at all. The limit and the deadline are the caller's.
//! - **The beginning of a refusal** ([`read_refusal`]): at most
//!   [`MAX_REFUSAL`] bytes within [`REFUSAL_WAIT`], kept whatever became of
//!   the rest, because a refusal is read for the sentence at its start.
//!
//! Both bounded reads take one byte past their limit before they stop, since
//! a body that ends at the limit and one the limit cut are otherwise the same
//! bytes; they stop as soon as that byte arrives, without waiting for a rest
//! that may never come. Their deadline is the whole read's, counted from
//! when it starts, so a peer that trickles a byte into every gap is given up
//! on all the same.
//!
//! Dropping a [`Chunks`], or the future [`read_limited`] or [`read_refusal`]
//! returns, drops the body; dropping only the future [`Chunks::next`]
//! returns does not. Once the body is dropped, hyper takes one more read of
//! what has arrived and, if the body has still not ended, closes the
//! connection it was arriving on. A body whose
//! [`Http`](crate::Http) was dropped while it was still arriving fails as
//! [`BodyError::Incomplete`], because the client's connections end with it.

use std::error::Error as _;
use std::fmt;
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::time::Duration;

use hyper::body::{Body as _, Bytes, Frame, Incoming};
use tokio::time::{Instant, timeout, timeout_at};

/// How long [`Chunks::next`] waits for bytes before it says none came: a
/// quarter of a second, as long as the previous client's body reader waited
/// before it said the same.
pub const QUIET: Duration = Duration::from_millis(250);

/// The most of a refusal's body [`read_refusal`] keeps.
///
/// A refusal is a sentence. Anything much larger is most likely a proxy's
/// error page, and the sentence, when there is one, is at its start.
pub const MAX_REFUSAL: usize = 8 * 1024;

/// How long [`read_refusal`] reads for, altogether.
///
/// A refusal that has not finished arriving in ten seconds is not going to,
/// and what a caller holding a refusal competes with is its user learning
/// nothing.
pub const REFUSAL_WAIT: Duration = Duration::from_secs(10);

/// A body that could not be read as far as its reader asked.
#[derive(Debug, thiserror::Error)]
pub enum BodyError {
    /// The connection closed before the body ended: the peer closed it
    /// before the end the response's framing promised, or the body's client
    /// was dropped and its connections with it. The source is hyper's error,
    /// which says which.
    #[error("the response body stopped before it ended")]
    Incomplete(#[source] hyper::Error),
    /// Reading the body failed any other way; the source says how.
    #[error("the response body could not be read")]
    Failed(#[source] hyper::Error),
    /// A byte past the reader's limit arrived.
    #[error("the response body was longer than its limit")]
    TooLarge,
    /// The read outlived its deadline.
    #[error("the response body was not read within its deadline")]
    Deadline,
}

impl BodyError {
    /// Sorts the error hyper gave for a body by whether the body was cut
    /// short: hyper's own incomplete message, or an end of file met before
    /// the end the response's framing promised.
    fn reading(error: hyper::Error) -> Self {
        let mut cause = error.source();
        let mut short = error.is_incomplete_message();
        while let Some(step) = cause.filter(|_| !short) {
            short = step
                .downcast_ref::<io::Error>()
                .is_some_and(|io| io.kind() == io::ErrorKind::UnexpectedEof);
            cause = step.source();
        }
        if short {
            Self::Incomplete(error)
        } else {
            Self::Failed(error)
        }
    }
}

/// What the next read of a streamed body found.
///
/// Its `Debug` shows how many bytes came, never the bytes, for the same
/// reason as [`Refusal`]'s.
pub enum Chunk {
    /// Bytes of the body, as hyper handed them over.
    Data(Bytes),
    /// Nothing, for [`QUIET`].
    Quiet,
}

/// A body read as it arrives, with no deadline and no limit of its own.
///
/// Whatever bounds a stream is its reader's: a stream that goes quiet is
/// waited for until it speaks, ends, fails or is dropped.
#[derive(Debug)]
pub struct Chunks {
    body: Incoming,
}

impl Chunks {
    /// Reads `body` as it arrives.
    #[must_use]
    pub fn new(body: Incoming) -> Self {
        Self { body }
    }

    /// The next bytes of the body, or [`Chunk::Quiet`] once [`QUIET`] has
    /// passed without any; `None` once the body has ended.
    ///
    /// Dropping the future this returns loses no bytes, so it can be raced
    /// against something else: bytes are taken from the body only in the
    /// poll that hands them back.
    ///
    /// # Errors
    ///
    /// [`BodyError::Incomplete`] or [`BodyError::Failed`] when the body
    /// could not be read to its end.
    pub async fn next(&mut self) -> Option<Result<Chunk, BodyError>> {
        match timeout(QUIET, data(&mut self.body)).await {
            Ok(next) => next.map(|read| read.map(Chunk::Data)),
            Err(_) => Some(Ok(Chunk::Quiet)),
        }
    }
}

/// How a bounded read ended.
#[derive(Debug)]
pub enum End {
    /// The body ended within the bound: what was kept is all of it.
    Whole,
    /// A byte past the bound arrived: what was kept is only its beginning.
    Cut,
    /// The read stopped before the body ended or reached the bound: what
    /// was kept is what arrived before it did.
    Failed(BodyError),
}

/// The beginning of a refused response's body, and how its reading ended.
///
/// Its `Debug` shows how many bytes were kept, never the bytes: a refusal
/// can echo a credential the request was sent with.
pub struct Refusal {
    /// At most [`MAX_REFUSAL`] bytes from the start of the body.
    pub said: Vec<u8>,
    /// Whether that is all of it, or why not.
    pub end: End,
}

impl fmt::Debug for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refusal")
            .field("said", &Length(self.said.len()))
            .field("end", &self.end)
            .finish()
    }
}

impl fmt::Debug for Chunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Data(bytes) => f.debug_tuple("Data").field(&Length(bytes.len())).finish(),
            Self::Quiet => f.write_str("Quiet"),
        }
    }
}

/// A count of bytes, shown in place of them.
struct Length(usize);

impl fmt::Debug for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\"{} bytes\"", self.0)
    }
}

/// Reads all of `body`, if it is at most `limit` bytes and arrives within
/// `within`, counted from now.
///
/// A `within` too long for the clock to count to is no deadline.
///
/// # Errors
///
/// [`BodyError::TooLarge`] once a byte past `limit` arrives,
/// [`BodyError::Deadline`] when `within` passes first, and
/// [`BodyError::Incomplete`] or [`BodyError::Failed`] when the body could
/// not be read to its end.
pub async fn read_limited(
    body: Incoming,
    limit: usize,
    within: Duration,
) -> Result<Vec<u8>, BodyError> {
    let (kept, end) = gather(body, limit, within).await;
    match end {
        End::Whole => Ok(kept),
        End::Cut => Err(BodyError::TooLarge),
        End::Failed(error) => Err(error),
    }
}

/// Reads the beginning of a refused response's body: at most
/// [`MAX_REFUSAL`] bytes, within [`REFUSAL_WAIT`], keeping what arrived
/// however the reading ended.
pub async fn read_refusal(body: Incoming) -> Refusal {
    let (said, end) = gather(body, MAX_REFUSAL, REFUSAL_WAIT).await;
    Refusal { said, end }
}

/// Reads `body` until it ends, a byte past `limit` arrives, the read fails
/// or `within` passes, keeping at most `limit` bytes.
async fn gather(mut body: Incoming, limit: usize, within: Duration) -> (Vec<u8>, End) {
    let deadline = Instant::now().checked_add(within);
    let mut kept = Vec::new();
    loop {
        let next = match deadline {
            Some(deadline) => match timeout_at(deadline, data(&mut body)).await {
                Ok(next) => next,
                Err(_) => return (kept, End::Failed(BodyError::Deadline)),
            },
            None => data(&mut body).await,
        };
        let bytes = match next {
            None => return (kept, End::Whole),
            Some(Err(error)) => return (kept, End::Failed(error)),
            Some(Ok(bytes)) => bytes,
        };
        let room = limit.saturating_sub(kept.len());
        if let Some(fits) = bytes.get(..room).filter(|_| bytes.len() > room) {
            kept.extend_from_slice(fits);
            return (kept, End::Cut);
        }
        kept.extend_from_slice(&bytes);
    }
}

/// The next bytes of `body`, passing over trailers; `None` at its end.
async fn data(body: &mut Incoming) -> Option<Result<Bytes, BodyError>> {
    loop {
        match frame(body).await? {
            Ok(frame) => {
                if let Ok(bytes) = frame.into_data() {
                    return Some(Ok(bytes));
                }
            }
            Err(error) => return Some(Err(BodyError::reading(error))),
        }
    }
}

async fn frame(body: &mut Incoming) -> Option<Result<Frame<Bytes>, hyper::Error>> {
    poll_fn(|cx| Pin::new(&mut *body).poll_frame(cx)).await
}

#[cfg(test)]
mod tests;
