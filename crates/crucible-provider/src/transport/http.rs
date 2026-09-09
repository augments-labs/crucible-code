//! The one file that knows which HTTP client this is.
//!
//! Everything above it sees [`Transport`] and nothing else, the same way the
//! renderer sees a terminal port rather than a terminal library. Replacing the
//! client is this file.
//!
//! Blocking on purpose. A turn owns a thread for as long as the model is
//! talking, so there is nothing here for an async runtime to interleave. The
//! blocking spans that cannot inspect the turn's cancel — request setup, and
//! each read of the body — each run on an owned worker while the provider
//! thread waits on a channel it can stop waiting on.

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, TryLockError};
use std::thread;
use std::time::Duration;

use crucible_core::{Cancel, Outgoing};

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
/// wakeups a second are nothing beside the parsing they interleave with. The
/// wait is this file's own rather than the client's: it is measured on the
/// thread that asked, against a worker that holds the socket, so nothing about
/// how long the response has already been arriving changes what one read does.
const TIMEOUT_QUIET: Duration = Duration::from_millis(250);

/// How often setup hands control back to check cancellation.
///
/// This wait is local — the blocking network operation remains on its worker
/// until it returns — so shortening it adds no socket traffic.
const CANCEL_POLL: Duration = Duration::from_millis(50);

/// How long to wait for a connection.
const TIMEOUT_CONNECT: Duration = Duration::from_secs(15);

/// How long a platform hostname lookup may retain request setup.
///
/// `getaddrinfo` has no portable cancellation operation. Ureq bounds its caller
/// with a resolver thread; if that deadline expires, every process-shared
/// transport fails fast so replacements cannot accumulate those threads.
const TIMEOUT_RESOLVE: Duration = Duration::from_secs(5);

/// An HTTPS transport.
#[derive(Debug)]
pub struct Https {
    shared: Arc<Shared>,
}

/// Process-lifetime connection and request-setup state.
///
/// Provider replacement constructs another [`Https`], so keeping the setup
/// slot on the handle would detach its worker when the old provider was
/// dropped. Every replacement instead reaps the same slot.
#[derive(Debug)]
struct Shared {
    agent: ureq::Agent,
    /// A cancelled setup whose blocking operation has not returned yet.
    setup: Mutex<Option<thread::JoinHandle<()>>>,
    /// Set after the platform resolver outlives its deadline.
    poisoned: Arc<AtomicBool>,
}

static SHARED: OnceLock<Arc<Shared>> = OnceLock::new();

impl Shared {
    fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(TIMEOUT_CONNECT))
            .timeout_send_request(Some(TIMEOUT_HEAD))
            .timeout_send_body(Some(TIMEOUT_HEAD))
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
            setup: Mutex::new(None),
            poisoned: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl Https {
    /// A transport over the process's pooled agent and request setup slot.
    ///
    /// The agent is what keeps the TLS handshake off every turn after the
    /// first, which is the difference between a turn starting in milliseconds
    /// and starting a network request.
    #[must_use]
    pub fn new() -> Self {
        Self {
            shared: Arc::clone(SHARED.get_or_init(|| Arc::new(Shared::new()))),
        }
    }

    /// A transport with isolated state for an adversarial socket test.
    #[cfg(test)]
    fn isolated() -> Self {
        Self {
            shared: Arc::new(Shared::new()),
        }
    }

    /// Exclusively owns request setup without making contention uninterruptible.
    fn setup_slot<'a>(
        &'a self,
        cancel: &Cancel,
    ) -> Result<MutexGuard<'a, Option<thread::JoinHandle<()>>>, TransportError> {
        loop {
            if cancel.requested() {
                return Err(TransportError::Cancelled);
            }

            match self.shared.setup.try_lock() {
                Ok(slot) => return Ok(slot),
                Err(TryLockError::Poisoned(problem)) => return Ok(problem.into_inner()),
                Err(TryLockError::WouldBlock) => thread::sleep(CANCEL_POLL),
            }
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
    /// `lifetime` covers resolution through the last response-body byte.
    ///
    /// # Errors
    ///
    /// [`TransportError`] if the request could not be sent. A response with a
    /// status the caller dislikes is not an error.
    pub fn get(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        lifetime: Duration,
    ) -> Result<Response, TransportError> {
        let mut request = self
            .shared
            .agent
            .get(url)
            .config()
            .timeout_global(Some(lifetime))
            .timeout_resolve(Some(lifetime))
            .build();
        for (name, value) in headers {
            request = request.header(*name, *value);
        }

        match request.call() {
            Ok(response) => Ok(Response {
                status: response.status().as_u16(),
                body: reader(response.into_body()),
            }),
            Err(problem) => Err(request_problem(&problem, &AtomicBool::new(false))),
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
        headers: Outgoing,
        body: String,
        cancel: &Cancel,
    ) -> Result<Response, TransportError> {
        if self.shared.poisoned.load(Ordering::Acquire) {
            return Err(TransportError::ResolveStalled);
        }
        if cancel.requested() {
            return Err(TransportError::Cancelled);
        }

        let mut setup = self.setup_slot(cancel)?;

        // A prior turn may have returned while `ureq` was still blocked. Wait
        // for that one worker rather than letting repeated cancels accumulate
        // threads and serialized request bodies.
        if let Some(worker) = setup.take() {
            while !worker.is_finished() {
                if cancel.requested() {
                    *setup = Some(worker);
                    return Err(TransportError::Cancelled);
                }
                thread::sleep(CANCEL_POLL);
            }
            worker.join().map_err(|_| TransportError::SetupStopped)?;
        }

        if self.shared.poisoned.load(Ordering::Acquire) {
            return Err(TransportError::ResolveStalled);
        }
        if cancel.requested() {
            return Err(TransportError::Cancelled);
        }

        // `ureq` is blocking across DNS, connect, TLS, request send and the
        // response headers. Ownership moves into this one worker so the caller
        // can keep asking the turn's flag without cloning the request body.
        let agent = self.shared.agent.clone();
        let poisoned = Arc::clone(&self.shared.poisoned);
        let url = url.to_owned();
        let (finished, result) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name("crucible-http-setup".to_owned())
            .spawn(move || {
                let _ = finished.send(send(&agent, &url, &headers, body, &poisoned));
            })
            .map_err(|problem| TransportError::Unreachable(problem.to_string().into()))?;

        let response = loop {
            if cancel.requested() {
                *setup = Some(worker);
                return Err(TransportError::Cancelled);
            }

            match result.recv_timeout(CANCEL_POLL) {
                Ok(response) => break response,
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    worker.join().map_err(|_| TransportError::SetupStopped)?;
                    return Err(TransportError::SetupStopped);
                }
            }
        };

        worker.join().map_err(|_| TransportError::SetupStopped)?;
        if cancel.requested() {
            Err(TransportError::Cancelled)
        } else {
            response
        }
    }
}

/// Performs the blocking part of one request on its setup thread.
fn send(
    agent: &ureq::Agent,
    url: &str,
    headers: &Outgoing,
    body: String,
    poisoned: &AtomicBool,
) -> Result<Response, TransportError> {
    // On this request and not on the agent: a stalled resolver poisons the
    // transport it happened on, and that verdict belongs to one request rather
    // than to every request the process will ever make.
    //
    // The body is left with no client-side clock at all. What bounds it is
    // [`Waiting`], because a clock cannot tell a long answer from a dead
    // connection and the user can.
    let mut request = agent
        .post(url)
        .config()
        .timeout_resolve(Some(TIMEOUT_RESOLVE))
        .timeout_recv_body(None)
        .build();
    for (name, value) in headers.headers() {
        request = request.header(&**name, &**value);
    }

    // Every status is a response; only a request that never produced one is an
    // error here, which is why this arm does not inspect the failure.
    match request.send(body) {
        Ok(response) => Ok(Response {
            status: response.status().as_u16(),
            body: Box::new(Waiting::new(reader(response.into_body()))?),
        }),
        Err(problem) => Err(request_problem(&problem, poisoned)),
    }
}

/// Maps client errors without retaining or displaying a configured URL.
fn request_problem(problem: &ureq::Error, poisoned: &AtomicBool) -> TransportError {
    if matches!(problem, ureq::Error::Timeout(ureq::Timeout::Resolve)) {
        poisoned.store(true, Ordering::Release);
        return TransportError::ResolveStalled;
    }

    let said = match problem {
        ureq::Error::Timeout(_) => "request timed out",
        ureq::Error::HostNotFound => "host was not found",
        ureq::Error::ConnectionFailed => "connection failed",
        ureq::Error::Tls(_) | ureq::Error::Rustls(_) => "TLS setup failed",
        ureq::Error::Io(_) => "connection I/O failed",
        ureq::Error::Protocol(_) => "HTTP protocol failed",
        ureq::Error::BadUri(_) => "request URL was invalid",
        _ => "HTTP request failed",
    };
    TransportError::Unreachable(said.into())
}

/// How much of the body one read of the socket may take off it.
///
/// The pump reads into a buffer this size and hands on what it got, so what is
/// held ahead of the caller is at most this and the one chunk the channel
/// carries. Small enough that a first token is not waiting behind a full one,
/// large enough that a long answer is not a wakeup per line.
const CHUNK: usize = 8 * 1024;

/// A body whose reads give up waiting instead of holding the thread.
///
/// The wait expiring is not a failure — the response is still open and the model
/// is still thinking — so it arrives as [`io::ErrorKind::Interrupted`], the one
/// kind the `Read` contract says to retry. The streaming reader reads through
/// `fill_buf`, which does not retry it, so a pause hands the turn back and the
/// cancel gets looked at.
///
/// The wait is this file's rather than the client's. A read of the socket blocks
/// for as long as the peer stays silent and no setting asks it not to, so the
/// blocking read moves to a worker and what the caller waits on is a channel it
/// can stop waiting on. That is the same shape request setup uses, for the same
/// reason, and it is the only bound on a silent response: nothing here ends one
/// but the caller or the socket.
///
/// Which is why a caller that retries has to say when it will stop. `read_to_end`
/// does not: it retries an interruption for ever, so a body handed to it against
/// a peer that stalls without closing is a thread that never comes back. The one
/// caller that reads a whole body — [`refusal`] — carries its own deadline for
/// that reason, and no new one may read a body without one.
///
/// A body dropped part-read leaves its worker on the socket until the peer says
/// something or closes. It is one thread, it holds no lock and the connection it
/// owns is not returned to the pool, so what an abandoned response costs is
/// bounded by the peer rather than by how many turns have been cancelled.
///
/// [`refusal`]: crate::refusal
struct Waiting {
    /// What the worker has read, in the order it read it, and then the end.
    chunks: mpsc::Receiver<io::Result<Option<Vec<u8>>>>,

    /// Asks the worker to stop before its next read and after it.
    stop: Arc<AtomicBool>,

    /// The chunk being handed out, and how much of it has been.
    held: (Vec<u8>, usize),

    /// What the body ended as, once it has.
    ended: Option<Ended>,
}

/// How a body stopped producing bytes.
///
/// Kept because `Read` is asked again after it answers. A failure that answered
/// `Ok(0)` the second time would read as a complete response, and the turn would
/// accept a truncated one as the whole of what the model said.
enum Ended {
    /// The worker said it had reached the end of the body.
    Complete,

    /// The worker reported this, and will report nothing else.
    Failed(io::ErrorKind),
}

/// What a body that stopped without saying why is reported as.
///
/// The worker says the body ended by sending the end, so the channel closing on
/// its own is the worker gone — it panicked, or it was unwound. Silence is what
/// a complete response also looks like, which is exactly why this cannot be read
/// as one: the bytes that did arrive would be accepted as the whole answer.
const CUT_SHORT: &str = "the response body stopped before it ended";

impl Waiting {
    /// Starts the worker that holds the socket for this body.
    fn new(body: Box<dyn Read + Send>) -> Result<Self, TransportError> {
        let (read, chunks) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        thread::Builder::new()
            .name("crucible-http-body".to_owned())
            .spawn(move || pump(body, &read, &stopping))
            .map_err(|problem| TransportError::Unreachable(problem.to_string().into()))?;

        Ok(Self {
            chunks,
            stop,
            held: (Vec::new(), 0),
            ended: None,
        })
    }

    /// Hands out what is left of the chunk in hand.
    fn hand_out(&mut self, into: &mut [u8]) -> usize {
        let (chunk, taken) = &mut self.held;
        let left = chunk.get(*taken..).unwrap_or_default();
        let giving = left.len().min(into.len());
        let (from, to) = (left.get(..giving), into.get_mut(..giving));
        if let (Some(from), Some(to)) = (from, to) {
            to.copy_from_slice(from);
        }
        *taken += giving;
        giving
    }
}

impl Read for Waiting {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        if into.is_empty() {
            return Ok(0);
        }

        let handed = self.hand_out(into);
        if handed > 0 {
            return Ok(handed);
        }

        match &self.ended {
            Some(Ended::Complete) => return Ok(0),
            Some(Ended::Failed(kind)) => return Err((*kind).into()),
            None => {}
        }

        match self.chunks.recv_timeout(TIMEOUT_QUIET) {
            Ok(Ok(Some(chunk))) => {
                self.held = (chunk, 0);
                Ok(self.hand_out(into))
            }
            Ok(Ok(None)) => {
                self.ended = Some(Ended::Complete);
                Ok(0)
            }
            Ok(Err(problem)) => {
                self.ended = Some(Ended::Failed(problem.kind()));
                Err(problem)
            }
            Err(RecvTimeoutError::Timeout) => Err(io::ErrorKind::Interrupted.into()),
            Err(RecvTimeoutError::Disconnected) => {
                self.ended = Some(Ended::Failed(io::ErrorKind::UnexpectedEof));
                Err(io::Error::new(io::ErrorKind::UnexpectedEof, CUT_SHORT))
            }
        }
    }
}

impl Drop for Waiting {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

/// Reads the body on its own thread for as long as anyone is still reading it.
///
/// Stops on the end of the body, on a failure, on the reader going away — the
/// channel reports that — and on the flag, which is checked on both sides of the
/// blocking read so a cancel that arrives during one is not slept through.
///
/// The end of the body is sent rather than left to the channel closing, because
/// closing is also what this thread dying looks like and the two must not be
/// answered the same way.
fn pump(
    mut body: Box<dyn Read + Send>,
    read: &mpsc::SyncSender<io::Result<Option<Vec<u8>>>>,
    stop: &AtomicBool,
) {
    let mut into = vec![0_u8; CHUNK];
    while !stop.load(Ordering::Acquire) {
        let got = body.read(&mut into);
        if stop.load(Ordering::Acquire) {
            return;
        }

        let sending = match got {
            Ok(0) => Ok(None),
            Ok(read) => Ok(Some(into.get(..read).unwrap_or_default().to_vec())),
            // The one kind the contract says to ask again about, and this is
            // the thread whose job is asking again.
            Err(problem) if problem.kind() == io::ErrorKind::Interrupted => continue,
            Err(problem) => Err(problem),
        };

        let last = !matches!(sending, Ok(Some(_)));
        if read.send(sending).is_err() || last {
            return;
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
    use std::sync::mpsc::{self, Receiver, Sender};
    use std::thread;
    use std::time::Instant;

    use super::*;

    /// How long a cancelled request may take to come back.
    ///
    /// Not a multiple of [`CANCEL_POLL`], deliberately. What these tests ask is
    /// whether a cancelled request returns at all or waits on the server, and
    /// the servers they run against never answer — so what is on the far side
    /// of this bound is not a slower cancellation, it is one that never
    /// arrives. A ceiling counted in polls says something narrower than that
    /// and something the machine can decide: a runner that descheduled a
    /// thread for a fifth of a second fails it, and the red test is about the
    /// runner rather than about the transport.
    const PROMPTLY: Duration = Duration::from_secs(2);

    /// Sends one test request through the owned transport boundary.
    fn post(
        transport: &Https,
        url: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Result<Response, TransportError> {
        let mut outgoing = Outgoing::new();
        for (name, value) in headers {
            outgoing.set_header(*name, *value);
        }
        transport.post(url, outgoing, body.to_owned(), &Cancel::new())
    }

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

    /// Starts a GET response, then withholds its end past the request lifetime.
    fn slow_get() -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                heard(&stream);
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 8\r\n\r\nhalf");
                let _ = stream.flush();
                thread::sleep(Duration::from_millis(400));
                let _ = stream.write_all(b"done");
            }
        });

        (format!("http://{address}/latest"), server)
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

    /// Accepts one request and withholds every response byte until released.
    fn stalling() -> (
        String,
        Receiver<()>,
        Sender<()>,
        Receiver<bool>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (arrived, accepted) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (reported, repeated) = mpsc::channel();

        let server = thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                heard(&stream);
                let _ = arrived.send(());
                let _ = released.recv_timeout(Duration::from_secs(2));
                drop(stream);

                listener.set_nonblocking(true).unwrap();
                let until = Instant::now() + Duration::from_millis(250);
                while Instant::now() < until {
                    match listener.accept() {
                        Ok(_) => {
                            let _ = reported.send(true);
                            return;
                        }
                        Err(problem) if problem.kind() == io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(_) => break,
                    }
                }
            }
            let _ = reported.send(false);
        });

        (
            format!("http://{address}/v1/messages"),
            accepted,
            release,
            repeated,
            server,
        )
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

        let mut response = post(&Https::new(), &url, &[], "{}").unwrap();
        let mut body = String::new();
        response.body.read_to_string(&mut body).unwrap();

        assert_eq!(response.status, 404);
        assert_eq!(body, said);
    }

    #[test]
    fn a_get_lifetime_includes_a_body_that_started_but_never_finished() {
        let (url, server) = slow_get();
        let since = Instant::now();
        let mut response = Https::isolated()
            .get(&url, &[], Duration::from_millis(100))
            .expect("the response head arrived");
        let mut body = String::new();

        let problem = response
            .body
            .read_to_string(&mut body)
            .expect_err("the body exceeded the request lifetime");
        let elapsed = since.elapsed();
        server.join().unwrap();

        assert!(elapsed < Duration::from_millis(300), "waited {elapsed:?}");
        assert_eq!(body, "half");
        assert!(problem.to_string().contains("timeout"), "{problem}");
    }

    #[test]
    fn a_redirect_cannot_choose_a_new_recipient_for_a_credential() {
        let (url, reached) = redirecting();
        let headers = [("x-api-key", "canary-secret")];

        let response = post(&Https::new(), &url, &headers, "{}").unwrap();

        assert_eq!(response.status, 302, "the redirect was followed");
        assert!(
            !reached.recv().unwrap(),
            "the redirect target was contacted"
        );
    }

    #[test]
    fn a_failed_request_does_not_repeat_an_endpoints_query_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || drop(listener.accept()));
        let endpoint = crate::Endpoint::parse(&format!("http://{address}/v1?token=hunter2"))
            .expect("the loopback endpoint to be accepted");

        let problem = post(&Https::new(), endpoint.as_str(), &[], "{}")
            .expect_err("the peer closed before a response");

        server.join().unwrap();
        assert!(!problem.to_string().contains("hunter2"), "{problem}");
        assert!(!format!("{problem:?}").contains("hunter2"));
    }

    #[test]
    fn a_read_that_gives_up_waiting_says_so_rather_than_reporting_a_broken_stream() {
        // The whole of what the streaming path needs: the pause comes back as
        // something to retry, and the answer that follows it still arrives. A
        // pause read as a failure is a model that thought for too long and lost
        // the turn for it.
        let url = pausing("half ", "and the rest");
        let mut response = post(&Https::new(), &url, &[], "{}").unwrap();

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
        let mut response = post(&Https::new(), &url, &[], "{}").unwrap();

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

    /// A body whose reads never arrive and never fail.
    struct Silent;

    impl Read for Silent {
        fn read(&mut self, _into: &mut [u8]) -> io::Result<usize> {
            thread::sleep(PROMPTLY);
            Ok(0)
        }
    }

    /// Reads `waiting` until it says something other than "nothing yet".
    ///
    /// Bounded, because the thing being proved is what the failure came back
    /// as, and a test that hangs to say so has stopped proving it.
    fn settled(waiting: &mut Waiting) -> io::Result<usize> {
        let by = Instant::now() + PROMPTLY;
        while Instant::now() < by {
            match waiting.read(&mut [0_u8; 8]) {
                Err(problem) if problem.kind() == io::ErrorKind::Interrupted => {}
                settled => return settled,
            }
        }
        panic!("the body never stopped waiting");
    }

    /// What reading `body` came back as, once it stopped waiting.
    fn read(body: io::Error) -> io::Error {
        let mut waiting =
            Waiting::new(Box::new(Failing(Some(body)))).expect("a reader for the body");

        settled(&mut waiting).expect_err("the body was given a failure to report")
    }

    #[test]
    fn a_connection_that_broke_is_not_mistaken_for_a_wait() {
        // A stream retrying a dead socket forever is a turn that never comes
        // back and never says why. Only this file's own wait is a wait; what
        // the socket said is what it said.
        let broken = io::Error::from(io::ErrorKind::ConnectionReset);

        assert_eq!(read(broken).kind(), io::ErrorKind::ConnectionReset);
    }

    #[test]
    fn a_body_that_failed_does_not_go_on_to_report_an_ending() {
        // `Read` gets asked again after it answers, and a failure that answered
        // `Ok(0)` the second time would read as a body that ended. The turn
        // would take a truncated answer for the whole of what the model said.
        let broken = io::Error::from(io::ErrorKind::ConnectionReset);
        let mut waiting =
            Waiting::new(Box::new(Failing(Some(broken)))).expect("a reader for the body");

        let first = settled(&mut waiting).expect_err("the body was given a failure to report");
        let again = settled(&mut waiting).expect_err("a failed body reported an ending");

        assert_eq!(first.kind(), io::ErrorKind::ConnectionReset);
        assert_eq!(again.kind(), io::ErrorKind::ConnectionReset);
    }

    #[test]
    fn a_worker_that_stopped_without_saying_so_is_not_a_body_that_ended() {
        // The worker sends the end, so the channel closing on its own is the
        // worker gone rather than the body finished. Answering that with
        // `Ok(0)` would hand the turn the bytes that did arrive as though they
        // were the whole of what the model said.
        let (sending, chunks) = mpsc::sync_channel::<io::Result<Option<Vec<u8>>>>(1);
        drop(sending);
        let mut waiting = Waiting {
            chunks,
            stop: Arc::new(AtomicBool::new(false)),
            held: (Vec::new(), 0),
            ended: None,
        };

        let first = settled(&mut waiting).expect_err("a body cut short reported an ending");
        let again = settled(&mut waiting).expect_err("a body cut short reported an ending");

        assert_eq!(first.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(again.kind(), io::ErrorKind::UnexpectedEof);
    }

    #[test]
    fn a_body_nobody_reads_any_more_does_not_hold_the_thread_that_dropped_it() {
        // What a cancelled turn does: stop reading and let go. Waiting for the
        // worker here would put the peer's silence back on the user, which is
        // the whole of what this shape exists to prevent.
        let waiting = Waiting::new(Box::new(Silent)).expect("a reader for the body");

        let began = Instant::now();
        drop(waiting);

        assert!(began.elapsed() < PROMPTLY, "{:?}", began.elapsed());
    }

    #[test]
    fn a_transport_is_debug_without_naming_a_request() {
        // `Transport` requires `Debug`, and everything sent through one carries
        // a key. Nothing here holds one, and nothing here may start to.
        let shown = format!("{:?}", Https::new());
        assert!(shown.starts_with("Https"), "unexpected debug: {shown}");
    }

    #[test]
    fn every_transport_reuses_the_process_connection_pool() {
        let first = Https::new();
        let replacement = Https::new();

        assert!(Arc::ptr_eq(&first.shared, &replacement.shared));
    }

    #[test]
    fn cancellation_while_response_headers_stall_returns_promptly() {
        let (url, accepted, release, _repeated, server) = stalling();
        let cancel = Cancel::new();
        let raise = cancel.clone();
        let (reported, raised_at) = mpsc::channel();
        let raiser = thread::spawn(move || {
            accepted.recv().unwrap();
            let now = Instant::now();
            raise.request();
            reported.send(now).unwrap();
        });

        let problem = Https::isolated()
            .post(&url, Outgoing::new(), "{}".to_owned(), &cancel)
            .expect_err("the stalled response was cancelled");
        let returned_at = Instant::now();
        let raised_at = raised_at.recv().unwrap();

        let _ = release.send(());
        raiser.join().unwrap();
        server.join().unwrap();

        assert!(matches!(problem, TransportError::Cancelled));
        assert!(
            returned_at.saturating_duration_since(raised_at) <= PROMPTLY,
            "cancellation took {:?}",
            returned_at.saturating_duration_since(raised_at)
        );
    }

    #[test]
    fn replacement_cannot_abandon_more_setup_workers() {
        let (url, accepted, release, repeated, server) = stalling();
        let transport = Https::isolated();
        let shared = Arc::clone(&transport.shared);
        let first = Cancel::new();
        let raise = first.clone();
        let first_raiser = thread::spawn(move || {
            accepted.recv().unwrap();
            raise.request();
        });

        let problem = transport
            .post(&url, Outgoing::new(), "first".to_owned(), &first)
            .expect_err("the first stalled response was cancelled");
        assert!(matches!(problem, TransportError::Cancelled));
        first_raiser.join().unwrap();
        drop(transport);

        for replacement in 0..3 {
            let transport = Https {
                shared: Arc::clone(&shared),
            };
            let cancel = Cancel::new();
            let raise = cancel.clone();
            let (reported, raised_at) = mpsc::channel();
            let raiser = thread::spawn(move || {
                thread::sleep(CANCEL_POLL * 2);
                let now = Instant::now();
                raise.request();
                reported.send(now).unwrap();
            });

            let problem = transport
                .post(
                    &url,
                    Outgoing::new(),
                    format!("replacement-{replacement}"),
                    &cancel,
                )
                .expect_err("waiting for prior setup remained cancellable");
            let returned_at = Instant::now();
            let raised_at = raised_at.recv().unwrap();

            raiser.join().unwrap();
            assert!(matches!(problem, TransportError::Cancelled));
            assert!(
                returned_at.saturating_duration_since(raised_at) <= PROMPTLY,
                "replacement {replacement} cancellation took {:?}",
                returned_at.saturating_duration_since(raised_at)
            );
        }

        let _ = release.send(());
        server.join().unwrap();
        assert!(
            !repeated.recv().unwrap(),
            "replacement setup reached server"
        );

        let url = once(refusal("200 OK", "ready"));
        let transport = Https { shared };
        let mut response = transport
            .post(&url, Outgoing::new(), "final".to_owned(), &Cancel::new())
            .expect("finished setup was reaped and the transport was reusable");
        let mut body = String::new();
        response.body.read_to_string(&mut body).unwrap();
        assert_eq!(body, "ready");
    }

    #[test]
    fn a_stalled_resolver_poisons_provider_replacements() {
        let transport = Https::isolated();
        let shared = Arc::clone(&transport.shared);
        let first = request_problem(
            &ureq::Error::Timeout(ureq::Timeout::Resolve),
            &transport.shared.poisoned,
        );
        assert!(matches!(first, TransportError::ResolveStalled));
        drop(transport);

        let replacement = Https { shared };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1/messages", listener.local_addr().unwrap());
        let problem = replacement
            .post(
                &url,
                Outgoing::new(),
                "x".repeat(1024 * 1024),
                &Cancel::new(),
            )
            .expect_err("a poisoned transport must not start another lookup");

        assert!(matches!(problem, TransportError::ResolveStalled));

        // That nothing was connected to, rather than that the refusal was
        // quick. The two say the same thing and only one of them is a fact
        // about this machine: a listener that accepted nothing is proof no
        // lookup was started, whatever the clock read while it was not.
        assert!(
            matches!(listener.accept(), Err(problem) if problem.kind() == io::ErrorKind::WouldBlock)
        );
    }

    #[test]
    fn a_host_that_does_not_resolve_is_unreachable_rather_than_a_status() {
        // `.invalid` is reserved by RFC 6761 and never resolves, so this test
        // needs no network and cannot reach anything if it has one.
        //
        // Isolated because it is the one test here that asks a real resolver
        // anything: a resolver that stalls sets the flag every later request
        // reads, and on the process-wide transport that is every other test in
        // this file failing for a lookup none of them made.
        let transport = Https::isolated();
        let problem = post(
            &transport,
            "https://crucible.invalid/v1/messages",
            &[],
            "{}",
        )
        .unwrap_err();

        // Refused or never answered — a resolver is not obliged to say which,
        // and both are this name failing to become a status.
        assert!(
            matches!(
                problem,
                TransportError::Unreachable(_) | TransportError::ResolveStalled
            ),
            "expected a host that does not resolve, got {problem:?}"
        );
    }

    #[test]
    fn a_stall_in_one_transport_is_not_the_whole_process() {
        let mine = Https::isolated();
        let problem = request_problem(
            &ureq::Error::Timeout(ureq::Timeout::Resolve),
            &mine.shared.poisoned,
        );

        assert!(matches!(problem, TransportError::ResolveStalled));
        assert!(mine.shared.poisoned.load(Ordering::Acquire));
        // The flag every other test in this file is read against. A test that
        // provokes a stall on the shared transport leaves this set, and the
        // tests that run after it fail on a lookup they never made.
        assert!(
            !Https::new().shared.poisoned.load(Ordering::Acquire),
            "a test poisoned the transport the rest of them share"
        );
    }

    #[test]
    fn a_cancelled_request_is_not_sent() {
        let cancel = Cancel::new();
        cancel.request();

        let problem = Https::new()
            .post(
                "https://crucible.invalid/v1/messages",
                Outgoing::new(),
                "{}".to_owned(),
                &cancel,
            )
            .unwrap_err();

        assert!(matches!(problem, TransportError::Cancelled));
    }
}
