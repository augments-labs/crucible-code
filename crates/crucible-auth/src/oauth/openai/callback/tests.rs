//! The loopback callback, driven over real sockets on a test runtime.

use super::*;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;

/// A runtime shaped as the application's: several workers, a clock and
/// sockets.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn percent_decoding_is_strict_and_bounded() {
    assert_eq!(decoded("a%2Fb+c").unwrap().as_ref(), "a/b c");
    assert!(decoded("%2").is_err());
    assert!(decoded("%GG").is_err());
    assert!(decoded(&"x".repeat(MAX_VALUE + 1)).is_err());
}

#[test]
fn a_forged_state_does_not_consume_the_real_callback() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
    let port = server.port;
    let launch = server.launch_uri();
    let (_input, mut submitted) = tokio::sync::mpsc::channel(1);
    let waiting = runtime.spawn(async move {
        server
            .wait("https://example.invalid/authorize", "right", &mut submitted)
            .await
    });

    let path = launch
        .strip_prefix(&format!("http://localhost:{port}"))
        .expect("the launch address names this server");
    let launched = get(port, path);
    assert!(launched.starts_with("HTTP/1.1 302"));
    assert!(launched.contains("Location: https://example.invalid/authorize"));

    let forged = get(port, "/auth/callback?code=stolen&state=wrong");
    assert!(forged.starts_with("HTTP/1.1 400"));
    let accepted = get(port, "/auth/callback?code=kept%2Fcode&state=right");
    assert!(accepted.starts_with("HTTP/1.1 200"));
    assert_eq!(
        runtime.block_on(waiting).unwrap().unwrap().as_ref(),
        "kept/code"
    );
}

#[test]
fn a_launch_without_its_token_reveals_nothing() {
    // The launch address is handed to the user's own terminal and browser
    // and nowhere else. Loopback is every local account's, not just this
    // one's, so a bare `/launch` polled by somebody else must not answer
    // with the authorization URI — the state inside it is what lets a
    // forged callback through.
    let runtime = runtime();
    let _entered = runtime.enter();
    let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
    let port = server.port;
    let (input, mut submitted) = tokio::sync::mpsc::channel(1);
    let waiting = runtime.spawn(async move {
        server
            .wait("https://example.invalid/authorize", "right", &mut submitted)
            .await
    });

    let bare = get(port, "/launch");
    assert!(bare.starts_with("HTTP/1.1 400"), "{bare}");
    assert!(!bare.contains("example.invalid"), "{bare}");
    let guessed = get(port, "/launch/wrong-token");
    assert!(guessed.starts_with("HTTP/1.1 400"), "{guessed}");

    // The attempt that could paste a value is gone.
    drop(input);
    assert!(matches!(
        runtime.block_on(waiting).unwrap(),
        Err(OAuthError::Cancelled)
    ));
}

#[test]
fn aborting_an_idle_callback_closes_its_port_promptly() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
    let port = server.port;
    let (_input, mut submitted) = tokio::sync::mpsc::channel(1);
    let waiting = runtime.spawn(async move {
        server
            .wait("https://example.invalid", "state", &mut submitted)
            .await
    });

    waiting.abort();
    let ended =
        runtime.block_on(async { tokio::time::timeout(Duration::from_millis(200), waiting).await });

    assert!(
        matches!(&ended, Ok(Err(stopped)) if stopped.is_cancelled()),
        "the aborted callback did not end within 200 ms: {ended:?}"
    );
    assert!(
        TcpStream::connect((Ipv4Addr::LOCALHOST, port)).is_err(),
        "the aborted callback's port {port} still took a connection"
    );
}

#[test]
fn a_callback_nobody_answers_expires_at_its_lifetime() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let server = Server::bind(&[0], Duration::from_millis(100)).unwrap();
    let (_input, mut submitted) = tokio::sync::mpsc::channel(1);

    let waited = runtime.block_on(async {
        tokio::time::timeout(
            Duration::from_secs(5),
            server.wait("https://example.invalid", "state", &mut submitted),
        )
        .await
    });

    assert!(
        matches!(waited, Ok(Err(OAuthError::Expired))),
        "an unanswered callback did not expire at its lifetime: {waited:?}"
    );
}

#[test]
fn manual_input_accepts_a_code_or_the_matching_redirect_only() {
    let runtime = runtime();
    let _entered = runtime.enter();
    let server = Server::bind(&[0], Duration::from_secs(2)).unwrap();
    assert_eq!(
        server.manual("raw-code", "state").unwrap().as_ref(),
        "raw-code"
    );
    let callback = format!("{}?code=kept%2Fcode&state=right", server.redirect_uri());
    assert_eq!(
        server.manual(&callback, "right").unwrap().as_ref(),
        "kept/code"
    );
    assert!(matches!(
        server.manual(&callback, "wrong"),
        Err(OAuthError::State)
    ));
    assert!(server.manual("not a code", "state").is_err());
}

fn get(port: u16, target: &str) -> String {
    let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}
