use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use crucible_config::Driver;
use crucible_execution::{Event, Outcome, SessionRef, Sink, TurnRequest};
use crucible_external_harnesses::fake_peer::{self, Plain, Reply, Script};
use crucible_external_harnesses::{Account, Control, Discovery};
use crucible_sandbox::{
    SandboxFilesystemAccess, SandboxNetworkPolicy, SandboxPolicy, SandboxService,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::TurnId;
use tokio::runtime::Handle;

use super::*;
use crate::access::Route;
use crate::backends::{Registry, Row};
use crate::content_use::{Consent, Routes};
use crate::sample::Sample;

/// The plain protocol, held under Claude Code's route and shipped.
static SHIPPED: [Row; 1] = [Row {
    driver: Driver::Claude,
    dialect: || Box::new(Plain),
    profile_key: "configDir",
    route: "agent:claude",
    docs_section: "claude-code",
    reaches: &[],
    shipping: true,
}];

/// The same row, held back from this release.
static HELD_BACK: [Row; 1] = [Row {
    shipping: false,
    ..SHIPPED[0]
}];

/// Keeps every event it is posted.
#[derive(Default)]
struct Kept(Mutex<Vec<Event>>);

impl Sink for Kept {
    fn post(&self, event: Event) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

impl Kept {
    fn events(&self) -> Vec<Event> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

fn local() -> Arc<dyn SandboxService> {
    Arc::new(LocalSandbox::new().watching_on(Handle::current()))
}

/// The workspace's own policy, with nothing enforced so that the peer runs
/// on any machine that tests.
fn conversation(sample: &Sample) -> SandboxPolicy {
    SandboxPolicy::standard(&sample.workspace())
        .expect("a policy")
        .with_enabled(false)
}

/// An `externalAgents` entry for Claude Code, its program at `executable`.
fn route(executable: &Path, profile: Option<&Path>) -> Route {
    Route::External {
        provider: "anthropic",
        instance: "work".into(),
        driver: Driver::Claude,
        executable: executable.to_string_lossy().into(),
        profile: profile.map(|profile| profile.to_string_lossy().into()),
        enabled: true,
    }
}

/// The fake peer playing `script`, installed in the workspace as the plain
/// protocol's program, and where it was put.
fn installed(sample: &Sample, script: &Script) -> PathBuf {
    let here = sample.root().join("peer");
    fake_peer::install(&here, "plain", script).expect("the fake peer is built")
}

/// The agent `registry` makes of an entry whose program is at `executable`.
fn agent(registry: &Registry, executable: &Path) -> Agent {
    Agent::routed(&route(executable, None), registry, |_| None).expect("a row")
}

/// A consent that has said yes to Claude Code.
fn granted() -> Consent {
    let consent = Consent::new(Routes::production());
    consent.give("agent:claude");
    consent
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clean_turn_through_a_launch_is_complete_and_speaks_the_dialects_arguments() {
    let sample = Sample::new("launch-clean");
    let peer = installed(
        &sample,
        &Script::of([Reply::new().hear(1).say("say hello").say("end")]),
    );
    let consent = granted();
    let policy = conversation(&sample);
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");

    let mut host = launcher
        .launch(&agent(&registry, &peer), &control, &Plain, None)
        .await
        .expect("started");
    let kept = Kept::default();
    let ended = host
        .turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &kept,
        )
        .await;
    host.stop(crucible_external_harnesses::STOP_GRACE)
        .await
        .expect("stopped");

    assert_eq!(ended, Ok(Outcome::Answered));
    let events = kept.events();
    assert!(
        matches!(events.first(), Some(Event::Started { .. })),
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(Event::Finished(Outcome::Answered))),
        "{events:?}"
    );
    assert_eq!(
        fake_peer::heard(peer.parent().expect("a directory")),
        vec!["turn go".to_owned()]
    );
}

/// The profile directory is the one place outside the workspace the child
/// may write, and what the workspace protects stays protected.
#[test]
fn the_childs_policy_opens_its_profile_and_keeps_protected_metadata() {
    let sample = Sample::new("launch-policy");
    fs::create_dir_all(sample.root().join(".git")).expect("a repository");
    let profile = std::env::temp_dir().join("crucible-launch-policy-profile");
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");

    let child = policy(&conversation, Some(&profile), &["api.anthropic.com"]).expect("a policy");

    assert!(child.permits_path(
        &profile.join("auth.json"),
        SandboxFilesystemAccess::ReadWrite
    ));
    assert!(!child.permits_path(
        &sample.root().join(".git").join("config"),
        SandboxFilesystemAccess::ReadWrite
    ));
    assert!(!conversation.permits_path(&profile, SandboxFilesystemAccess::ReadOnly));
    assert!(matches!(
        child.network(),
        SandboxNetworkPolicy::Domains(domains) if domains.allowed().len() == 1
    ));
    assert_eq!(child.enabled(), conversation.enabled());
}

/// A profile is a directory of the user's own, and what it holds as a
/// repository or as crucible's own state is protected there as everywhere.
#[test]
fn the_metadata_inside_a_profile_stays_protected() {
    let sample = Sample::new("launch-profile-metadata");
    let profile = sample.root().join("profile");
    for name in [".git", ".crucible"] {
        fs::create_dir_all(profile.join(name)).expect("metadata");
    }
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");

    let child = policy(&conversation, Some(&profile), &[]).expect("a policy");

    assert!(child.permits_path(
        &profile.join("auth.json"),
        SandboxFilesystemAccess::ReadWrite
    ));
    for inside in [
        profile.join(".git").join("hooks").join("pre-commit"),
        profile.join(".crucible").join("settings.json"),
    ] {
        assert!(
            !child.permits_path(&inside, SandboxFilesystemAccess::ReadWrite),
            "{} is writable",
            inside.display()
        );
    }
}

/// A profile under protected metadata is refused, not opened.
#[test]
fn a_profile_inside_protected_metadata_is_not_opened() {
    let sample = Sample::new("launch-protected");
    fs::create_dir_all(sample.root().join(".git")).expect("a repository");
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");
    let inside = sample.root().join(".git").join("profile");

    let child = policy(&conversation, Some(&inside), &[]);

    assert!(
        child.map_or(true, |child| !child.permits_path(
            &inside.join("auth.json"),
            SandboxFilesystemAccess::ReadWrite
        )),
        "a profile under .git was made writable"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_or_a_probe_without_a_yes_starts_nothing() {
    let sample = Sample::new("launch-unheld");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let consent = Consent::new(Routes::production());
    let policy = conversation(&sample);
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
    let registry = Registry::new(&SHIPPED);
    let agent = agent(&registry, &peer);
    let control = Control::new().expect("a control");

    let turn = launcher.launch(&agent, &control, &Plain, None).await;
    let probe = launcher
        .account(&agent, &control, &Plain, Account::Version)
        .await;
    let status = launcher
        .account(&agent, &control, &Plain, Account::Status)
        .await;

    assert!(matches!(turn, Err(Unlaunched::Asks(_))));
    assert!(matches!(probe, Err(Unlaunched::Asks(_))));
    assert!(matches!(status, Err(Unlaunched::Asks(_))));
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_probe_with_a_yes_reads_the_version() {
    let sample = Sample::new("launch-probe");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.2.3"])]));
    let consent = granted();
    let policy = conversation(&sample);
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");

    let read = launcher
        .account(&agent(&registry, &peer), &control, &Plain, Account::Version)
        .await
        .expect("started");

    assert!(
        matches!(&read, crucible_external_harnesses::Reading::Version(version) if version.as_str() == "1.2.3"),
        "{read:?}"
    );
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_held_back_is_refused_with_its_documentation_and_nothing_internal() {
    let sample = Sample::new("launch-held-back");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let consent = granted();
    let policy = conversation(&sample);
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
    let registry = Registry::new(&HELD_BACK);
    let control = Control::new().expect("a control");

    let refused = launcher
        .launch(&agent(&registry, &peer), &control, &Plain, None)
        .await
        .expect_err("held back");

    let said = refused.to_string();
    assert!(matches!(refused, Unlaunched::Withheld { .. }), "{said}");
    assert!(
        said.contains("docs/providers/agents.md#claude-code"),
        "{said}"
    );
    assert!(said.starts_with("Claude Code "), "{said}");
    assert!(
        !said
            .split(|letter: char| !letter.is_ascii_alphanumeric())
            .any(|word| word.len() == 3
                && word.starts_with('X')
                && word[1..].bytes().all(|byte| byte.is_ascii_digit())),
        "{said}"
    );
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_that_is_not_there_is_refused_with_how_to_install_it() {
    let sample = Sample::new("launch-missing");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let consent = granted();
    let policy = conversation(&sample);
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");
    // Looked for by name on a `PATH` with nothing on it.
    let missing = Agent::routed(&route(Path::new("plain"), None), &registry, |_| {
        Some(OsString::from(sample.root().join("empty")))
    })
    .expect("a row");
    assert!(matches!(missing.discovery(), Discovery::Missing));

    let refused = launcher
        .launch(&missing, &control, &Plain, None)
        .await
        .expect_err("missing");

    assert_eq!(
        refused.to_string(),
        "Not installed: no plain command on PATH and none at the configured executable. \
         Install Claude Code, then choose Check again."
    );
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 0);
}

#[test]
fn a_bare_name_is_found_on_path_and_an_absolute_one_where_it_is_written() {
    let sample = Sample::new("launch-discover");
    let peer = installed(&sample, &Script::of([]));
    let directory = peer.parent().expect("a directory").to_path_buf();

    let by_name = discover("plain", |name| {
        (name == "PATH").then(|| OsString::from(&directory))
    });
    let written = discover(&peer.to_string_lossy(), |_| None);
    // A relative `PATH` element is a workspace's, and is never searched, even
    // where it names the peer's own directory from here.
    let here = std::env::current_dir().expect("a working directory");
    let mut toward = PathBuf::new();
    for _ in here.components().skip(1) {
        toward.push("..");
    }
    toward.extend(directory.components().skip(1));
    assert!(toward.is_relative() && toward.join(peer.file_name().expect("a name")).is_file());
    let relative = discover("plain", |_| Some(OsString::from(&toward)));

    assert!(matches!(by_name, Discovery::Found(found) if found.path() == peer));
    assert!(matches!(written, Discovery::Found(found) if found.path() == peer));
    assert!(matches!(relative, Discovery::Missing));
}
