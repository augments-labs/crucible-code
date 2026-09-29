//! Real loopback observations of a command's allowed traffic leaving through
//! the proxy crucible's own environment names; no external DNS.
//!
//! Each fake upstream proxy and origin is a loopback listener. One that is
//! meant to stay untouched is left non-blocking and asked for a connection
//! only after the command has its whole answer, by which time any connection
//! the mediator made to it is already waiting in its queue.

use std::io::{BufReader, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crucible_http::ProxyEnv;
use crucible_sandbox::{SandboxDomainPattern, SandboxDomainPolicy, SandboxNetworkProvenance};
use crucible_types::SandboxId;

use super::super::Mediator;

/// The user information the upstream proxy's address carries.
const USERINFO: &str = "user:secret";
/// That user information as the upstream is sent it.
const BASIC: &str = "Basic dXNlcjpzZWNyZXQ=";
/// What an upstream that opens a tunnel answers.
const OPENED: &[u8] = b"HTTP/1.1 200 Connection established\r\n\r\n";

/// The proxy settings of an environment naming `proxy`, and `bypass` as its
/// `NO_PROXY`.
fn settings(proxy: &str, bypass: Option<&str>) -> Arc<ProxyEnv> {
    let proxy = proxy.to_owned();
    let bypass = bypass.map(str::to_owned);
    Arc::new(ProxyEnv::read(move |name| match name {
        "HTTPS_PROXY" => Some(proxy.clone()),
        "NO_PROXY" => bypass.clone(),
        _ => None,
    }))
}

/// `http://user:secret@127.0.0.1:{port}`, the upstream a test listens as.
fn at(port: u16) -> String {
    format!("http://{USERINFO}@127.0.0.1:{port}")
}

fn policy(allowed: &[&str], denied: &[&str]) -> SandboxDomainPolicy {
    SandboxDomainPolicy::new(
        allowed
            .iter()
            .map(|host| SandboxDomainPattern::new(host).unwrap()),
        denied
            .iter()
            .map(|host| SandboxDomainPattern::new(host).unwrap()),
        false,
        [],
        SandboxNetworkProvenance::User,
    )
    .unwrap()
}

/// `localhost`, with consent to its loopback address.
fn loopback() -> SandboxDomainPolicy {
    policy(&["localhost", "127.0.0.1"], &[])
}

/// A loopback listener nothing is meant to reach, and its port.
fn untouched() -> (TcpListener, u16) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    (listener, port)
}

fn never_reached(listener: &TcpListener, what: &str) {
    assert_eq!(
        listener.accept().map(|_| ()).unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock,
        "{what} was connected to"
    );
}

/// What a fake upstream proxy does once it has read a request's head.
enum Answer {
    /// Opens the tunnel, then answers "ping" with "pong" or a request with
    /// `204 No Content`.
    Opens,
    /// Sends these bytes, then holds the connection until the mediator
    /// closes it.
    Sends(Vec<u8>),
    /// Closes the connection without a word.
    Closes,
    /// Says nothing, and holds the connection until the mediator closes it.
    Holds,
}

/// What a fake upstream proxy was sent.
struct Seen {
    /// The head it was asked to open a tunnel with.
    head: String,
    /// What arrived inside the tunnel it opened.
    inside: String,
}

/// A fake upstream proxy taking one connection, for up to five seconds.
struct Upstream {
    port: u16,
    seen: JoinHandle<Option<Seen>>,
}

impl Upstream {
    fn answering(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let seen = std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(_) if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => return None,
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(10)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut stream = BufReader::new(stream);
            let head = head(&mut stream);
            let mut inside = String::new();
            match answer {
                Answer::Opens => {
                    stream.get_mut().write_all(OPENED).unwrap();
                    inside = tunnelled(&mut stream);
                    let reply: &[u8] = if inside == "ping" {
                        b"pong"
                    } else {
                        b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    };
                    stream.get_mut().write_all(reply).unwrap();
                }
                Answer::Sends(bytes) => {
                    let _ = stream.get_mut().write_all(&bytes);
                    let _ = stream.read_to_end(&mut Vec::new());
                }
                Answer::Closes => {}
                Answer::Holds => {
                    let _ = stream.read_to_end(&mut Vec::new());
                }
            }
            Some(Seen { head, inside })
        });
        Self { port, seen }
    }

    /// What it was sent; `None` when nothing connected to it.
    fn seen(self) -> Option<Seen> {
        self.seen.join().unwrap()
    }
}

/// A request head, read up to and including its blank line.
fn head(stream: &mut impl std::io::BufRead) -> String {
    let mut head = String::new();
    loop {
        let start = head.len();
        if stream.read_line(&mut head).unwrap() == 0 || head.get(start..) == Some("\r\n") {
            return head;
        }
    }
}

/// What a tunnel carried: "ping", or a request up to its blank line.
fn tunnelled(stream: &mut impl std::io::Read) -> String {
    let mut inside = Vec::new();
    let mut byte = [0];
    while inside != b"ping" && !inside.ends_with(b"\r\n\r\n") && inside.len() < 16 * 1024 {
        stream.read_exact(&mut byte).unwrap();
        inside.extend_from_slice(&byte);
    }
    String::from_utf8(inside).unwrap()
}

/// The value of the field `name` in `head`, if it has exactly that field.
fn field<'h>(head: &'h str, name: &str) -> Option<&'h str> {
    head.lines().find_map(|line| {
        let (field, value) = line.split_once(": ")?;
        field.eq_ignore_ascii_case(name).then_some(value)
    })
}

/// A command's connection to `proxy`, having sent it `request` with this
/// command's credential.
fn ask(proxy: &Mediator, request: &str) -> BufReader<TcpStream> {
    let mut client = TcpStream::connect(proxy.address()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    client
        .set_write_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let (line, fields) = request.split_once("\r\n").unwrap();
    write!(
        client,
        "{line}\r\nProxy-Authorization: {}\r\n{fields}",
        proxy.authorization()
    )
    .unwrap();
    BufReader::new(client)
}

/// `CONNECT` to `target`, as a command asks for a tunnel.
fn connect(target: &str) -> String {
    format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n")
}

/// Everything the mediator answered before closing the command's connection,
/// or before the command stopped waiting for more.
fn whole(mut client: BufReader<TcpStream>) -> String {
    let mut answer = Vec::new();
    let _ = client.read_to_end(&mut answer);
    String::from_utf8_lossy(&answer).into_owned()
}

/// Neither form of the upstream's credential is in `text`.
fn credential_absent(text: &str) {
    for form in ["secret", USERINFO, BASIC, "dXNlcjpzZWNyZXQ="] {
        assert!(
            !text.contains(form),
            "the upstream credential leaked: {text}"
        );
    }
}

#[test]
fn an_allowed_host_is_tunnelled_through_the_upstream_proxy() {
    let (origin, port) = untouched();
    let upstream = Upstream::answering(Answer::Opens);
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream.port), None),
    )
    .unwrap();
    let mut client = ask(&proxy, &connect(&format!("localhost:{port}")));
    assert_eq!(
        head(&mut client),
        "HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    client.get_mut().write_all(b"ping").unwrap();

    let Some(seen) = upstream.seen() else {
        panic!("the upstream proxy saw nothing: the mediator dialled the host itself");
    };
    // The address the policy checked, never the name it was asked for, so
    // what the upstream reaches is what was permitted.
    assert!(
        seen.head
            .starts_with(&format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n")),
        "{}",
        seen.head
    );
    assert_eq!(
        field(&seen.head, "host"),
        Some(format!("127.0.0.1:{port}").as_str())
    );
    assert_eq!(field(&seen.head, "proxy-authorization"), Some(BASIC));
    let own = proxy.authorization().strip_prefix("Basic ").unwrap();
    assert!(
        !seen.head.contains(own),
        "the command's own credential was sent upstream"
    );
    assert_eq!(seen.inside, "ping");
    let mut pong = [0; 4];
    client.read_exact(&mut pong).unwrap();
    assert_eq!(&pong, b"pong");
    never_reached(&origin, "the origin");
}

#[test]
fn a_far_end_that_speaks_first_loses_nothing_to_the_proxy_s_answer() {
    // A server that greets first, as mail and SSH servers do, can have its
    // greeting arrive in the same write as the upstream's `200`. The answer is
    // read up to its blank line and no further, or the greeting goes with it.
    let (origin, port) = untouched();
    let upstream = Upstream::answering(Answer::Sends([OPENED, b"220 hello\r\n"].concat()));
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream.port), None),
    )
    .unwrap();
    let mut client = ask(&proxy, &connect(&format!("localhost:{port}")));
    assert_eq!(
        head(&mut client),
        "HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    let mut greeting = [0; 11];
    client
        .read_exact(&mut greeting)
        .expect("the far end's greeting was lost with the upstream's answer");
    assert_eq!(&greeting, b"220 hello\r\n");
    drop(client);

    assert!(upstream.seen().is_some());
    never_reached(&origin, "the origin");
}

#[test]
fn a_mapped_address_is_asked_for_in_the_form_the_policy_checked() {
    // The policy reads an IPv4 address written as IPv6 as the IPv4 address it
    // is, and the upstream is asked for that same form: several proxies refuse
    // or mishandle the mapped one.
    let (origin, port) = untouched();
    let upstream = Upstream::answering(Answer::Opens);
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream.port), None),
    )
    .unwrap();
    let mut client = ask(&proxy, &connect(&format!("[::ffff:127.0.0.1]:{port}")));
    assert_eq!(
        head(&mut client),
        "HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    client.get_mut().write_all(b"ping").unwrap();

    let Some(seen) = upstream.seen() else {
        panic!("the upstream proxy saw nothing");
    };
    assert!(
        seen.head
            .starts_with(&format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n")),
        "{}",
        seen.head
    );
    assert_eq!(
        field(&seen.head, "host"),
        Some(format!("127.0.0.1:{port}").as_str())
    );
    never_reached(&origin, "the origin");
}

#[test]
fn a_plain_request_is_sent_inside_a_tunnel_through_the_upstream_proxy() {
    let (origin, port) = untouched();
    let upstream = Upstream::answering(Answer::Opens);
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream.port), None),
    )
    .unwrap();
    let request =
        format!("GET http://localhost:{port}/path HTTP/1.1\r\nHost: localhost:{port}\r\n\r\n");
    let answer = whole(ask(&proxy, &request));
    assert!(
        answer.starts_with("HTTP/1.1 204 No Content\r\n"),
        "{answer}"
    );

    let Some(seen) = upstream.seen() else {
        panic!("the upstream proxy saw nothing: the mediator dialled the host itself");
    };
    assert!(
        seen.head
            .starts_with(&format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\n")),
        "{}",
        seen.head
    );
    assert_eq!(
        seen.inside,
        format!("GET /path HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n")
    );
    never_reached(&origin, "the origin");
}

#[test]
fn a_denied_host_never_reaches_the_upstream_proxy() {
    let (origin, port) = untouched();
    // Denied by name, and allowed by name but resolving to an address it was
    // given no consent for: both are settled before anything leaves.
    for policy in [
        policy(&[], &[]),
        policy(&["localhost", "127.0.0.1"], &["localhost"]),
        policy(&["localhost"], &[]),
    ] {
        let (upstream, upstream_port) = untouched();
        let proxy = Mediator::tcp(
            policy,
            SandboxId::new(),
            Some(Duration::from_secs(10)),
            settings(&at(upstream_port), None),
        )
        .unwrap();
        let answer = whole(ask(&proxy, &connect(&format!("localhost:{port}"))));
        assert!(answer.starts_with("HTTP/1.1 403 "), "{answer}");
        never_reached(&upstream, "the upstream proxy");
        never_reached(&origin, "the origin");
    }
}

/// Under a proxy that looks names up itself, an allowed host this machine
/// cannot resolve is the common case. The command is told that, not the
/// `403` of a denial, and the proxy is still not asked, since there is no
/// checked address to ask it for.
#[test]
fn an_allowed_host_this_machine_cannot_resolve_is_not_answered_as_a_denial() {
    let (upstream, upstream_port) = untouched();
    let proxy = Mediator::tcp(
        policy(&["localhost"], &[]),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream_port), None),
    )
    .unwrap();
    proxy.inject_lookup_failure();
    let answer = whole(ask(&proxy, &connect("localhost:443")));
    assert!(
        answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{answer}"
    );
    assert!(
        answer.ends_with("\r\n\r\nthe host could not be resolved on this machine\n"),
        "{answer}"
    );
    never_reached(&upstream, "the upstream proxy");
}

#[test]
fn a_host_no_proxy_names_is_reached_directly() {
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = origin.local_addr().unwrap().port();
    let (upstream, upstream_port) = untouched();
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream_port), Some("example.test,LOCALHOST")),
    )
    .unwrap();
    tunnelled_directly(&proxy, &origin, port);
    never_reached(&upstream, "the upstream proxy");
}

/// A SOCKS proxy that crucible's own requests connect past, handing it
/// nothing, is connected past for a command's allowed traffic too, so a
/// network where going direct works keeps working for confined commands.
#[test]
fn a_socks_proxy_crucible_connects_past_is_connected_past_for_a_command_too() {
    for scheme in ["socks5", "socks4", "socks"] {
        let origin = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = origin.local_addr().unwrap().port();
        let (upstream, upstream_port) = untouched();
        let proxy = Mediator::tcp(
            loopback(),
            SandboxId::new(),
            Some(Duration::from_secs(10)),
            settings(
                &format!("{scheme}://{USERINFO}@127.0.0.1:{upstream_port}"),
                None,
            ),
        )
        .unwrap();
        tunnelled_directly(&proxy, &origin, port);
        never_reached(&upstream, "the SOCKS proxy");
    }
}

/// Asks `proxy` for a tunnel to `localhost:{port}` and checks that it is
/// opened straight to `origin`, carrying bytes both ways.
fn tunnelled_directly(proxy: &Mediator, origin: &TcpListener, port: u16) {
    let mut client = ask(proxy, &connect(&format!("localhost:{port}")));
    assert_eq!(
        head(&mut client),
        "HTTP/1.1 200 Connection Established\r\n\r\n"
    );
    client.get_mut().write_all(b"ping").unwrap();
    let (mut reached, _) = origin.accept().unwrap();
    reached
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut heard = [0; 4];
    reached.read_exact(&mut heard).unwrap();
    assert_eq!(&heard, b"ping");
    reached.write_all(b"pong").unwrap();
    let mut answered = [0; 4];
    client.read_exact(&mut answered).unwrap();
    assert_eq!(&answered, b"pong");
}

#[test]
fn an_upstream_that_does_not_open_the_tunnel_fails_the_connection() {
    let mut oversized = b"HTTP/1.1 200 Connection established\r\n".to_vec();
    while oversized.len() <= 17 * 1024 {
        oversized.extend_from_slice(b"X-Filler: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
    }
    for (answer, reason) in [
        (
            Answer::Sends(
                b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n".to_vec(),
            ),
            "refused the connection with status 407",
        ),
        (Answer::Sends(oversized), "gave no usable answer"),
        (
            Answer::Sends(b"hello from another protocol\r\n\r\n".to_vec()),
            "gave no usable answer",
        ),
        (Answer::Closes, "gave no usable answer"),
    ] {
        let (origin, port) = untouched();
        let upstream = Upstream::answering(answer);
        let proxy = Mediator::tcp(
            loopback(),
            SandboxId::new(),
            Some(Duration::from_secs(10)),
            settings(&at(upstream.port), None),
        )
        .unwrap();
        let answer = whole(ask(&proxy, &connect(&format!("localhost:{port}"))));
        assert!(
            answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
            "{answer}"
        );
        assert!(answer.contains(reason), "{answer}");
        credential_absent(&answer);
        assert!(
            upstream.seen().is_some(),
            "the upstream proxy was never asked"
        );
        never_reached(&origin, "the origin");
    }
}

/// A fake upstream proxy answering each connection it takes with the next of
/// its answers, the last one repeating, until the command has its answer or
/// for up to ten seconds.
struct Counting {
    port: u16,
    done: Arc<AtomicBool>,
    asked: JoinHandle<usize>,
}

impl Counting {
    fn answering(answers: &'static [&'static [u8]]) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let done = Arc::new(AtomicBool::new(false));
        let asked = {
            let done = Arc::clone(&done);
            std::thread::spawn(move || {
                let deadline = Instant::now() + Duration::from_secs(10);
                let mut asked = 0;
                while !done.load(Ordering::Acquire) && Instant::now() < deadline {
                    let Ok((stream, _)) = listener.accept() else {
                        std::thread::sleep(Duration::from_millis(5));
                        continue;
                    };
                    stream.set_nonblocking(false).unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut stream = BufReader::new(stream);
                    head(&mut stream);
                    let answer = answers.get(asked).or(answers.last()).unwrap();
                    stream.get_mut().write_all(answer).unwrap();
                    asked += 1;
                }
                asked
            })
        };
        Self { port, done, asked }
    }

    /// How many connections it took, once the command has its answer.
    fn asked(self) -> usize {
        self.done.store(true, Ordering::Release);
        self.asked.join().unwrap()
    }
}

/// A proxy that turns crucible's credential down would turn it down for
/// every address the host has, so it is sent the credential once rather than
/// once for each of them.
#[test]
fn an_upstream_that_refuses_the_credential_is_not_sent_it_again() {
    let upstream = Counting::answering(&[
        b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
    ]);
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream.port), None),
    )
    .unwrap();
    proxy.inject_repeated_addresses();
    let answer = whole(ask(&proxy, &connect("localhost:443")));
    assert!(
        answer.contains("refused the connection with status 407"),
        "{answer}"
    );
    credential_absent(&answer);
    assert_eq!(
        upstream.asked(),
        1,
        "the proxy was sent the credential it had refused again"
    );
}

/// Any other refusal may be about the one address, so the proxy is still
/// asked for the host's next one.
#[test]
fn an_upstream_that_refuses_one_address_is_asked_for_the_next() {
    let upstream = Counting::answering(&[
        b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n",
        OPENED,
    ]);
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(upstream.port), None),
    )
    .unwrap();
    proxy.inject_repeated_addresses();
    let answer = whole(ask(&proxy, &connect("localhost:443")));
    assert!(answer.starts_with("HTTP/1.1 200 "), "{answer}");
    assert_eq!(upstream.asked(), 2, "the next address was not asked for");
}

#[test]
fn an_upstream_that_never_answers_is_given_up_within_the_handshake_bound() {
    const MARGIN: Duration = Duration::from_secs(4);

    let (origin, port) = untouched();
    let upstream = Upstream::answering(Answer::Holds);
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(30)),
        settings(&at(upstream.port), None),
    )
    .unwrap();
    let started = Instant::now();
    let answer = whole(ask(&proxy, &connect(&format!("localhost:{port}"))));
    let took = started.elapsed();
    assert!(
        answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{answer}"
    );
    assert!(
        took < super::super::HANDSHAKE + MARGIN,
        "gave up after {took:?}"
    );
    assert!(
        upstream.seen().is_some(),
        "the upstream proxy was never asked"
    );
    never_reached(&origin, "the origin");
}

/// An `https://` proxy carries crucible's own requests, so going around it
/// would bypass it; a `socks5h://` or `socks4a://` one, which was to resolve
/// the host itself, is one crucible's own requests refuse. Neither is spoken
/// to or connected around.
#[test]
fn an_upstream_the_mediator_cannot_speak_through_is_refused_rather_than_bypassed() {
    let (origin, port) = untouched();
    for scheme in ["https", "socks5h", "socks4a"] {
        let (upstream, upstream_port) = untouched();
        let proxy = Mediator::tcp(
            loopback(),
            SandboxId::new(),
            Some(Duration::from_secs(10)),
            settings(
                &format!("{scheme}://{USERINFO}@127.0.0.1:{upstream_port}"),
                None,
            ),
        )
        .unwrap();
        let answer = whole(ask(&proxy, &connect(&format!("localhost:{port}"))));
        assert!(
            answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
            "{scheme}: {answer}"
        );
        assert!(answer.contains("cannot carry"), "{scheme}: {answer}");
        credential_absent(&answer);
        never_reached(&upstream, "the upstream proxy");
        never_reached(&origin, "the origin");
    }
    // An address that cannot be rebuilt without its user information names
    // no proxy that could be reached, and is refused the same way.
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings("http://[a@p]:1", None),
    )
    .unwrap();
    let answer = whole(ask(&proxy, &connect(&format!("localhost:{port}"))));
    assert!(
        answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{answer}"
    );
    assert!(answer.contains("cannot carry"), "{answer}");
    never_reached(&origin, "the origin");
}

#[test]
fn an_unreachable_upstream_fails_the_connection_without_a_direct_dial() {
    let (origin, port) = untouched();
    // A port nothing listens on: bound, then let go.
    let closed = TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let proxy = Mediator::tcp(
        loopback(),
        SandboxId::new(),
        Some(Duration::from_secs(10)),
        settings(&at(closed), None),
    )
    .unwrap();
    let answer = whole(ask(&proxy, &connect(&format!("localhost:{port}"))));
    assert!(
        answer.starts_with("HTTP/1.1 502 Bad Gateway\r\n"),
        "{answer}"
    );
    assert!(answer.contains("could not be reached"), "{answer}");
    credential_absent(&answer);
    never_reached(&origin, "the origin");
}
