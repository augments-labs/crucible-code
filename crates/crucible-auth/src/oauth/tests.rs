//! Protocol, cancellation and secrecy tests for subscription login.

use super::openai::{CLIENT_ID, Flow, VERIFY, now};
use super::*;

use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use crucible_core::Outgoing;

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-oauth-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Request {
    target: String,
    body: String,
}

fn server(
    responses: Vec<String>,
) -> (
    String,
    std::sync::mpsc::Receiver<Request>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (send, requests) = mpsc::channel();
    let worker = thread::spawn(move || {
        for response in responses {
            let (mut stream, _) = listener.accept().unwrap();
            let request = read_request(&mut stream);
            send.send(request).unwrap();
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
                response.len()
            )
            .unwrap();
        }
    });
    (base, requests, worker)
}

fn read_request(stream: &mut TcpStream) -> Request {
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    let header_end = loop {
        let read = stream.read(&mut buffer).unwrap();
        assert_ne!(read, 0, "request ended before its headers");
        bytes.extend_from_slice(buffer.get(..read).unwrap());
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let headers = String::from_utf8(bytes.get(..header_end).unwrap().to_vec()).unwrap();
    let length = headers
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("content-length: ")
                .and_then(|value| value.parse::<usize>().ok())
        })
        .unwrap_or_default();
    while bytes.len() < header_end + length {
        let read = stream.read(&mut buffer).unwrap();
        assert_ne!(read, 0, "request ended before its body");
        bytes.extend_from_slice(buffer.get(..read).unwrap());
    }
    let body_end = header_end.checked_add(length).unwrap();
    Request {
        target: headers
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap()
            .to_owned(),
        body: String::from_utf8(bytes.get(header_end..body_end).unwrap().to_vec()).unwrap(),
    }
}

fn callback(port: u16, target: &str) -> String {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(
        stream,
        "GET {target} HTTP/1.1\r\nHost: localhost:{port}\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn jwt(value: &serde_json::Value) -> String {
    let encoded =
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(serde_json::to_vec(value).unwrap());
    format!("e30.{encoded}.signature")
}

fn tokens(access: &str, refresh: &str, account: &str, expires: u64) -> String {
    serde_json::json!({
        "access_token": jwt(&serde_json::json!({ "exp": expires })),
        "refresh_token": refresh,
        "id_token": jwt(&serde_json::json!({
            "https://api.openai.com/auth": { "chatgpt_account_id": account }
        })),
        "token_type": "Bearer",
        "canary": access,
    })
    .to_string()
}

fn attempt() -> (
    LoginAttempt,
    mpsc::SyncSender<Result<LoginUpdate, OAuthError>>,
    mpsc::Receiver<Box<str>>,
) {
    let (updates, received) = mpsc::sync_channel(3);
    let (input, submitted) = mpsc::sync_channel(1);
    (
        LoginAttempt {
            updates: received,
            input,
            cancel: Cancel::new(),
        },
        updates,
        submitted,
    )
}

#[test]
fn method_names_are_owned_by_the_implementation_that_declares_them() {
    const DEVICE: LoginMethod = LoginMethod::new("device");

    assert_eq!(DEVICE.as_str(), "device");
    assert_eq!(format!("{DEVICE:?}"), "LoginMethod(\"device\")");
}

#[test]
fn authorization_uris_and_codes_are_redacted_from_debug_output() {
    let update = LoginUpdate::Authorize {
        browser_uri: "https://example.test/?secret=browser-canary".into(),
        shown_uri: "https://example.test/short-canary".into(),
        user_code: Some("code-canary".into()),
        manual: true,
    };

    let shown = format!("{update:?}");
    for canary in ["browser-canary", "short-canary", "code-canary"] {
        assert!(
            !shown.contains(canary),
            "authorization material reached Debug: {shown}"
        );
    }
}

#[test]
fn tokens_are_redacted_in_debug_output() {
    let tokens = Tokens::new("access-canary".into(), "refresh-canary".into(), 1, 1)
        .with_detail("account_id", "account-canary");

    let shown = format!("{tokens:?}");
    for canary in ["access-canary", "refresh-canary", "account-canary"] {
        assert!(!shown.contains(canary), "a token reached Debug: {shown}");
    }
}

#[test]
fn an_attempt_delivers_bounded_updates_without_busy_waiting() {
    let (attempt, updates, _) = attempt();
    updates.send(Ok(LoginUpdate::Complete)).unwrap();

    assert_eq!(
        attempt.wait(Duration::from_millis(10)).unwrap(),
        Some(LoginUpdate::Complete)
    );
    assert_eq!(attempt.wait(Duration::from_millis(1)).unwrap(), None);
}

#[test]
fn manual_input_is_trimmed_bounded_and_never_printed() {
    let (attempt, _, submitted) = attempt();

    attempt.submit("  authorization-canary  ").unwrap();
    assert_eq!(&*submitted.recv().unwrap(), "authorization-canary");
    assert!(matches!(
        attempt.submit("  "),
        Err(OAuthError::Invalid { .. })
    ));
    assert!(matches!(
        attempt.submit(&"x".repeat(16 * 1024 + 1)),
        Err(OAuthError::Invalid { .. })
    ));
    assert!(!format!("{attempt:?}").contains("authorization-canary"));
}

#[test]
fn dropping_an_attempt_requests_cancellation() {
    let (attempt, _, _) = attempt();
    let cancel = attempt.cancel.clone();

    drop(attempt);

    assert!(cancel.requested());
}

#[test]
fn device_login_follows_the_protocol_and_persists_before_completion() {
    let expires = now() + 3600;
    let (base, requests, server) = server(vec![
        serde_json::json!({
            "device_auth_id": "device-id",
            "user_code": "ABCD-EFGH",
            "interval": 0,
        })
        .to_string(),
        serde_json::json!({
            "authorization_code": "authorization-code",
            "code_verifier": "verifier",
        })
        .to_string(),
        tokens("unused-canary", "refresh-one", "account-one", expires),
    ]);
    let flow = Flow::testing(&base);
    let oauth = OpenAiOAuth::testing(flow);
    let scratch = Scratch::new("device");
    let store = Store::in_home(scratch.path());

    let attempt = oauth.start(OpenAiOAuth::DEVICE, store.clone()).unwrap();
    let first = attempt.wait(PATIENCE).unwrap().unwrap();
    assert_eq!(
        first,
        LoginUpdate::Authorize {
            browser_uri: VERIFY.into(),
            shown_uri: VERIFY.into(),
            user_code: Some("ABCD-EFGH".into()),
            manual: false,
        }
    );
    assert_eq!(
        attempt.wait(PATIENCE).unwrap(),
        Some(LoginUpdate::Progress {
            message: "finishing device authorization…",
        })
    );
    assert_eq!(attempt.wait(PATIENCE).unwrap(), Some(LoginUpdate::Complete));

    let sent: Vec<_> = (0..3)
        .map(|_| requests.recv_timeout(PATIENCE).unwrap())
        .collect();
    server.join().unwrap();
    assert_eq!(
        sent.iter()
            .map(|request| request.target.as_str())
            .collect::<Vec<_>>(),
        [
            "/api/accounts/deviceauth/usercode",
            "/api/accounts/deviceauth/token",
            "/oauth/token",
        ]
    );
    let [device, token, exchange] = sent.as_slice() else {
        panic!("three requests were expected");
    };
    assert!(device.body.contains(CLIENT_ID));
    assert!(token.body.contains("device-id"));
    assert!(exchange.body.contains("authorization-code"));
    assert!(exchange.body.contains("code_verifier=verifier"));

    let keys = store.read();
    assert!(keys.has("openai"), "completion preceded persistence");
    let credential = oauth.credential(&keys).unwrap();
    let mut outgoing = Outgoing::new();
    credential.authorize(&mut outgoing).unwrap();
    let headers: std::collections::BTreeMap<_, _> = outgoing
        .headers()
        .iter()
        .map(|(name, value)| (name.as_ref(), value.as_ref()))
        .collect();
    assert_eq!(headers.get("chatgpt-account-id"), Some(&"account-one"));
    assert_eq!(headers.get("originator"), Some(&"crucible-code"));
    assert!(
        headers
            .get("authorization")
            .is_some_and(|value| value.starts_with("Bearer e30."))
    );
}

#[test]
fn browser_login_binds_state_pkce_and_the_loopback_redirect() {
    let expires = now() + 3600;
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "browser-refresh",
        "browser-account",
        expires,
    )]);
    let oauth = OpenAiOAuth::testing(Flow::testing(&base));
    let scratch = Scratch::new("browser");
    let store = Store::in_home(scratch.path());
    let attempt = oauth.start(OpenAiOAuth::BROWSER, store.clone()).unwrap();

    let (authorization, launch) = match attempt.wait(PATIENCE).unwrap().unwrap() {
        LoginUpdate::Authorize {
            browser_uri,
            shown_uri,
            user_code: None,
            manual: true,
        } => (browser_uri, shown_uri),
        other => panic!("expected browser authorization, got {other:?}"),
    };
    assert!(authorization.starts_with(&format!("{base}/oauth/authorize?")));
    assert!(authorization.contains("code_challenge_method=S256"));
    assert!(authorization.contains("originator=crucible-code"));
    let state = authorization
        .split('?')
        .nth(1)
        .and_then(|query| {
            query
                .split('&')
                .find_map(|field| field.strip_prefix("state="))
        })
        .unwrap();
    let port = launch
        .strip_prefix("http://localhost:")
        .and_then(|rest| rest.split('/').next())
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap();

    let response = callback(
        port,
        &format!("/auth/callback?code=browser-code&state={state}"),
    );
    assert!(response.starts_with("HTTP/1.1 200"));
    assert_eq!(
        attempt.wait(PATIENCE).unwrap(),
        Some(LoginUpdate::Progress {
            message: "finishing browser authorization…",
        })
    );
    assert_eq!(attempt.wait(PATIENCE).unwrap(), Some(LoginUpdate::Complete));

    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert_eq!(sent.target, "/oauth/token");
    assert!(sent.body.contains("code=browser-code"));
    assert!(sent.body.contains("code_verifier="));
    assert!(sent.body.contains(&format!(
        "redirect_uri=http%3A%2F%2Flocalhost%3A{port}%2Fauth%2Fcallback"
    )));
    assert!(store.read().has("openai"));
}

#[test]
fn browser_login_can_finish_with_a_code_pasted_into_the_terminal() {
    let expires = now() + 3600;
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "manual-refresh",
        "manual-account",
        expires,
    )]);
    let oauth = OpenAiOAuth::testing(Flow::testing(&base));
    let scratch = Scratch::new("browser-manual");
    let store = Store::in_home(scratch.path());
    let attempt = oauth.start(OpenAiOAuth::BROWSER, store.clone()).unwrap();

    assert!(matches!(
        attempt.wait(PATIENCE).unwrap(),
        Some(LoginUpdate::Authorize { manual: true, .. })
    ));
    attempt.submit("manual-code").unwrap();
    assert_eq!(
        attempt.wait(PATIENCE).unwrap(),
        Some(LoginUpdate::Progress {
            message: "finishing browser authorization…",
        })
    );
    assert_eq!(attempt.wait(PATIENCE).unwrap(), Some(LoginUpdate::Complete));

    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert!(sent.body.contains("code=manual-code"));
    assert!(store.read().has("openai"));
}

#[test]
fn an_expired_rotation_is_refreshed_and_rewritten_before_use() {
    let expires = now() + 3600;
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "refresh-new",
        "account-new",
        expires,
    )]);
    let flow = Flow::testing(&base);
    let oauth = OpenAiOAuth::testing(flow);
    let scratch = Scratch::new("refresh");
    let store = Store::in_home(scratch.path());
    store
        .keep_subscription(
            "openai",
            Tokens::new(
                jwt(&serde_json::json!({ "exp": 1 })).into(),
                "refresh-old".into(),
                1,
                1,
            )
            .with_detail("account_id", "account-old"),
        )
        .unwrap();

    let credential = oauth.credential(&store.read()).unwrap();
    let mut outgoing = Outgoing::new();
    credential.authorize(&mut outgoing).unwrap();

    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert_eq!(sent.target, "/oauth/token");
    assert!(sent.body.contains("refresh-old"));
    assert_eq!(
        outgoing
            .headers()
            .iter()
            .find(|(name, _)| name.as_ref() == "chatgpt-account-id")
            .map(|(_, value)| value.as_ref()),
        Some("account-new")
    );

    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(text.contains("refresh-new"));
    assert!(!text.contains("refresh-old"));
}
