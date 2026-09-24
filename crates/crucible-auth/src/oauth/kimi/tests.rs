use super::*;
use crate::oauth::PATIENCE;

use std::collections::BTreeMap;
use std::io::Write as _;
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::task::{Context, Poll, Waker};
use std::thread;

use crucible_core::{Authorization, Outgoing};

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
    let oauth = KimiOAuth::testing(Flow::testing(&base));

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
    let flow = Flow::at(&base, VERIFY, PATIENCE, PATIENCE, Duration::from_millis(1));
    let identity = Identity::new("01234567-89ab-4cde-8fab-0123456789ab".to_owned()).unwrap();

    let device = flow.request_device(&identity).unwrap();

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
    let oauth = KimiOAuth::testing(Flow::testing(&base));
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
    authorized(credential.authorize(&mut outgoing)).unwrap();
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
    // false and the flow's address is never read.
    let oauth = KimiOAuth::testing(Flow::testing("http://127.0.0.1:1"));
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
    const STABLE: &str = "01234567-89ab-4cde-8fab-0123456789ab";
    // A server that would actually renew the credential if it were ever
    // reached: a missing or misplaced guard then rewrites the store with
    // these fresh tokens, which the byte comparison below would catch. An
    // address nothing answers cannot prove this — the store would stay
    // unchanged whether or not the guard ran, because the renewal itself
    // would fail before writing anything.
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
    let oauth = KimiOAuth::testing(Flow::testing(&base));
    let scratch = Scratch::new("worker-refusal");
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
    let oauth = KimiOAuth::new();
    let scratch = Scratch::new("method");
    let problem = oauth
        .start(LoginMethod::new("browser"), Store::in_home(scratch.path()))
        .unwrap_err();
    assert!(matches!(problem, OAuthError::Method));
}
