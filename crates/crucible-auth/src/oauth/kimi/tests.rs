use super::*;
use crate::oauth::PATIENCE;

use std::collections::BTreeMap;
use std::io::{Read as _, Write as _};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::task::{Context, Poll, Waker};
use std::thread;

use crucible_core::{Authorization, Outgoing};

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
            std::env::temp_dir().join(format!("crucible-kimi-oauth-{name}-{}", std::process::id()));
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
    headers: BTreeMap<String, String>,
    body: String,
}

fn server(
    responses: impl FnOnce(&str) -> Vec<(u16, String)>,
) -> (String, mpsc::Receiver<Request>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let responses = responses(&base);
    let (send, requests) = mpsc::channel();
    let worker = thread::spawn(move || {
        for (status, response) in responses {
            let (mut stream, _) = listener.accept().unwrap();
            send.send(read_request(&mut stream)).unwrap();
            write!(
                stream,
                "HTTP/1.1 {status} status\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{response}",
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
        assert_ne!(read, 0);
        bytes.extend_from_slice(buffer.get(..read).unwrap());
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8(bytes.get(..header_end).unwrap().to_vec()).unwrap();
    let mut lines = head.lines();
    let target = lines
        .next()
        .and_then(|line| line.split_ascii_whitespace().nth(1))
        .unwrap()
        .to_owned();
    let headers: BTreeMap<_, _> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or_default();
    while bytes.len() < header_end + length {
        let read = stream.read(&mut buffer).unwrap();
        assert_ne!(read, 0);
        bytes.extend_from_slice(buffer.get(..read).unwrap());
    }
    let body_end = header_end.checked_add(length).unwrap();
    Request {
        target,
        headers,
        body: String::from_utf8(bytes.get(header_end..body_end).unwrap().to_vec()).unwrap(),
    }
}

#[test]
fn device_login_uses_crucibles_identity_and_persists_before_completion() {
    let (base, requests, server) = server(|base| {
        vec![
            (
                200,
                serde_json::json!({
                    "device_code": "device-code",
                    "user_code": "ABCD-EFGH",
                    "verification_uri": format!("{base}/device"),
                    "verification_uri_complete": format!("{base}/device?code=ABCD-EFGH"),
                    "expires_in": 600,
                    "interval": 1,
                })
                .to_string(),
            ),
            (
                200,
                serde_json::json!({
                    "access_token": "access-canary",
                    "refresh_token": "refresh-canary",
                    "expires_in": 3600,
                })
                .to_string(),
            ),
        ]
    });
    let scratch = Scratch::new("device");
    let store = Store::in_home(scratch.path());
    let runtime = runtime();
    let oauth = KimiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));

    let attempt = oauth.start(KimiOAuth::DEVICE, store.clone()).unwrap();
    let authorize = attempt.wait(PATIENCE).unwrap().unwrap();
    assert_eq!(
        authorize,
        LoginUpdate::Authorize {
            browser_uri: format!("{base}/device?code=ABCD-EFGH").into(),
            shown_uri: format!("{base}/device").into(),
            user_code: Some("ABCD-EFGH".into()),
            manual: false,
        }
    );
    assert_eq!(attempt.wait(PATIENCE).unwrap(), Some(LoginUpdate::Complete));

    let sent: Vec<_> = (0..2)
        .map(|_| requests.recv_timeout(PATIENCE).unwrap())
        .collect();
    server.join().unwrap();
    assert_eq!(
        sent.iter()
            .map(|request| request.target.as_str())
            .collect::<Vec<_>>(),
        ["/api/oauth/device_authorization", "/api/oauth/token"]
    );
    let [authorize, token] = sent.as_slice() else {
        panic!("two requests were expected");
    };
    assert_eq!(
        authorize.headers.get("x-msh-platform").unwrap(),
        "crucible-code"
    );
    assert!(
        authorize
            .headers
            .get("user-agent")
            .unwrap()
            .starts_with("crucible-code/")
    );
    let device_id = authorize.headers.get("x-msh-device-id").unwrap().clone();
    assert_eq!(token.headers.get("x-msh-device-id"), Some(&device_id));
    assert!(authorize.body.contains(CLIENT_ID));
    assert!(token.body.contains("device-code"));

    let keys = store.read();
    assert!(keys.has("moonshot"));
    let credential = oauth.credential(&keys).unwrap();
    let mut outgoing = Outgoing::new();
    authorized(credential.authorize(&mut outgoing)).unwrap();
    let headers: BTreeMap<_, _> = outgoing
        .headers()
        .iter()
        .map(|(name, value)| (name.as_ref(), value.as_ref()))
        .collect();
    assert_eq!(headers.get("x-msh-device-id"), Some(&device_id.as_str()));
    assert_eq!(headers.get("x-msh-platform"), Some(&"crucible-code"));
    assert_eq!(headers.get("authorization"), Some(&"Bearer access-canary"));
}

#[test]
fn production_browser_addresses_are_separate_from_the_token_service() {
    let (base, requests, server) = server(|_| {
        vec![(
            200,
            serde_json::json!({
                "device_code": "device-code",
                "user_code": "ABCD-EFGH",
                "verification_uri_complete": "https://www.kimi.com/code?user_code=ABCD-EFGH",
                "expires_in": 600,
                "interval": 5,
            })
            .to_string(),
        )]
    });
    let runtime = runtime();
    let flow = Flow::at(
        renewing(&runtime),
        &base,
        VERIFY,
        PATIENCE,
        PATIENCE,
        Duration::from_millis(1),
    );
    let identity = Identity::new("01234567-89ab-4cde-8fab-0123456789ab".to_owned()).unwrap();

    let device = runtime.block_on(flow.request_device(&identity)).unwrap();

    let request = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert_eq!(request.target, "/api/oauth/device_authorization");
    assert_eq!(
        device.verification.as_ref(),
        "https://www.kimi.com/code?user_code=ABCD-EFGH"
    );
    assert_eq!(
        device.complete.as_ref(),
        "https://www.kimi.com/code?user_code=ABCD-EFGH"
    );
}

#[test]
fn a_device_response_cannot_send_the_browser_to_an_untrusted_origin() {
    for address in [
        "https://www.kimi.com.evil.example/code",
        "https://www.kimi.com@evil.example/code",
        "http://www.kimi.com/code",
    ] {
        assert!(!within(VERIFY, address), "accepted {address}");
    }
}

#[test]
fn renewal_keeps_the_installation_identity() {
    const STABLE: &str = "01234567-89ab-4cde-8fab-0123456789ab";
    let (base, requests, server) = server(|_| {
        vec![(
            200,
            serde_json::json!({
                "access_token": "access-new",
                "refresh_token": "refresh-new",
                "expires_in": 3600,
            })
            .to_string(),
        )]
    });
    let runtime = runtime();
    let oauth = KimiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("refresh");
    let store = Store::in_home(scratch.path());
    store
        .keep_subscription(
            "moonshot",
            Tokens::new("access-old".into(), "refresh-old".into(), 1, 1)
                .with_detail(DEVICE_ID, STABLE)
                .with_detail(EXPIRES_IN, "3600"),
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
    assert!(sent.body.contains("refresh-old"));
    assert_eq!(sent.headers.get("x-msh-device-id").unwrap(), STABLE);
    let headers: BTreeMap<_, _> = outgoing
        .headers()
        .iter()
        .map(|(name, value)| (name.as_ref(), value.as_ref()))
        .collect();
    assert_eq!(headers.get("authorization"), Some(&"Bearer access-new"));
    assert_eq!(headers.get("x-msh-device-id"), Some(&STABLE));

    let reconstructed = oauth.credential(&store.read()).unwrap();
    assert_eq!(reconstructed.scope(), scope);
}

#[test]
fn authorize_answers_at_its_first_poll_when_nothing_needs_renewing() {
    // A tool run's crossing polls a future once; a fresh token that pended
    // there would be refused on every request, exactly like a pending
    // renewal, even though nothing here has anything to wait for.
    const STABLE: &str = "01234567-89ab-4cde-8fab-0123456789ab";
    // Nothing here is ever dialed: the token is fresh, so `needs_refresh` is
    // false and the flow's address is never read. The owner has no runtime,
    // so a renewal started here would be refused rather than pend.
    let oauth = KimiOAuth::testing(Flow::testing("http://127.0.0.1:1", &Renewals::new()));
    let scratch = Scratch::new("fresh-token");
    let store = Store::in_home(scratch.path());
    store
        .keep_subscription(
            "moonshot",
            Tokens::new("access-fresh".into(), "refresh-fresh".into(), u64::MAX, 0)
                .with_detail(DEVICE_ID, STABLE)
                .with_detail(EXPIRES_IN, "3600"),
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

/// A renewal is a rotation its owner runs as a task of its own, and an
/// authorization only waits for it, so one polled as a runtime worker task
/// waits the way any task does — without holding the worker — and completes,
/// keeping the installation's identity. It used to refuse there with a typed
/// error, because the renewal then ran inside the poll: this is that test,
/// inverted. The server really renews, so the store's bytes say whether the
/// rotation was written.
#[test]
fn a_renewal_awaited_from_a_runtime_task_completes_and_writes_its_rotation() {
    const STABLE: &str = "01234567-89ab-4cde-8fab-0123456789ab";
    let (base, requests, server) = server(|_| {
        vec![(
            200,
            serde_json::json!({
                "access_token": "access-fresh",
                "refresh_token": "refresh-fresh",
                "expires_in": 3600,
            })
            .to_string(),
        )]
    });
    let runtime = runtime();
    let oauth = KimiOAuth::testing(Flow::testing(&base, &renewing(&runtime)));
    let scratch = Scratch::new("worker-renewal");
    let store = Store::in_home(scratch.path());
    store
        .keep_subscription(
            "moonshot",
            Tokens::new("access-old".into(), "refresh-old".into(), 1, 1)
                .with_detail(DEVICE_ID, STABLE)
                .with_detail(EXPIRES_IN, "3600"),
        )
        .unwrap();
    let credential = oauth.credential(&store.read()).unwrap();

    let answered = runtime.block_on(async move {
        tokio::spawn(async move {
            let mut outgoing = Outgoing::new();
            credential.authorize(&mut outgoing).await.map(|()| {
                outgoing
                    .headers()
                    .iter()
                    .map(|(name, value)| (name.to_string(), value.to_string()))
                    .collect::<BTreeMap<_, _>>()
            })
        })
        .await
        .unwrap()
    });

    let headers = answered.unwrap_or_else(|problem| {
        panic!("an authorization polled as a runtime worker task did not complete: {problem}")
    });
    assert_eq!(
        headers.get("authorization").map(String::as_str),
        Some("Bearer access-fresh")
    );
    assert_eq!(
        headers.get("x-msh-device-id").map(String::as_str),
        Some(STABLE)
    );
    let sent = requests.recv_timeout(PATIENCE).unwrap();
    server.join().unwrap();
    assert!(sent.body.contains("refresh-old"));
    assert_eq!(sent.headers.get("x-msh-device-id").unwrap(), STABLE);
    let text = std::fs::read_to_string(scratch.path().join("auth.json")).unwrap();
    assert!(text.contains("refresh-fresh") && !text.contains("refresh-old"));
}

#[test]
fn identity_and_tokens_are_redacted_from_debug() {
    let identity = Identity::new("deadbeef-dead-4eef-8ead-deadbeefcafe".to_owned()).unwrap();
    let tokens = Tokens::new("access-canary".into(), "refresh-canary".into(), 1, 1)
        .with_detail(DEVICE_ID, "device-canary");
    let shown = format!("{identity:?} {tokens:?}");
    for canary in [
        "deadbeef-dead-4eef-8ead-deadbeefcafe",
        "access-canary",
        "refresh-canary",
    ] {
        assert!(
            !shown.contains(canary),
            "a private value reached Debug: {shown}"
        );
    }
}

#[test]
fn an_unknown_method_is_rejected_before_a_worker_starts() {
    let oauth = KimiOAuth::new(Renewals::new());
    let scratch = Scratch::new("method");
    let problem = oauth
        .start(LoginMethod::new("browser"), Store::in_home(scratch.path()))
        .unwrap_err();
    assert!(matches!(problem, OAuthError::Method));
}
