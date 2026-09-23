//! The client, and the deadlines a request is sent under.
//!
//! hyper has no deadline for any part of a request, and no event for "the
//! head was written" or "the body was written". What it does have is the
//! order it works in: it takes the body's first frame only once the head is
//! encoded, and each later frame only while its write buffer has room, so the
//! body sees each step as it happens. [`Body`] hands its bytes over in 16 KiB
//! frames and marks the moment its first frame is asked for and the moment
//! its last one is taken; the client marks the moment it has a connection;
//! the response future resolving ends the last step. Each of the three steps
//! between those moments gets a minute of its own ([`Phase`]), timed from when
//! it began, so a slow upload does not use up the time a slow answer needs.
//! The head's own minute ends when hyper has encoded it, before it is written,
//! so writing it counts towards the body's or the answer's minute: about two
//! minutes before the response head at worst, where the previous client gave
//! three.
//!
//! What the steps measure is what hyper has been handed, which can lead what
//! the socket has taken by hyper's 64 KiB buffer and one more 16 KiB frame,
//! and over TLS by up to another 64 KiB that rustls holds.

use std::convert::Infallible;
use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::{Pin, pin};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};
use std::time::Duration;

use crucible_credentials::Outgoing;
use hyper::body::{Bytes, Frame, Incoming, SizeHint};
use hyper::header::{HeaderValue, USER_AGENT};
use hyper::{Method, Request, Response};
use hyper_util::client::legacy::Client;
use hyper_util::client::legacy::connect::{Connect, capture_connection};
use tokio::time::{Instant, sleep_until};

use crate::connect::{ConnectError, Connector, Tls};
use crate::dns::{Lookups, PlainLookups};
use crate::proxy::{ProxyEnv, Route, select};
use crate::tasks::Tasks;

/// The `user-agent` a request is sent with when it names none: what the
/// previous client sent, so a server sees the same user-agent. That client
/// also added `accept: */*` to a request naming no `accept`; this one does not.
pub const DEFAULT_USER_AGENT: &str = "ureq/3.4.2";

/// How long each part of a request may take before the response head.
///
/// The response body has no clock here. It is the model talking, which can
/// take minutes, and a deadline cannot tell a long answer from a dead peer.
const TIMEOUT_HEAD: Duration = Duration::from_mins(1);

/// How much of a response head may be buffered without finding its end, and
/// the most fields it may have.
///
/// hyper parses what it has before each read and refuses once this much is
/// buffered with no end in sight, but one read may take up to this much
/// again: a head that ends inside the read crossing the bound still parses.
/// The largest head accepted is therefore under 128 KiB, where the previous
/// client cut at exactly 64 KiB; anything larger is refused.
const MAX_HEAD: usize = 64 * 1024;
const MAX_FIELDS: usize = 128;

/// How much of a request body is handed to hyper at a time.
const FRAME: usize = 16 * 1024;

/// A pooled HTTP/1.1 client, and the policy it sends under.
///
/// A 3xx is handed back as the response it is and never followed, and every
/// other status the same way: nothing here decides a status is an error.
///
/// The tasks hyper-util spawns for it (its connections and the waits around
/// them) are its own, and are aborted when the last clone of it is dropped, a
/// response still being read with them. A hostname lookup already on a
/// blocking worker is not one of them: it runs until the platform answers.
#[derive(Clone, Debug)]
pub struct Http {
    client: Client<Connector, Body>,
    /// Held, never read: dropping the last one aborts every task in it.
    _tasks: Arc<Tasks>,
    /// The proxy settings its connector routes by, to find the credential a
    /// request will travel with.
    env: Arc<ProxyEnv>,
}

/// The part of a request whose minute ran out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Phase {
    /// From having a connection to the head being encoded, which comes before
    /// it is written, so this minute ends almost at once.
    Head,
    /// From the body's first frame to its last.
    Body,
    /// From the body's last frame to the response head.
    Answer,
}

/// A request that produced no response.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The request could not be built from what it was given.
    #[error("request URL or header was invalid")]
    Invalid(#[from] hyper::http::Error),
    /// The URL names neither `http` nor `https`, so it would have been sent
    /// without the scheme that says whether to verify its recipient; or it
    /// names no host, so there is no recipient to verify, look up or ask a
    /// proxy for.
    #[error("request URL was invalid")]
    Unverifiable,
    /// One part of the request outlived its minute.
    #[error("request timed out")]
    Stalled(Phase),
    /// The exchange failed: see [`HttpError::connect`] for a connection that
    /// was never made. A response head over its limits is hyper's
    /// `is_parse_too_large`.
    #[error("HTTP request failed")]
    Exchange(#[from] hyper_util::client::legacy::Error),
}

impl HttpError {
    /// The connection that could not be made, when that is why.
    #[must_use]
    pub fn connect(&self) -> Option<&ConnectError> {
        match self {
            Self::Exchange(error) => std::error::Error::source(error)?.downcast_ref(),
            Self::Invalid(_) | Self::Unverifiable | Self::Stalled(_) => None,
        }
    }
}

impl Http {
    /// A client that routes each request as `env` says, looks a target it
    /// connects to directly up with `target`, and a proxy's host with
    /// `proxy_host`.
    ///
    /// Each client has its own pool: a client is what requests that may
    /// share connections share.
    #[must_use]
    pub fn new(tls: &Tls, target: Lookups, proxy_host: PlainLookups, env: ProxyEnv) -> Self {
        let env = Arc::new(env);
        let tasks = Tasks::new();
        let connector = Connector::new(tls, target, proxy_host, Arc::clone(&env));
        Self {
            client: client(connector, &tasks),
            _tasks: tasks,
            env,
        }
    }

    /// Sends one request and hands back its response head, whatever its
    /// status.
    ///
    /// `headers` go out as given, with [`DEFAULT_USER_AGENT`] added when they
    /// name no `user-agent`. When the request is to go through a proxy that
    /// is sent a credential, that credential is registered on `headers` for
    /// redaction ([`Outgoing::protect`]).
    ///
    /// # Errors
    ///
    /// [`HttpError`] when no response head arrived.
    pub async fn send(
        &self,
        method: Method,
        url: &str,
        headers: &mut Outgoing,
        body: String,
    ) -> Result<Response<Incoming>, HttpError> {
        let mut request = Request::builder().method(method).uri(url);
        for (name, value) in headers.headers() {
            request = request.header(&**name, &**value);
        }
        let mut request = request.body(body)?;
        let uri = request.uri();
        if !matches!(uri.scheme_str(), Some("http" | "https"))
            || uri.host().is_none_or(str::is_empty)
        {
            return Err(HttpError::Unverifiable);
        }
        if let Route::Tunnel(proxy) = select(&self.env, request.uri()) {
            proxy.protect(headers);
        }
        if !request.headers().contains_key(USER_AGENT) {
            let agent = HeaderValue::from_static(DEFAULT_USER_AGENT);
            request.headers_mut().insert(USER_AGENT, agent);
        }
        exchange(&self.client, request).await
    }
}

/// The client every [`Http`] is, over whatever connects it, spawning into
/// `tasks`.
pub(crate) fn client<C: Connect + Clone>(connector: C, tasks: &Arc<Tasks>) -> Client<C, Body> {
    Client::builder(tasks.spawner())
        .http1_max_buf_size(MAX_HEAD)
        .http1_max_headers(MAX_FIELDS)
        .build(connector)
}

/// Sends `request`, giving each [`Phase`] of it a minute.
pub(crate) async fn exchange<C>(
    client: &Client<C, Body>,
    request: Request<String>,
) -> Result<Response<Incoming>, HttpError>
where
    C: Connect + Clone + Send + Sync + 'static,
{
    let stage = Arc::new(Stage(Mutex::new(None)));
    let mut request = request.map(|text| Body {
        rest: Bytes::from(text),
        stage: Arc::clone(&stage),
    });
    let mut chosen = capture_connection(&mut request);
    let mut answer = pin!(client.request(request));
    let mut connected = pin!(async move {
        let _ = chosen.wait_for_connection_metadata().await;
    });
    let mut connecting = true;
    let mut clock = pin!(sleep_until(Instant::now()));
    poll_fn(|cx| {
        if let Poll::Ready(answered) = answer.as_mut().poll(cx) {
            return Poll::Ready(answered.map_err(HttpError::from));
        }
        if connecting && connected.as_mut().poll(cx).is_ready() {
            connecting = false;
            stage.reach(Phase::Head);
        }
        // A step that ended since the clock was set moves the clock on to
        // the next step's minute; one that did not has outlived its own.
        while let Some((phase, deadline)) = stage.deadline() {
            if clock.deadline() != deadline {
                clock.as_mut().reset(deadline);
            }
            if clock.as_mut().poll(cx).is_pending() {
                return Poll::Pending;
            }
            if stage.deadline().map(|(now, _)| now) == Some(phase) {
                return Poll::Ready(Err(HttpError::Stalled(phase)));
            }
        }
        Poll::Pending
    })
    .await
}

/// Which part of a request is under way, and since when; `None` until a
/// connection is had, which has a deadline of its own.
struct Stage(Mutex<Option<(Phase, Instant)>>);

impl Stage {
    /// Moves on to `phase` now, unless the request is already past it.
    fn reach(&self, phase: Phase) {
        let mut stage = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if stage.is_none_or(|(now, _)| now < phase) {
            *stage = Some((phase, Instant::now()));
        }
    }

    fn deadline(&self) -> Option<(Phase, Instant)> {
        let stage = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        stage.map(|(phase, since)| (phase, since + TIMEOUT_HEAD))
    }
}

/// A request body, handed over a frame at a time so its progress can be seen.
///
/// The text is moved in, not copied, and each frame is a slice of it.
pub(crate) struct Body {
    rest: Bytes,
    stage: Arc<Stage>,
}

impl hyper::body::Body for Body {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Bytes>, Infallible>>> {
        self.stage.reach(Phase::Body);
        let take = self.rest.len().min(FRAME);
        let frame = self.rest.split_to(take);
        if self.rest.is_empty() {
            self.stage.reach(Phase::Answer);
        }
        Poll::Ready((!frame.is_empty()).then(|| Ok(Frame::data(frame))))
    }

    fn is_end_stream(&self) -> bool {
        self.rest.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::with_exact(u64::try_from(self.rest.len()).unwrap_or(u64::MAX))
    }
}

impl Drop for Body {
    /// hyper drops a body once it has taken all of it, and at once when there
    /// is none, so either way what is left is the answer.
    fn drop(&mut self) {
        self.stage.reach(Phase::Answer);
    }
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body")
            .field("left", &self.rest.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests;
