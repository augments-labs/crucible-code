use std::convert::Infallible;
use std::error::Error;
use std::io;
use std::net::SocketAddr;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
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
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream, copy_bidirectional, duplex,
};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;
use tower_service::Service;

use super::{Body, Http, HttpError, Phase, client, exchange};
use crate::body::{BodyError, Chunk, Chunks};
use crate::connect::{Conn, ConnectError, Connector, MAX_SETUPS, Setups, Tls};
use crate::dns::tests::{Answer, Stall, raised, settle, stalled, stalled_plain, until_inside};
use crate::dns::{Lookup, Lookups, PlainLookups, Poison};
use crate::proxy::ProxyEnv;
use crate::read_limited;
use crate::tasks::Tasks;

/// A self-signed certificate for 127.0.0.1, and its key. It protects nothing:
/// the key is here so that a test server can present the certificate.
const CERT: &[u8] = include_bytes!("../../fixtures/server.cert.der");
const KEY: &[u8] = include_bytes!("../../fixtures/server.key.der");

/// A platform lookup these tests must never reach: every target here is an IP
/// address, a name only a proxy resolves, or one refused before any lookup.
struct Never;

impl Lookup for Never {
    fn lookup(&self, _host: &str) -> io::Result<Vec<SocketAddr>> {
        Err(io::Error::other("a test looked a host up"))
    }
}

fn http(tls: &Tls) -> Http {
    proxied(tls, ProxyEnv::read(|_| None))
}

/// A client routed as `env` says, that looks no name up itself.
fn proxied(tls: &Tls, env: ProxyEnv) -> Http {
    let target = Lookups::with(NonZeroUsize::MIN, None, Arc::new(Never));
    let proxy_host = PlainLookups::with(NonZeroUsize::MIN, Arc::new(Never));
    Http::new(tls, target, proxy_host, env)
}

/// An environment whose only proxy variable is `name`, set to `value`.
fn proxy_env(name: &'static str, value: String) -> ProxyEnv {
    ProxyEnv::read(move |asked| (asked == name).then(|| value.clone()))
}

async fn get(http: &Http, url: &str) -> Result<Response<Incoming>, HttpError> {
    http.send(Method::GET, url, &mut Outgoing::new(), String::new())
        .await
}

/// A source that answers one request and records the head it was asked with.
async fn asking_once(said: &Heard) -> String {
    let (listener, url) = listen("http").await;
    let heard = Arc::clone(said);
    tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        answer(tcp, move |request| {
            heard
                .lock()
                .unwrap()
                .push(format!("{:?} {:?}", request.method(), request.headers()));
            Response::new(String::from("{}"))
        })
        .await;
    });

    url
}

/// The route documented as taking no caller header accepts no name and value
/// here that a caller holding one could send. The library still adds its own
/// default `User-Agent`.
#[tokio::test]
async fn an_uncredentialed_get_carries_no_caller_header() {
    let said = Heard::default();
    let url = asking_once(&said).await;

    let response = http(&Tls::new().unwrap()).get(&url).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let asked = said.lock().unwrap().clone();
    assert_eq!(asked.len(), 1, "{asked:?}");
    for forbidden in [
        "authorization",
        "proxy-authorization",
        "cookie",
        "x-api-key",
    ] {
        assert!(
            !asked.first().is_some_and(|head| head.contains(forbidden)),
            "{forbidden} went out of the uncredentialed route: {asked:?}"
        );
    }
}

/// The fixed release route sends its two non-secret headers and accepts no
/// caller header of its own.
#[tokio::test]
async fn a_release_get_sends_its_fixed_headers() {
    let said = Heard::default();
    let url = asking_once(&said).await;

    let response = http(&Tls::new().unwrap()).get_release(&url).await.unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        read_limited(response.into_body(), 2, Duration::from_secs(1))
            .await
            .unwrap(),
        b"{}"
    );
    let asked = said.lock().unwrap().clone();
    assert!(
        asked.first().is_some_and(|head| {
            head.contains("application/vnd.github+json")
                && head.contains(&format!("crucible-code/{}", env!("CARGO_PKG_VERSION")))
        }),
        "{asked:?}"
    );
}

#[tokio::test]
async fn separate_clients_keep_separate_pools() {
    let (listener, url) = listen("http").await;
    let accepted = Arc::new(AtomicUsize::new(0));
    let server_accepted = Arc::clone(&accepted);
    let server = tokio::spawn(async move {
        let (first, _) = listener.accept().await.unwrap();
        server_accepted.fetch_add(1, Ordering::AcqRel);
        let first = tokio::spawn(answer(first, |_| Response::new(String::new())));

        if let Ok(Ok((second, _))) =
            tokio::time::timeout(Duration::from_millis(500), listener.accept()).await
        {
            server_accepted.fetch_add(1, Ordering::AcqRel);
            tokio::spawn(answer(second, |_| Response::new(String::new())));
        }
        first.abort();
    });

    let tls = Tls::new().unwrap();
    let first = http(&tls);
    let second = http(&tls);
    for client in [&first, &second] {
        let response = client.get(&url).await.unwrap();
        let _ = read_limited(response.into_body(), 1, Duration::from_secs(1))
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert_eq!(accepted.load(Ordering::Acquire), 2);
    server.abort();
}

#[tokio::test(start_paused = true)]
async fn idle_connections_are_reaped_by_the_pool_timer() {
    let (listener, url) = listen("http").await;
    let peer = tokio::spawn(async move {
        let (tcp, _) = listener.accept().await.unwrap();
        answer(tcp, |_| Response::new(String::new())).await;
    });
    let http = http(&Tls::new().unwrap());
    let response = get(&http, &url).await.unwrap();
    let _ = read_limited(response.into_body(), 1, Duration::from_secs(1))
        .await
        .unwrap();

    settle().await;
    tokio::time::advance(Duration::from_millis(15_001)).await;
    settle().await;

    assert!(
        tokio::time::timeout(Duration::from_secs(1), peer)
            .await
            .is_ok(),
        "the idle connection outlived the pool timer"
    );
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
        .send(Method::POST, &url, &mut headers, "{}".to_owned())
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
    let http = Http::new(
        &Tls::new().unwrap(),
        lookups,
        PlainLookups::with(NonZeroUsize::MIN, Arc::new(Never)),
        ProxyEnv::read(|_| None),
    );
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
        let mut headers = Outgoing::new();
        let sent = http.send(Method::CONNECT, &target, &mut headers, String::new());
        sent.await.err()
    });
    let reached = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
    assert!(reached.is_err(), "a CONNECT with no scheme was dialled");
    let error = sending.await.unwrap().unwrap();
    assert!(matches!(error, HttpError::Unverifiable), "{error:?}");
}

/// A target is spoken to in plaintext only when its scheme is `http` and it
/// names a host: any other scheme, or no host, is refused by `send` before it
/// is dialled. Every name resolves to the listener, so a dial would be seen
/// there first.
#[tokio::test]
async fn a_target_that_cannot_be_verified_is_never_spoken_to_in_plaintext() {
    for (scheme, host) in [("ftp", "127.0.0.1"), ("https", ""), ("http", "")] {
        let (listener, url) = listen(scheme).await;
        let url = url.replace("127.0.0.1", host);
        let lookups = Lookups::with(NonZeroUsize::MIN, None, Arc::new(Answer));
        let proxy_host = PlainLookups::with(NonZeroUsize::MIN, Arc::new(Never));
        let http = Http::new(
            &Tls::new().unwrap(),
            lookups,
            proxy_host,
            ProxyEnv::read(|_| None),
        );
        let sending = tokio::spawn(async move { get(&http, &url).await.err() });
        let reached = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(reached.is_err(), "{scheme} with host {host:?} was dialled");
        let error = sending.await.unwrap().unwrap();
        let refused = matches!(error, HttpError::Unverifiable);
        assert!(refused, "{scheme} with host {host:?}: {error:?}");
    }
}

/// A `CONNECT` proxy for one connection: reads the `CONNECT` head, noting it
/// in `heard`, answers with `reply`, and after a 200 carries bytes both ways
/// between the client and the address the `CONNECT` named.
async fn tunnel<S>(mut client: S, reply: &'static str, heard: Heard)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let (connect, _) = head(&mut client).await;
    heard.lock().unwrap().push(connect.clone());
    client.write_all(reply.as_bytes()).await.unwrap();
    if reply.starts_with("HTTP/1.1 200") {
        let target = connect.split(' ').nth(1).unwrap();
        let mut target = TcpStream::connect(target).await.unwrap();
        let _ = copy_bidirectional(&mut client, &mut target).await;
    }
}

const OPENED: &str = "HTTP/1.1 200 Connection established\r\n\r\n";

/// Through a proxy, the target is named in the `CONNECT` and never looked up
/// here, the proxy is sent the previous client's fields and the credential,
/// and the credential is registered for redaction on the request's headers.
#[tokio::test]
async fn a_proxied_request_goes_through_connect_and_the_proxy_resolves_its_target() {
    let (listener, proxy) = listen("http").await;
    let tunnelled = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let connect = head(&mut tcp).await.0;
        tcp.write_all(OPENED.as_bytes()).await.unwrap();
        let request = head(&mut tcp).await.0;
        tcp.write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .await
            .unwrap();
        (connect, request)
    });
    let proxy = proxy.replace("http://", "http://user:pass@");
    let mut headers = Outgoing::new();
    let response = proxied(&Tls::new().unwrap(), proxy_env("HTTPS_PROXY", proxy))
        .send(
            Method::GET,
            "http://vendor.test/v1",
            &mut headers,
            String::new(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let (connect, request) = tunnelled.await.unwrap();
    assert_eq!(
        connect,
        "CONNECT vendor.test:80 HTTP/1.1\r\nHost: vendor.test:80\r\nuser-agent: ureq/3.4.2\r\n\
         proxy-connection: Keep-Alive\r\nproxy-authorization: Basic dXNlcjpwYXNz\r\n\r\n"
    );
    assert!(request.starts_with("GET /v1 HTTP/1.1\r\n"), "{request}");
    let shown = headers.redactions().redact("proxy said dXNlcjpwYXNz");
    assert!(!shown.contains("dXNlcjpwYXNz"), "{shown}");
}

/// An `https` target is reached through an `http://` proxy and through an
/// `https://` one, which is itself spoken to over TLS; either way the
/// target's own TLS runs inside the tunnel, so the proxy carries only bytes
/// it cannot read.
#[tokio::test]
async fn a_request_reaches_its_target_through_an_http_or_https_proxy_with_the_targets_tls_inside() {
    for scheme in ["http", "https"] {
        let (target, url) = listen("https").await;
        let said = Heard::default();
        let heard = Arc::clone(&said);
        tokio::spawn(async move {
            let (tcp, _) = target.accept().await.unwrap();
            let tls = acceptor(&[]).accept(tcp).await.unwrap();
            answer(tls, move |request| {
                let line = format!("{} {}", request.method(), request.uri());
                heard.lock().unwrap().push(line);
                Response::new(String::new())
            })
            .await;
        });
        let (listener, proxy) = listen(scheme).await;
        let connects = Heard::default();
        let noted = Arc::clone(&connects);
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            if scheme == "https" {
                let tls = acceptor(&[]).accept(tcp).await.unwrap();
                tunnel(tls, OPENED, noted).await;
            } else {
                tunnel(tcp, OPENED, noted).await;
            }
        });
        let tls = Tls::trusting(CertificateDer::from(CERT.to_vec()));
        let http = proxied(&tls, proxy_env("ALL_PROXY", proxy));
        let response = get(&http, &format!("{url}v1")).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK, "{scheme}");
        let authority = url.trim_start_matches("https://").trim_end_matches('/');
        let connects = connects.lock().unwrap().clone();
        let expected = format!("CONNECT {authority} HTTP/1.1\r\n");
        assert!(
            matches!(connects.as_slice(), [only] if only.starts_with(&expected)),
            "{scheme} proxy saw {connects:?}"
        );
        assert_eq!(*said.lock().unwrap(), ["GET /v1"], "{scheme}");
    }
}

/// A proxy's answer is taken whole however it is split into writes: a 200
/// whose first piece is shorter than its status line still opens the tunnel.
#[tokio::test]
async fn a_proxys_answer_split_before_its_status_still_opens_the_tunnel() {
    let (listener, proxy) = listen("http").await;
    tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let _ = head(&mut tcp).await;
        tcp.write_all(b"HTTP/1.1 ").await.unwrap();
        tcp.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        tcp.write_all(b"200 Connection established\r\n\r\n")
            .await
            .unwrap();
        let _ = head(&mut tcp).await;
        tcp.write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .await
            .unwrap();
        let _ = tcp.read(&mut [0; 1]).await;
    });
    let http = proxied(&Tls::new().unwrap(), proxy_env("ALL_PROXY", proxy));
    let response = get(&http, "http://vendor.test/").await;
    let status = response.as_ref().map(Response::status);
    assert_eq!(status.ok(), Some(StatusCode::NO_CONTENT), "{response:?}");
}

/// Bytes a proxy writes behind its 200 head, in the same write, are the
/// tunnel's first bytes: the tunnel opens at once and hands them on as they
/// came, where waiting for the head to end the read would wait out the
/// connect deadline.
#[tokio::test]
async fn bytes_behind_a_proxys_answer_are_handed_on_through_the_tunnel() {
    let (listener, proxy) = listen("http").await;
    tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let _ = head(&mut tcp).await;
        tcp.write_all(b"HTTP/1.1 200 Connection established\r\n\r\nearly")
            .await
            .unwrap();
        let _ = tcp.read(&mut [0; 1]).await;
    });
    let lookups = Lookups::with(NonZeroUsize::MIN, None, Arc::new(Never));
    let proxy_host = PlainLookups::with(NonZeroUsize::MIN, Arc::new(Never));
    let env = Arc::new(proxy_env("ALL_PROXY", proxy));
    let mut connector = Connector::new(&Tls::new().unwrap(), lookups, proxy_host, env);
    let connecting = connector.call(Uri::from_static("http://vendor.test/"));
    let opened = tokio::time::timeout(Duration::from_secs(5), connecting).await;
    let mut tunnel = opened
        .expect("the tunnel was still opening after 5 s")
        .unwrap()
        .into_inner();
    let mut early = [0; 5];
    tunnel.read_exact(&mut early).await.unwrap();
    assert_eq!(&early, b"early");
}

/// Every source and message of `error`, and its `Debug`.
fn everything_said(error: &HttpError) -> String {
    let mut said = format!("{error:?}");
    let mut cause: Option<&dyn Error> = Some(error);
    while let Some(step) = cause {
        said.push('\n');
        said.push_str(&step.to_string());
        cause = step.source();
    }
    said
}

/// A proxy that answers a `CONNECT` with anything but a 200 has refused the
/// connection, and the request fails as a connection that was not made. What
/// the failure says holds neither the credential nor its encoding, and the
/// encoding is registered for redaction all the same.
#[tokio::test]
async fn a_refused_connect_is_a_connect_failure_that_shows_no_credential() {
    for reply in [
        "HTTP/1.1 407 Proxy Authentication Required\r\n\r\n",
        "HTTP/1.1 403 Forbidden\r\n\r\n",
    ] {
        let (listener, proxy) = listen("http").await;
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            tunnel(tcp, reply, Heard::default()).await;
        });
        let proxy = proxy.replace("http://", "http://user:secret@");
        let mut headers = Outgoing::new();
        let error = proxied(&Tls::new().unwrap(), proxy_env("ALL_PROXY", proxy))
            .send(
                Method::GET,
                "https://vendor.test/",
                &mut headers,
                String::new(),
            )
            .await
            .unwrap_err();
        let refused = matches!(error.connect(), Some(ConnectError::Tunnel(_)));
        assert!(refused, "{reply:?}: {error:?}");
        let said = everything_said(&error);
        for secret in ["secret", "dXNlcjpzZWNyZXQ="] {
            assert!(!said.contains(secret), "{said}");
        }
        let shown = headers.redactions().redact("dXNlcjpzZWNyZXQ=");
        assert!(!shown.contains("dXNlcjpzZWNyZXQ="), "{shown}");
    }
}

/// A SOCKS proxy that would have resolved the target is one the previous
/// client, built without SOCKS, had no address for: the connection is
/// refused, and neither the proxy nor the target is dialled.
#[tokio::test]
async fn a_socks_proxy_that_resolves_the_target_is_refused_without_a_dial() {
    let (proxy, proxy_url) = listen("http").await;
    let (target, url) = listen("http").await;
    let socks = proxy_url.replace("http://", "socks5h://");
    let http = proxied(&Tls::new().unwrap(), proxy_env("ALL_PROXY", socks));
    let sending = tokio::spawn(async move { get(&http, &url).await.err() });
    for (listener, what) in [(proxy, "proxy"), (target, "target")] {
        let dialled = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(dialled.is_err(), "the {what} was dialled");
    }
    let error = sending.await.unwrap().unwrap();
    let Some(ConnectError::Tcp(refused)) = error.connect() else {
        panic!("not refused: {error:?}");
    };
    let refused = refused.downcast_ref::<io::Error>().map(io::Error::kind);
    assert_eq!(refused, Some(io::ErrorKind::ConnectionRefused));
}

/// A request that cannot be verified is refused before a route is chosen, so
/// the proxy the environment names is never dialled for it either.
#[tokio::test]
async fn a_target_that_cannot_be_verified_is_refused_before_a_proxy_is_dialled() {
    for url in ["ftp://vendor.test/", "https://:443/", "http://:80/"] {
        let (listener, proxy) = listen("http").await;
        let http = proxied(&Tls::new().unwrap(), proxy_env("ALL_PROXY", proxy));
        let sending = tokio::spawn(async move { get(&http, url).await.err() });
        let dialled = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(dialled.is_err(), "the proxy was dialled for {url}");
        let error = sending.await.unwrap().unwrap();
        let refused = matches!(error, HttpError::Unverifiable);
        assert!(refused, "{url}: {error:?}");
    }
}

/// Waits, on the wall clock, for the first of: a lookup inside `proxy_host`,
/// one inside `target`, or `sending` ending; and says which it was.
async fn first_lookup<T>(
    proxy_host: &Stall,
    target: &Stall,
    sending: &tokio::task::JoinHandle<T>,
) -> &'static str {
    loop {
        if proxy_host.counts().0 > 0 {
            return "the proxy host";
        }
        if target.counts().0 > 0 {
            return "the target";
        }
        if sending.is_finished() {
            return "none, the request ended";
        }
        tokio::task::yield_now().await;
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// A cloned client keeps the connector's poisoned target owner, so every
/// provider clone made from the application's service obeys the same verdict.
#[tokio::test]
async fn a_client_clone_obeys_the_same_poisoned_target_lookup() {
    let poison = raised();
    let target = Lookups::poisoned(NonZeroUsize::MIN, &poison);
    let proxy_host = PlainLookups::with(NonZeroUsize::MIN, Arc::new(Never));
    let client = Http::new(
        &Tls::new().unwrap(),
        target,
        proxy_host,
        ProxyEnv::read(|_| None),
    );
    let clone = client.clone();

    let error = get(&clone, "http://api.test/").await.unwrap_err();

    assert!(
        matches!(error.connect(), Some(ConnectError::ResolveStalled)),
        "{error:?}"
    );
}

/// A proxy's host is looked up with a plain owner, under the connect deadline
/// alone: a raised poison does not stop it, a stall in it raises none, and
/// the connection is given up at 15 s like any other.
#[tokio::test(start_paused = true)]
async fn a_proxy_host_is_looked_up_under_the_connect_deadline_and_never_the_poison() {
    for poison in [raised(), Poison::default()] {
        let (target, target_stall, _released) = stalled(1, Some(&poison));
        let (proxy_host, stall, _release) = stalled_plain(1);
        let env = proxy_env("HTTPS_PROXY", "http://proxy.test:3128".to_owned());
        let http = Http::new(&Tls::new().unwrap(), target, proxy_host, env);
        let sending = tokio::spawn(async move { get(&http, "https://api.test/").await.err() });
        let was_raised = poison.is_raised();
        let first = first_lookup(&stall, &target_stall, &sending).await;
        assert_eq!(first, "the proxy host", "poison raised: {was_raised}");
        tokio::time::advance(Duration::from_millis(14_999)).await;
        settle().await;
        assert!(!sending.is_finished(), "gave up before the deadline");
        assert_eq!(poison.is_raised(), was_raised, "the poison changed");
        tokio::time::advance(Duration::from_millis(1)).await;
        settle().await;
        assert!(sending.is_finished(), "still connecting at the deadline");
        let error = sending.await.unwrap().unwrap();
        let deadline = matches!(error.connect(), Some(ConnectError::Deadline));
        assert!(deadline, "{error:?}");
        assert_eq!(poison.is_raised(), was_raised, "the poison changed");
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
fn piped(capacity: usize) -> (Client<Setups<Pipe>, Body>, DuplexStream, Arc<Tasks>) {
    let (near, far) = duplex(capacity);
    let tasks = Tasks::new();
    let client = client(Pipe(Arc::new(Mutex::new(Some(near)))), &tasks);
    (client, far, tasks)
}

/// Sends a POST of `size` bytes over `client`, from its own task.
fn post(
    client: Client<Setups<Pipe>, Body>,
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

/// A `CONNECT` proxy's end of each connection made to it, with the head of
/// the `CONNECT` that opened it, handed over as each arrives.
fn proxy_accepting(listener: TcpListener) -> tokio::sync::mpsc::Receiver<(String, TcpStream)> {
    let (made, accepted) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        loop {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let made = made.clone();
            tokio::spawn(async move {
                let (connect, _) = head(&mut tcp).await;
                let _ = made.send((connect, tcp)).await;
            });
        }
    });
    accepted
}

/// The next connection made to the proxy, if one is made within `within`.
async fn next_connect(
    accepted: &mut tokio::sync::mpsc::Receiver<(String, TcpStream)>,
    within: Duration,
) -> Option<(String, TcpStream)> {
    tokio::time::timeout(within, accepted.recv())
        .await
        .ok()
        .flatten()
}

/// Opens the tunnel on `tcp` and answers one request through it with a 204,
/// once `go` says to.
async fn answer_through(tcp: &mut TcpStream, go: tokio::sync::oneshot::Receiver<()>) {
    tcp.write_all(OPENED.as_bytes()).await.unwrap();
    let _ = head(tcp).await;
    go.await.unwrap();
    tcp.write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
        .await
        .unwrap();
}

/// A proxied GET of `url` from a task of its own, answering its status.
fn proxied_get(http: &Http, url: String) -> tokio::task::JoinHandle<StatusCode> {
    let http = http.clone();
    tokio::spawn(async move { get(&http, &url).await.unwrap().status() })
}

const VENDOR: &str = "http://vendor.test/";

/// A second request to the proxied host, raced: its connection is being
/// made, with the `CONNECT` and its credential already at the proxy and no
/// answer, when the first request's connection comes free, so it takes that
/// one and sends its head there. Hands back the second request, the proxy's
/// socket for the connection it had started, and the pooled one.
async fn lost_a_race(
    http: &Http,
    accepted: &mut tokio::sync::mpsc::Receiver<(String, TcpStream)>,
) -> (tokio::task::JoinHandle<StatusCode>, TcpStream, TcpStream) {
    let first = proxied_get(http, VENDOR.to_owned());
    let (_, mut pooled) = next_connect(accepted, Duration::from_secs(5))
        .await
        .unwrap();
    let (answer_first, first_answered) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        answer_through(&mut pooled, first_answered).await;
        pooled
    });
    let second = proxied_get(http, VENDOR.to_owned());
    let (connect, raced) = next_connect(accepted, Duration::from_secs(5))
        .await
        .unwrap();
    assert!(connect.contains("proxy-authorization: Basic"), "{connect}");
    answer_first.send(()).unwrap();
    assert_eq!(first.await.unwrap(), StatusCode::NO_CONTENT);
    let mut pooled = serving.await.unwrap();
    let _ = head(&mut pooled).await;
    (second, raced, pooled)
}

/// Whether `MAX_SETUPS` more connections can be started at once.
async fn every_slot_is_free(
    http: &Http,
    accepted: &mut tokio::sync::mpsc::Receiver<(String, TcpStream)>,
) {
    let more: Vec<_> = (0..MAX_SETUPS)
        .map(|at| proxied_get(http, format!("http://more{at}.test/")))
        .collect();
    for at in 0..MAX_SETUPS {
        let made = next_connect(accepted, Duration::from_secs(5)).await;
        assert!(made.is_some(), "only {at} setups could start");
    }
    for request in more {
        request.abort();
    }
}

fn credentialed_proxy(proxy: &str) -> Http {
    let proxy = proxy.replace("http://", "http://user:secret@");
    proxied(&Tls::new().unwrap(), proxy_env("ALL_PROXY", proxy))
}

/// A request whose connection is still being made when another request's
/// connection to the same host comes free takes that one, and the connection
/// it had started ends once the request has its answer: the proxy's socket
/// for the `CONNECT` it sent with the credential ends, and its setup slot is
/// free again for another four to be made at once.
#[tokio::test]
async fn a_connect_that_lost_the_race_to_a_pooled_connection_ends_with_its_answer() {
    let (listener, proxy) = listen("http").await;
    let mut accepted = proxy_accepting(listener);
    let http = credentialed_proxy(&proxy);
    let (second, mut raced, mut pooled) = lost_a_race(&http, &mut accepted).await;
    pooled
        .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(second.await.unwrap(), StatusCode::NO_CONTENT);
    assert_eq!(until_closed(&mut raced).await, Seen::End);
    every_slot_is_free(&http, &mut accepted).await;
}

/// The same race, cancelled while the answer is awaited on the pooled
/// connection: both the pooled connection and the one the request had
/// started end, and the slot is free again.
#[tokio::test]
async fn a_connect_that_lost_the_race_to_a_pooled_connection_ends_when_its_request_is_cancelled() {
    let (listener, proxy) = listen("http").await;
    let mut accepted = proxy_accepting(listener);
    let http = credentialed_proxy(&proxy);
    let (second, mut raced, mut pooled) = lost_a_race(&http, &mut accepted).await;
    second.abort();
    assert!(second.await.unwrap_err().is_cancelled());
    assert_eq!(until_closed(&mut pooled).await, Seen::End);
    assert_eq!(until_closed(&mut raced).await, Seen::End);
    every_slot_is_free(&http, &mut accepted).await;
}

/// A request that takes a pooled connection while its own is still waiting
/// for a setup slot never has that connection made: once the request has
/// its answer, a slot coming free sends no `CONNECT` for it.
#[tokio::test]
async fn a_connect_waiting_for_a_slot_is_never_made_once_its_request_is_gone() {
    let (listener, proxy) = listen("http").await;
    let mut accepted = proxy_accepting(listener);
    let http = proxied(&Tls::new().unwrap(), proxy_env("ALL_PROXY", proxy));
    let first = proxied_get(&http, VENDOR.to_owned());
    let (_, mut pooled) = next_connect(&mut accepted, Duration::from_secs(5))
        .await
        .unwrap();
    let (answer_first, first_answered) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(async move {
        answer_through(&mut pooled, first_answered).await;
        pooled
    });
    let holders: Vec<_> = (0..MAX_SETUPS)
        .map(|at| proxied_get(&http, format!("http://holder{at}.test/")))
        .collect();
    let mut held = Vec::new();
    for _ in 0..MAX_SETUPS {
        held.push(
            next_connect(&mut accepted, Duration::from_secs(5))
                .await
                .unwrap(),
        );
    }
    let second = proxied_get(&http, VENDOR.to_owned());
    let early = next_connect(&mut accepted, Duration::from_millis(200)).await;
    assert!(early.is_none(), "a setup past the bound was made");
    answer_first.send(()).unwrap();
    assert_eq!(first.await.unwrap(), StatusCode::NO_CONTENT);
    let mut pooled = serving.await.unwrap();
    let _ = head(&mut pooled).await;
    pooled
        .write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
        .await
        .unwrap();
    assert_eq!(second.await.unwrap(), StatusCode::NO_CONTENT);
    let (freed, mut freed_tcp) = held.swap_remove(0);
    let holder = freed.split(' ').nth(1).unwrap().trim_end_matches(":80");
    let at = holders
        .iter()
        .enumerate()
        .position(|(at, _)| holder == format!("holder{at}.test"))
        .unwrap();
    holders.get(at).unwrap().abort();
    assert_eq!(until_closed(&mut freed_tcp).await, Seen::End);
    let late = next_connect(&mut accepted, Duration::from_millis(500)).await;
    assert!(
        late.is_none(),
        "a CONNECT was sent after its request was gone: {:?}",
        late.map(|(connect, _)| connect)
    );
    for holder in holders {
        holder.abort();
    }
}

/// A request body goes out whole, framed by its length rather than in chunks,
/// and no `accept-encoding` is asked for, so an answer comes back as it was
/// sent.
#[tokio::test]
async fn a_request_body_goes_out_whole_with_its_length_and_asks_for_no_encoding() {
    let size = 100_000;
    let (listener, url) = listen("http").await;
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let (head, mut arrived) = head(&mut tcp).await;
        while arrived < size {
            let read = tcp.read(&mut vec![0; 64 * 1024]).await.unwrap();
            assert!(read > 0, "the body ended after {arrived} bytes");
            arrived += read;
        }
        tcp.write_all(b"HTTP/1.1 204 No Content\r\n\r\n")
            .await
            .unwrap();
        (head.to_ascii_lowercase(), arrived)
    });
    let response = http(&Tls::new().unwrap())
        .send(Method::POST, &url, &mut Outgoing::new(), "x".repeat(size))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let (head, arrived) = server.await.unwrap();
    assert!(
        head.contains(&format!("\r\ncontent-length: {size}\r\n")),
        "{head}"
    );
    for asked in ["transfer-encoding", "accept-encoding"] {
        assert!(!head.contains(asked), "{head}");
    }
    assert_eq!(arrived, size);
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

/// What a server saw of a connection once the client let go of it.
#[derive(Debug, PartialEq, Eq)]
enum Seen {
    /// The stream ended: the client closed its side.
    End,
    /// The client reset the connection.
    Reset,
    /// Nothing ended within five seconds.
    StillOpen,
}

/// Reads whatever else the client sends until its side of `tcp` ends, and
/// says how it ended.
async fn until_closed(tcp: &mut TcpStream) -> Seen {
    let mut rest = [0; 4096];
    loop {
        match tokio::time::timeout(Duration::from_secs(5), tcp.read(&mut rest)).await {
            Ok(Ok(0)) => return Seen::End,
            Ok(Ok(_)) => {}
            Ok(Err(_)) => return Seen::Reset,
            Err(_) => return Seen::StillOpen,
        }
    }
}

/// The headers of a request that carries a credential; the value is fake.
fn credentialed() -> Outgoing {
    let mut headers = Outgoing::new();
    headers.set_header("authorization", "Bearer not-a-real-token");
    headers
}

/// Sends a credentialed GET to `url` from a task of its own, as a consumer
/// awaiting it would, so that cancelling is dropping that task's future.
fn sending(
    http: &Http,
    url: String,
) -> tokio::task::JoinHandle<Result<Response<Incoming>, HttpError>> {
    let http = http.clone();
    tokio::spawn(async move {
        let mut headers = credentialed();
        http.send(Method::GET, &url, &mut headers, String::new())
            .await
    })
}

/// How many of `tasks` are alive once they have had five seconds, on the
/// wall clock, to end.
async fn left_running(tasks: &Tasks) -> usize {
    let started = std::time::Instant::now();
    while tasks.live() > 0 && started.elapsed() < Duration::from_secs(5) {
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    tasks.live()
}

/// Cancelled during setup: the production connector has stalled in the TLS
/// handshake, the server having taken the client's first flight and
/// answered nothing. Dropping the request closes the connection being made.
/// The request, credential and all, is still in the future that was
/// dropped: nothing of the client's is spawned before it has a connection.
#[tokio::test]
async fn cancelling_while_a_connection_is_made_closes_it() {
    let (listener, url) = listen("https").await;
    let (hello, heard) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        let read = tcp.read(&mut [0; 4096]).await.unwrap();
        hello.send(read).unwrap();
        until_closed(&mut tcp).await
    });
    let http = http(&Tls::new().unwrap());
    let request = sending(&http, url);
    assert!(heard.await.unwrap() > 0, "no handshake began");
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert_eq!(server.await.unwrap(), Seen::End);
}

/// Cancelled while the response head is awaited: the server has the whole
/// request, credential included, and has not answered. Dropping the request
/// closes the connection, and no task of the client's is left holding what
/// it sent.
#[tokio::test]
async fn cancelling_while_the_head_is_awaited_closes_the_connection() {
    let (listener, url) = listen("http").await;
    let (got, heard) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut tcp, _) = listener.accept().await.unwrap();
        got.send(head(&mut tcp).await.0).unwrap();
        until_closed(&mut tcp).await
    });
    let http = http(&Tls::new().unwrap());
    let request = sending(&http, url);
    let sent = heard.await.unwrap();
    assert!(
        sent.contains("authorization: Bearer not-a-real-token"),
        "{sent}"
    );
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    assert_eq!(server.await.unwrap(), Seen::End);
    assert_eq!(left_running(http.tasks()).await, 0);
}

/// Answers the request on `listener` with a head promising 100 bytes and the
/// first 10 of them, then says how the connection ended.
async fn half_a_body(listener: TcpListener, sent: tokio::sync::oneshot::Sender<()>) -> Seen {
    let (mut tcp, _) = listener.accept().await.unwrap();
    let _ = head(&mut tcp).await;
    tcp.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 100\r\n\r\n0123456789")
        .await
        .unwrap();
    sent.send(()).unwrap();
    until_closed(&mut tcp).await
}

/// Cancelled mid-stream: the head and the first bytes of the body have been
/// read, and the rest has not come. Dropping the reader closes the
/// connection, and leaves no task of the client's.
#[tokio::test]
async fn cancelling_mid_stream_closes_the_connection() {
    let (listener, url) = listen("http").await;
    let (sent, written) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(half_a_body(listener, sent));
    let http = http(&Tls::new().unwrap());
    let (first, taken) = tokio::sync::oneshot::channel();
    let reading = tokio::spawn({
        let http = http.clone();
        async move {
            let mut chunks = Chunks::new(get(&http, &url).await.unwrap().into_body());
            let mut first = Some(first);
            while let Some(next) = chunks.next().await {
                if let (Ok(Chunk::Data(data)), Some(first)) = (next, first.take()) {
                    first.send(data).unwrap();
                }
            }
        }
    });
    written.await.unwrap();
    assert_eq!(&taken.await.unwrap()[..], b"0123456789");
    reading.abort();
    assert!(reading.await.unwrap_err().is_cancelled());
    assert_eq!(server.await.unwrap(), Seen::End);
    assert_eq!(left_running(http.tasks()).await, 0);
}

/// A body being read when its client is dropped stops as incomplete: the
/// client's tasks, the connection among them, are aborted, and what hyper
/// says then, that the message did not complete, is what the error holds.
#[tokio::test]
async fn a_body_whose_client_is_dropped_is_incomplete() {
    let (listener, url) = listen("http").await;
    let (sent, written) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(half_a_body(listener, sent));
    let http = http(&Tls::new().unwrap());
    let mut chunks = Chunks::new(get(&http, &url).await.unwrap().into_body());
    written.await.unwrap();
    let mut data = Vec::new();
    let reading = tokio::time::timeout(Duration::from_secs(5), async {
        while data.len() < 10 {
            match chunks.next().await {
                Some(Ok(Chunk::Data(more))) => data.extend_from_slice(&more),
                Some(Ok(Chunk::Quiet)) => {}
                other => panic!("the body stopped after {} bytes: {other:?}", data.len()),
            }
        }
    });
    reading.await.expect("ten bytes never came");
    assert_eq!(data, b"0123456789");
    drop(http);
    let next = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match chunks.next().await {
                Some(Ok(Chunk::Quiet)) => {}
                next => break next,
            }
        }
    });
    let next = next.await.expect("the body neither failed nor ended");
    let Some(Err(BodyError::Incomplete(error))) = next else {
        panic!("not incomplete: {next:?}");
    };
    assert!(error.is_incomplete_message(), "{error:?}");
    assert_eq!(server.await.unwrap(), Seen::End);
}

/// A connector that dials the test's server, tells it which host the request
/// was for, and then holds that connection without ever handing it over:
/// a setup that stalls until its request is dropped.
#[derive(Clone)]
struct Stalling(SocketAddr);

impl Service<Uri> for Stalling {
    type Response = TokioIo<Conn>;
    type Error = io::Error;
    type Future = BoxFuture<'static, io::Result<TokioIo<Conn>>>;

    fn poll_ready(&mut self, _: &mut std::task::Context<'_>) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Ok(()))
    }

    fn call(&mut self, target: Uri) -> Self::Future {
        let server = self.0;
        Box::pin(async move {
            let mut tcp = TcpStream::connect(server).await?;
            let host = target.host().unwrap_or_default();
            tcp.write_all(format!("{host}\n").as_bytes()).await?;
            std::future::pending::<()>().await;
            Ok(TokioIo::new(Conn::new(tcp)))
        })
    }
}

/// Accepts each connection on `listener`, and hands it on with the host its
/// setup named.
fn accepting(listener: TcpListener) -> tokio::sync::mpsc::Receiver<(String, TcpStream)> {
    let (made, accepted) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        loop {
            let (mut tcp, _) = listener.accept().await.unwrap();
            let mut host = Vec::new();
            let mut byte = [0; 1];
            while tcp.read_exact(&mut byte).await.is_ok() && byte != *b"\n" {
                host.extend_from_slice(&byte);
            }
            if made
                .send((String::from_utf8(host).unwrap(), tcp))
                .await
                .is_err()
            {
                return;
            }
        }
    });
    accepted
}

/// A client makes at most `MAX_SETUPS` connections at once, whatever hosts
/// they are for; a request past that waits for a slot rather than failing,
/// and takes the one a cancelled setup gives up. The cancelled setup's
/// connection ends as the server sees it.
#[tokio::test]
async fn setups_past_the_bound_wait_for_a_slot() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let stalling = Stalling(listener.local_addr().unwrap());
    let mut accepted = accepting(listener);
    let tasks = Tasks::new();
    let client = client(stalling, &tasks);
    let hosts: Vec<String> = (0..=MAX_SETUPS)
        .map(|at| format!("peer{at}.test"))
        .collect();
    let requests: Vec<_> = hosts
        .iter()
        .map(|host| {
            let client = client.clone();
            let request = Request::get(format!("http://{host}/")).body(String::new());
            tokio::spawn(async move { exchange(&client, request.unwrap()).await.err() })
        })
        .collect();
    let mut held = Vec::new();
    for _ in 0..MAX_SETUPS {
        held.push(accepted.recv().await.unwrap());
    }
    let past = tokio::time::timeout(Duration::from_millis(200), accepted.recv()).await;
    assert!(past.is_err(), "a setup past the bound was made: {past:?}");
    let (host, mut first) = held.swap_remove(0);
    let at = hosts.iter().position(|each| *each == host).unwrap();
    requests.get(at).unwrap().abort();
    assert_eq!(until_closed(&mut first).await, Seen::End);
    let waited = tokio::time::timeout(Duration::from_secs(5), accepted.recv()).await;
    let (waited, _) = waited
        .expect("the waiting setup never took the slot")
        .unwrap();
    let entered: Vec<&String> = held.iter().map(|(host, _)| host).collect();
    assert!(
        waited != host && !entered.contains(&&waited),
        "{waited} again"
    );
    for request in requests {
        request.abort();
    }
}
