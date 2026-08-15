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

use std::fmt;
use std::io::Read;

use crucible_core::{Cancel, Outgoing, ProviderError};

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

    /// The isolated request-setup worker stopped without reporting an outcome.
    #[error("request setup stopped unexpectedly")]
    SetupStopped,

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
            Self::SetupStopped => ProviderError::Transport {
                provider,
                problem: "request setup stopped unexpectedly".into(),
            },
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

/// Somewhere to send a request.
pub trait Transport: Send + Sync + fmt::Debug {
    /// Posts `body` and returns the response as it begins to arrive.
    ///
    /// Returns rather than reads: the body is a stream of events that lasts as
    /// long as the model is talking, so reading it here would mean waiting for
    /// the whole answer before showing any of it.
    ///
    /// Headers and body move into the transport because blocking setup may
    /// finish after a cancelled caller returns. Moving them gives that setup
    /// an owned lifetime without duplicating a transcript-sized body.
    ///
    /// # Errors
    ///
    /// [`TransportError`] if the request could not be sent or was cancelled
    /// before its response headers arrived. A response with a status the caller
    /// dislikes is not an error.
    fn post(
        &self,
        url: &str,
        headers: Outgoing,
        body: String,
        cancel: &Cancel,
    ) -> Result<Response, TransportError>;
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
    fn post(
        &self,
        url: &str,
        headers: Outgoing,
        body: String,
        cancel: &Cancel,
    ) -> Result<Response, TransportError> {
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

        Ok(Response {
            status: self.status,
            body: Box::new(std::io::Cursor::new(self.body.clone().into_bytes())),
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

/// A shared transport is still a transport.
///
/// Only tests need this: a provider takes ownership of the one it sends
/// through, and a test wants a second handle to ask what went out.
#[cfg(test)]
impl<T: Transport> Transport for std::sync::Arc<T> {
    fn post(
        &self,
        url: &str,
        headers: Outgoing,
        body: String,
        cancel: &Cancel,
    ) -> Result<Response, TransportError> {
        (**self).post(url, headers, body, cancel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_keeps_what_was_sent() {
        let replay = Replay::new(200, "body");
        let mut headers = Outgoing::new();
        headers.set_header("x-key", "value");

        replay
            .post(
                "https://example.test/v1",
                headers,
                "{}".to_owned(),
                &Cancel::new(),
            )
            .unwrap();

        let sent = replay.sent();
        assert_eq!(sent.url, "https://example.test/v1");
        assert_eq!(sent.body, "{}");
        assert_eq!(sent.headers.first().unwrap().0, "x-key");
    }

    #[test]
    fn a_replay_answers_with_the_recorded_response() {
        let replay = Replay::new(429, "slow down");

        let mut response = replay
            .post(
                "https://example.test/v1",
                Outgoing::new(),
                "{}".to_owned(),
                &Cancel::new(),
            )
            .unwrap();
        let mut read = String::new();
        response.body.read_to_string(&mut read).unwrap();

        assert_eq!(response.status, 429);
        assert_eq!(read, "slow down");
    }

    #[test]
    fn transport_cancellation_stays_a_cancel_at_the_provider_boundary() {
        let problem = TransportError::Cancelled.for_provider("test");

        assert!(matches!(problem, ProviderError::Cancelled("test")));
    }
}
