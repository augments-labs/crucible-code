//! The one file that knows which HTTP client this is.
//!
//! Everything above it sees [`Transport`] and nothing else, the same way the
//! renderer sees a terminal port rather than a terminal library. Replacing the
//! client is this file.
//!
//! Blocking on purpose. A turn owns a thread for as long as the model is
//! talking, so there is nothing here for an async runtime to interleave — it
//! would be a scheduler brought in to serve one socket.

use std::io::{self, Read};
use std::time::Duration;

use super::{Response, Transport, TransportError};

/// How long to wait for the response to *start*.
///
/// Only the head is given a deadline. The body is the model talking, which
/// legitimately takes minutes, and a deadline cannot tell a long answer from a
/// dead connection — the user can, which is why what bounds the body is
/// [`TIMEOUT_QUIET`] and a cancel rather than a clock.
const TIMEOUT_HEAD: Duration = Duration::from_mins(1);

/// How long one read of the body waits before saying nothing came.
///
/// Not a deadline: a read that expires means "nothing yet", and the caller asks
/// again. What it buys is that the wait is *interruptible*. Left blocking, a
/// read sits on the socket for as long as the peer stays silent, so a user who
/// presses Esc against a provider that has stopped talking waits alongside it —
/// with no bound at all, because the body deliberately has none.
///
/// A quarter of a second is below what a person reads as an instant, and four
/// wakeups a second are nothing beside the parsing they interleave with. This
/// client measures a body read against the head's deadline as well, so a read
/// waits a second rather than this once a response has been arriving for longer
/// than [`TIMEOUT_HEAD`]. Both expire the same way.
const TIMEOUT_QUIET: Duration = Duration::from_millis(250);

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
            // Every model request carries a credential. A redirect is a new
            // recipient, and non-standard credential headers such as
            // `x-api-key` are not covered by a client's cross-host stripping.
            // Hand the 3xx back to the provider as a refusal instead.
            .max_redirects(0)
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

impl Https {
    /// Fetches `url`, for the caller asking a question of a server rather than
    /// of a model.
    ///
    /// Inherent rather than a second method on [`Transport`]: that port is what
    /// a provider sends through, and one method is the whole of what a provider
    /// needs. Widening it would make every fake in this workspace implement a
    /// verb no provider uses.
    ///
    /// # Errors
    ///
    /// [`TransportError`] if the request could not be sent. A response with a
    /// status the caller dislikes is not an error.
    pub fn get(&self, url: &str, headers: &[(&str, &str)]) -> Result<Response, TransportError> {
        let mut request = self.agent.get(url);
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        match request.call() {
            Ok(response) => Ok(Response {
                status: response.status().as_u16(),
                body: reader(response.into_body()),
            }),
            Err(problem) => Err(TransportError::Unreachable(problem.to_string().into())),
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
        // On this request and not on the agent: it is the answer to a model
        // that pauses mid-sentence, and the caller reading it is the one that
        // holds a cancel. The other direction — a body fetched to the end with
        // nobody waiting on it — is better off blocking, and keeps the deadline
        // it has.
        let mut request = self
            .agent
            .post(url)
            .config()
            .timeout_recv_body(Some(TIMEOUT_QUIET))
            .build();
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
                    body: Box::new(Waiting(reader(response.into_body()))),
                })
            }
            Err(problem) => Err(TransportError::Unreachable(problem.to_string().into())),
        }
    }
}

/// A body whose reads give up waiting instead of holding the thread.
///
/// The wait expiring is not a failure — the response is still open and the model
/// is still thinking — so it arrives as [`io::ErrorKind::Interrupted`], the one
/// kind the `Read` contract says to retry. The streaming reader reads through
/// `fill_buf`, which does not retry it, so a pause hands the turn back and the
/// cancel gets looked at.
///
/// What it gives up is a bound nobody meant to have: this client measures a body
/// read against the head's deadline too, so a peer that went silent used to fail
/// the turn a minute after the head arrived — and so did a model that thought for
/// that long, which is the failure worth losing the other one to prevent. Nothing
/// here ends a silent response now but the caller or the socket.
///
/// Which is why a caller that retries has to say when it will stop. `read_to_end`
/// does not: it retries an interruption for ever, so a body handed to it against
/// a peer that stalls without closing is a thread that never comes back. The one
/// caller that reads a whole body — [`refusal`] — carries its own deadline for
/// that reason, and no new one may read a body without one.
///
/// [`refusal`]: crate::refusal
struct Waiting(Box<dyn Read + Send>);

impl Read for Waiting {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        match self.0.read(into) {
            Err(problem) if expired(&problem) => Err(io::ErrorKind::Interrupted.into()),
            read => read,
        }
    }
}

/// Whether a failed read is this client's own wait running out.
///
/// Any of its clocks, because a response still arriving after a minute is the
/// ordinary case here rather than a broken one.
fn expired(problem: &io::Error) -> bool {
    problem
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<ureq::Error>())
        .is_some_and(|inner| matches!(inner, ureq::Error::Timeout(_)))
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
    use std::sync::mpsc;
    use std::thread;
    use std::time::Instant;

    use super::*;

    /// Serves a body in two halves with a pause between them, the way a model
    /// that stops to think does. The pause is longer than [`TIMEOUT_QUIET`], so
    /// a read of the second half is one that gave up waiting first.
    fn pausing(first: &'static str, then: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                heard(&stream);
                let head = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\n\r\n{first}",
                    first.len() + then.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.flush();
                thread::sleep(TIMEOUT_QUIET * 2);
                let _ = stream.write_all(then.as_bytes());
                let _ = stream.flush();
            }
        });

        format!("http://{address}/v1/messages")
    }

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

    /// Serves a redirect and reports whether the client contacted its target.
    fn redirecting() -> (String, mpsc::Receiver<bool>) {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_address = target.local_addr().unwrap();
        let (reported, reached) = mpsc::channel();

        thread::spawn(move || {
            let until = Instant::now() + Duration::from_millis(250);
            while Instant::now() < until {
                match target.accept() {
                    Ok((stream, _)) => {
                        heard(&stream);
                        let _ = reported.send(true);
                        return;
                    }
                    Err(problem) if problem.kind() == io::ErrorKind::WouldBlock => {
                        thread::yield_now();
                    }
                    Err(_) => break,
                }
            }
            let _ = reported.send(false);
        });

        let response = format!(
            "HTTP/1.1 302 Found\r\nlocation: http://{target_address}/stolen\r\ncontent-length: 0\r\n\r\n"
        );
        (once(response), reached)
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
    fn a_redirect_cannot_choose_a_new_recipient_for_a_credential() {
        let (url, reached) = redirecting();
        let headers = [(Box::from("x-api-key"), Box::from("canary-secret"))];

        let response = Https::new().post(&url, &headers, "{}").unwrap();

        assert_eq!(response.status, 302, "the redirect was followed");
        assert!(
            !reached.recv().unwrap(),
            "the redirect target was contacted"
        );
    }

    #[test]
    fn a_read_that_gives_up_waiting_says_so_rather_than_reporting_a_broken_stream() {
        // The whole of what the streaming path needs: the pause comes back as
        // something to retry, and the answer that follows it still arrives. A
        // pause read as a failure is a model that thought for too long and lost
        // the turn for it.
        let url = pausing("half ", "and the rest");
        let mut response = Https::new().post(&url, &[], "{}").unwrap();

        let mut said = Vec::new();
        let mut waited = 0;
        let mut into = [0_u8; 64];
        loop {
            match response.body.read(&mut into) {
                Ok(0) => break,
                Ok(read) => said.extend_from_slice(into.get(..read).unwrap_or_default()),
                Err(problem) if problem.kind() == io::ErrorKind::Interrupted => waited += 1,
                Err(problem) => panic!("the pause was reported as a failure: {problem}"),
            }
        }

        assert!(waited > 0, "the read never gave up waiting");
        assert_eq!(String::from_utf8(said).unwrap(), "half and the rest");
    }

    #[test]
    fn a_body_read_to_the_end_waits_through_the_pauses_in_it() {
        // What a caller reading a whole body has to survive: a message that
        // arrived in two pieces, with a wait between them that expired. Read
        // any other way, the pause would come back as a read error and the
        // sentence naming the model that does not exist would be lost.
        //
        // `read_to_end` is what this test uses and what a refusal must not:
        // waiting through a pause and waiting for ever are the same behaviour
        // until the peer stops talking, which is `refusal`'s deadline's job and
        // is proved there.
        let url = pausing("{\"error\":", "\"nope\"}");
        let mut response = Https::new().post(&url, &[], "{}").unwrap();

        let mut said = String::new();
        response.body.read_to_string(&mut said).unwrap();

        assert_eq!(said, "{\"error\":\"nope\"}");
    }

    /// A body that fails its first read with something the client said.
    struct Failing(Option<io::Error>);

    impl Read for Failing {
        fn read(&mut self, _into: &mut [u8]) -> io::Result<usize> {
            Err(self
                .0
                .take()
                .unwrap_or_else(|| io::ErrorKind::UnexpectedEof.into()))
        }
    }

    /// What one read of `body` came back as.
    fn read(body: io::Error) -> io::Error {
        Waiting(Box::new(Failing(Some(body))))
            .read(&mut [0_u8; 8])
            .expect_err("the body was given a failure to report")
    }

    #[test]
    fn a_wait_the_head_deadline_ended_is_still_only_a_wait() {
        // This client measures a body read against the head's deadline as well,
        // so every read of a response that has been arriving for longer than
        // that ends this way. Taken for a broken connection it fails an answer
        // that ran past a minute and then paused, which is an ordinary answer.
        let expired = io::Error::other(ureq::Error::Timeout(ureq::Timeout::RecvResponse));

        assert_eq!(read(expired).kind(), io::ErrorKind::Interrupted);
    }

    #[test]
    fn a_connection_that_broke_is_not_mistaken_for_a_wait() {
        // The other half: a stream retrying a dead socket forever is a turn
        // that never comes back and never says why.
        let broken = io::Error::from(io::ErrorKind::ConnectionReset);

        assert_eq!(read(broken).kind(), io::ErrorKind::ConnectionReset);
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
