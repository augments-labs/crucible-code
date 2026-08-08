//! The port every provider sends through.
//!
//! One method, because one method is all a provider needs: post a body, get a
//! status and a stream back. Keeping it this narrow is what lets the whole wire
//! protocol be tested against a recorded response, with no socket and no server
//! anywhere in the test.
//!
//! It is also the seam for the HTTP client itself. [`crate::http`] is the only
//! file that names one; swapping it is that file and nothing else.

use std::fmt;
use std::io::{self, Read};

/// Why a request did not produce a response.
///
/// A response that arrived and said no is not an error here — that is a status,
/// and the provider turns it into its own refusal with the message the vendor
/// sent.
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// The request could not be sent, or the connection failed.
    #[error("{0}")]
    Unreachable(Box<str>),

    /// The connection broke while the response was being read.
    #[error("{0}")]
    Io(#[from] io::Error),
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
    /// # Errors
    ///
    /// [`TransportError`] if the request could not be sent. A response with a
    /// status the caller dislikes is not an error.
    fn post(
        &self,
        url: &str,
        headers: &[(Box<str>, Box<str>)],
        body: &str,
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
        headers: &[(Box<str>, Box<str>)],
        body: &str,
    ) -> Result<Response, TransportError> {
        if let Ok(mut sent) = self.sent.lock() {
            sent.push(Sent {
                url: url.to_owned(),
                headers: headers
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect(),
                body: body.to_owned(),
            });
        }

        Ok(Response {
            status: self.status,
            body: Box::new(io::Cursor::new(self.body.clone().into_bytes())),
        })
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
        headers: &[(Box<str>, Box<str>)],
        body: &str,
    ) -> Result<Response, TransportError> {
        (**self).post(url, headers, body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_replay_keeps_what_was_sent() {
        let replay = Replay::new(200, "body");
        let headers = [("x-key".into(), "value".into())];

        replay
            .post("https://example.test/v1", &headers, "{}")
            .unwrap();

        let sent = replay.sent();
        assert_eq!(sent.url, "https://example.test/v1");
        assert_eq!(sent.body, "{}");
        assert_eq!(sent.headers.first().unwrap().0, "x-key");
    }

    #[test]
    fn a_replay_answers_with_the_recorded_response() {
        let replay = Replay::new(429, "slow down");

        let mut response = replay.post("https://example.test/v1", &[], "{}").unwrap();
        let mut read = String::new();
        response.body.read_to_string(&mut read).unwrap();

        assert_eq!(response.status, 429);
        assert_eq!(read, "slow down");
    }
}
