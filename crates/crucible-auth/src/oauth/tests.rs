//! Protocol, cancellation and secrecy tests for subscription login.

use super::openai::{CLIENT_ID, Flow, PORTS, VERIFY, now};
use super::*;

use std::io::{Read as _, Write as _};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::task::{Context, Poll, Waker};

use base64::Engine as _;
use crucible_core::{Authorization, CredentialError, Outgoing};

/// Polls `authorizing` once and panics if it was not ready: every credential
/// this crate ships answers at its first poll.
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
    let oauth = OpenAiOAuth::testing(Flow::testing("http://127.0.0.1:9"));

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
    let oauth = OpenAiOAuth::testing(Flow::testing(&base));
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
    assert_eq!(Flow::production().callback_ports(), &PORTS);
    assert_eq!(Flow::testing("http://127.0.0.1:1").callback_ports(), &[0]);
}

#[test]
fn authorize_answers_at_its_first_poll_when_nothing_needs_renewing() {
    // A tool run's crossing polls a future once; a fresh token that pended
    // there would be refused on every request, exactly like a pending
    // renewal, even though nothing here has anything to wait for.
    let oauth = OpenAiOAuth::testing(Flow::testing("http://127.0.0.1:1"));
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
    let scope = credential.scope();
    let mut outgoing = Outgoing::new();
    authorized(credential.authorize(&mut outgoing)).unwrap();
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
    let oauth = OpenAiOAuth::testing(Flow::testing(&base));
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
    let problem = authorized(credential.authorize(&mut outgoing)).unwrap_err();

    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert!(sent.body.contains("refresh-original"));
    assert!(problem.to_string().contains("refreshed account identity"));
    assert!(outgoing.headers().is_empty());

    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(text.contains("refresh-original"));
    assert!(!text.contains("refresh-other"));
}

/// The renewal is not owned work yet, so it must never run where a runtime
/// could offer it a worker task: everything it does — the cross-process lock,
/// the network call — runs inside this future's one poll, and a worker task
/// blocked there is exactly what making it owned work exists to fix. Until
/// then, polling from a worker refuses instead of running, proven here by
/// actually polling from one rather than trusting the call site, and by
/// checking what the refusal actually left behind rather than a timing
/// window or a channel that would report nothing either way.
#[test]
fn renewal_refuses_when_polled_as_a_runtime_task_and_touches_neither_lock_nor_store() {
    // A server that would actually renew the credential if it were ever
    // reached: a missing or misplaced guard then rewrites the store with
    // these fresh tokens, which the byte comparison below would catch. An
    // address nothing answers cannot prove this — the store would stay
    // unchanged whether or not the guard ran, because the renewal itself
    // would fail before writing anything.
    // The account must match the stored one: the refresh closure checks the
    // identity-bound scope before anything is written, and a mismatch would
    // be caught there instead of by the assertions this test is about.
    let (base, requests, server) = server(vec![tokens(
        "unused-canary",
        "refresh-fresh",
        "account-old",
        now() + 3600,
    )]);
    let oauth = OpenAiOAuth::testing(Flow::testing(&base));
    let scratch = Scratch::new("worker-refusal");
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
    let store_path = scratch.path().join("auth.json");
    let lock_path = scratch.path().join("auth.lock");
    let store_before = std::fs::read(&store_path).unwrap();
    // `keep_subscription` already took and released this lock file, which
    // leaves it present but empty either way — its bytes cannot say whether
    // the poll below touched it. Removed here so its *presence* after the
    // poll is the signal: `Lock::take` recreates it with `O_CREAT` the
    // moment anything takes it again.
    std::fs::remove_file(&lock_path).unwrap();

    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let refused = runtime.block_on(async move {
        tokio::spawn(async move {
            let mut outgoing = Outgoing::new();
            authorized(credential.authorize(&mut outgoing))
        })
        .await
        .unwrap()
    });

    assert!(
        matches!(refused, Err(CredentialError::RenewalOnWorker)),
        "polling the renewal from a worker task did not refuse with the typed error: {refused:?}"
    );
    assert_eq!(
        std::fs::read(&store_path).unwrap(),
        store_before,
        "a refused renewal rewrote the store with the server's fresh tokens"
    );
    assert!(
        !lock_path.exists(),
        "a refused renewal recreated the lock file"
    );
    assert!(
        requests.try_recv().is_err(),
        "a refused renewal reached the token server"
    );
    // The server thread is blocked in `accept` forever when the guard holds
    // (as it should here): nothing to join, since nothing was ever sent.
    drop(server);
}

/// The uncontended case (no renewal in flight) never asks `not_worker`
/// anything: `try_lock` answers at once regardless of which thread asks. The
/// only thread that can hold `tokens` for any length of time is one already
/// inside `refresh_subscription`'s network call, so that is the one case
/// checked here — with the mutex held by this test's own thread standing in
/// for that renewal, and the poll coming from an actual spawned task.
#[test]
fn lock_tokens_refuses_a_worker_task_instead_of_waiting_on_another_threads_renewal() {
    let tokens = std::sync::Arc::new(Mutex::new(Tokens::new(
        "access".into(),
        "refresh".into(),
        u64::MAX,
        0,
    )));
    let held = tokens
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    // The poll runs on a thread of its own, so a regressed guard blocks that
    // thread rather than this one: `held` is on this thread, and a worker
    // that called the blocking `lock()` would deadlock against it if the two
    // shared a thread, turning a real failure into a hang instead of the
    // bounded, named one below.
    let (send, recv) = mpsc::channel();
    let for_worker = std::sync::Arc::clone(&tokens);
    let probe = thread::Builder::new()
        .name("lock-tokens-probe".into())
        .spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .unwrap();
            let refused = runtime.block_on(async move {
                tokio::spawn(async move {
                    lock_tokens(&for_worker).map(|guard| guard.access().to_owned())
                })
                .await
                .unwrap()
            });
            // A closed channel means the `recv_timeout` below already gave
            // up; nothing left to report to.
            let _ = send.send(refused);
        })
        .unwrap();

    let refused = recv.recv_timeout(Duration::from_secs(5)).expect(
        "a regressed guard blocks a worker instead of refusing it; this bound turns that into \
         a fast, named failure rather than a hang",
    );

    drop(held);
    let _ = probe.join();

    assert!(
        matches!(refused, Err(CredentialError::RenewalOnWorker)),
        "a worker task waited on the mutex instead of refusing: {refused:?}"
    );
}
