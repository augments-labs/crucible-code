//! The catalog is fetched from the one address it is published at, within its
//! bounds, and handed to the trust state; whatever fails to be believed leaves
//! what was believed before it as it was.

use std::collections::HashMap;
use std::future::Future;
use std::io::{Read as _, Write as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use crucible_external_harnesses::catalog::fixture::{Signed, signed};
use crucible_external_harnesses::catalog::{
    CatalogError, INDEX_BYTES, SNAPSHOT, Trust, TrustError,
};
use crucible_http::{BodyError, Origin};
use crucible_types::AgentKey;
use serde_json::{Value, json};

use super::*;
use crate::sample::Sample;
use crate::services::serving;

/// How long a test waits for something another thread does before it says
/// what it was waiting for.
const PATIENCE: Duration = Duration::from_secs(20);

/// What a fake host answers a request for a path with.
#[derive(Clone)]
enum Reply {
    /// A 200 with these bytes.
    Body(Vec<u8>),
    /// A 302 to this address.
    Redirect(String),
    /// A 200 that promises more bytes than it sends, and closes.
    Short(Vec<u8>),
    /// The connection is held open and nothing is said.
    Stall,
}

/// A host on this machine standing where the catalog is published, answering
/// by path and remembering which paths it was asked for.
struct Server {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Reply>>>,
    asked: Arc<Mutex<Vec<String>>>,
    done: Arc<AtomicBool>,
}

impl Server {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a free port");
        let port = listener.local_addr().expect("an address").port();
        let routes: Arc<Mutex<HashMap<String, Reply>>> = Arc::default();
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        let (routing, remembering, finishing) =
            (Arc::clone(&routes), Arc::clone(&asked), Arc::clone(&done));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if finishing.load(Ordering::Acquire) {
                    break;
                }
                let (routes, asked, done) = (
                    Arc::clone(&routing),
                    Arc::clone(&remembering),
                    Arc::clone(&finishing),
                );
                std::thread::spawn(move || answer(stream, &routes, &asked, &done));
            }
        });
        Self {
            port,
            routes,
            asked,
            done,
        }
    }

    /// Answers a request for `path` with `reply`.
    fn serve(&self, path: &str, reply: Reply) {
        self.routes.lock().unwrap().insert(path.to_owned(), reply);
    }

    /// Where the catalog is published on this host.
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}/catalog/", self.port)
    }

    /// The address of `path` on this host.
    fn at(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }

    /// The paths asked for so far, in order.
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }

    fn forget(&self) {
        self.asked.lock().unwrap().clear();
    }

    /// Serves `signed` as the catalog.
    fn publish(&self, signed: &Signed) {
        self.serve("/catalog/index.json", Reply::Body(signed.index.clone()));
        self.serve(
            "/catalog/index.json.minisig",
            Reply::Body(signed.signature.clone()),
        );
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        // Wakes the accepting thread so it sees it is done.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// One connection: the path asked for, and the reply that path has.
fn answer(
    mut stream: TcpStream,
    routes: &Mutex<HashMap<String, Reply>>,
    asked: &Mutex<Vec<String>>,
    done: &AtomicBool,
) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut head = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !head.windows(4).any(|end| end == b"\r\n\r\n") {
        match stream.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(read) => head.extend_from_slice(buffer.get(..read).unwrap_or_default()),
        }
    }
    let text = String::from_utf8_lossy(&head).into_owned();
    let path = text
        .lines()
        .next()
        .and_then(|line| line.split(' ').nth(1))
        .unwrap_or_default()
        .split('?')
        .next()
        .unwrap_or_default()
        .to_owned();
    if path.is_empty() {
        return;
    }
    asked.lock().unwrap().push(path.clone());
    let reply = routes.lock().unwrap().get(&path).cloned();
    let (status, extra, body, promised) = match reply {
        None => ("404 Not Found", String::new(), Vec::new(), 0),
        Some(Reply::Body(body)) => {
            let promised = body.len();
            ("200 OK", String::new(), body, promised)
        }
        Some(Reply::Redirect(to)) => ("302 Found", format!("location: {to}\r\n"), Vec::new(), 0),
        Some(Reply::Short(body)) => {
            let promised = body.len() + 10;
            ("200 OK", String::new(), body, promised)
        }
        Some(Reply::Stall) => {
            while !done.load(Ordering::Acquire) {
                std::thread::sleep(Duration::from_millis(20));
            }
            return;
        }
    };
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nconnection: close\r\n{extra}content-length: {promised}\r\n\r\n"
    );
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

/// How this machine's platform is spelled in a catalog.
fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// An agent called `goose` at `version`, published for this machine.
fn goose(version: &str) -> Value {
    json!({
        "key": "goose",
        "registry_id": "goose-acp",
        "version": version,
        "kind": "binary",
        "license": "Apache-2.0",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/12",
        "admits": {"name": "goose", "version": version},
        "targets": [{
            "platform": platform(),
            "cmd": "goose",
            "args": ["acp"],
            "hosts": ["github.com"],
            "artifacts": [{
                "url": "https://github.com/example/goose/releases/download/v1/goose.zip",
                "sha256": "a".repeat(64),
                "size": 1000,
                "format": "zip",
                "unpacked": 3000,
                "decompressed": 4000,
                "files": [{"path": "goose", "size": 2900, "sha256": "b".repeat(64), "exec": true}],
                "kept": ["goose"]
            }]
        }]
    })
}

/// A catalog of `goose` at `version`, with no dates of its own.
fn catalog(serial: u64, version: &str) -> Value {
    json!({"schema": 1, "serial": serial, "entries": [goose(version)], "revoked": []})
}

fn sign(catalog: &Value) -> Signed {
    signed(&serde_json::to_vec(catalog).expect("a catalog is JSON"))
}

/// `catalog` dated as signed `signed_at` seconds and expiring `expires`
/// seconds from the real clock now, either of them negative for a time past.
fn dated(mut catalog: Value, signed_at: i64, expires: i64) -> Value {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    let at = |offset: i64| now.checked_add_signed(offset).expect("a time");
    let object = catalog.as_object_mut().expect("a catalog is an object");
    object.insert("signed_at".to_owned(), json!(at(signed_at)));
    object.insert("expires".to_owned(), json!(at(expires)));
    catalog
}

/// The days, in seconds, the dates of a test catalog are counted in.
const DAY: i64 = 24 * 60 * 60;

/// The address of the catalog on `server`, believed under the keys the test
/// catalogs are signed with, and following a redirect to this machine alone.
fn source(server: &Server) -> Source {
    Source {
        base: server.base(),
        keys: signed(b"{}").keys,
        hosts: &["127.0.0.1"],
    }
}

/// Runs `work` to its end on a runtime of its own.
fn run<F: Future>(work: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("a runtime")
        .block_on(work)
}

/// A state directory of its own, with the tree it is in.
fn state(sample: &Sample) -> PathBuf {
    state_in(&sample.home())
}

/// The versions of `goose` the trust state kept under `state` offers.
fn offered(state: &Path) -> Vec<String> {
    let key = AgentKey::new("goose").expect("a key");
    Trust::load(state, SystemTime::now())
        .offered(&key)
        .into_iter()
        .map(|pin| pin.version.to_string())
        .collect()
}

/// Waits, within [`PATIENCE`], for `ready`.
fn until(what: &str, ready: impl Fn() -> bool) {
    let started = Instant::now();
    while !ready() {
        assert!(started.elapsed() < PATIENCE, "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A server publishing serial 5 of `goose` at 1.0.0, the state it was kept in
/// and the result of refreshing once more after `then` has changed what is
/// published.
fn refreshed_again(
    then: impl FnOnce(&Server),
) -> (Result<(), FetchError>, Vec<String>, Vec<u8>, Vec<u8>) {
    let sample = Sample::new("catalog-fetch-again");
    let state = state(&sample);
    let server = Server::start();
    let source = source(&server);
    server.publish(&sign(&catalog(5, "1.0.0")));
    run(refresh(&state, &source, Trigger::Opening)).expect("the first catalog is kept");
    assert_eq!(offered(&state), ["1.0.0"]);
    let before = std::fs::read(state.join(TRUST_FILE)).expect("the trust state");

    then(&server);
    let result = run(refresh(&state, &source, Trigger::Opening));
    let after = std::fs::read(state.join(TRUST_FILE)).expect("the trust state");
    (result, offered(&state), before, after)
}

#[test]
fn a_catalog_is_fetched_from_its_address_and_believed_under_its_keys() {
    let server = Server::start();
    server.publish(&sign(&catalog(5, "1.0.0")));

    let believed = run(fetch(&source(&server), Trigger::Opening)).expect("a believed catalog");

    assert_eq!(believed.serial(), 5);
    assert_eq!(
        server.asked(),
        ["/catalog/index.json", "/catalog/index.json.minisig"]
    );
}

#[test]
fn a_fetch_refuses_a_serial_below_the_snapshots() {
    let server = Server::start();
    server.publish(&sign(&catalog(SNAPSHOT.serial() - 1, "1.0.0")));

    let refused = run(fetch(&source(&server), Trigger::Opening));

    assert!(
        matches!(
            refused,
            Err(FetchError::Catalog(CatalogError::Serial { held, .. })) if held == SNAPSHOT.serial()
        ),
        "{refused:?}"
    );
}

#[test]
fn a_refresh_hands_the_catalog_to_the_trust_state() {
    let sample = Sample::new("catalog-fetch-keeps");
    let state = state(&sample);
    let server = Server::start();
    server.publish(&sign(&catalog(5, "1.0.0")));

    run(refresh(&state, &source(&server), Trigger::Opening)).expect("a kept catalog");

    assert_eq!(offered(&state), ["1.0.0"]);
}

#[test]
fn the_cadence_reads_the_file_the_trust_state_writes() {
    // One name is spelled in two places, the trust state's and the cadence's;
    // this is what holds them to each other.
    let sample = Sample::new("catalog-fetch-file");
    let state = state(&sample);
    let server = Server::start();
    server.publish(&sign(&catalog(5, "1.0.0")));

    run(refresh(&state, &source(&server), Trigger::Opening)).expect("a kept catalog");

    assert!(state.join(TRUST_FILE).is_file());
}

#[test]
fn a_lower_serial_keeps_the_held_catalog() {
    let (refused, offered, _, _) =
        refreshed_again(|server| server.publish(&sign(&catalog(4, "2.0.0"))));

    assert!(
        matches!(
            refused,
            Err(FetchError::Trust(TrustError::Catalog(
                CatalogError::Serial {
                    offered: 4,
                    held: 5
                }
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(offered, ["1.0.0"]);
}

#[test]
fn a_bad_signature_keeps_the_held_catalog() {
    let (refused, offered, _, _) = refreshed_again(|server| {
        // Signed as one catalog and served as another.
        let genuine = sign(&catalog(6, "2.0.0"));
        let other = sign(&catalog(7, "3.0.0"));
        server.serve("/catalog/index.json", Reply::Body(other.index));
        server.serve(
            "/catalog/index.json.minisig",
            Reply::Body(genuine.signature),
        );
    });

    assert!(
        matches!(
            refused,
            Err(FetchError::Trust(TrustError::Catalog(
                CatalogError::Signature
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(offered, ["1.0.0"]);
}

#[test]
fn a_catalog_that_expired_eight_days_ago_keeps_the_held_catalog() {
    let (refused, offered, _, _) = refreshed_again(|server| {
        let expired = dated(catalog(6, "2.0.0"), -38 * DAY, -8 * DAY);
        server.publish(&sign(&expired));
    });

    assert!(
        matches!(
            refused,
            Err(FetchError::Trust(TrustError::Catalog(
                CatalogError::Expired
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(offered, ["1.0.0"]);
}

#[test]
fn an_unknown_schema_keeps_the_held_catalog() {
    let (refused, offered, _, _) = refreshed_again(|server| {
        let mut later = catalog(6, "2.0.0");
        later
            .as_object_mut()
            .expect("a catalog is an object")
            .insert("schema".to_owned(), json!(2));
        server.publish(&sign(&later));
    });

    assert!(
        matches!(
            refused,
            Err(FetchError::Trust(TrustError::Catalog(
                CatalogError::Schema(_)
            )))
        ),
        "{refused:?}"
    );
    assert_eq!(offered, ["1.0.0"]);
}

#[test]
fn a_serial_already_held_is_no_error_and_changes_nothing() {
    let (again, offered, before, after) = refreshed_again(|_| {});

    assert!(again.is_ok(), "{again:?}");
    assert_eq!(offered, ["1.0.0"]);
    assert_eq!(before, after);
}

#[test]
fn an_index_past_its_ceiling_is_refused_and_nothing_is_kept() {
    let sample = Sample::new("catalog-fetch-ceiling");
    let state = state(&sample);
    let server = Server::start();
    let big = sign(&catalog(5, "1.0.0"));
    server.serve(
        "/catalog/index.json",
        Reply::Body(vec![b' '; INDEX_BYTES + 1]),
    );
    server.serve("/catalog/index.json.minisig", Reply::Body(big.signature));

    let refused = run(refresh(&state, &source(&server), Trigger::Opening));

    assert!(
        matches!(refused, Err(FetchError::Body(BodyError::TooLarge))),
        "{refused:?}"
    );
    assert!(!state.exists(), "the state was written for a refused index");
}

#[test]
fn an_index_cut_short_is_a_failure_and_never_a_partial_catalog() {
    let sample = Sample::new("catalog-fetch-short");
    let state = state(&sample);
    let server = Server::start();
    let genuine = sign(&catalog(5, "1.0.0"));
    let mut cut = genuine.index.clone();
    cut.truncate(cut.len() / 2);
    server.serve("/catalog/index.json", Reply::Short(cut));
    server.serve(
        "/catalog/index.json.minisig",
        Reply::Body(genuine.signature),
    );

    let refused = run(refresh(&state, &source(&server), Trigger::Opening));

    assert!(matches!(refused, Err(FetchError::Body(_))), "{refused:?}");
    assert!(!state.exists());
}

#[test]
fn a_host_that_goes_quiet_is_given_up_on_at_the_bound() {
    let server = Server::start();
    server.serve("/catalog/index.json", Reply::Stall);

    let started = Instant::now();
    let given_up = run(download(&source(&server), Duration::from_millis(300)));

    assert!(
        matches!(given_up, Err(FetchError::Late)),
        "{:?}",
        given_up.map(|_| ())
    );
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_bounds_the_documentation_names_are_the_ones_applied() {
    // The module documentation and `docs/` say thirty seconds and four.
    assert_eq!(LIFETIME, Duration::from_secs(30));
    assert_eq!(HOPS, 4);
}

#[test]
fn an_answer_that_is_not_a_catalog_is_a_failure_with_its_status() {
    let server = Server::start();

    let refused = run(fetch(&source(&server), Trigger::Opening));

    assert!(
        matches!(refused, Err(FetchError::Status(404))),
        "{refused:?}"
    );
}

#[test]
fn a_redirect_to_a_listed_host_is_followed() {
    let server = Server::start();
    let genuine = sign(&catalog(5, "1.0.0"));
    server.serve(
        "/catalog/index.json",
        Reply::Redirect(server.at("/storage/index.json")),
    );
    server.serve("/storage/index.json", Reply::Body(genuine.index));
    server.serve(
        "/catalog/index.json.minisig",
        Reply::Body(genuine.signature),
    );

    let believed = run(fetch(&source(&server), Trigger::Opening)).expect("a catalog");

    assert_eq!(believed.serial(), 5);
    assert_eq!(
        server.asked(),
        [
            "/catalog/index.json",
            "/storage/index.json",
            "/catalog/index.json.minisig"
        ]
    );
}

#[test]
fn a_redirect_to_a_host_that_is_not_listed_is_refused_before_anything_is_sent_there() {
    let server = Server::start();
    // The same machine under another name, which is not a listed host.
    server.serve(
        "/catalog/index.json",
        Reply::Redirect(format!(
            "http://localhost:{}/storage/index.json",
            server.port
        )),
    );

    let refused = run(fetch(&source(&server), Trigger::Opening));

    assert!(matches!(refused, Err(FetchError::Redirect)), "{refused:?}");
    assert_eq!(server.asked(), ["/catalog/index.json"]);
}

#[test]
fn a_redirect_naming_a_listed_host_with_user_information_is_refused() {
    let server = Server::start();
    server.serve(
        "/catalog/index.json",
        Reply::Redirect(format!(
            "http://someone@127.0.0.1:{}/storage/index.json",
            server.port
        )),
    );

    let refused = run(fetch(&source(&server), Trigger::Opening));

    assert!(matches!(refused, Err(FetchError::Redirect)), "{refused:?}");
    assert_eq!(server.asked(), ["/catalog/index.json"]);
}

#[test]
fn a_redirect_that_is_not_an_address_is_refused() {
    let server = Server::start();
    server.serve(
        "/catalog/index.json",
        Reply::Redirect("/storage/index.json".to_owned()),
    );

    let refused = run(fetch(&source(&server), Trigger::Opening));

    assert!(matches!(refused, Err(FetchError::Redirect)), "{refused:?}");
    assert_eq!(server.asked(), ["/catalog/index.json"]);
}

#[test]
fn a_redirect_that_leads_back_to_itself_is_followed_only_so_far() {
    let server = Server::start();
    server.serve(
        "/catalog/index.json",
        Reply::Redirect(server.at("/catalog/index.json")),
    );

    let refused = run(fetch(&source(&server), Trigger::Opening));

    assert!(matches!(refused, Err(FetchError::Redirects)), "{refused:?}");
    assert_eq!(server.asked().len(), HOPS + 1);
}

#[test]
fn a_redirect_is_followed_only_to_an_exact_listed_host_over_the_scheme_asked_for() {
    let listed = ["github.com", "objects.githubusercontent.com"];
    let follows = |location: &str| redirect_to(location, "https", &listed);

    assert!(follows("https://github.com/a/b").is_some());
    assert!(follows("https://objects.githubusercontent.com/a?b=c").is_some());
    assert!(follows("https://GitHub.com./a").is_some());
    assert!(follows("https://github.com:8443/a").is_some());

    for refused in [
        "http://github.com/a",
        "https://github.com.example.com/a",
        "https://notgithub.com/a",
        "https://example.com/github.com",
        "https://example.com?github.com",
        "https://github.com@example.com/a",
        "https://example.com@github.com/a",
        "https://github.com:pw@github.com/a",
        "https://*.githubusercontent.com/a",
        "https://githubusercontent.com/a",
        "//github.com/a",
        "/a",
        "github.com/a",
        "ftp://github.com/a",
        "https://github.com/a b",
        "https://github.com/a\r\nhost: example.com",
        "https://github.com\\@example.com/a",
        "https://[::1]/a",
        "https://git\u{2024}hub.com/a",
        "",
    ] {
        assert_eq!(follows(refused), None, "{refused:?} was followed");
    }
}

#[test]
fn the_client_is_held_to_the_listed_hosts_and_the_one_it_was_pointed_at() {
    let held = Limited::to("github.com", &["objects.githubusercontent.com"]);
    let origin = |url: &str| Origin::of(url).expect("an origin");

    assert!(held.held(&origin("https://github.com/a")).is_none());
    assert!(
        held.held(&origin("https://objects.githubusercontent.com/a"))
            .is_none()
    );
    assert!(held.held(&origin("https://example.com/a")).is_some());
    assert!(
        held.held(&origin("https://github.com.example.com/a"))
            .is_some()
    );
}

/// A refresh of the catalog a server publishes at serial 5, triggered as
/// `trigger` says, with the trust state last written `kept` seconds ago, if it
/// has been written at all: the paths the server was asked for.
fn asked_when(trigger: Trigger, kept: Option<i64>) -> Vec<String> {
    let sample = Sample::new("catalog-fetch-cadence");
    let state = state(&sample);
    let server = Server::start();
    let source = source(&server);
    server.publish(&sign(&catalog(5, "1.0.0")));
    if let Some(kept) = kept {
        run(refresh(&state, &source, Trigger::Opening)).expect("a first catalog");
        let file = std::fs::File::options()
            .write(true)
            .open(state.join(TRUST_FILE))
            .expect("the trust state");
        file.set_modified(ago(kept)).expect("a modified time");
        server.forget();
    }

    run(refresh(&state, &source, trigger)).expect("a refresh");
    server.asked()
}

/// The instant `seconds` before now, or after it where they are negative.
fn ago(seconds: i64) -> SystemTime {
    let by = Duration::from_secs(seconds.unsigned_abs());
    let now = SystemTime::now();
    if seconds < 0 {
        now.checked_add(by)
    } else {
        now.checked_sub(by)
    }
    .expect("a time")
}

#[test]
fn the_background_fetch_waits_out_the_release_checks_interval_after_a_catalog_was_kept() {
    let recently = i64::try_from(crucible_update::ASK_AFTER.as_secs()).expect("seconds") - 60;

    assert_eq!(
        asked_when(Trigger::Background, Some(recently)),
        [] as [&str; 0]
    );
}

#[test]
fn the_background_fetch_asks_again_once_that_interval_has_passed() {
    let long_ago = i64::try_from(crucible_update::ASK_AFTER.as_secs()).expect("seconds") + 60;

    assert_eq!(
        asked_when(Trigger::Background, Some(long_ago)),
        ["/catalog/index.json", "/catalog/index.json.minisig"]
    );
}

#[test]
fn the_background_fetch_asks_when_nothing_was_ever_kept() {
    assert_eq!(
        asked_when(Trigger::Background, None),
        ["/catalog/index.json", "/catalog/index.json.minisig"]
    );
}

#[test]
fn the_background_fetch_asks_when_the_state_claims_to_be_written_in_the_future() {
    // A clock that ran ahead must not hold revocations back for as long as it
    // ran ahead by.
    assert_eq!(
        asked_when(Trigger::Background, Some(-DAY)),
        ["/catalog/index.json", "/catalog/index.json.minisig"]
    );
}

#[test]
fn an_explicit_request_fetches_however_recently_a_catalog_was_kept() {
    assert_eq!(
        asked_when(Trigger::Opening, Some(60)),
        ["/catalog/index.json", "/catalog/index.json.minisig"]
    );
}

#[test]
fn the_background_owner_fetches_and_keeps_on_the_runtime_it_is_given() {
    let sample = Sample::new("catalog-fetch-owner");
    let state = state(&sample);
    let server = Server::start();
    server.publish(&sign(&catalog(5, "1.0.0")));

    let (kept, shutdown) = serving(|services| {
        let runtime = services.runtime().handle().expect("a runtime");
        services.catalog().runs_on(runtime);
        services.catalog().refresh(&state, source(&server));
        until("the catalog was not kept", || offered(&state) == ["1.0.0"]);
        offered(&state)
    });

    assert_eq!(kept, ["1.0.0"]);
    assert!(shutdown.is_ok(), "{shutdown:?}");
}

#[test]
fn the_background_owner_does_nothing_before_it_is_given_a_runtime() {
    let sample = Sample::new("catalog-fetch-unarmed");
    let state = state(&sample);
    let server = Server::start();
    server.publish(&sign(&catalog(5, "1.0.0")));

    let (asked, _) = serving(|services| {
        services.catalog().refresh(&state, source(&server));
        std::thread::sleep(Duration::from_millis(300));
        server.asked()
    });

    assert!(asked.is_empty(), "{asked:?}");
}

#[test]
fn the_background_owner_holds_one_fetch_at_a_time_and_stops_it_when_asked() {
    let sample = Sample::new("catalog-fetch-one");
    let state = state(&sample);
    let server = Server::start();
    server.serve("/catalog/index.json", Reply::Stall);

    let (joined, shutdown) = serving(|services| {
        let runtime = services.runtime().handle().expect("a runtime");
        services.catalog().runs_on(runtime);
        services.catalog().refresh(&state, source(&server));
        until("the fetch did not start", || !server.asked().is_empty());
        // Left alone, not started a second time.
        services.catalog().refresh(&state, source(&server));
        std::thread::sleep(Duration::from_millis(300));
        let asked = server.asked();
        let started = Instant::now();
        let joined = services.catalog().join_within(Duration::from_secs(2));
        (asked, joined, started.elapsed())
    });

    let (asked, joined, took) = joined;
    assert_eq!(asked, ["/catalog/index.json"]);
    assert!(joined.is_ok(), "{joined:?}");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(shutdown.is_ok(), "{shutdown:?}");
}

#[test]
fn a_run_is_not_held_up_at_its_end_by_a_catalog_fetch_still_waiting() {
    let sample = Sample::new("catalog-fetch-shutdown");
    let state = state(&sample);
    let server = Server::start();
    server.serve("/catalog/index.json", Reply::Stall);

    let started = Instant::now();
    let ((), shutdown) = serving(|services| {
        let runtime = services.runtime().handle().expect("a runtime");
        services.catalog().runs_on(runtime);
        services.catalog().refresh(&state, source(&server));
        until("the fetch did not start", || !server.asked().is_empty());
    });

    assert!(shutdown.is_ok(), "{shutdown:?}");
    assert!(started.elapsed() < Duration::from_secs(10));
}

#[test]
fn the_published_source_names_the_catalog_release_and_the_keys_compiled_in() {
    let published = Source::published();

    assert_eq!(
        published.base,
        "https://github.com/augments-labs/crucible-agent-catalog/releases/download/catalog/"
    );
    assert_eq!(published.keys, crucible_external_harnesses::catalog::KEYS);
    assert_eq!(published.hosts, HOSTS);
    assert!(published.hosts.contains(&"github.com"));
    assert!(published.base.ends_with('/'));
}
