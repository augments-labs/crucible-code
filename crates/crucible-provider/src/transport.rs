//! The port every provider sends through.
//!
//! One method, because one method is all a provider needs: post a body, get a
//! status and a stream back. Keeping it this narrow is what lets the whole wire
//! protocol be tested against a recorded response, with no socket and no server
//! anywhere in the test.
//!
//! It is also the seam for the HTTP client itself. [`http`] is the only file
//! that names one; swapping it is that file and nothing else.

pub(crate) mod http;

use std::error::Error as _;
use std::fmt;
use std::io::{self, Read};
use std::pin::Pin;
use std::task::{Context, Poll};

use crucible_credentials::Outgoing;
use crucible_http::BodyError;
use crucible_models::ProviderError;
use crucible_runtime::{BoxFuture, Cancel};
use hyper::body::{Body as _, Bytes, Incoming};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

/// Why a request did not produce a response.
///
/// A response that arrived and said no is not an error here — that is a status,
/// and the provider turns it into its own refusal with the message the vendor
/// sent. Neither is a connection that breaks part-way through a body: [`post`]
/// has returned by then and the break arrives through the reader it handed
/// back, which is where the stream turns it into a failed turn.
///
/// [`post`]: Transport::post
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The user cancelled before response headers arrived.
    #[error("request cancelled before a response arrived")]
    Cancelled,

    /// The platform resolver outlived its deadline and cannot be reaped.
    #[error("hostname resolution stalled; restart crucible before trying another provider request")]
    ResolveStalled,

    /// The request could not be sent, or the connection failed.
    #[error("{0}")]
    Unreachable(Box<str>),
}

impl TransportError {
    /// Adds the provider name while preserving cancellation as its own domain
    /// outcome rather than presenting it as a network failure.
    pub(crate) fn for_provider(self, provider: &'static str) -> ProviderError {
        match self {
            Self::Cancelled => ProviderError::Cancelled(provider),
            Self::ResolveStalled => ProviderError::Transport {
                provider,
                problem: "hostname resolution stalled; restart crucible before trying another provider request"
                    .into(),
            },
            Self::Unreachable(problem) => ProviderError::Transport { provider, problem },
        }
    }
}

/// What came back.
pub struct Response {
    /// The HTTP status.
    pub status: u16,
    /// The body, still arriving.
    pub body: Box<dyn Read + Send>,
}

impl fmt::Debug for Response {
    /// By hand, because a body being read cannot be shown without consuming it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// What an asynchronous post produced.
pub struct PostResponse {
    status: u16,
    body: PostBody,
}

/// The body behind a [`PostResponse`].
///
/// The network arm keeps hyper's incoming body intact so its bounded readers
/// remain the readers used in production. The reader arm is for recorded
/// transports and external test doubles; it is never selected by [`http::HttpTurns`].
enum PostBody {
    Network(Incoming),
    Reader(Box<dyn AsyncRead + Send + Unpin>),
}

impl PostResponse {
    /// A response whose body is read by the caller.
    pub fn recorded(status: u16, body: impl AsyncRead + Send + Unpin + 'static) -> Self {
        Self {
            status,
            body: PostBody::Reader(Box::new(body)),
        }
    }

    /// A response carrying the body returned by the shared HTTP client.
    pub(crate) fn network(status: u16, body: Incoming) -> Self {
        Self {
            status,
            body: PostBody::Network(body),
        }
    }

    /// The response status. Every status is an answer to the protocol reading it.
    pub(crate) fn status(&self) -> u16 {
        self.status
    }

    /// Takes the body for streaming.
    pub(crate) fn into_reader(self) -> Box<dyn AsyncRead + Send + Unpin> {
        match self.body {
            PostBody::Network(body) => Box::new(Arriving::new(body)),
            PostBody::Reader(body) => body,
        }
    }

    /// Takes the body for a whole bounded read.
    pub(crate) async fn read_limited(
        self,
        limit: usize,
        within: std::time::Duration,
    ) -> Result<Vec<u8>, PostBodyError> {
        match self.body {
            PostBody::Network(body) => crucible_http::read_limited(body, limit, within)
                .await
                .map_err(PostBodyError::Http),
            PostBody::Reader(mut body) => read_reader(&mut body, limit, within).await,
        }
    }
}

impl fmt::Debug for PostResponse {
    /// By hand, for the same reason as [`Response`].
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostResponse")
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

/// Why an asynchronous response body was not read whole.
#[derive(Debug)]
pub(crate) enum PostBodyError {
    /// The shared HTTP body reader refused or failed.
    Http(BodyError),
    /// A recorded or external transport reader failed.
    Read(io::Error),
    /// A byte past the caller's limit arrived.
    TooLarge,
    /// The caller's whole-read deadline passed.
    Deadline,
}

/// Reads a recorded or external body with the same whole-read contract as the
/// shared HTTP body reader.
async fn read_reader(
    body: &mut (impl AsyncRead + Unpin + ?Sized),
    limit: usize,
    within: std::time::Duration,
) -> Result<Vec<u8>, PostBodyError> {
    let deadline = tokio::time::Instant::now().checked_add(within);
    let mut kept = Vec::new();
    let mut into = [0_u8; 8 * 1024];
    loop {
        let next = match deadline {
            Some(deadline) => tokio::time::timeout_at(deadline, body.read(&mut into))
                .await
                .map_err(|_| PostBodyError::Deadline)?,
            None => body.read(&mut into).await,
        };
        let read = next.map_err(PostBodyError::Read)?;
        if read == 0 {
            return Ok(kept);
        }
        let bytes = into.get(..read).unwrap_or_default();
        if bytes.len() > limit.saturating_sub(kept.len()) {
            return Err(PostBodyError::TooLarge);
        }
        kept.extend_from_slice(bytes);
    }
}

/// A hyper body as the asynchronous reader the provider formats expect.
struct Arriving {
    body: Incoming,
    current: Bytes,
    ended: bool,
    quiet: Option<Pin<Box<tokio::time::Sleep>>>,
}

impl Arriving {
    fn new(body: Incoming) -> Self {
        Self {
            body,
            current: Bytes::new(),
            ended: false,
            quiet: None,
        }
    }
}

impl AsyncRead for Arriving {
    fn poll_read(
        mut self: Pin<&mut Self>,
        context: &mut Context<'_>,
        into: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.as_mut().get_mut();
        loop {
            if !this.current.is_empty() {
                let amount = this.current.len().min(into.remaining());
                let chunk = this.current.split_to(amount);
                into.put_slice(&chunk);
                return Poll::Ready(Ok(()));
            }
            if this.ended {
                return Poll::Ready(Ok(()));
            }

            match Pin::new(&mut this.body).poll_frame(context) {
                Poll::Ready(Some(Ok(frame))) => {
                    this.quiet = None;
                    if let Ok(bytes) = frame.into_data() {
                        this.current = bytes;
                    }
                }
                Poll::Ready(None) => {
                    this.ended = true;
                    this.quiet = None;
                    return Poll::Ready(Ok(()));
                }
                Poll::Ready(Some(Err(problem))) => {
                    this.quiet = None;
                    return Poll::Ready(Err(body_error(problem)));
                }
                Poll::Pending => {
                    let quiet = this
                        .quiet
                        .get_or_insert_with(|| Box::pin(tokio::time::sleep(crucible_http::QUIET)));
                    if quiet.as_mut().poll(context).is_ready() {
                        this.quiet = None;
                        return Poll::Ready(Err(io::ErrorKind::Interrupted.into()));
                    }
                    return Poll::Pending;
                }
            }
        }
    }
}

/// Maps the body failure hyper reports to the reader error the stream expects.
fn body_error(problem: hyper::Error) -> io::Error {
    let mut short = problem.is_incomplete_message();
    let mut source = problem.source();
    while let Some(cause) = source {
        if cause
            .downcast_ref::<io::Error>()
            .is_some_and(|error| error.kind() == io::ErrorKind::UnexpectedEof)
        {
            short = true;
        }
        source = cause.source();
    }
    if short {
        io::Error::new(io::ErrorKind::UnexpectedEof, problem)
    } else {
        io::Error::other(problem)
    }
}

/// Somewhere to send a request.
pub trait Transport: Send + Sync + fmt::Debug {
    /// Posts `body` and returns the response as it begins to arrive.
    ///
    /// Returns rather than reads: the body is a stream of events that lasts as
    /// long as the model is talking, so reading it here would mean waiting for
    /// the whole answer before showing any of it.
    ///
    /// Headers and body belong to the asynchronous send while its future is
    /// being polled. Dropping that future is how a caller stops waiting and
    /// lets the shared client end the connection and any setup work with it.
    ///
    /// # Errors
    ///
    /// [`TransportError`] if the request could not be sent or was cancelled
    /// before its response headers arrived. A response with a status the caller
    /// dislikes is not an error.
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PostResponse, TransportError>>;
}

/// A transport that answers from a script instead of a network.
///
/// Every wire-protocol test runs through this: the provider builds a real
/// request, and this hands back a real recorded response.
#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Replay {
    status: u16,
    body: String,
    sent: std::sync::Mutex<Vec<Sent>>,
}

/// One request the provider made.
#[cfg(test)]
#[derive(Debug, Clone)]
pub(crate) struct Sent {
    pub(crate) url: String,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) body: String,
}

#[cfg(test)]
impl Replay {
    /// Answers every request with `status` and `body`.
    pub(crate) fn new(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
            sent: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The last request made, for asserting on what went out.
    pub(crate) fn sent(&self) -> Sent {
        self.sent
            .lock()
            .ok()
            .and_then(|sent| sent.last().cloned())
            .unwrap_or_else(|| Sent {
                url: String::new(),
                headers: Vec::new(),
                body: String::new(),
            })
    }
}

#[cfg(test)]
impl Transport for Replay {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PostResponse, TransportError>> {
        Box::pin(async move {
            if cancel.requested() {
                return Err(TransportError::Cancelled);
            }

            if let Ok(mut sent) = self.sent.lock() {
                sent.push(Sent {
                    url: url.to_owned(),
                    headers: headers
                        .headers()
                        .iter()
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                        .collect(),
                    body,
                });
            }

            Ok(PostResponse::recorded(
                self.status,
                std::io::Cursor::new(self.body.clone().into_bytes()),
            ))
        })
    }
}

/// A response body with pauses in it.
///
/// A recorded body in a `Cursor` never pauses and never goes quiet, so every
/// test that reads one is a test about an answer that had already arrived. This
/// is the other half: it says its pieces in order, reports a wait that expired
/// the way [`http`] does — an interruption, the kind the `Read` contract says to
/// retry — and closes when it runs out.
#[cfg(test)]
pub(crate) struct Paused {
    said: std::collections::VecDeque<Said>,
    meanwhile: Box<dyn FnMut() + Send>,
}

/// One thing a paused body does when it is read from.
#[cfg(test)]
pub(crate) enum Said {
    /// These bytes, as one read delivers them.
    Bytes(Vec<u8>),
    /// Nothing, for as long as one read waits for it.
    Nothing,
}

#[cfg(test)]
impl Paused {
    /// Says these, in order, and then closes.
    pub(crate) fn saying(said: impl IntoIterator<Item = Said>) -> Self {
        Self {
            said: said.into_iter().collect(),
            meanwhile: Box::new(|| {}),
        }
    }

    /// Says `text` in pieces of `at_a_time` bytes, quiet between each, so that
    /// a pause falls in the middle of a line as well as between events.
    pub(crate) fn dawdling(text: &str, at_a_time: usize) -> Self {
        let said = text
            .as_bytes()
            .chunks(at_a_time)
            .flat_map(|piece| [Said::Nothing, Said::Bytes(piece.to_vec())]);

        Self::saying(said)
    }

    /// Runs `meanwhile` every time it goes quiet.
    ///
    /// Where a test raises a cancel: inside the wait, which is where a user
    /// raises one and the only place worth proving anything about.
    pub(crate) fn meanwhile(mut self, meanwhile: impl FnMut() + Send + 'static) -> Self {
        self.meanwhile = Box::new(meanwhile);
        self
    }
}

#[cfg(test)]
impl Read for Paused {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        match self.said.pop_front() {
            // Everything it had, said: the socket closed.
            None => Ok(0),
            Some(Said::Nothing) => {
                (self.meanwhile)();
                Err(std::io::ErrorKind::Interrupted.into())
            }
            Some(Said::Bytes(bytes)) => {
                let took = bytes.len().min(into.len());
                let (taken, left) = bytes.split_at(took);
                into.get_mut(..took)
                    .unwrap_or_default()
                    .copy_from_slice(taken);

                if !left.is_empty() {
                    self.said.push_front(Said::Bytes(left.to_vec()));
                }

                Ok(took)
            }
        }
    }
}

/// A recorded reader exposed through the asynchronous body contract.
///
/// Test-only: shipped responses come from hyper, while recorded transports need
/// the same parser without a socket.
#[cfg(test)]
pub(crate) struct SyncReader<T>(pub(crate) T);

#[cfg(test)]
impl<T> SyncReader<T> {
    pub(crate) fn new(reader: T) -> Self {
        Self(reader)
    }
}

#[cfg(test)]
impl<T> AsyncRead for SyncReader<T>
where
    T: Read + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        _context: &mut Context<'_>,
        into: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if into.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        let mut bytes = [0_u8; 8 * 1024];
        match self.0.read(&mut bytes) {
            Ok(read) => {
                into.put_slice(bytes.get(..read).unwrap_or_default());
                Poll::Ready(Ok(()))
            }
            Err(problem) => Poll::Ready(Err(problem)),
        }
    }
}

/// A shared transport is still a transport.
///
/// Only tests need this: a provider takes ownership of the one it sends
/// through, and a test wants a second handle to ask what went out.
#[cfg(test)]
impl<T: Transport> Transport for std::sync::Arc<T> {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PostResponse, TransportError>> {
        (**self).post(url, headers, body, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_replay_keeps_what_was_sent() {
        let replay = Replay::new(200, "body");
        let mut headers = Outgoing::new();
        headers.set_header("x-key", "value");

        replay
            .post(
                "https://example.test/v1",
                &mut headers,
                "{}".to_owned(),
                &Cancel::new(),
            )
            .await
            .unwrap();

        let sent = replay.sent();
        assert_eq!(sent.url, "https://example.test/v1");
        assert_eq!(sent.body, "{}");
        assert_eq!(sent.headers.first().unwrap().0, "x-key");
    }

    #[tokio::test]
    async fn a_replay_answers_with_the_recorded_response() {
        let replay = Replay::new(429, "slow down");

        let response = replay
            .post(
                "https://example.test/v1",
                &mut Outgoing::new(),
                "{}".to_owned(),
                &Cancel::new(),
            )
            .await
            .unwrap();
        let status = response.status();
        let read = response
            .read_limited(1024, std::time::Duration::from_secs(1))
            .await
            .unwrap();

        assert_eq!(429, status);
        assert_eq!(String::from_utf8(read).unwrap(), "slow down");
    }

    #[test]
    fn transport_cancellation_stays_a_cancel_at_the_provider_boundary() {
        let problem = TransportError::Cancelled.for_provider("test");

        assert!(matches!(problem, ProviderError::Cancelled("test")));
    }
}
