//! Real loopback observations of mediation, authority and shutdown; no external DNS.

use std::io::{BufRead as _, Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crucible_core::{
    SandboxDomainPattern, SandboxDomainPolicy, SandboxId, SandboxNetworkProvenance,
};

use super::Mediator;

fn policy(allow: bool) -> SandboxDomainPolicy {
    SandboxDomainPolicy::new(
        if allow {
            vec![SandboxDomainPattern::new("127.0.0.1").unwrap()]
        } else {
            Vec::new()
        },
        [],
        false,
        [],
        SandboxNetworkProvenance::User,
    )
    .unwrap()
}

#[test]
fn an_authenticated_tunnel_reaches_only_an_authorized_pinned_address() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = listener.local_addr().unwrap();
    let echo = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = [0; 4];
        stream.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ping");
        stream.write_all(b"pong").unwrap();
    });
    let proxy =
        Mediator::tcp(policy(true), SandboxId::new(), Some(Duration::from_secs(5))).unwrap();
    let mut stream = TcpStream::connect(proxy.address()).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        stream,
        "CONNECT {endpoint} HTTP/1.1\r\nProxy-Authorization: {}\r\n\r\n",
        proxy.authorization()
    )
    .unwrap();
    let mut head = [0; 39];
    stream.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 200 Connection Established\r\n\r\n");
    stream.write_all(b"ping").unwrap();
    let mut answer = [0; 4];
    stream.read_exact(&mut answer).unwrap();
    assert_eq!(&answer, b"pong");
    drop(stream);
    echo.join().unwrap();
}

#[test]
fn refused_targets_and_other_command_credentials_never_reach_the_origin() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let endpoint = listener.local_addr().unwrap();
    let allowed =
        Mediator::tcp(policy(true), SandboxId::new(), Some(Duration::from_secs(5))).unwrap();
    let denied = Mediator::tcp(
        policy(false),
        SandboxId::new(),
        Some(Duration::from_secs(5)),
    )
    .unwrap();
    for (address, authorization) in [
        (denied.address(), denied.authorization()),
        (allowed.address(), denied.authorization()),
    ] {
        let mut stream = TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(
            stream,
            "CONNECT {endpoint} HTTP/1.1\r\nProxy-Authorization: {authorization}\r\n\r\n"
        )
        .unwrap();
        let mut reply = String::new();
        stream.read_to_string(&mut reply).unwrap();
        assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
        assert_eq!(
            listener.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn an_idle_client_does_not_keep_a_cancelled_mediator_or_listener_alive() {
    let proxy =
        Mediator::tcp(policy(true), SandboxId::new(), Some(Duration::from_mins(1))).unwrap();
    let address = proxy.address();
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream.write_all(b"CONNECT ").unwrap();
    let started = Instant::now();
    drop(proxy);
    assert!(started.elapsed() < Duration::from_secs(2));
    let mut reply = [0; 1];
    assert!(stream.read(&mut reply).map_or(true, |count| count == 0));
    assert!(TcpStream::connect_timeout(&address, Duration::from_millis(100)).is_err());
}

#[cfg(unix)]
#[test]
fn the_private_unix_transport_enforces_the_same_authenticated_policy() {
    use std::os::unix::net::UnixStream;

    let sample = crate::sample::Sample::socket("sandbox-proxy-unix");
    let path = sample.root().join("proxy.sock");
    let proxy = Mediator::unix(
        &path,
        policy(false),
        SandboxId::new(),
        Some(Duration::from_secs(5)),
    )
    .unwrap();
    let mut stream = UnixStream::connect(&path).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    stream
        .write_all(b"CONNECT denied.test:443 HTTP/1.1\r\n\r\n")
        .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 403 "), "{response}");
    drop(proxy);
    assert!(
        !path.exists(),
        "the mediator disposes its own socket pathname"
    );
}

#[test]
fn listener_failure_cannot_be_erased_by_a_second_stop() {
    let mut proxy = Mediator::tcp(
        policy(false),
        SandboxId::new(),
        Some(Duration::from_secs(5)),
    )
    .unwrap();
    proxy.stop().unwrap();
    proxy.listener = Some(std::thread::spawn(|| {
        Err(std::io::Error::other("injected listener failure"))
    }));
    assert!(proxy.stop().is_err());
    assert!(
        proxy.stop().is_err(),
        "consuming the listener handle must not erase its failed result"
    );
}

#[test]
fn relay_thread_creation_failure_reaches_mediator_cleanup() {
    let mut proxy = Mediator::tcp(policy(false), SandboxId::new(), None).unwrap();
    proxy.inject_relay_spawn_failure();
    let mut client = TcpStream::connect(proxy.address()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    let mut reply = Vec::new();
    let _ = client.read_to_end(&mut reply);
    assert!(proxy.stop().is_err());
}

#[test]
fn response_worker_panic_reaches_mediator_cleanup() {
    let origin = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = origin.local_addr().unwrap();
    let mut proxy = Mediator::tcp(policy(true), SandboxId::new(), None).unwrap();
    proxy.inject_response_panic();
    let mut client = TcpStream::connect(proxy.address()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write!(
        client,
        "CONNECT {endpoint} HTTP/1.1\r\nProxy-Authorization: {}\r\n\r\n",
        proxy.authorization()
    )
    .unwrap();
    // Wait until the relay accepted CONNECT before cancelling its client.
    // Otherwise cancellation can win before the response worker is started.
    let mut reply = [0_u8; b"HTTP/1.1 200 Connection Established\r\n\r\n".len()];
    client.read_exact(&mut reply).unwrap();
    assert_eq!(&reply, b"HTTP/1.1 200 Connection Established\r\n\r\n");
    let accepted = origin.accept().unwrap().0;
    drop(accepted);
    drop(client);
    assert!(proxy.stop().is_err());
}

#[test]
fn no_command_deadline_keeps_the_mediator_owned_until_stop() {
    let mut proxy = Mediator::tcp(policy(false), SandboxId::new(), None).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let mut client = TcpStream::connect(proxy.address()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    client
        .write_all(b"CONNECT denied.test:443 HTTP/1.1\r\n\r\n")
        .unwrap();
    let mut reply = String::new();
    client.read_to_string(&mut reply).unwrap();
    assert!(reply.starts_with("HTTP/1.1 403 "), "{reply}");
    proxy.stop().unwrap();
}

#[test]
fn proxy_environment_encodes_each_commands_credential_and_clears_bypasses() {
    use base64::Engine as _;

    let proxy = Mediator::tcp(
        policy(false),
        SandboxId::new(),
        Some(Duration::from_secs(5)),
    )
    .unwrap();
    let endpoint = "127.0.0.1:31337".parse().unwrap();
    let environment = proxy.environment(endpoint);
    let values: std::collections::BTreeMap<_, _> = environment.into_iter().collect();
    for name in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        let value = values.get(name).expect("proxy environment");
        let uri: http::Uri = value.parse().expect("proxy URI");
        let authority = uri.authority().expect("proxy authority").as_str();
        let (userinfo, address) = authority.split_once('@').expect("proxy credential");
        assert_eq!(address, "127.0.0.1:31337");
        let expected = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(userinfo)
        );
        let credential_matches = expected == proxy.authorization();
        assert!(
            credential_matches,
            "proxy URL credential differs from listener authorization"
        );
    }
    assert!(values.get("NO_PROXY").is_some_and(String::is_empty));
    assert!(values.get("no_proxy").is_some_and(String::is_empty));
    assert!(!format!("{proxy:?}").contains(proxy.authorization()));
}

#[test]
fn hostname_resolution_requires_private_address_consent_and_respects_denies() {
    for (private_consent, denied_name, permitted) in [
        (false, false, false),
        (true, false, true),
        (true, true, false),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let names = if private_consent {
            vec!["localhost", "127.0.0.1"]
        } else {
            vec!["localhost"]
        };
        let denied = if denied_name {
            vec!["localhost"]
        } else {
            Vec::new()
        };
        let policy = SandboxDomainPolicy::new(
            names
                .into_iter()
                .map(|host| SandboxDomainPattern::new(host).unwrap()),
            denied
                .into_iter()
                .map(|host| SandboxDomainPattern::new(host).unwrap()),
            false,
            [],
            SandboxNetworkProvenance::User,
        )
        .unwrap();
        let proxy = Mediator::tcp(policy, SandboxId::new(), Some(Duration::from_secs(8))).unwrap();
        let mut client = TcpStream::connect(proxy.address()).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(6)))
            .unwrap();
        client
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        write!(
            client,
            "CONNECT localhost:{port} HTTP/1.1\r\nProxy-Authorization: {}\r\n\r\n",
            proxy.authorization()
        )
        .unwrap();
        let mut client = std::io::BufReader::new(client);
        let mut status = String::new();
        client.read_line(&mut status).unwrap();
        if !permitted {
            assert!(status.starts_with("HTTP/1.1 403 "), "{status}");
            assert_eq!(
                listener.accept().unwrap_err().kind(),
                std::io::ErrorKind::WouldBlock
            );
            continue;
        }
        assert_eq!(status, "HTTP/1.1 200 Connection Established\r\n");
        let mut blank = String::new();
        client.read_line(&mut blank).unwrap();
        assert_eq!(blank, "\r\n");
        client.get_mut().write_all(b"ping").unwrap();
        let (mut origin, _) = listener.accept().unwrap();
        origin.set_nonblocking(false).unwrap();
        origin
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        origin
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let mut bytes = [0; 4];
        origin.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"ping");
        origin.write_all(b"pong").unwrap();
        client.read_exact(&mut bytes).unwrap();
        assert_eq!(&bytes, b"pong");
    }
}
