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
use std::io;
#[cfg(test)]
use std::io::Read;
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

    /// The client held the request back until the person answers for the
    /// route it would have gone to: nothing was sent, and asking again sends
    /// nothing either.
    #[error("{0}")]
    Held(Box<str>),
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
            // Its own failure: not retried, and known to have reached no host.
            Self::Held(problem) => ProviderError::Held(format!("{provider}: {problem}").into()),
        }
    }
}

/// The response header a vendor says the tier that served a request in: the
/// one header the transport reads itself. A provider is handed others only by
/// naming them ([`Named`]); every other header is left where it arrived.
pub(crate) const SERVED_TIER: &str = "x-gemini-service-tier";

/// What a response's [`SERVED_TIER`] header said.
///
/// A byte rather than the words: it sits in the room the status leaves, so a
/// response of every other provider is no larger for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Tier {
    /// No such header.
    Unsaid,
    /// `priority`, the tier a fast request asks for.
    Priority,
    /// Any other tier.
    Other,
}

impl Tier {
    /// What a [`SERVED_TIER`] header of `value` said, or none where there is
    /// no such header.
    pub(crate) fn read(value: Option<&str>) -> Self {
        match value {
            None => Self::Unsaid,
            Some("priority") => Self::Priority,
            Some(_) => Self::Other,
        }
    }
}

/// The most bytes of one response header a provider is handed.
///
/// A header a provider names is one it reads a figure or a time out of; a
/// value longer than this is not one, and is left where it arrived.
pub(crate) const NAMED_HEADER_BYTES: usize = 256;

/// The response headers a provider named when it posted, as they arrived.
///
/// Only those names, at most one value each, each within
/// [`NAMED_HEADER_BYTES`]: everything else a response carries stays where it
/// arrived, and nothing here is read by the transport.
///
/// Behind one pointer, and none where nothing was named, so that a response
/// of a provider that names nothing is no larger for it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Named(Option<Box<Pairs>>);

/// Each named header that arrived, and what it said.
type Pairs = Vec<(&'static str, Box<str>)>;

impl Named {
    /// The headers among `asked` that `value_of` finds, each kept only when it
    /// is text within [`NAMED_HEADER_BYTES`].
    pub(crate) fn kept<'v>(
        asked: &'static [&'static str],
        value_of: impl Fn(&str) -> Option<&'v str>,
    ) -> Self {
        let kept: Vec<_> = asked
            .iter()
            .filter_map(|name| {
                value_of(name)
                    .filter(|value| value.len() <= NAMED_HEADER_BYTES)
                    .map(|value| (*name, value.into()))
            })
            .collect();
        Self((!kept.is_empty()).then(|| Box::new(kept)))
    }

    /// What the header called `name` said, where the provider named it and it
    /// arrived within bounds.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        self.0
            .as_deref()?
            .iter()
            .find(|(named, _)| *named == name)
            .map(|(_, value)| &**value)
    }
}

/// What an asynchronous post produced.
pub struct PostResponse(Posted);

/// The status, what the head said, and the body behind a [`PostResponse`].
///
/// The network arm keeps hyper's incoming body intact so its bounded readers
/// remain the readers used in production. The reader arm is for recorded
/// transports and external test doubles; it is never selected by [`http::HttpTurns`].
///
/// The status and what the head said are written into each arm rather than
/// beside the enum, because there they sit in the room its tag leaves: a
/// response is no larger for the headers a provider named, and that is a
/// response every request of every provider waits on.
enum Posted {
    Network {
        status: u16,
        /// What the [`SERVED_TIER`] header said.
        tier: Tier,
        /// The headers the provider named when it posted.
        named: Named,
        body: Incoming,
    },
    Reader {
        status: u16,
        /// What the [`SERVED_TIER`] header said.
        tier: Tier,
        /// The headers the provider named when it posted.
        named: Named,
        body: Box<dyn AsyncRead + Send + Unpin>,
    },
}

impl PostResponse {
    /// A response whose body is read by the caller.
    pub fn recorded(status: u16, body: impl AsyncRead + Send + Unpin + 'static) -> Self {
        Self(Posted::Reader {
            status,
            tier: Tier::Unsaid,
            named: Named::default(),
            body: Box::new(body),
        })
    }

    /// A response carrying the body returned by the shared HTTP client.
    pub(crate) fn network(status: u16, body: Incoming) -> Self {
        Self(Posted::Network {
            status,
            tier: Tier::Unsaid,
            named: Named::default(),
            body,
        })
    }

    /// The same response, its [`SERVED_TIER`] header having said `tier`.
    pub(crate) fn with_tier(mut self, said: Tier) -> Self {
        let (Posted::Network { tier, .. } | Posted::Reader { tier, .. }) = &mut self.0;
        *tier = said;
        self
    }

    /// What its [`SERVED_TIER`] header said.
    pub(crate) const fn tier(&self) -> Tier {
        let (Posted::Network { tier, .. } | Posted::Reader { tier, .. }) = &self.0;
        *tier
    }

    /// The same response, carrying the headers the provider named.
    pub(crate) fn with_named(mut self, kept: Named) -> Self {
        let (Posted::Network { named, .. } | Posted::Reader { named, .. }) = &mut self.0;
        *named = kept;
        self
    }

    /// The headers the provider named when it posted, as far as they arrived.
    pub(crate) const fn named(&self) -> &Named {
        let (Posted::Network { named, .. } | Posted::Reader { named, .. }) = &self.0;
        named
    }

    /// The response status. Every status is an answer to the protocol reading it.
    pub(crate) fn status(&self) -> u16 {
        let (Posted::Network { status, .. } | Posted::Reader { status, .. }) = &self.0;
        *status
    }

    /// Takes the body for streaming.
    pub(crate) fn into_reader(self) -> Box<dyn AsyncRead + Send + Unpin> {
        match self.0 {
            Posted::Network { body, .. } => Box::new(Arriving::new(body)),
            Posted::Reader { body, .. } => body,
        }
    }

    /// Takes the body for a whole bounded read.
    pub(crate) async fn read_limited(
        self,
        limit: usize,
        within: std::time::Duration,
    ) -> Result<Vec<u8>, PostBodyError> {
        match self.0 {
            Posted::Network { body, .. } => crucible_http::read_limited(body, limit, within)
                .await
                .map_err(PostBodyError::Http),
            Posted::Reader { mut body, .. } => read_reader(&mut body, limit, within).await,
        }
    }
}

impl fmt::Debug for PostResponse {
    /// By hand, because a body being read cannot be shown without consuming it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PostResponse")
            .field("status", &self.status())
            .field("tier", &self.tier())
            .field("named", self.named())
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

    /// The same post, handing back the response headers named in `reading`.
    ///
    /// A provider names the headers it reads, and only those come back to it,
    /// each within 256 bytes; every other header stays where
    /// it arrived. A transport that answers from somewhere other than a
    /// response's head hands back none, which is what this does unless it is
    /// overridden.
    ///
    /// # Errors
    ///
    /// As [`post`](Self::post).
    #[allow(
        clippy::too_many_arguments,
        reason = "the post's own four, and the response headers it reads"
    )]
    fn post_reading<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        cancel: &'a Cancel,
        reading: &'static [&'static str],
    ) -> BoxFuture<'a, Result<PostResponse, TransportError>> {
        let _ = reading;
        self.post(url, headers, body, cancel)
    }
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
    tier: Option<String>,
    /// Response headers it answers with, which a provider sees only by name.
    answering: Vec<(&'static str, String)>,
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
            tier: None,
            answering: Vec::new(),
            sent: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// The same, answering with a response header `name` saying `value`.
    pub(crate) fn answering(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.answering.push((name, value.into()));
        self
    }

    /// The same, answering with `tier` in the [`SERVED_TIER`] header.
    pub(crate) fn tiered(self, tier: &str) -> Self {
        Self {
            tier: Some(tier.to_owned()),
            ..self
        }
    }

    /// How many requests reached this transport, so a test can say that none
    /// did. The absence of a request is the harder half of a guard to show, and
    /// `sent` cannot show it: a record with nothing in it and a record that
    /// cannot be read both answer a `Sent` empty in every field, and an empty
    /// answer does not say which of the two it is.
    ///
    /// `None` when the count cannot be read at all, which is not the same claim
    /// as zero and must not be read as it. The lock is only poisoned by a panic
    /// while it is held, so the state is rare, and a caller acting on the
    /// absence of a request is the one caller that cannot afford to guess at it.
    pub(crate) fn sent_count(&self) -> Option<usize> {
        self.sent.lock().ok().map(|sent| sent.len())
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

    /// Poisons the record of what was sent, so a test can show that a count it
    /// cannot give is not read as a count of nothing.
    ///
    /// A `Mutex` is poisoned by a panic while it is held, and that is the only
    /// way to reach the state, so the unwind is here and the caller catches it.
    /// The flag is what outlives the call.
    pub(crate) fn poison(&self) {
        let _held = self.sent.lock();
        panic!("poison the record of what the transport was asked for");
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
        self.post_reading(url, headers, body, cancel, &[])
    }

    fn post_reading<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        cancel: &'a Cancel,
        reading: &'static [&'static str],
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

            let named = Named::kept(reading, |name| {
                self.answering
                    .iter()
                    .find(|(answered, _)| *answered == name)
                    .map(|(_, value)| value.as_str())
            });
            Ok(PostResponse::recorded(
                self.status,
                std::io::Cursor::new(self.body.clone().into_bytes()),
            )
            .with_tier(Tier::read(self.tier.as_deref()))
            .with_named(named))
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

    fn post_reading<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        cancel: &'a Cancel,
        reading: &'static [&'static str],
    ) -> BoxFuture<'a, Result<PostResponse, TransportError>> {
        (**self).post_reading(url, headers, body, cancel, reading)
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

#[cfg(test)]
mod held_tests {
    use super::*;

    /// A request held back is not a network that failed for a moment: it is
    /// not retried, and it is known to have been sent nowhere.
    #[test]
    fn a_held_request_is_not_retried_and_was_sent_nowhere() {
        let said = "nothing was sent: key:google waits for an answer";
        let error = TransportError::Held(said.into()).for_provider("google");
        assert!(!error.transient(), "{error:?}");
        assert!(matches!(&error, ProviderError::Held(_)), "{error:?}");
        assert!(error.to_string().contains(said), "{error}");
    }
}
