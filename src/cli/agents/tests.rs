//! What the `crucible agents` commands write: those asked of the agents door
//! against a catalog a host on this machine publishes and signs at test time,
//! and `unpack-check` and `probe`.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read as _};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser as _;
use crucible_app::catalog_fetch::Source;
use crucible_external_harnesses::catalog::SNAPSHOT;
use crucible_external_harnesses::catalog::fixture::signed;
use crucible_external_harnesses::fake_peer::scripts::acp;
use crucible_external_harnesses::fake_peer::{self, Reply, Script};
use crucible_external_harnesses::release::proof;
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxError, SandboxRequest, SandboxSession,
};
use tokio::runtime::Handle;

use super::super::{AgentsAction, Cli, Command};
use super::*;

/// The file list `unpack-check` prints for the release [`proof::catalog_release`]
/// publishes, committed for the catalog's intake to read.
const SURVEYED: &str =
    include_str!("../../../tests/fixtures/agent-catalog/unpack-check/binary.json");

/// A directory of the test's own, removed when it is dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let at = std::env::temp_dir().join(format!(
            "crucible-cli-{name}-{unique}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&at).expect("a directory");
        Self(at)
    }

    /// The artifact `described` and its download `archive`, written here.
    fn published(&self, described: &[u8], archive: &[u8]) -> (PathBuf, PathBuf) {
        let entry = self.0.join("entry.json");
        let download = self.0.join("agent.tar.gz");
        fs::write(&entry, described).expect("the artifact");
        fs::write(&download, archive).expect("the download");
        (entry, download)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the intake records for a release is what `unpack-check` prints, byte
/// for byte the committed list, and the release is left unpacked where the
/// command was told to put it.
#[test]
fn unpack_check_prints_the_file_list_committed_for_the_intake() {
    let scratch = Scratch::new("unpack-check");
    let (archive, described) = proof::catalog_release();
    let (entry, download) = scratch.published(&described, &archive);
    let out = scratch.0.join("out");

    let printed = checked(&entry, &download, &out);

    assert_eq!(printed.as_deref(), Ok(SURVEYED));
    assert_eq!(
        fs::read(out.join("lib").join("harness")).ok(),
        Some(proof::COMPANION.to_vec())
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(&out).map_or(0, |metadata| metadata.permissions().mode() & 0o777);
        assert_eq!(mode, 0o700, "only its owner reaches what was unpacked");
    }
}

/// The artifact `described` spells, pinned instead at `archive`.
fn repinned(described: &[u8], archive: &[u8]) -> Vec<u8> {
    use sha2::{Digest as _, Sha256};

    let mut artifact: serde_json::Value = serde_json::from_slice(described).expect("an artifact");
    let sha256 = Sha256::digest(archive)
        .iter()
        .fold(String::new(), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        });
    if let Some(fields) = artifact.as_object_mut() {
        fields.insert("sha256".to_owned(), sha256.into());
        fields.insert("size".to_owned(), archive.len().into());
    }
    artifact.to_string().into_bytes()
}

/// A refusal says what was wrong with what it was given, as the reader that
/// refused it found it, and not only that it was refused.
#[test]
fn unpack_check_says_the_cause_of_a_refusal() {
    let scratch = Scratch::new("unpack-check-cause");
    let (mut archive, described) = proof::catalog_release();
    archive.extend_from_slice(b"one byte more and then some");
    let described = repinned(&described, &archive);
    let (entry, download) = scratch.published(&described, &archive);
    let out = scratch.0.join("out");

    let refused = checked(&entry, &download, &out).expect_err("bytes after the stream");

    assert!(refused.contains("could not be read: "), "{refused}");
    assert!(refused.contains("past its gzip stream"), "{refused}");
    assert_eq!(refused.lines().count(), 1, "{refused}");
    assert!(!out.exists(), "nothing is left where the release would go");
}

/// The command line takes `agents unpack-check` with its three paths, as the
/// intake runs it, and leaves it out of what `--help` lists.
#[test]
fn unpack_check_is_parsed_from_its_paths_and_left_out_of_the_help() {
    let scratch = Scratch::new("unpack-check-parsed");
    let (archive, described) = proof::catalog_release();
    let (entry, download) = scratch.published(&described, &archive);
    let out = scratch.0.join("out");
    let words: Vec<std::ffi::OsString> = vec![
        "crucible".into(),
        "agents".into(),
        "unpack-check".into(),
        entry.clone().into(),
        download.clone().into(),
        out.clone().into(),
    ];

    let parsed = Cli::try_parse_from(words).expect("the command");

    let Some(Command::Agents {
        action:
            AgentsAction::UnpackCheck {
                entry: parsed_entry,
                artifact,
                out: parsed_out,
            },
    }) = parsed.command
    else {
        panic!("not unpack-check: {:?}", parsed.command);
    };
    assert_eq!(
        (&parsed_entry, &artifact, &parsed_out),
        (&entry, &download, &out)
    );
    assert_eq!(
        checked(&parsed_entry, &artifact, &parsed_out).as_deref(),
        Ok(SURVEYED)
    );
    let help = Cli::try_parse_from(["crucible", "agents", "--help"])
        .expect_err("the help")
        .to_string();
    assert!(help.contains("install"), "{help}");
    assert!(!help.contains("unpack-check"), "{help}");
}

/// A download that is not the artifact's, and a directory already where the
/// release would go, are each refused in one line, with nothing unpacked.
#[test]
fn unpack_check_refuses_in_one_line_and_leaves_nothing_unpacked() {
    let scratch = Scratch::new("unpack-check-refused");
    let (archive, described) = proof::catalog_release();
    let (entry, download) = scratch.published(&described, &archive);
    fs::write(&download, b"another release").expect("the download");
    let out = scratch.0.join("out");

    let refused = checked(&entry, &download, &out).expect_err("another release is refused");

    assert!(refused.starts_with("crucible: "), "{refused}");
    assert_eq!(refused.lines().count(), 1, "{refused}");
    assert!(!out.exists(), "nothing is left where the release would go");

    fs::write(&download, &archive).expect("the download");
    fs::create_dir(&out).expect("a directory in the way");
    let refused = checked(&entry, &download, &out).expect_err("a directory in the way");
    assert!(
        refused.contains("could not be made to unpack into: "),
        "{refused}"
    );
    assert!(
        refused.contains("(os error"),
        "the system's own words: {refused}"
    );
}

/// An agent is named as the catalog names it, a built-in one included, and a
/// version only where one is wanted.
#[test]
fn an_agent_is_named_as_the_catalog_names_it() {
    let parsed = |words: &[&str]| {
        Cli::try_parse_from(["crucible", "agents"].iter().chain(words))
            .expect("the command")
            .command
    };

    let Some(Command::Agents {
        action: AgentsAction::Install { agent, version },
    }) = parsed(&["install", "antigravity"])
    else {
        panic!("install antigravity is not an install");
    };
    assert_eq!((agent.as_str(), version), ("antigravity", None));
    let Some(Command::Agents {
        action: AgentsAction::Add { agent, version },
    }) = parsed(&["add", "goose", "1.0.0"])
    else {
        panic!("add goose 1.0.0 is not an add");
    };
    assert_eq!(
        (agent.as_str(), version.as_deref()),
        ("goose", Some("1.0.0"))
    );
    let Some(Command::Agents {
        action: AgentsAction::SignIn { instance, method },
    }) = parsed(&["sign-in", "work", "--method", "api-key"])
    else {
        panic!("sign-in work --method api-key is not a sign-in");
    };
    assert_eq!(
        (instance.as_str(), method.as_deref()),
        ("work", Some("api-key"))
    );
}

/// `catalog` is no driver of its own to install: it reaches the door, which
/// finds no agent the catalog names so and refuses it, naming it.
#[test]
fn a_catalog_agent_is_added_rather_than_installed() {
    let bench = Bench::new("install-catalog");
    bench.host.publish(1, &[goose("1.0.0", ONE)]);

    for words in [
        &["install", "catalog"][..],
        &["install", "catalog", "1.0.0"],
    ] {
        let answer = bench.carried(&action(words), Some(installs), &mut unasked);

        let Answer::Refused(refused) = &answer else {
            panic!("{words:?} was not refused: {answer:?}");
        };
        assert!(
            refused.contains("of catalog for this platform"),
            "{refused}"
        );
    }
    assert!(!bench.heard().contains("Which one?"), "nothing was asked");
}

#[test]
fn an_install_says_where_the_release_is_and_how_to_use_it() {
    let at = Path::new("/home/me/.crucible/programs/antigravity/1.3.0/agy");
    assert_eq!(
        installed("antigravity", "1.3.0", Some(at), true),
        "Installed antigravity 1.3.0 at /home/me/.crucible/programs/antigravity/1.3.0/agy. \
         /login offers it under Use an installed coding agent.\n"
    );
    assert_eq!(
        installed("antigravity", "1.3.0", Some(at), false),
        "antigravity 1.3.0 is already installed at \
         /home/me/.crucible/programs/antigravity/1.3.0/agy.\n"
    );
}

#[test]
fn a_terminal_is_told_each_new_percent_on_one_line() {
    let mut progress = Progress::new("antigravity".to_owned(), true);
    assert_eq!(progress.ended(), "");
    assert_eq!(
        progress.heard(1_000_000, 4_000_000),
        "\rDownloading antigravity (4 MB):  25%"
    );
    assert_eq!(progress.heard(1_000_001, 4_000_000), "");
    assert_eq!(
        progress.heard(4_000_000, 4_000_000),
        "\rDownloading antigravity (4 MB): 100%"
    );
    assert_eq!(progress.ended(), "\n");
}

#[test]
fn anything_else_is_told_once_that_the_download_began() {
    let mut progress = Progress::new("antigravity".to_owned(), false);
    assert_eq!(
        progress.heard(1_000_000, 4_000_000),
        "Downloading antigravity (4 MB)…\n"
    );
    assert_eq!(progress.heard(4_000_000, 4_000_000), "");
    assert_eq!(progress.ended(), "");
}

/// A host on this machine standing where the catalog is published, answering
/// each path it was given bytes for and every other with a 404.
struct Host {
    port: u16,
    routes: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    done: Arc<AtomicBool>,
}

impl Host {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a port");
        let port = listener.local_addr().expect("its address").port();
        let routes: Arc<Mutex<HashMap<String, Vec<u8>>>> = Arc::default();
        let done = Arc::new(AtomicBool::new(false));
        let (routing, finishing) = (Arc::clone(&routes), Arc::clone(&done));
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                if finishing.load(Ordering::Acquire) {
                    break;
                }
                let routes = Arc::clone(&routing);
                std::thread::spawn(move || replied(stream, &routes));
            }
        });
        Self { port, routes, done }
    }

    fn serve(&self, path: &str, body: Vec<u8>) {
        self.routes
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_owned(), body);
    }

    /// The catalog on this host, signed with the test key, whose redirects
    /// reach this host alone.
    fn source(&self) -> Source {
        let listed: &'static str = Box::leak(self.named().into_boxed_str());
        Source {
            base: format!("http://{}/catalog/", self.named()),
            keys: signed(b"{}").keys,
            hosts: Box::leak(Box::new([listed])),
            ..Source::published(&|_| None)
        }
    }

    /// This host as an address names it.
    fn named(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    /// Publishes `entries`, a release in each kept on this host by its
    /// digest, as a catalog newer by `serial` than the one built in.
    fn publish(&self, serial: u64, entries: &[(serde_json::Value, &[u8])]) {
        for (_, program) in entries {
            self.serve(&format!("/catalog/{}", digest(program)), program.to_vec());
        }
        let catalog = serde_json::json!({
            "schema": 1,
            "serial": SNAPSHOT.serial() + serial,
            "entries": entries.iter().map(|(entry, _)| entry).collect::<Vec<_>>(),
            "revoked": [],
        });
        let signed = signed(&serde_json::to_vec(&catalog).expect("the catalog"));
        self.serve("/catalog/index.json", signed.index);
        self.serve("/catalog/index.json.minisig", signed.signature);
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Release);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

/// Answers the one request `stream` carries from `routes`.
fn replied(mut stream: TcpStream, routes: &Mutex<HashMap<String, Vec<u8>>>) {
    use std::io::Write as _;

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
        .to_owned();
    let body = routes
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&path)
        .cloned();
    let (status, body) = body.map_or(("404 Not Found", Vec::new()), |body| ("200 OK", body));
    let _ = write!(
        stream,
        "HTTP/1.1 {status}\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Write);
}

fn digest(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};

    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut text, byte| {
            use std::fmt::Write as _;
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// How this machine's platform is spelled in a catalog.
fn platform() -> String {
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };
    format!("{os}-{}", std::env::consts::ARCH)
}

/// `key` at `version`, whose program `program` was built at review and is
/// kept on the catalog's own host.
fn entry(key: &str, version: &str, program: &[u8]) -> serde_json::Value {
    serde_json::json!({
        "key": key,
        "registry_id": format!("{key}-acp"),
        "version": version,
        "kind": "binary",
        "license": "Apache-2.0",
        "terms": "https://example.com/terms",
        "caution": "Read the terms first.",
        "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/12",
        "admits": {"name": key, "version": version},
        "targets": [{
            "platform": platform(),
            "cmd": "goose",
            "args": ["acp"],
            "env": {"GOOSE_NO_UPDATE": "1"},
            "hosts": ["downloads.example.com"],
            "artifacts": [{
                "url": "https://downloads.example.com/goose",
                "sha256": "a".repeat(64),
                "size": 1000,
                "format": "bare",
                "unpacked": program.len(),
                "decompressed": program.len(),
                "files": [{
                    "path": "goose",
                    "size": program.len(),
                    "sha256": digest(program),
                    "exec": true
                }],
                "replaced_by": {"sha256": digest(program), "size": program.len()}
            }]
        }]
    })
}

const ONE: &[u8] = b"goose 1.0.0, as reviewed";
const TWO: &[u8] = b"goose 2.0.0, as reviewed, and longer";

/// `goose` at `version`, built from `program`, as [`Host::publish`] takes it.
fn goose(version: &str, program: &'static [u8]) -> (serde_json::Value, &'static [u8]) {
    (entry("goose", version, program), program)
}

/// `goose` at 1.0.0, offering a key, a sign-in the agent runs and one at a
/// terminal, in that order.
fn signing_in() -> (serde_json::Value, &'static [u8]) {
    let (mut release, program) = goose("1.0.0", ONE);
    if let Some(object) = release.as_object_mut() {
        object.insert(
            "auth_methods".into(),
            serde_json::json!([
                {"id": "api-key", "kind": "env_var", "name": "API key", "var": "GOOSE_API_KEY"},
                {"id": "browser", "kind": "agent", "name": "Sign in with a browser"},
                {"id": "terminal", "kind": "terminal", "name": "Sign in at a terminal",
                 "command": "goose", "args": ["login"], "env": {}, "route": "piped"},
            ]),
        );
        object.insert(
            "sign_in_hosts".into(),
            serde_json::json!(["accounts.vendor.example"]),
        );
        object.insert(
            "content_use".into(),
            serde_json::json!({
                "text": "The vendor may use what you send to improve goose.",
                "source": "https://vendor.example/terms/data",
                "date": "24 Oct 2026",
                "origins": ["https://api.vendor.example"],
            }),
        );
    }
    (release, program)
}

/// A home of the test's own, the host its catalog is published on, the
/// runtime the door is waited for on, and what has been said on standard
/// error.
struct Bench {
    home: Home,
    host: Host,
    runtime: tokio::runtime::Runtime,
    heard: Mutex<String>,
    _scratch: Scratch,
}

impl Bench {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        let path = scratch
            .0
            .canonicalize()
            .expect("the scratch directory")
            .join("home");
        fs::create_dir_all(&path).expect("the home");
        let home = Home::find(&|name| {
            (name == crucible_config::HOME).then(|| path.clone().into_os_string())
        })
        .expect("the home");
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("a runtime");
        Self {
            home,
            host: Host::start(),
            runtime,
            heard: Mutex::new(String::new()),
            _scratch: scratch,
        }
    }

    /// `action` carried out, its question answered by `answer` where there
    /// is one, and a key asked of `key`.
    fn carried(
        &self,
        action: &AgentsAction,
        answer: Option<fn() -> String>,
        key: &mut dyn FnMut(&str) -> Result<Option<Typed>, Unstored>,
    ) -> Answer {
        let said = |text: &str| {
            self.heard
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push_str(text);
        };
        Desk {
            home: &self.home,
            shelf: Shelf {
                catalog: self.host.source(),
                home: &self.home,
                smoke: None,
                steps: None,
            },
            runtime: self.runtime.handle(),
            said: &said,
            terminal: false,
            answer,
        }
        .carried(action, key)
    }

    /// What has been said on standard error since it was last asked.
    fn heard(&self) -> String {
        std::mem::take(&mut *self.heard.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// `text` with this test's home and host written as placeholders and its
    /// paths with `/`, as it reads on any machine.
    fn shown(&self, text: &str) -> String {
        text.replace(&self.home.path().display().to_string(), "<home>")
            .replace(&self.host.named(), "<host>")
            .replace('\\', "/")
    }

    /// Where version `version` of goose's program is put.
    fn program(&self, version: &str) -> PathBuf {
        self.home
            .path()
            .join("programs")
            .join("goose")
            .join(version)
            .join("goose")
    }

    /// Writes the user file with `instances`.
    fn configure(&self, instances: &serde_json::Value) {
        let document = serde_json::json!({ "externalAgents": instances });
        fs::write(
            crucible_config::user(&self.home),
            serde_json::to_vec_pretty(&document).expect("the document"),
        )
        .expect("the user file");
    }
}

/// `crucible agents` and `words`, as the command line parses them.
fn action(words: &[&str]) -> AgentsAction {
    match Cli::try_parse_from(["crucible", "agents"].iter().chain(words))
        .expect("the command")
        .command
    {
        Some(Command::Agents { action }) => action,
        other => panic!("not an agents command: {other:?}"),
    }
}

/// A person answering a question with its first choice.
fn installs() -> String {
    "1".to_owned()
}

/// A person answering an install's question with its second choice, Cancel.
fn cancels() -> String {
    "2".to_owned()
}

/// A key source asked for nothing: a sign-in asked of it fails the test.
fn unasked(_: &str) -> Result<Option<Typed>, Unstored> {
    panic!("a key was asked for")
}

/// The text `answer` carries, where it did what it was asked.
fn done(answer: Answer) -> String {
    match answer {
        Answer::Done(text) => text,
        other => panic!("not done: {other:?}"),
    }
}

/// With no session, an install asks on this terminal, lands the program where
/// it says it did once the question is answered yes, and says so; an update of
/// an instance run from it then says it is already current there. A version
/// the command does not name is the newest the catalog offers.
#[test]
fn an_install_with_no_session_lands_the_program_where_it_says() {
    let bench = Bench::new("install");
    bench.host.publish(1, &[goose("1.0.0", ONE)]);

    let said = done(bench.carried(&action(&["install", "goose"]), Some(installs), &mut unasked));

    let program = bench.program("1.0.0");
    assert_eq!(fs::read(&program).ok().as_deref(), Some(ONE));
    assert!(
        said.contains(&format!(" at {}.", program.display())),
        "the install says where the program is: {said}"
    );
    insta::assert_snapshot!("install_asked", bench.shown(&bench.heard()));
    insta::assert_snapshot!("install_done", bench.shown(&said));
    bench.configure(&serde_json::json!({
        "work": {
            "driver": "catalog",
            "agent": "goose",
            "executable": program.to_string_lossy(),
        },
    }));

    let again = done(bench.carried(&action(&["update", "goose"]), Some(installs), &mut unasked));
    assert!(
        !bench.heard().contains("Which one?"),
        "nothing is asked of a current agent"
    );
    insta::assert_snapshot!("install_current", bench.shown(&again));
}

/// An install answered Cancel, or asked where there is no terminal to ask on,
/// changes nothing and says so; with no terminal nothing is asked at all.
#[test]
fn an_install_cancelled_or_unasked_changes_nothing() {
    let bench = Bench::new("install-cancelled");
    bench.host.publish(1, &[goose("1.0.0", ONE)]);

    let cancelled = bench.carried(&action(&["install", "goose"]), Some(cancels), &mut unasked);

    assert!(
        matches!(&cancelled, Answer::Refused(refused) if refused.contains("nothing was changed")),
        "{cancelled:?}"
    );
    assert!(
        bench.heard().contains("Which one?"),
        "the question was asked"
    );
    let unasked_here = bench.carried(&action(&["install", "goose", "1.0.0"]), None, &mut unasked);
    let Answer::Refused(refused) = &unasked_here else {
        panic!("an install with no terminal was not refused: {unasked_here:?}");
    };
    assert!(refused.contains("nothing was changed"), "{refused}");
    assert!(
        refused.contains("asks its question on a terminal"),
        "{refused}"
    );
    assert!(!bench.heard().contains("Which one?"), "nothing was asked");
    assert!(!bench.program("1.0.0").exists(), "nothing was installed");
}

/// `search` lists each agent the catalog offers, its versions newest first and
/// what is installed, keeping only those named like the text it is given.
#[test]
fn search_lists_what_each_agent_offers_and_has_installed() {
    let bench = Bench::new("search");
    bench.host.publish(
        1,
        &[
            goose("2.0.0", TWO),
            goose("1.0.0", ONE),
            (entry("codex", "0.1.0", b"codex 0.1.0"), b"codex 0.1.0"),
        ],
    );
    done(bench.carried(
        &action(&["install", "goose", "1.0.0"]),
        Some(installs),
        &mut unasked,
    ));

    let listed = done(bench.carried(&action(&["search"]), None, &mut unasked));
    let matched = done(bench.carried(&action(&["search", "GOO"]), None, &mut unasked));
    let unmatched = done(bench.carried(&action(&["search", "nothing"]), None, &mut unasked));

    insta::assert_snapshot!("search", format!("{listed}--\n{matched}--\n{unmatched}"));
}

/// `verify` says each installed version is as it was installed, and ends 1
/// naming the file where one is not; `remove` then asks and takes it.
#[test]
fn verify_ends_differing_where_a_file_changed_and_remove_takes_it() {
    let bench = Bench::new("verify");
    bench.host.publish(1, &[goose("1.0.0", ONE)]);
    done(bench.carried(&action(&["install", "goose"]), Some(installs), &mut unasked));

    let intact = done(bench.carried(&action(&["verify", "goose"]), None, &mut unasked));
    assert_eq!(intact, "goose 1.0.0 is as it was installed.\n");
    let program = bench.program("1.0.0");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&program, fs::Permissions::from_mode(0o700)).expect("writable");
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(&program).expect("the program").permissions();
        #[allow(clippy::permissions_set_readonly_false)]
        permissions.set_readonly(false);
        fs::set_permissions(&program, permissions).expect("writable");
    }
    fs::write(&program, b"another program").expect("the program changed");
    let changed = bench.carried(&action(&["verify", "goose"]), None, &mut unasked);
    let Answer::Differs(differs) = &changed else {
        panic!("a changed file was not found: {changed:?}");
    };
    assert_eq!(
        differs,
        "goose 1.0.0 differs from what was installed at goose.\n"
    );

    let removed = done(bench.carried(&action(&["remove", "goose"]), Some(installs), &mut unasked));
    assert_eq!(removed, "Removed goose 1.0.0.\n");
    assert!(!program.exists(), "the program was removed");
}

/// With no method named, a sign-in beside an instance whose agent signs in by
/// several is refused, naming the instance and each of them, and asks for no
/// key.
#[test]
fn a_sign_in_naming_no_method_beside_several_is_refused_with_their_names() {
    let bench = Bench::new("sign-in-methods");
    bench.configure(&serde_json::json!({
        "google": {
            "driver": "antigravity",
            "executable": bench.home.path().join("agy").to_string_lossy(),
            "profileDir": bench.home.path().join("agents").join("antigravity").to_string_lossy(),
        },
    }));

    let answer = bench.carried(&action(&["sign-in", "google"]), None, &mut unasked);

    let Answer::Refused(refused) = &answer else {
        panic!("a sign-in with several methods was not refused: {answer:?}");
    };
    assert_eq!(
        refused,
        "google signs in by several methods: oauth-personal, oauth-business, \
         gemini-api-key, agent-platform; name one with --method"
    );
}

/// A key a sign-in needs and finds none kept for is asked for once and kept,
/// and is never said; a sign-in after it finds it kept and asks nothing.
#[test]
fn a_key_is_asked_for_once_kept_and_never_said() {
    const KEY: &str = "goose-key-never-shown-0123456789";
    let bench = Bench::new("sign-in-key");
    bench.host.publish(1, &[signing_in()]);
    bench.configure(&serde_json::json!({
        "work": {
            "driver": "catalog",
            "agent": "goose",
            "executable": bench.program("1.0.0").to_string_lossy(),
        },
    }));
    let asked = AtomicUsize::new(0);
    let mut key = |_: &str| {
        asked.fetch_add(1, Ordering::Relaxed);
        Ok(Some(Typed::new(KEY.to_owned())))
    };
    let sign_in = action(&["sign-in", "work", "--method", "api-key"]);

    let first = bench.carried(&sign_in, None, &mut key);
    let again = bench.carried(&sign_in, None, &mut key);

    let kept = "Kept the key for work to sign in by api-key; its agent is given it the next time \
                it starts.\n";
    for answer in [&first, &again] {
        assert!(
            matches!(answer, Answer::Done(said) if said == kept),
            "the key was not kept"
        );
    }
    assert_eq!(
        asked.load(Ordering::Relaxed),
        1,
        "the key was asked for once"
    );
    let heard = bench.heard();
    let answered = format!("{first:?}{again:?}");
    assert!(
        !heard.contains(KEY) && !answered.contains(KEY),
        "the key was said"
    );
}

/// A key is asked for only where standard input, output and error are each a
/// terminal, and is otherwise refused with what to do instead.
#[test]
fn a_key_is_asked_for_only_on_a_terminal() {
    for ends in [
        [false, true, true],
        [true, false, true],
        [true, true, false],
    ] {
        let Err(Unstored::Refused(refused)) = prompted(ends, "work") else {
            panic!("a key was asked for with {ends:?} terminals");
        };
        assert!(
            refused.contains("run `crucible agents sign-in` on a terminal"),
            "{refused}"
        );
    }
}

/// What `probe` prints for the fake peer in a sandbox that closes the network,
/// committed for the catalog's intake to read.
const PROBED: &str = include_str!("../../../tests/fixtures/agent-catalog/probe/fake-peer.json");

/// What `probe --unconfined` prints for the fake peer where the sandbox
/// cannot close the network.
const PROBED_UNCONFINED: &str =
    include_str!("../../../tests/fixtures/agent-catalog/probe/unconfined.json");

/// How long the probe tests give a peer that is meant to answer.
const ANSWERING: Duration = Duration::from_secs(20);

/// A sandbox that runs what it is given as the local one does, but whose
/// backend cannot be proven to close the network, as FreeBSD's cannot.
struct Open(Arc<dyn SandboxService>);

impl SandboxService for Open {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async {
            Err(SandboxError::BackendUnavailable {
                reason: "no network sandbox on this machine".into(),
            })
        })
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        self.0.prepare(request)
    }
}

fn open(runtime: Handle) -> Arc<dyn SandboxService> {
    Arc::new(Open(probing::local(runtime)))
}

/// A directory where a release was unpacked, with the fake peer in it as the
/// agent's program playing `reply`, and the entry that lists it.
struct Unpacked {
    scratch: Scratch,
    root: PathBuf,
    entry: PathBuf,
}

impl Unpacked {
    fn new(name: &str, reply: Reply) -> Self {
        let scratch = Scratch::new(name);
        let root = scratch.0.join("release");
        let program = format!("agent{}", std::env::consts::EXE_SUFFIX);
        fake_peer::install(&root, "agent", &Script::once(reply)).expect("the fake peer is built");
        let platforms = [
            "linux-x86_64",
            "linux-aarch64",
            "darwin-x86_64",
            "darwin-aarch64",
            "windows-x86_64",
            "windows-aarch64",
            "freebsd-x86_64",
        ];
        let target = |platform: &str| {
            serde_json::json!({
                "platform": platform,
                "cmd": program,
                "args": ["acp"],
                "env": {},
                "hosts": ["github.com"],
                "artifacts": [{
                    "url": "https://github.com/example/agent/releases/download/v1/agent.zip",
                    "sha256": "a".repeat(64),
                    "size": 1000,
                    "format": "zip",
                    "unpacked": 3000,
                    "decompressed": 4000,
                    "files": [{
                        "path": program, "size": 2900, "sha256": "b".repeat(64), "exec": true
                    }],
                    "kept": [program],
                }],
            })
        };
        let entry = serde_json::json!({
            "key": "fake-agent",
            "registry_id": "fake-agent-acp",
            "version": "1.0.0",
            "kind": "binary",
            "license": "MIT",
            "review": "https://github.com/augments-labs/crucible-agent-catalog/pull/1",
            "admits": {"name": "fake-agent", "version": "1.0.0"},
            "reach": [],
            "sign_in_hosts": [],
            "state_paths": [],
            "auth_methods": [],
            "targets": platforms.map(target),
        });
        let entry_file = scratch.0.join("entry.json");
        fs::write(&entry_file, entry.to_string()).expect("the entry");
        Self {
            scratch,
            root,
            entry: entry_file,
        }
    }

    /// What `probe` writes, the agent given `within` to answer.
    fn probed(
        &self,
        unconfined: bool,
        sandbox: &dyn Fn(Handle) -> Arc<dyn SandboxService>,
        within: Duration,
    ) -> Result<String, String> {
        let _ = &self.scratch;
        let mode = moded(unconfined, &disposable).expect("a runner to be thrown away");
        probed(&self.root, &self.entry, mode, sandbox, within)
    }
}

/// The environment of a disposable runner: `CI` is `true`.
fn disposable(name: &str) -> Option<std::ffi::OsString> {
    (name == "CI").then(|| "true".into())
}

/// The fake peer answers `initialize` when it hears it.
fn answering() -> Reply {
    Reply::new().hear(1).say(acp::probed())
}

/// Whether this machine's sandbox can close the network, which a test of what
/// it refuses needs; said when it cannot, so that a skipped test says why.
fn confining(unpacked: &Unpacked) -> bool {
    match unpacked.probed(false, &probing::local, ANSWERING) {
        Err(refused) if refused.contains("cannot close the network") => {
            eprintln!("skipped: {refused}");
            false
        }
        _ => true,
    }
}

/// What the intake records for an agent is what `probe` prints, byte for byte
/// the committed JSON: its `agentInfo`, both sign-in methods, and that the
/// conformance cases were not run.
#[test]
fn probe_prints_the_json_committed_for_the_intake() {
    let unpacked = Unpacked::new("probe", answering());
    if !confining(&unpacked) {
        return;
    }

    let printed = unpacked.probed(false, &probing::local, ANSWERING);

    assert_eq!(printed.as_deref(), Ok(PROBED));
}

/// A program that tries to reach a socket is refused by the sandbox, and the
/// probe says so by what the program went on to say; the same program run
/// without the sandbox does reach it.
#[test]
fn probe_closes_the_network_to_the_agent() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
    listener.set_nonblocking(true).expect("non-blocking");
    let to = listener.local_addr().expect("its address");
    let said = |name: &str| acp::probed().replace("fake-agent", name);
    let reply = Reply::new()
        .hear(1)
        .reach(to, said("refused"), said("reached"));
    let unpacked = Unpacked::new("probe-socket", reply);
    if !confining(&unpacked) {
        return;
    }

    let confined = unpacked
        .probed(false, &probing::local, ANSWERING)
        .expect("probed");

    assert!(confined.contains("\"name\": \"refused\""), "{confined}");
    assert!(
        matches!(listener.accept(), Err(ref none) if none.kind() == io::ErrorKind::WouldBlock),
        "nothing reached the listener"
    );
    assert!(
        fake_peer::heard(&unpacked.root).is_empty(),
        "the program could not write where its release is"
    );

    let unconfined = unpacked
        .probed(true, &open, ANSWERING)
        .expect("probed without the sandbox");

    assert!(unconfined.contains("\"name\": \"reached\""), "{unconfined}");
    assert!(
        listener.accept().is_ok(),
        "the program reached the listener"
    );
    assert!(
        !fake_peer::heard(&unpacked.root).is_empty(),
        "without the sandbox it wrote where its release is"
    );
}

/// A program that never answers is stopped at the bound, and the probe says
/// that it did not answer.
#[test]
fn probe_ends_at_the_bound_for_an_agent_that_never_answers() {
    let unpacked = Unpacked::new(
        "probe-silent",
        Reply::new().hear(1).pause(Duration::from_mins(1)),
    );
    let began = Instant::now();

    let refused = unpacked
        .probed(true, &probing::local, Duration::from_millis(500))
        .expect_err("no answer");

    assert!(began.elapsed() < Duration::from_secs(30), "{refused}");
    assert!(refused.starts_with("crucible: "), "{refused}");
    assert!(refused.contains("did not finish"), "{refused}");
    assert_eq!(refused.lines().count(), 1, "{refused}");
}

/// Where the sandbox cannot close the network the probe starts nothing unless
/// it is told the run is unconfined, and then it runs `initialize` alone and
/// marks the output, byte for byte the committed JSON.
#[test]
fn probe_refuses_without_a_sandbox_unless_it_is_told_to_go_unconfined() {
    let unpacked = Unpacked::new("probe-unconfined", answering());

    let refused = unpacked
        .probed(false, &open, ANSWERING)
        .expect_err("no sandbox that closes the network");

    assert!(refused.starts_with("crucible: "), "{refused}");
    assert!(refused.contains("cannot close the network"), "{refused}");
    assert!(refused.contains("--unconfined"), "{refused}");
    assert_eq!(refused.lines().count(), 1, "{refused}");
    assert_eq!(
        fake_peer::played(&unpacked.root),
        0,
        "nothing was started before the refusal"
    );

    let printed = unpacked.probed(true, &open, ANSWERING);

    assert_eq!(printed.as_deref(), Ok(PROBED_UNCONFINED));
}

/// The command line takes `agents probe` with its two paths and `--unconfined`,
/// and leaves it out of what `--help` lists.
#[test]
fn probe_is_parsed_from_its_paths_and_left_out_of_the_help() {
    let words: Vec<std::ffi::OsString> = vec![
        "crucible".into(),
        "agents".into(),
        "probe".into(),
        "/release".into(),
        "/entry.json".into(),
        "--unconfined".into(),
    ];

    let parsed = Cli::try_parse_from(words).expect("the command");

    let Some(Command::Agents {
        action:
            AgentsAction::Probe {
                root,
                entry,
                unconfined,
            },
    }) = parsed.command
    else {
        panic!("not probe: {:?}", parsed.command);
    };
    assert_eq!(
        (root, entry, unconfined),
        (
            PathBuf::from("/release"),
            PathBuf::from("/entry.json"),
            true
        )
    );
    let help = Cli::try_parse_from(["crucible", "agents", "--help"])
        .expect_err("the help")
        .to_string();
    assert!(!help.contains("probe"), "{help}");
}

/// Starting an agent without the sandbox is allowed only on a process that
/// says it is on a disposable runner: `CI` equal to `true`. Anything else is
/// refused in one line naming that condition, and asking for the sandbox needs
/// nothing.
#[test]
fn unconfined_is_allowed_only_where_the_runner_says_it_is_disposable() {
    let says = |value: &'static str| {
        move |name: &str| (name == "CI").then(|| std::ffi::OsString::from(value))
    };

    assert!(moded(true, &disposable).is_ok());
    assert!(moded(false, &|_| None).is_ok(), "the sandbox needs no word");
    for lookup in [
        &(|_: &str| None) as &dyn Fn(&str) -> Option<std::ffi::OsString>,
        &says("1"),
        &says("false"),
        &says("TRUE"),
        &says(""),
    ] {
        let refused = moded(true, lookup).expect_err("not a disposable runner");

        assert!(refused.starts_with("crucible: "), "{refused}");
        assert!(refused.contains("CI is true"), "{refused}");
        assert!(refused.contains("--unconfined"), "{refused}");
        assert_eq!(refused.lines().count(), 1, "{refused}");
    }
}
