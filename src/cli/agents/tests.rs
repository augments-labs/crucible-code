//! What `crucible agents install`, `crucible agents unpack-check` and
//! `crucible agents probe` write.

use std::fs;
use std::io;
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use clap::Parser as _;
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

#[test]
fn a_driver_is_named_as_externalagents_names_it() {
    assert_eq!(driver("antigravity"), Ok(Driver::Antigravity));
    assert_eq!(driver("claude"), Ok(Driver::Claude));
    assert_eq!(
        driver("Antigravity"),
        Err("expected one of claude, antigravity".to_owned())
    );
}

/// A catalog agent is not installed by its driver, which names no agent of
/// its own, so `catalog` is refused with the command that adds one, and only
/// the built-in drivers are listed.
#[test]
fn a_catalog_agent_is_added_rather_than_installed() {
    let refused = driver("catalog").expect_err("catalog is not installed by its driver");
    assert!(refused.contains("crucible agents add"), "{refused}");
    assert!(refused.contains("claude, antigravity"), "{refused}");
}

#[test]
fn an_install_says_where_the_release_is_and_how_to_use_it() {
    let installed = Installed {
        version: "1.3.0".into(),
        executable: PathBuf::from("/home/me/.crucible/programs/antigravity/1.3.0/agy"),
        kept: false,
    };
    assert_eq!(
        answered(Driver::Antigravity, &installed),
        "Installed antigravity 1.3.0 at /home/me/.crucible/programs/antigravity/1.3.0/agy. \
         /login offers it under Use an installed coding agent.\n"
    );
    let kept = Installed {
        kept: true,
        ..installed
    };
    assert_eq!(
        answered(Driver::Antigravity, &kept),
        "antigravity 1.3.0 is already installed at \
         /home/me/.crucible/programs/antigravity/1.3.0/agy.\n"
    );
}

#[test]
fn a_terminal_is_told_each_new_percent_on_one_line() {
    let mut progress = Progress::new(Driver::Antigravity, true);
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
    let mut progress = Progress::new(Driver::Antigravity, false);
    assert_eq!(
        progress.heard(1_000_000, 4_000_000),
        "Downloading antigravity (4 MB)…\n"
    );
    assert_eq!(progress.heard(4_000_000, 4_000_000), "");
    assert_eq!(progress.ended(), "");
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
