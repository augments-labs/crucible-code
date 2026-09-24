//! Protocol, cancellation and secrecy tests for subscription login.

use super::openai::{CLIENT_ID, Flow, PORTS, VERIFY, now};
use super::*;

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::task::{Context, Poll, Waker};

use base64::Engine as _;
use crucible_core::{Authorization, CredentialError, Outgoing};

/// Polls `authorizing` once and panics if it was not ready: a credential with
/// nothing to renew answers at its first poll.
fn authorized(authorizing: Authorization<'_>) -> Result<(), CredentialError> {
    let mut authorizing = authorizing;
    match authorizing
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(answer) => answer,
        Poll::Pending => panic!("the credential would have had to wait"),
    }
}

/// The runtime a test's renewals and login requests run on, shaped as the
/// application's: several workers, a clock and sockets.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap()
}

/// An owner of renewals that runs them on `runtime`.
fn renewing(runtime: &tokio::runtime::Runtime) -> Renewals {
    let renewals = Renewals::new();
    renewals.runs_on(runtime.handle().clone());
    renewals
}

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
fn an_openai_account_scope_survives_store_reconstruction_without_token_identity() {
    let scratch = Scratch::new("stable-scope");
    let store = Store::in_home(scratch.path());
    store
        .keep_subscription(
            "openai",
            Tokens::new("access-one".into(), "refresh-one".into(), u64::MAX, 1)
                .with_detail("account_id", "account-stable"),
        )
        .unwrap();
    let oauth = OpenAiOAuth::testing(Flow::testing("http://127.0.0.1:9", &Renewals::new()));

    let first = oauth.credential(&store.read()).unwrap().scope();
    store
        .keep_subscription(
            "openai",
            Tokens::new("access-two".into(), "refresh-two".into(), u64::MAX, 2)
                .with_detail("account_id", "account-stable"),
        )
        .unwrap();
    let reconstructed = oauth.credential(&store.read()).unwrap().scope();

    assert_eq!(first, reconstructed);
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
    let runtime = runtime();
    let flow = Flow::testing(&base, &renewing(&runtime));
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
    authorized(credential.authorize(&mut outgoing)).unwrap();
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
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
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
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
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

/// Holds a port for as long as it is alive, or reports that somebody else has.
fn occupied(port: u16) -> Option<TcpListener> {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok()
}

#[test]
fn a_browser_login_is_not_refused_because_the_registered_ports_are_busy() {
    // The provider redirects to one of two registered ports, so production has
    // to ask for those and nothing else. A test cannot: the ports belong to the
    // host, another process or a second test in this binary may hold them, and a
    // login that failed for that reason would read as a broken login rather than
    // a busy machine. Occupying both proves the test flow does not want them.
    let _busy: Vec<_> = PORTS.iter().filter_map(|port| occupied(*port)).collect();

    let expires = now() + 3600;
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "busy-refresh",
        "busy-account",
        expires,
    )]);
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("browser-busy");
    let store = Store::in_home(scratch.path());
    let attempt = oauth.start(OpenAiOAuth::BROWSER, store.clone()).unwrap();

    let launch = match attempt.wait(PATIENCE).unwrap().unwrap() {
        LoginUpdate::Authorize { shown_uri, .. } => shown_uri,
        other => panic!("expected browser authorization, got {other:?}"),
    };
    let port = launch
        .strip_prefix("http://localhost:")
        .and_then(|rest| rest.split('/').next())
        .and_then(|port| port.parse::<u16>().ok())
        .unwrap();
    assert!(
        !PORTS.contains(&port),
        "a test answered on the registered port {port}"
    );

    attempt.submit("busy-code").unwrap();
    assert_eq!(
        attempt.wait(PATIENCE).unwrap(),
        Some(LoginUpdate::Progress {
            message: "finishing browser authorization…",
        })
    );
    assert_eq!(attempt.wait(PATIENCE).unwrap(), Some(LoginUpdate::Complete));
    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert!(sent.body.contains("code=busy-code"));
    assert!(store.read().has("openai"));
}

#[test]
fn a_real_login_still_answers_only_where_the_provider_will_redirect() {
    // The ephemeral port above is a test convenience. A production flow that
    // took one would receive no callback at all, because the provider refuses a
    // redirect it never registered, so the two must not converge.
    assert_eq!(Flow::production(Renewals::new()).callback_ports(), &PORTS);
    assert_eq!(
        Flow::testing("http://127.0.0.1:1", &Renewals::new()).callback_ports(),
        &[0]
    );
}

#[test]
fn authorize_answers_at_its_first_poll_when_nothing_needs_renewing() {
    // A tool run's crossing polls a future once; a fresh token that pended
    // there would be refused on every request, even though nothing here has
    // anything to wait for. The owner has no runtime, so a renewal started
    // here would be refused rather than pend.
    let oauth = OpenAiOAuth::testing(Flow::testing("http://127.0.0.1:1", &Renewals::new()));
    let scratch = Scratch::new("fresh-token");
    let store = Store::in_home(scratch.path());
    let fresh = now() + 30 * 24 * 60 * 60;
    store
        .keep_subscription(
            "openai",
            Tokens::new("access-fresh".into(), "refresh-fresh".into(), fresh, now())
                .with_detail("account_id", "account-fresh"),
        )
        .unwrap();
    let credential = oauth.credential(&store.read()).unwrap();
    let mut request = Outgoing::new();
    let mut authorizing = credential.authorize(&mut request);

    assert!(
        matches!(
            authorizing
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(Ok(()))
        ),
        "the future was still pending after one poll"
    );
}

#[test]
fn an_expired_rotation_is_refreshed_and_rewritten_before_use() {
    let expires = now() + 3600;
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "refresh-new",
        "account-old",
        expires,
    )]);
    let runtime = runtime();
    let flow = Flow::testing(&base, &renewing(&runtime));
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
    let scope = credential.scope();
    let mut outgoing = Outgoing::new();
    runtime
        .block_on(credential.authorize(&mut outgoing))
        .unwrap();
    assert_eq!(credential.scope(), scope);

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
        Some("account-old")
    );

    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(text.contains("refresh-new"));
    assert!(!text.contains("refresh-old"));
}

#[test]
fn a_refresh_cannot_move_a_live_credential_scope_to_another_account() {
    let expires = now() + 3600;
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "refresh-other",
        "account-other",
        expires,
    )]);
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("refresh-scope-change");
    let store = Store::in_home(scratch.path());
    store
        .keep_subscription(
            "openai",
            Tokens::new(
                jwt(&serde_json::json!({ "exp": 1 })).into(),
                "refresh-original".into(),
                1,
                1,
            )
            .with_detail("account_id", "account-original"),
        )
        .unwrap();

    let credential = oauth.credential(&store.read()).unwrap();
    let mut outgoing = Outgoing::new();
    let problem = runtime
        .block_on(credential.authorize(&mut outgoing))
        .unwrap_err();

    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert!(sent.body.contains("refresh-original"));
    assert!(problem.to_string().contains("refreshed account identity"));
    assert!(outgoing.headers().is_empty());

    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(text.contains("refresh-original"));
    assert!(!text.contains("refresh-other"));
}

/// A renewal is a rotation its owner runs as a task of its own, and an
/// authorization only waits for it, so one polled as a runtime worker task
/// waits the way any task does — without holding the worker — and completes.
/// It used to refuse there with a typed error, because the renewal then ran
/// inside the poll: this is that test, inverted. The server really renews, so
/// the store's bytes say whether the rotation was written.
#[test]
fn a_renewal_awaited_from_a_runtime_task_completes_and_writes_its_rotation() {
    // The account must match the stored one: the refresh checks the
    // identity-bound scope before anything is written.
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "refresh-fresh",
        "account-old",
        now() + 3600,
    )]);
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("worker-renewal");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let credential = oauth.credential(&store.read()).unwrap();

    let answered = runtime.block_on(async move {
        tokio::spawn(async move {
            let mut outgoing = Outgoing::new();
            credential
                .authorize(&mut outgoing)
                .await
                .map(|()| header(&outgoing, "chatgpt-account-id"))
        })
        .await
        .unwrap()
    });

    assert!(
        matches!(answered, Ok(Some(ref account)) if account == "account-old"),
        "an authorization polled as a runtime worker task did not complete: {answered:?}"
    );
    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert!(sent.body.contains("refresh-old"));
    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(text.contains("refresh-fresh") && !text.contains("refresh-old"));
}

/// Worker tasks authorizing with credentials for one account at once — the
/// turn's and a web source's are separate credentials — wait for the one
/// rotation in flight for that account rather than refusing, and rather than
/// each starting their own. A second rotation of the same expired tokens would
/// be seen as a second request after the first was refused; one rotation
/// shares its refusal with every waiter.
#[test]
fn worker_tasks_renewing_one_account_at_once_share_one_rotation() {
    const WAITERS: usize = 4;
    let expires = now() + 3600;
    let fresh = jwt(&serde_json::json!({ "exp": expires }));
    let (base, requests, server) = holding_server(
        tokens("unused-canary", "refresh-new", "account-old", expires),
        Duration::from_millis(300),
    );
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("shared-rotation");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let credentials: Vec<_> = (0..WAITERS)
        .map(|_| oauth.credential(&store.read()).unwrap())
        .collect();

    let answers = runtime.block_on(async move {
        let waiting: Vec<_> = credentials
            .into_iter()
            .map(|credential| {
                tokio::spawn(async move {
                    let mut outgoing = Outgoing::new();
                    credential
                        .authorize(&mut outgoing)
                        .await
                        .map(|()| header(&outgoing, "authorization"))
                })
            })
            .collect();
        let mut answers = Vec::new();
        for waiter in waiting {
            answers.push(waiter.await.unwrap());
        }
        answers
    });

    let expected = format!("Bearer {fresh}");
    for answer in &answers {
        assert!(
            matches!(answer, Ok(Some(said)) if *said == expected),
            "a waiter did not end on the shared rotation: {answer:?}"
        );
    }
    let sent: Vec<_> = requests.try_iter().collect();
    assert_eq!(sent.len(), 1, "{} rotations were sent", sent.len());
    assert!(persisted(scratch.path(), "refresh-new"));
    drop(server);
}

/// The other half of the test above: one rotation's refusal is every
/// waiter's, so the refresh token is presented once however many are waiting
/// and whether or not the service accepts it.
#[test]
fn worker_tasks_renewing_one_account_at_once_share_one_refusal() {
    const WAITERS: usize = 4;
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (heard, requests) = mpsc::channel();
    let server = thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            if heard.send(read_request(&mut stream)).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(300));
            let _ = write!(
                stream,
                "HTTP/1.1 400 Bad Request\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
            );
        }
    });
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("shared-refusal");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let credentials: Vec<_> = (0..WAITERS)
        .map(|_| oauth.credential(&store.read()).unwrap())
        .collect();

    let answers = runtime.block_on(async move {
        let waiting: Vec<_> = credentials
            .into_iter()
            .map(|credential| {
                tokio::spawn(async move {
                    let mut outgoing = Outgoing::new();
                    credential.authorize(&mut outgoing).await
                })
            })
            .collect();
        let mut answers = Vec::new();
        for waiter in waiting {
            answers.push(waiter.await.unwrap());
        }
        answers
    });

    for answer in &answers {
        assert!(
            matches!(answer, Err(CredentialError::NotRenewed(said)) if said.contains("HTTP 400")),
            "a waiter was not handed the rotation's refusal: {answer:?}"
        );
    }
    let sent: Vec<_> = requests.try_iter().collect();
    assert_eq!(
        sent.len(),
        1,
        "the expired rotation was presented {} times",
        sent.len()
    );
    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(
        text.contains("refresh-old"),
        "a refused rotation changed the store"
    );
    drop(server);
}

/// A token server that answers its one renewal only once `hold` has passed
/// since the request arrived, and reports every request it is sent, answered
/// or not, so a second rotation cannot hide behind the first one's answer.
fn holding_server(
    response: String,
    hold: Duration,
) -> (
    String,
    std::sync::mpsc::Receiver<Request>,
    thread::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (send, requests) = mpsc::channel();
    let worker = thread::spawn(move || {
        let mut answered = false;
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let request = read_request(&mut stream);
            if send.send(request).is_err() {
                return;
            }
            // Only the first request is answered with the rotation; a second
            // one is a second rotation, which the test counts and refuses.
            let body = if answered {
                "{}".to_owned()
            } else {
                thread::sleep(hold);
                response.clone()
            };
            answered = true;
            let _ = write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
        }
    });
    (base, requests, worker)
}

/// An `openai` subscription whose access token has expired, so the next
/// authorization renews it with `refresh-old`.
fn expired(store: &Store) {
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
}

/// Waits, within [`PATIENCE`], for the store to hold `refresh`.
fn persisted(store: &Path, refresh: &str) -> bool {
    let until = std::time::Instant::now() + PATIENCE;
    while std::time::Instant::now() < until {
        if std::fs::read_to_string(store.join("auth.json")).is_ok_and(|text| text.contains(refresh))
        {
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// The header a credential set, by name.
fn header(outgoing: &Outgoing, name: &str) -> Option<String> {
    outgoing
        .headers()
        .iter()
        .find(|(present, _)| present.as_ref() == name)
        .map(|(_, value)| value.to_string())
}

/// A turn cancelled while its credential renews stops waiting for that
/// renewal: whoever waits drops its wait, and nothing else. The rotation it
/// started is not the waiter's to end — the token server may already have
/// spent the old refresh token — so it still finishes and is written down.
///
/// The server holds its answer for [`HOLD`] seconds, and the waiter gives up
/// after [`GIVES_UP`], as a waiting crossing does once its cancel is raised.
/// A renewal done inside the waiter's own poll cannot be given up on at all:
/// the waiter comes back only once the server has answered.
#[test]
fn a_waiter_given_up_on_mid_renewal_returns_at_once_and_the_rotation_still_lands() {
    const HOLD: Duration = Duration::from_secs(3);
    const GIVES_UP: Duration = Duration::from_millis(250);
    let (base, requests, server) = holding_server(
        tokens("unused-canary", "refresh-new", "account-old", now() + 3600),
        HOLD,
    );
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("given-up-waiter");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let credential = oauth.credential(&store.read()).unwrap();

    let begun = std::time::Instant::now();
    let given_up = runtime.block_on(async {
        let mut outgoing = Outgoing::new();
        tokio::time::timeout(GIVES_UP, credential.authorize(&mut outgoing))
            .await
            .is_err()
    });
    let took = begun.elapsed();

    assert!(
        took < HOLD / 2,
        "the waiter came back {took:?} after it began, not within its bound of {GIVES_UP:?}: \
         it could not stop waiting until the token server answered"
    );
    assert!(given_up, "the waiter was answered rather than given up on");
    let sent = requests.recv_timeout(PATIENCE).unwrap();
    assert!(sent.body.contains("refresh-old"));
    assert!(
        persisted(scratch.path(), "refresh-new"),
        "the rotation the waiter started was not written down after the waiter gave up"
    );
    assert!(
        requests.try_recv().is_err(),
        "a second rotation was sent after the first"
    );
    drop(server);
}

/// Where the second process of the two-process renewal test finds its store,
/// its token server and the file it writes the header it was given into.
const SECOND_HOME: &str = "CRUCIBLE_TEST_SECOND_RENEWAL_HOME";
const SECOND_BASE: &str = "CRUCIBLE_TEST_SECOND_RENEWAL_BASE";

/// The second process of the test below, which runs this test binary again
/// with only this test selected. Run any other way it has no store to renew
/// and does nothing.
#[test]
fn a_second_process_renewing() {
    let (Some(home), Some(base)) = (std::env::var_os(SECOND_HOME), std::env::var_os(SECOND_BASE))
    else {
        return;
    };
    let home = PathBuf::from(home);
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(base.to_str().unwrap(), &renewing(&runtime)));
    let credential = oauth.credential(&Store::in_home(&home).read()).unwrap();
    let mut outgoing = Outgoing::new();
    runtime
        .block_on(credential.authorize(&mut outgoing))
        .unwrap();
    std::fs::write(
        home.join("second-authorization"),
        header(&outgoing, "authorization").unwrap(),
    )
    .unwrap();
}

/// Two crucibles renewing the same expired rotation at once present the
/// one-use refresh token once between them: the second takes `auth.lock`
/// after the first, rereads the store inside it, and uses the rotation the
/// first wrote rather than spending the one it read at startup.
#[test]
fn two_processes_renewing_at_once_rotate_once_and_persist_once() {
    // Longer than a second process takes to start and reach the lock, and
    // shorter than the 5 s it waits there before giving up.
    const HOLD: Duration = Duration::from_secs(2);
    let expires = now() + 3600;
    let fresh = jwt(&serde_json::json!({ "exp": expires }));
    let (base, requests, server) = holding_server(
        tokens("unused-canary", "refresh-new", "account-old", expires),
        HOLD,
    );
    let scratch = Scratch::new("two-processes");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let credential = oauth.credential(&store.read()).unwrap();

    let second = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "oauth::tests::a_second_process_renewing"])
        .env(SECOND_HOME, scratch.path())
        .env(SECOND_BASE, &base)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let mut outgoing = Outgoing::new();
    runtime
        .block_on(credential.authorize(&mut outgoing))
        .unwrap();
    let finished = second.wait_with_output().unwrap();

    assert!(
        finished.status.success(),
        "the second process failed to renew"
    );
    let second = std::fs::read_to_string(scratch.path().join("second-authorization")).unwrap();
    let first = header(&outgoing, "authorization").unwrap();
    assert_eq!(first, format!("Bearer {fresh}"));
    assert_eq!(
        second, first,
        "the two processes ended on different rotations"
    );
    let sent: Vec<_> = requests.try_iter().collect();
    assert_eq!(
        sent.len(),
        1,
        "the two processes sent {} renewals between them",
        sent.len()
    );
    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(text.contains("refresh-new") && !text.contains("refresh-old"));
    drop(server);
}

/// The store's lock, opened on its own as another process opens it: a lock
/// taken on this description contends with the store's exactly as another
/// crucible's does.
fn another_process_lock(home: &Path) -> std::fs::File {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(home.join("auth.lock"))
        .unwrap()
}

/// Whether another process could take the store's lock within a second.
fn lock_released(home: &Path) -> bool {
    let other = another_process_lock(home);
    let until = std::time::Instant::now() + Duration::from_secs(1);
    while std::time::Instant::now() < until {
        if other.try_lock().is_ok() {
            let _ = other.unlock();
            return true;
        }
        thread::sleep(Duration::from_millis(10));
    }
    false
}

/// The blocking adapter's bound: a renewal whose lock another process holds
/// waits for it 250 times 20 ms apart, then says another crucible is writing,
/// having sent nothing and written nothing.
#[test]
fn a_renewal_waits_five_seconds_for_a_lock_another_process_holds_then_says_so() {
    let (base, requests, server) = holding_server(
        tokens("unused-canary", "refresh-new", "account-old", now() + 3600),
        Duration::ZERO,
    );
    let runtime = runtime();
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("lock-held-elsewhere");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let before = std::fs::read(scratch.path().join("auth.json")).unwrap();
    let credential = oauth.credential(&store.read()).unwrap();
    let other = another_process_lock(scratch.path());
    other.lock().unwrap();

    let begun = std::time::Instant::now();
    let mut outgoing = Outgoing::new();
    let refused = runtime.block_on(credential.authorize(&mut outgoing));
    let took = begun.elapsed();
    other.unlock().unwrap();

    assert!(
        matches!(&refused, Err(CredentialError::NotRenewed(said)) if said.contains("another crucible is writing")),
        "a renewal behind another process's lock was not refused as busy: {refused:?}"
    );
    assert!(
        took >= Duration::from_secs(5) && took < Duration::from_secs(10),
        "the wait for another process's lock took {took:?}, not the 5 s it is bounded by"
    );
    assert!(
        requests.try_recv().is_err(),
        "a busy renewal reached the token server"
    );
    assert_eq!(
        std::fs::read(scratch.path().join("auth.json")).unwrap(),
        before
    );
    assert!(outgoing.headers().is_empty());
    drop(server);
}

/// The blocking adapter's cleanup, and the owner's bound: a rotation still
/// waiting for its answer when the owner stops waiting for it is aborted, the
/// owner says how many it abandoned, and the lock it held is released with
/// nothing written.
#[test]
fn the_owner_aborts_a_rotation_it_stopped_waiting_for_and_its_lock_is_released() {
    let (base, requests, server) = holding_server(
        tokens("unused-canary", "refresh-new", "account-old", now() + 3600),
        Duration::from_secs(5),
    );
    let runtime = runtime();
    let renewals = renewing(&runtime);
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewals));
    let scratch = Scratch::new("abandoned-rotation");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let before = std::fs::read(scratch.path().join("auth.json")).unwrap();
    let credential = oauth.credential(&store.read()).unwrap();
    let given_up = runtime.block_on(async {
        let mut outgoing = Outgoing::new();
        tokio::time::timeout(
            Duration::from_millis(50),
            credential.authorize(&mut outgoing),
        )
        .await
        .is_err()
    });
    assert!(given_up);
    // The rotation's request is in flight, holding the lock.
    requests.recv_timeout(PATIENCE).unwrap();
    assert!(
        !lock_released(scratch.path()),
        "the rotation in flight held no lock"
    );

    let begun = std::time::Instant::now();
    let joined = renewals.join_within(Duration::from_millis(100));
    let took = begun.elapsed();

    assert_eq!(
        joined.map_err(|unjoined| unjoined.to_string()),
        Err(
            "1 of the account renewals in flight had not finished 100 ms after the run ended, \
             and were abandoned; whether their new tokens were written down is unconfirmed"
                .to_owned()
        )
    );
    assert!(took < Duration::from_secs(1), "the owner waited {took:?}");
    assert!(
        lock_released(scratch.path()),
        "the aborted rotation kept the store's lock"
    );
    assert_eq!(
        std::fs::read(scratch.path().join("auth.json")).unwrap(),
        before
    );
    drop(server);
}

/// Shutdown joins the owner within its bound: a rotation whose answer arrives
/// inside the bound is written down before the join returns.
#[test]
fn the_owner_joins_a_rotation_that_ends_within_its_bound() {
    let (base, requests, server) = holding_server(
        tokens("unused-canary", "refresh-new", "account-old", now() + 3600),
        Duration::from_millis(300),
    );
    let runtime = runtime();
    let renewals = renewing(&runtime);
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewals));
    let scratch = Scratch::new("joined-rotation");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let credential = oauth.credential(&store.read()).unwrap();
    runtime.block_on(async {
        let mut outgoing = Outgoing::new();
        let _ = tokio::time::timeout(
            Duration::from_millis(50),
            credential.authorize(&mut outgoing),
        )
        .await;
    });
    requests.recv_timeout(PATIENCE).unwrap();

    let begun = std::time::Instant::now();
    let joined = renewals.join_within(PATIENCE);
    let took = begun.elapsed();

    assert_eq!(joined, Ok(()));
    assert!(took < PATIENCE / 2, "the join took {took:?}");
    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(
        text.contains("refresh-new") && !text.contains("refresh-old"),
        "the joined rotation was not written down by the time the join returned"
    );
    assert!(lock_released(scratch.path()));
    drop(server);
}

/// Each request is given its deadline from the start, so the wait for one of
/// the client's four connection slots counts against it: with four
/// connections stalled in their handshake, a fifth request gives up at its
/// own deadline rather than at the slots' 15 s.
#[test]
fn a_request_waiting_for_a_connection_slot_gives_up_at_its_own_deadline() {
    const DEADLINE: Duration = Duration::from_millis(500);
    // Accepts every connection and never answers, so a TLS handshake stalls
    // holding its slot.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let held = thread::spawn(move || {
        let mut open = Vec::new();
        for stream in listener.incoming().take(5) {
            open.push(stream);
        }
        thread::sleep(PATIENCE);
    });
    let runtime = runtime();
    let flow = Flow::testing_within(&format!("https://{address}"), &renewing(&runtime), DEADLINE);

    let begun = std::time::Instant::now();
    let answers = runtime.block_on(async {
        let requests: Vec<_> = (0..5)
            .map(|_| {
                let flow = flow.clone();
                tokio::spawn(async move {
                    let current = Tokens::new("access".into(), "refresh".into(), 1, 1);
                    flow.refresh(&current).await
                })
            })
            .collect();
        let mut answers = Vec::new();
        for request in requests {
            answers.push(request.await.unwrap());
        }
        answers
    });
    let took = begun.elapsed();

    assert!(
        answers
            .iter()
            .all(|answer| matches!(answer, Err(OAuthError::Unreachable))),
        "a stalled request did not give up as unreachable: {answers:?}"
    );
    assert!(
        took < DEADLINE * 4,
        "five requests with a {DEADLINE:?} deadline took {took:?}: one waited for a slot past it"
    );
    drop(held);
}

/// A rotation whose waiter was dropped — a crossing that polls once gave up on
/// it — still lands, and then every credential for the account answers at its
/// first poll with it: the one that started it, whose own copy was never
/// replaced by the waiter it lost, and another built from the store before the
/// rotation. Without that, each would start a rotation of its own at every
/// first poll, and a crossing that polls once would refuse it every time.
#[test]
fn a_rotation_whose_waiter_was_dropped_answers_every_credential_at_its_first_poll() {
    let expires = now() + 3600;
    let fresh = jwt(&serde_json::json!({ "exp": expires }));
    let (base, requests, server) = holding_server(
        tokens("unused-canary", "refresh-new", "account-old", expires),
        Duration::from_millis(200),
    );
    let runtime = runtime();
    let renewals = renewing(&runtime);
    let oauth = OpenAiOAuth::testing(Flow::testing(&base, &renewals));
    let scratch = Scratch::new("dropped-waiter");
    let store = Store::in_home(scratch.path());
    expired(&store);
    let started = oauth.credential(&store.read()).unwrap();
    let other = oauth.credential(&store.read()).unwrap();

    let mut dropped = Outgoing::new();
    assert!(
        matches!(
            started
                .authorize(&mut dropped)
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ),
        "a renewal answered at its first poll"
    );
    requests.recv_timeout(PATIENCE).unwrap();
    assert_eq!(renewals.join_within(PATIENCE), Ok(()));

    for credential in [&started, &other] {
        let mut outgoing = Outgoing::new();
        authorized(credential.authorize(&mut outgoing)).unwrap();
        assert_eq!(
            header(&outgoing, "authorization"),
            Some(format!("Bearer {fresh}"))
        );
    }
    assert!(requests.try_recv().is_err(), "a second rotation was sent");
    drop(server);
}
