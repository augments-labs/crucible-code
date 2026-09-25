//! The HTTP client provider requests are sent through.
//!
//! [`HttpTurns`] is the shared asynchronous service used by model turns and by
//! the web sources' `Search` and `Fetch` requests, which are the same post over
//! the same client. It is constructed by the application, lends cancellation
//! and an overall request deadline to every post, and drops the shared client's
//! future when either ends. It is the only client this crate reaches for: a
//! request whose URL a caller named is checked here, before the shared client
//! is handed it.

use std::error::Error as _;
use std::future::Future;
#[cfg(test)]
use std::io;
use std::time::{Duration, Instant};

use crucible_credentials::Outgoing;
use crucible_http::{ConnectError, Http, HttpError};
use crucible_runtime::{BoxFuture, Cancel};
use hyper::Method;

use super::{PostResponse, Transport, TransportError};
use crate::Endpoint;

/// The longest any one post takes from entering the shared client to its
/// response head, including the wait for one of the client's four setup slots.
///
/// Three minutes preserves the old client's all-post bound while putting one
/// whole bound around the shared client's otherwise unbounded slot wait and
/// its 2 min 15 s connection-and-head bound.
const REQUEST: Duration = Duration::from_mins(3);

/// Provider posts over one application-owned shared HTTP client.
#[derive(Clone, Debug)]
pub struct HttpTurns {
    http: Option<Http>,
}

impl HttpTurns {
    /// Wraps the shared client the application constructed.
    #[must_use]
    pub fn new(http: Http) -> Self {
        Self { http: Some(http) }
    }

    /// A transport that refuses every request because its TLS configuration
    /// could not be built.
    ///
    /// The application uses this rather than dropping to a plaintext client or
    /// inventing a second startup error. The refusal keeps the established TLS
    /// phrase at the point a request would otherwise have been sent.
    #[must_use]
    pub fn unavailable() -> Self {
        Self { http: None }
    }

    /// Awaits one post under the caller's cancel and the whole request bound.
    async fn send(
        &self,
        url: &str,
        headers: &mut Outgoing,
        body: String,
        cancel: &Cancel,
    ) -> Result<PostResponse, TransportError> {
        if cancel.requested() {
            return Err(TransportError::Cancelled);
        }
        if url.chars().any(char::is_whitespace) || Endpoint::parse(url).is_err() {
            return Err(TransportError::Unreachable(
                "request URL was invalid".into(),
            ));
        }
        let Some(http) = self.http.as_ref() else {
            return Err(TransportError::Unreachable("TLS setup failed".into()));
        };

        let sent = bounded(cancel, REQUEST, http.send(Method::POST, url, headers, body)).await;
        let Some(sent) = sent else {
            return Err(if cancel.requested() {
                TransportError::Cancelled
            } else {
                TransportError::Unreachable("request timed out".into())
            });
        };
        if cancel.requested() {
            return Err(TransportError::Cancelled);
        }

        sent.map(|response| PostResponse::network(response.status().as_u16(), response.into_body()))
            .map_err(|problem| request_problem(&problem))
    }
}

impl Transport for HttpTurns {
    fn post<'a>(
        &'a self,
        url: &'a str,
        headers: &'a mut Outgoing,
        body: String,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<PostResponse, TransportError>> {
        Box::pin(self.send(url, headers, body, cancel))
    }
}

/// Awaits `work` until it answers, the caller stops it, or `within` passes.
async fn bounded<T>(cancel: &Cancel, within: Duration, work: impl Future<Output = T>) -> Option<T> {
    let request = cancel.child_until(Instant::now().checked_add(within));
    request.race(work).await
}

/// Maps the shared client's errors to the phrases the provider client used.
fn request_problem(problem: &HttpError) -> TransportError {
    if head_refused(problem) {
        return TransportError::Unreachable("HTTP protocol failed".into());
    }

    match problem {
        HttpError::Invalid(_) => TransportError::Unreachable("HTTP protocol failed".into()),
        HttpError::Unverifiable => TransportError::Unreachable("request URL was invalid".into()),
        HttpError::Stalled(_) => TransportError::Unreachable("request timed out".into()),
        HttpError::Exchange(_) => problem.connect().map_or_else(
            || TransportError::Unreachable("HTTP request failed".into()),
            connection_problem,
        ),
    }
}

/// Maps a connection step to the target-shaped phrase the old client exposed.
fn connection_problem(problem: &ConnectError) -> TransportError {
    let said = match problem {
        ConnectError::ResolveStalled => return TransportError::ResolveStalled,
        ConnectError::Lookup(_) => "host was not found",
        ConnectError::Tcp(_) | ConnectError::Tunnel(_) => "connection failed",
        ConnectError::ProxyTls(_) | ConnectError::Tls(_) => "TLS setup failed",
        ConnectError::Unverifiable => "request URL was invalid",
        ConnectError::Deadline => "request timed out",
    };
    TransportError::Unreachable(said.into())
}

/// Whether an exchange failed because the response head exceeded its bound.
fn head_refused(problem: &HttpError) -> bool {
    let HttpError::Exchange(exchange) = problem else {
        return false;
    };
    let mut source = exchange.source();
    while let Some(cause) = source {
        if cause
            .downcast_ref::<hyper::Error>()
            .is_some_and(hyper::Error::is_parse_too_large)
        {
            return true;
        }
        source = cause.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::num::NonZeroUsize;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, mpsc};
    use std::thread;
    use std::time::Instant as StdInstant;

    use crucible_http::{Lookups, Phase, Poison, ProxyEnv, Tls};
    use tokio::io::AsyncReadExt;

    use super::*;

    /// How long a cancelled request may take to come back.
    const PROMPTLY: Duration = Duration::from_secs(2);

    fn shared() -> HttpTurns {
        let tls = Tls::new().expect("the pinned TLS configuration to build");
        let poison = Poison::default();
        let target = Lookups::poisoned(NonZeroUsize::MIN, &poison);
        let proxy = Lookups::plain(NonZeroUsize::MIN);
        HttpTurns::new(Http::new(&tls, target, proxy, ProxyEnv::capture()))
    }

    async fn post(
        transport: &HttpTurns,
        url: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Result<PostResponse, TransportError> {
        let mut outgoing = Outgoing::new();
        for (name, value) in headers {
            outgoing.set_header(*name, *value);
        }
        transport
            .post(url, &mut outgoing, body.to_owned(), &Cancel::new())
            .await
    }

    fn refusal(status: &str, body: &str) -> String {
        format!(
            "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}",
            body.len()
        )
    }

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
                thread::sleep(Duration::from_millis(500));
                let _ = stream.write_all(then.as_bytes());
                let _ = stream.flush();
            }
        });
        format!("http://{address}/v1/messages")
    }

    fn heard(stream: &TcpStream) {
        let mut asked = BufReader::new(stream);
        let mut line = String::new();
        let mut body = 0;
        while asked.read_line(&mut line).unwrap_or(0) > 0 {
            if line.trim().is_empty() {
                break;
            }
            let said = line.to_ascii_lowercase();
            if let Some(length) = said.strip_prefix("content-length:") {
                body = length.trim().parse().unwrap_or(0);
            }
            line.clear();
        }
        let _ = asked.read_exact(&mut vec![0_u8; body]);
    }

    /// A loopback source that answers `answer` on every connection it is sent,
    /// counting how many connections it was sent, so a test can read the pool
    /// rather than infer it.
    ///
    /// One connection is answered as many times as it is asked, and is held open
    /// between them: a source that closed after each answer would give a client
    /// nothing to reuse, and every request would then open a connection whatever
    /// the pool said. Each request head is read a byte at a time for the same
    /// reason — a buffered reader would hold back the bytes of the request after
    /// this one, which this thread has no way to hand on.
    fn pooling(answer: &'static str, connections: Arc<AtomicUsize>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        thread::spawn(move || {
            for arrived in listener.incoming() {
                let Ok(mut stream) = arrived else { break };
                connections.fetch_add(1, Ordering::SeqCst);
                while asked(&mut stream) && stream.write_all(answer.as_bytes()).is_ok() {
                    let _ = stream.flush();
                }
            }
        });
        format!("http://{address}/v1/messages")
    }

    /// Reads one request head and the body it names, saying whether the peer
    /// sent one at all.
    fn asked(stream: &mut TcpStream) -> bool {
        let mut head = Vec::new();
        let mut byte = [0_u8; 1];
        while stream.read(&mut byte).unwrap_or(0) == 1 {
            head.push(byte[0]);
            if head.ends_with(b"\r\n\r\n") {
                break;
            }
        }
        if !head.ends_with(b"\r\n\r\n") {
            return false;
        }

        let said = String::from_utf8_lossy(&head).to_ascii_lowercase();
        let body = said
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|length| length.trim().parse::<usize>().ok())
            .unwrap_or(0);
        let mut rest = vec![0_u8; body];
        stream.read_exact(&mut rest).is_ok()
    }

    /// Reads one post's body whole, so its connection goes back to the pool.
    async fn spent(response: PostResponse) {
        let mut body = response.into_reader();
        let mut said = Vec::new();
        let read = body.read_to_end(&mut said).await;
        assert_eq!(read.unwrap_or(0), 0, "the answer carried a body: {said:?}");
    }

    #[tokio::test]
    async fn a_refusal_arrives_as_a_status_with_the_body_that_says_why() {
        let expected = r#"{"error":{"message":"model: claude-nope not found"}}"#;
        let url = once(refusal("404 Not Found", expected));
        let transport = shared();
        let response = post(&transport, &url, &[], "{}").await.unwrap();
        let status = response.status();
        let mut body = response.into_reader();

        assert_eq!(status, 404);
        let mut said = Vec::new();
        body.read_to_end(&mut said).await.unwrap();
        assert_eq!(said, expected.as_bytes());
    }

    /// The pool is the client's, and every transport the application builds for
    /// a turn or a web request is the same client. One connection serves both,
    /// which is what "share one pool" means and what a second client would cost.
    #[tokio::test]
    async fn a_turn_and_a_web_request_are_served_over_one_pool() {
        let connections = Arc::new(AtomicUsize::new(0));
        let url = pooling(
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n",
            Arc::clone(&connections),
        );
        let turns = shared();
        let web = turns.clone();

        spent(
            post(&turns, &url, &[("x-api-key", "turn")], "{}")
                .await
                .unwrap(),
        )
        .await;
        spent(
            post(&web, &url, &[("x-api-key", "web")], "{}")
                .await
                .unwrap(),
        )
        .await;

        assert_eq!(
            connections.load(Ordering::SeqCst),
            1,
            "a turn and a web request did not share the one pool"
        );
    }

    /// Two clients are two pools, and that is the reading the one above rests
    /// on: a count of one connection only means something if two can be counted.
    #[tokio::test]
    async fn two_clients_open_one_connection_each() {
        let connections = Arc::new(AtomicUsize::new(0));
        let url = pooling(
            "HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n",
            Arc::clone(&connections),
        );

        spent(post(&shared(), &url, &[], "{}").await.unwrap()).await;
        spent(post(&shared(), &url, &[], "{}").await.unwrap()).await;

        assert_eq!(
            connections.load(Ordering::SeqCst),
            2,
            "the count could not tell two pools from one"
        );
    }

    #[tokio::test]
    async fn a_redirect_cannot_choose_a_new_recipient_for_a_credential() {
        let target = TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_address = target.local_addr().unwrap();
        let (reported, reached) = mpsc::channel();
        thread::spawn(move || {
            let until = StdInstant::now() + Duration::from_millis(250);
            while StdInstant::now() < until {
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
        let url = once(format!(
            "HTTP/1.1 302 Found\r\nlocation: http://{target_address}/stolen\r\ncontent-length: 0\r\n\r\n"
        ));

        let response = post(&shared(), &url, &[("x-api-key", "canary-secret")], "{}")
            .await
            .unwrap();

        assert_eq!(response.status(), 302);
        assert!(!reached.recv().unwrap());
    }

    #[tokio::test]
    async fn a_failed_request_does_not_repeat_an_endpoints_query_secret() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = thread::spawn(move || drop(listener.accept()));
        let endpoint = crate::Endpoint::parse(&format!("http://{address}/v1?token=hunter2"))
            .expect("the loopback endpoint to be accepted");

        let problem = post(&shared(), endpoint.as_str(), &[], "{}")
            .await
            .expect_err("the peer closed before a response");

        server.join().unwrap();
        assert!(
            matches!(problem, TransportError::Unreachable(ref said) if said.as_ref() == "HTTP request failed"),
            "{problem}"
        );
        assert!(!problem.to_string().contains("hunter2"), "{problem}");
        assert!(!format!("{problem:?}").contains("hunter2"));
    }

    #[tokio::test]
    async fn a_read_that_gives_up_waiting_says_so_rather_than_reporting_a_broken_stream() {
        let url = pausing("half ", "and the rest");
        let transport = shared();
        let response = post(&transport, &url, &[], "{}").await.unwrap();
        let mut body = response.into_reader();
        let mut said = Vec::new();
        let mut waited = 0;
        let mut into = [0_u8; 64];
        loop {
            match body.read(&mut into).await {
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
    fn a_transport_is_debug_without_naming_a_request() {
        let shown = format!("{:?}", shared());
        assert!(shown.starts_with("HttpTurns"), "unexpected debug: {shown}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_while_response_headers_stall_closes_the_request() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (arrived, accepted) = mpsc::channel();
        let server = thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return false;
            };
            heard(&stream);
            if arrived.send(()).is_err() {
                return false;
            }

            stream.set_read_timeout(Some(PROMPTLY)).unwrap();
            match stream.read(&mut [0_u8; 1]) {
                Ok(0) => true,
                Ok(_) => false,
                Err(problem)
                    if matches!(
                        problem.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    true
                }
                Err(problem)
                    if matches!(
                        problem.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    false
                }
                Err(problem) => panic!("the request ended unexpectedly: {problem}"),
            }
        });

        let cancel = Cancel::new();
        let raise = cancel.clone();
        let raiser = thread::spawn(move || {
            accepted.recv().unwrap();
            raise.request();
        });
        let mut outgoing = Outgoing::new();
        let transport = shared();
        let problem = transport
            .post(
                &format!("http://{address}/v1/messages"),
                &mut outgoing,
                "{}".to_owned(),
                &cancel,
            )
            .await
            .expect_err("the stalled response was cancelled");
        drop(transport);
        raiser.join().unwrap();
        let closed = server.join().unwrap();

        assert!(matches!(problem, TransportError::Cancelled));
        assert!(closed, "cancelling left the request open on the server");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelling_mid_body_closes_the_request_at_the_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (arrived, accepted) = mpsc::channel();
        let server = thread::spawn(move || {
            let Ok((mut stream, _)) = listener.accept() else {
                return false;
            };
            heard(&stream);
            let response = b"HTTP/1.1 200 OK\r\ncontent-length: 8\r\n\r\npart";
            if stream.write_all(response).is_err() {
                return false;
            }
            let _ = stream.flush();
            if arrived.send(()).is_err() {
                return false;
            }

            stream.set_read_timeout(Some(PROMPTLY)).unwrap();
            match stream.read(&mut [0_u8; 1]) {
                Ok(0) => true,
                Ok(_) => false,
                Err(problem)
                    if matches!(
                        problem.kind(),
                        io::ErrorKind::ConnectionReset
                            | io::ErrorKind::ConnectionAborted
                            | io::ErrorKind::BrokenPipe
                            | io::ErrorKind::UnexpectedEof
                    ) =>
                {
                    true
                }
                Err(problem)
                    if matches!(
                        problem.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    false
                }
                Err(problem) => panic!("the request ended unexpectedly: {problem}"),
            }
        });

        let transport = shared();
        let response = post(
            &transport,
            &format!("http://{address}/v1/messages"),
            &[],
            "{}",
        )
        .await
        .unwrap();
        let mut body = response.into_reader();
        let mut first = [0_u8; 4];
        body.read_exact(&mut first).await.unwrap();
        assert_eq!(&first, b"part");
        accepted.recv_timeout(PROMPTLY).unwrap();

        let cancel = Cancel::new();
        let (started_tx, started_rx) = mpsc::channel();
        let read_cancel = cancel.clone();
        let waiting = tokio::spawn(async move {
            let mut one = [0_u8; 1];
            started_tx.send(()).unwrap_or(());
            read_cancel.race(body.read(&mut one)).await
        });
        started_rx
            .recv_timeout(PROMPTLY)
            .expect("the body read to begin");
        cancel.request();
        assert!(waiting.await.unwrap().is_none());
        drop(transport);

        assert!(
            server.join().unwrap(),
            "cancelling mid-body left the request open"
        );
    }

    #[tokio::test]
    async fn an_unavailable_client_keeps_the_tls_refusal_phrase() {
        let problem = post(
            &HttpTurns::unavailable(),
            "https://crucible.invalid/v1/messages",
            &[],
            "{}",
        )
        .await
        .expect_err("a client without TLS to refuse the post");

        assert!(
            matches!(problem, TransportError::Unreachable(ref said) if said.as_ref() == "TLS setup failed"),
            "{problem}"
        );
    }

    #[tokio::test]
    async fn an_untrusted_post_target_is_refused_before_the_shared_client() {
        let transport = HttpTurns::unavailable();
        for address in [
            "https://docs.rs@evil.example/",
            "https://good.example/ https://evil.example/",
            "https://good.example/#https://evil.example/",
            "ftp://good.example/",
            "https:///v1",
            "http://192.0.2.1/v1",
        ] {
            let mut outgoing = Outgoing::new();
            let problem = transport
                .post(address, &mut outgoing, "{}".to_owned(), &Cancel::new())
                .await
                .map(|_| ())
                .map_err(|error| error.to_string());
            assert!(
                matches!(problem.as_ref(), Err(said) if said == "request URL was invalid"),
                "{address}: {problem:?}"
            );
        }
    }

    #[tokio::test]
    async fn the_whole_request_bound_also_bounds_the_setup_wait() {
        let cancel = Cancel::new();
        let since = StdInstant::now();
        let answer = bounded(
            &cancel,
            Duration::from_millis(20),
            std::future::pending::<()>(),
        )
        .await;

        assert_eq!(answer, None);
        assert!(since.elapsed() >= Duration::from_millis(20));
        assert!(!cancel.requested(), "the request deadline ended its parent");
    }

    #[test]
    fn shared_client_errors_keep_the_transport_phrases() {
        assert!(matches!(
            connection_problem(&ConnectError::ResolveStalled),
            TransportError::ResolveStalled
        ));
        for (problem, phrase) in [
            (
                ConnectError::Lookup(Box::new(io::Error::other("lookup"))),
                "host was not found",
            ),
            (
                ConnectError::Tcp(Box::new(io::Error::other("tcp"))),
                "connection failed",
            ),
            (
                ConnectError::Tunnel(Box::new(io::Error::other("tunnel"))),
                "connection failed",
            ),
            (
                ConnectError::ProxyTls(io::Error::other("proxy tls")),
                "TLS setup failed",
            ),
            (
                ConnectError::Tls(io::Error::other("tls")),
                "TLS setup failed",
            ),
            (ConnectError::Unverifiable, "request URL was invalid"),
            (ConnectError::Deadline, "request timed out"),
        ] {
            let mapped = connection_problem(&problem);
            assert!(
                matches!(mapped, TransportError::Unreachable(ref said) if said.as_ref() == phrase),
                "{mapped}"
            );
        }
        assert!(matches!(
            request_problem(&HttpError::Unverifiable),
            TransportError::Unreachable(ref said) if said.as_ref() == "request URL was invalid"
        ));
        let invalid = hyper::Request::builder()
            .uri("http://[::1")
            .body(())
            .expect_err("the deliberately malformed URL to be rejected");
        for (problem, phrase) in [
            (HttpError::Stalled(Phase::Head), "request timed out"),
            (HttpError::Invalid(invalid), "HTTP protocol failed"),
        ] {
            let mapped = request_problem(&problem);
            assert!(
                matches!(mapped, TransportError::Unreachable(ref said) if said.as_ref() == phrase),
                "{mapped}"
            );
        }
    }

    #[tokio::test]
    async fn a_response_head_over_the_shared_clients_limit_keeps_the_protocol_phrase() {
        let field = format!("x-f: {}\r\n", "v".repeat(8));
        let response = format!(
            "HTTP/1.1 200 OK\r\n{}content-length: 0\r\n\r\n",
            field.repeat(129)
        );
        let url = once(response);
        let problem = post(&shared(), &url, &[], "{}")
            .await
            .expect_err("the oversized head to be refused");

        assert!(
            matches!(problem, TransportError::Unreachable(ref said) if said.as_ref() == "HTTP protocol failed"),
            "{problem}"
        );
    }

    #[tokio::test]
    async fn a_cancelled_request_is_not_sent() {
        let cancel = Cancel::new();
        cancel.request();
        let mut outgoing = Outgoing::new();

        let problem = shared()
            .post(
                "https://crucible.invalid/v1/messages",
                &mut outgoing,
                "{}".to_owned(),
                &cancel,
            )
            .await
            .unwrap_err();

        assert!(matches!(problem, TransportError::Cancelled));
    }
}
