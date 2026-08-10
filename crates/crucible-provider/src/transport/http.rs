//! The one file that knows which HTTP client this is.
//!
//! Everything above it sees [`Transport`] and nothing else, the same way the
//! renderer sees a terminal port rather than a terminal library. Replacing the
//! client is this file.
//!
//! Blocking on purpose. A turn owns a thread for as long as the model is
//! talking, so there is nothing here for an async runtime to interleave — it
//! would be a scheduler brought in to serve one socket.

use std::io::Read;
use std::time::Duration;

use super::{Response, Transport, TransportError};

/// How long to wait for the response to *start*.
///
/// Only the head is bounded. The body is the model talking, which legitimately
/// takes minutes, and a user who has stopped waiting presses Esc — a timeout
/// cannot tell a long answer from a dead connection, and the user can.
const TIMEOUT_HEAD: Duration = Duration::from_mins(1);

/// How long to wait for a connection.
const TIMEOUT_CONNECT: Duration = Duration::from_secs(15);

/// An HTTPS transport.
#[derive(Debug)]
pub struct Https {
    agent: ureq::Agent,
}

impl Https {
    /// A transport with one pooled agent.
    ///
    /// The agent is what keeps the TLS handshake off every turn after the
    /// first, which is the difference between a turn starting in milliseconds
    /// and starting in a round trip.
    #[must_use]
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(TIMEOUT_CONNECT))
            .timeout_recv_response(Some(TIMEOUT_HEAD))
            // A 4xx is an answer, not a failure to get one. Left as an error it
            // would arrive as a bare status with the body discarded, and the
            // body is the sentence naming the model that does not exist.
            .http_status_as_error(false)
            .build();

        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for Https {
    fn default() -> Self {
        Self::new()
    }
}

impl Transport for Https {
    fn post(
        &self,
        url: &str,
        headers: &[(Box<str>, Box<str>)],
        body: &str,
    ) -> Result<Response, TransportError> {
        let mut request = self.agent.post(url);
        for (name, value) in headers {
            request = request.header(&**name, &**value);
        }

        // Every status is a response; only a request that never produced one is
        // an error here, which is why this arm does not inspect the failure.
        match request.send(body) {
            Ok(response) => {
                let status = response.status().as_u16();
                Ok(Response {
                    status,
                    body: reader(response.into_body()),
                })
            }
            Err(problem) => Err(TransportError::Unreachable(problem.to_string().into())),
        }
    }
}

/// The body as something to read from.
///
/// Left unlimited on purpose: the framing above it bounds one event, which is
/// what stops a peer from exhausting memory. A ceiling on the whole response
/// would instead cut off a long answer part-way through.
fn reader(body: ureq::Body) -> Box<dyn Read + Send> {
    Box::new(body.into_reader())
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;

    use super::*;

    /// Serves one canned response on loopback and returns the URL for it.
    ///
    /// A real socket, because the branch worth testing is what the client does
    /// with a status — and a fake client would be asserting on the fake.
    fn once(response: String) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                heard(&stream);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });

        format!("http://{address}/v1/messages")
    }

    /// Reads the whole request off `stream`, headers and body both.
    ///
    /// All of it, because of what closing a socket does to what is left. A close
    /// with bytes still unread is a reset rather than a goodbye, and a reset
    /// throws away what was already sent — so the client is told the connection
    /// was aborted while the response it asked for sits unread in its own
    /// buffer. Stopping at the headers leaves the body behind, which is the same
    /// thing said a shorter way.
    fn heard(stream: &TcpStream) {
        let mut asked = BufReader::new(stream);
        let mut line = String::new();
        let mut body = 0;

        while asked.read_line(&mut line).unwrap_or(0) > 0 {
            if line.trim().is_empty() {
                break;
            }

            // Lowered because a header name is case-insensitive, and which case
            // a client picks is a detail of the client.
            let said = line.to_ascii_lowercase();
            if let Some(length) = said.strip_prefix("content-length:") {
                body = length.trim().parse().unwrap_or(0);
            }

            line.clear();
        }

        let _ = asked.read_exact(&mut vec![0_u8; body]);
    }

    /// A response with the status and body a vendor would send.
    fn refusal(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
    }

    #[test]
    fn a_refusal_arrives_as_a_status_with_the_body_that_says_why() {
        // The body is the whole value of a refusal: it is what names the model
        // that does not exist or the key that lacks access. A client that
        // reports the status and drops the body turns that into "HTTP 404".
        let said = r#"{"error":{"message":"model: claude-nope not found"}}"#;
        let url = once(refusal("404 Not Found", said));

        let mut response = Https::new().post(&url, &[], "{}").unwrap();
        let mut body = String::new();
        response.body.read_to_string(&mut body).unwrap();

        assert_eq!(response.status, 404);
        assert_eq!(body, said);
    }

    #[test]
    fn a_transport_is_debug_without_naming_a_request() {
        // `Transport` requires `Debug`, and everything sent through one carries
        // a key. Nothing here holds one, and nothing here may start to.
        let shown = format!("{:?}", Https::new());
        assert!(shown.starts_with("Https"), "unexpected debug: {shown}");
    }

    #[test]
    fn a_host_that_does_not_resolve_is_unreachable_rather_than_a_status() {
        // `.invalid` is reserved by RFC 6761 and never resolves, so this test
        // needs no network and cannot reach anything if it has one.
        let problem = Https::new()
            .post("https://crucible.invalid/v1/messages", &[], "{}")
            .unwrap_err();

        assert!(
            matches!(problem, TransportError::Unreachable(_)),
            "expected an unreachable host, got {problem:?}"
        );
    }
}
