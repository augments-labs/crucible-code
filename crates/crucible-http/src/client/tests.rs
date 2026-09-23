use std::convert::Infallible;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_credentials::Outgoing;
use crucible_runtime::BoxFuture;
use hyper::body::Incoming;
use hyper::header::{LOCATION, USER_AGENT};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Method, Request, Response, StatusCode, Uri};
use hyper_util::client::legacy::Client;
use hyper_util::rt::TokioIo;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, duplex};
use tokio::net::TcpListener;
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;
use tower_service::Service;

use super::{Body, Http, HttpError, Phase, client, exchange};
use crate::connect::{Conn, ConnectError, Tls};
use crate::dns::tests::{Answer, settle, stalled, until_inside};
use crate::dns::{Lookup, Lookups};
use crate::tasks::Tasks;

/// A self-signed certificate for 127.0.0.1, and its key. It protects nothing:
/// the key is here so that a test server can present the certificate.
const CERT: &[u8] = include_bytes!("../../fixtures/server.cert.der");
const KEY: &[u8] = include_bytes!("../../fixtures/server.key.der");

/// A platform lookup these tests must never reach: every target here is an IP
/// address.
struct Never;

impl Lookup for Never {
    fn lookup(&self, _host: &str) -> io::Result<Vec<SocketAddr>> {
        Err(io::Error::other("a test looked a host up"))
    }
}

fn http(tls: &Tls) -> Http {
    Http::new(tls, Lookups::with(NonZeroUsize::MIN, None, Arc::new(Never)))
}

async fn get(http: &Http, url: &str) -> Result<Response<Incoming>, HttpError> {
    http.send(Method::GET, url, &Outgoing::new(), String::new())
        .await
}

async fn listen(scheme: &str) -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("{scheme}://{}/", listener.local_addr().unwrap());
    (listener, url)
}

fn acceptor(alpn: &[&[u8]]) -> TlsAcceptor {
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(KEY.to_vec()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(CERT.to_vec())], key)
        .unwrap();
    config.alpn_protocols = alpn.iter().map(|name| name.to_vec()).collect();
    TlsAcceptor::from(Arc::new(config))
}

/// Serves one connection with hyper's own server, answering each request.
async fn answer<I, F>(io: I, reply: F)
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Fn(&Request<Incoming>) -> Response<String> + Send + 'static,
{
    let service = service_fn(move |request| {
        let response = reply(&request);
        async move { Ok::<_, Infallible>(response) }
    });
    let _ = http1::Builder::new()
        .serve_connection(TokioIo::new(io), service)
        .await;
}

/// Reads up to the end of a head, and returns it with how many bytes arrived
/// after it.
async fn head(io: &mut (impl AsyncRead + Unpin)) -> (String, usize) {
    let mut seen = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let read = io.read(&mut chunk).await.unwrap();
        assert!(read > 0, "the peer closed before its head ended");
        seen.extend_from_slice(chunk.get(..read).unwrap());
        if let Some(end) = seen.windows(4).position(|four| four == b"\r\n\r\n") {
            let after = seen.len() - end - 4;
            seen.truncate(end + 4);
            return (String::from_utf8(seen).unwrap(), after);
        }
    }
}

/// What a test server noticed, one line at a time.
type Heard = Arc<Mutex<Vec<String>>>;

#[tokio::test]
async fn production_tls_refuses_a_local_self_signed_certificate() {
    let (listener, url) = listen("https").await;
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let _ = acceptor(&[b"http/1.1"]).accept(tcp).await;
    });
    let error = get(&http(&Tls::new().unwrap()), &url).await.unwrap_err();
    let Some(ConnectError::Tls(refused)) = error.connect() else {
        panic!("not refused by TLS: {error:?}");
    };
    let refused = refused.get_ref().unwrap().downcast_ref::<rustls::Error>();
    let unknown = rustls::Error::InvalidCertificate(rustls::CertificateError::UnknownIssuer);
    assert_eq!(refused, Some(&unknown));
}

#[tokio::test]
async fn a_trusted_server_is_spoken_to_in_http_1_1_without_alpn() {
    let (listener, url) = listen("https").await;
    let said = Heard::default();
    let heard = Arc::clone(&said);
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        let tls = acceptor(&[b"h2", b"http/1.1"]).accept(tcp).await.unwrap();
        let alpn = tls.get_ref().1.alpn_protocol().map(<[u8]>::to_vec);
        heard.lock().unwrap().push(format!("alpn {alpn:?}"));
        answer(tls, move |request| {
            let agent = request.headers().get(USER_AGENT).cloned();
            heard
                .lock()
                .unwrap()
                .push(format!("{:?} {agent:?}", request.version()));
            Response::new(String::new())
        })
        .await;
    });
    let http = http(&Tls::trusting(CertificateDer::from(CERT.to_vec())));
    assert_eq!(get(&http, &url).await.unwrap().status(), StatusCode::OK);
    assert_eq!(
        *said.lock().unwrap(),
        ["alpn None", "HTTP/1.1 Some(\"ureq/3.4.2\")"]
    );
}

#[tokio::test]
async fn a_redirect_is_handed_back_and_never_followed() {
    let (elsewhere, other) = listen("http").await;
    let (listener, url) = listen("http").await;
    let said = Heard::default();
    let heard = Arc::clone(&said);
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        answer(tcp, move |request| {
            let agents = request.headers().get_all(USER_AGENT).iter();
            heard
                .lock()
                .unwrap()
                .extend(agents.map(|agent| format!("{agent:?}")));
            let mut response = Response::new(String::new());
            *response.status_mut() = StatusCode::FOUND;
            response
                .headers_mut()
                .insert(LOCATION, other.parse().unwrap());
            response
        })
        .await;
    });
    let mut headers = Outgoing::new();
    headers.set_header("User-Agent", "crucible/1");
    let response = http(&Tls::new().unwrap())
        .send(Method::POST, &url, &headers, "{}".to_owned())
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
    let followed = tokio::time::timeout(Duration::from_millis(200), elsewhere.accept()).await;
    assert!(followed.is_err(), "the redirect was followed");
    assert_eq!(*said.lock().unwrap(), ["\"crucible/1\""]);
}

/// Answers one request on `listener` with exactly `reply`, then holds the
/// connection until the client lets go of it.
async fn answer_raw(listener: TcpListener, reply: String) {
    let (mut tcp, _) = listener.accept().await.unwrap();
    let _ = head(&mut tcp).await;
    tcp.write_all(reply.as_bytes()).await.unwrap();
    let _ = tcp.read(&mut [0; 1]).await;
}

fn too_large(error: &HttpError) -> bool {
    let HttpError::Exchange(error) = error else {
        return false;
    };
    let cause = std::error::Error::source(error).and_then(|cause| cause.downcast_ref());
    cause.is_some_and(hyper::Error::is_parse_too_large)
}

#[tokio::test]
async fn a_response_head_over_its_limits_is_refused() {
    let fields = |count: usize, width: usize| {
        let field = format!("x-f: {}\r\n", "v".repeat(width));
        format!(
            "HTTP/1.1 200 OK\r\n{}content-length: 0\r\n\r\n",
            field.repeat(count)
        )
    };
    for (head, refused) in [
        (fields(127, 8), false),
        (fields(128, 8), true),
        (fields(1, 60_000), false),
        (fields(1, 200_000), true),
    ] {
        let (listener, url) = listen("http").await;
        tokio::spawn(answer_raw(listener, head));
        match get(&http(&Tls::new().unwrap()), &url).await {
            Ok(response) => assert!(!refused, "accepted {:?}", response.headers().len()),
            Err(error) => assert!(refused && too_large(&error), "{error:?}"),
        }
    }
}

/// The connection tasks the client spawned end with it: a connection whose
/// response is still being read is closed once the last handle to the client
/// is dropped.
#[tokio::test]
async fn dropping_the_client_ends_the_connections_it_spawned() {
    let (listener, url) = listen("http").await;
    let peer = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let _ = head(&mut tcp).await;
        tcp.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 10\r\n\r\nhalf")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut [0; 1])).await
    });
    let http = http(&Tls::new().unwrap());
    let response = get(&http, &url).await.unwrap();
    drop(http);
    let closed = peer.await.unwrap();
    assert!(
        matches!(closed, Ok(Ok(0))),
        "outlived its client: {closed:?}"
    );
    drop(response);
}

/// A connection is up within 15 s or not at all: the wait for a lookup that
/// stalls under a plain resolver, which has no deadline of its own, ends at the
/// connect deadline and not before. The lookup itself keeps its worker.
#[tokio::test(start_paused = true)]
async fn a_connection_not_made_in_fifteen_seconds_is_given_up() {
    let (lookups, stall, _release) = stalled(1, None);
    let http = Http::new(&Tls::new().unwrap(), lookups);
    let sending = tokio::spawn(async move { get(&http, "http://api.test/").await.err() });
    until_inside(&stall, 1).await;
    tokio::time::advance(Duration::from_millis(14_999)).await;
    settle().await;
    assert!(!sending.is_finished(), "gave up before the deadline");
    tokio::time::advance(Duration::from_millis(1)).await;
    settle().await;
    assert!(sending.is_finished(), "still connecting at the deadline");
    let error = sending.await.unwrap().unwrap();
    let deadline = matches!(error.connect(), Some(ConnectError::Deadline));
    assert!(deadline, "{error:?}");
}

/// A request that names no scheme is refused before it is sent: hyper-util
/// would give a `CONNECT` to any port but 443 the `http` scheme, and send it
/// with its headers in plaintext.
#[tokio::test]
async fn a_request_that_names_no_scheme_is_never_dialled() {
    let (listener, url) = listen("http").await;
    let target = url
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .to_owned();
    let http = http(&Tls::new().unwrap());
    let sending = tokio::spawn(async move {
        let headers = Outgoing::new();
        let sent = http.send(Method::CONNECT, &target, &headers, String::new());
        sent.await.err()
    });
    let reached = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
    assert!(reached.is_err(), "a CONNECT with no scheme was dialled");
    let error = sending.await.unwrap().unwrap();
    assert!(matches!(error, HttpError::Unverifiable), "{error:?}");
}

/// A target is spoken to in plaintext only when its scheme is `http`: any other
/// scheme, or `https` with no host to verify, is refused before it is dialled.
/// Every name resolves to the listener, so a dial would be seen there first.
/// `send` refuses the other scheme; the connector refuses `https` with no host.
#[tokio::test]
async fn a_target_that_cannot_be_verified_is_never_spoken_to_in_plaintext() {
    for (scheme, host) in [("ftp", "127.0.0.1"), ("https", "")] {
        let (listener, url) = listen(scheme).await;
        let url = url.replace("127.0.0.1", host);
        let lookups = Lookups::with(NonZeroUsize::MIN, None, Arc::new(Answer));
        let http = Http::new(&Tls::new().unwrap(), lookups);
        let sending = tokio::spawn(async move { get(&http, &url).await.err() });
        let reached = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(reached.is_err(), "{scheme} with host {host:?} was dialled");
        let error = sending.await.unwrap().unwrap();
        let refused = matches!(error, HttpError::Unverifiable)
            || matches!(error.connect(), Some(ConnectError::Unverifiable));
        assert!(refused, "{scheme}: {error:?}");
    }
}

/// A connector whose one connection is an in-memory pipe, so a peer that
/// stops reading or answering stops exactly where the test says.
#[derive(Clone)]
struct Pipe(Arc<Mutex<Option<DuplexStream>>>);

impl Service<Uri> for Pipe {
    type Response = TokioIo<Conn>;
    type Error = io::Error;
    type Future = BoxFuture<'static, io::Result<TokioIo<Conn>>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, _: Uri) -> Self::Future {
        let near = self.0.lock().unwrap().take();
        Box::pin(async move {
            let near = near.ok_or_else(|| io::Error::other("one connection only"))?;
            Ok(TokioIo::new(Conn::new(near)))
        })
    }
}

/// A client over one pipe, its peer's end, and the owner of its tasks.
fn piped(capacity: usize) -> (Client<Pipe, Body>, DuplexStream, Arc<Tasks>) {
    let (near, far) = duplex(capacity);
    let tasks = Tasks::new();
    let client = client(Pipe(Arc::new(Mutex::new(Some(near)))), &tasks);
    (client, far, tasks)
}

/// Sends a POST of `size` bytes over `client`, from its own task.
fn post(
    client: Client<Pipe, Body>,
    size: usize,
) -> tokio::task::JoinHandle<Result<StatusCode, HttpError>> {
    let request = Request::post("http://peer.test/").body("x".repeat(size));
    tokio::spawn(async move {
        let response = exchange(&client, request.unwrap()).await?;
        Ok(response.status())
    })
}

/// The error a request ended with, if it ended within five minutes.
async fn gave_up(sending: tokio::task::JoinHandle<Result<StatusCode, HttpError>>) -> HttpError {
    let ended = tokio::time::timeout(Duration::from_mins(5), sending).await;
    ended
        .expect("no deadline ended the request")
        .unwrap()
        .unwrap_err()
}

fn about_a_minute(started: Instant) -> bool {
    let waited = started.elapsed();
    waited >= Duration::from_mins(1) && waited < Duration::from_secs(61)
}

#[tokio::test(start_paused = true)]
async fn an_answer_that_never_starts_fails_a_minute_after_the_body_was_sent() {
    let (client, mut peer, _tasks) = piped(64 * 1024);
    let started = Instant::now();
    let sending = post(client, 10);
    let _ = head(&mut peer).await;
    let error = gave_up(sending).await;
    assert!(
        matches!(error, HttpError::Stalled(Phase::Answer)),
        "{error:?}"
    );
    assert!(about_a_minute(started), "{:?}", started.elapsed());
}

#[tokio::test(start_paused = true)]
async fn a_body_the_peer_stops_taking_fails_a_minute_after_it_began() {
    let (client, peer, _tasks) = piped(16 * 1024);
    let started = Instant::now();
    let error = gave_up(post(client, 1 << 20)).await;
    assert!(
        matches!(error, HttpError::Stalled(Phase::Body)),
        "{error:?}"
    );
    assert!(about_a_minute(started), "{:?}", started.elapsed());
    drop(peer);
}

/// A body taken slowly and an answer given slowly each fit their own minute,
/// though together they take longer than one.
#[tokio::test(start_paused = true)]
async fn each_part_of_a_request_has_a_minute_of_its_own() {
    let (client, mut peer, _tasks) = piped(64 * 1024);
    let started = Instant::now();
    let sending = post(client, 1 << 20);
    let (_, mut arrived) = head(&mut peer).await;
    while arrived < 1 << 20 {
        tokio::time::sleep(Duration::from_secs(3)).await;
        arrived += peer.read(&mut vec![0; 64 * 1024]).await.unwrap();
    }
    tokio::time::sleep(Duration::from_secs(40)).await;
    peer.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 0\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(sending.await.unwrap().unwrap(), StatusCode::OK);
    assert!(
        started.elapsed() > Duration::from_secs(80),
        "{:?}",
        started.elapsed()
    );
}

/// This crate's own source installs no logger or subscriber and writes nothing
/// to a terminal. What its dependencies emit is not covered here.
#[test]
fn nothing_here_logs_or_prints() {
    let forbidden = [
        "tracing::",
        "log::",
        "print!",
        "println!",
        "dbg!",
        "stdout(",
        "stderr(",
        "set_logger",
        "set_global_default",
    ];
    let mut found = Vec::new();
    let mut folders = vec![Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else if path.file_name().is_some_and(|name| name != "tests.rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                found.extend(
                    forbidden
                        .iter()
                        .filter(|word| text.contains(*word))
                        .map(|word| format!("{}: {word}", path.display())),
                );
            }
        }
    }
    assert!(found.is_empty(), "{found:#?}");
}
