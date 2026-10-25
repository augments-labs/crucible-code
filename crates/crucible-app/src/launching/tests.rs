use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crucible_config::{Driver, Serves};
use crucible_execution::{
    Authentication, Decision, Event, Failure, Outcome, SessionRef, Sink, Text, TurnRequest,
};
use crucible_external_harnesses::catalog::{Catalog, Held, Trust, Variable, fixture};
use crucible_external_harnesses::fake_peer::scripts::acp;
use crucible_external_harnesses::fake_peer::{self, Plain, Reply, Script};
use crucible_external_harnesses::{
    Account, Act, Control, Dialect, Discovery, Ran, Reading, Refusal, Request, Shape, Told,
};
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendIdentity, SandboxCapabilities, SandboxCommand, SandboxError,
    SandboxFilesystemAccess, SandboxInput, SandboxInspection, SandboxLaunch, SandboxNetworkPolicy,
    SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRequest, SandboxResourceLimits,
    SandboxService, SandboxSession, SandboxUsage, SandboxViolation,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::TurnId;
use tokio::runtime::Handle;

use super::*;
use crate::access::Route;
use crate::backends::tests::{release_of, snapshot_version, trust_of, trusting};
use crate::backends::{ANTIGRAVITY, CatalogRow, Registry, Row};
use crate::content_use::{Consent, Routes, Unstated};
use crate::sample::Sample;

/// The plain protocol, held under Claude Code's route and shipped.
static SHIPPED: [Row; 1] = [Row {
    driver: Driver::Claude,
    dialect: |_, _, _| Ok(Box::new(Plain)),
    profile_key: "configDir",
    route: "agent:claude",
    docs_section: "claude-code",
    reaches: &[],
    sign_in: |_| None,
    pinned: |_| None,
    catalogued: false,
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
        serves: Serves::Provider("anthropic"),
        instance: "work".into(),
        driver: Driver::Claude,
        executable: executable.to_string_lossy().into(),
        profile: profile.map(|profile| profile.to_string_lossy().into()),
        model: None,
        effort: None,
        enabled: true,
    }
}

/// The fake peer playing `script`, installed in the workspace as the plain
/// protocol's program, and where it was put.
fn installed(sample: &Sample, script: &Script) -> PathBuf {
    let here = sample.root().join("peer");
    fake_peer::install(&here, "plain", script).expect("the fake peer is built")
}

/// A directory beside this test's executable, removed when it is dropped.
struct Beside(PathBuf);

impl Beside {
    fn new() -> Self {
        let executable =
            fs::canonicalize(std::env::current_exe().expect("an executable")).expect("resolved");
        let directory = executable
            .parent()
            .expect("a directory")
            .join(format!("launch-relative-{}", std::process::id()));
        fs::create_dir_all(&directory).expect("created");
        Self(directory)
    }
}

impl Drop for Beside {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The way from `from` to `to`, both resolved, through the deepest directory
/// they share; none where they share no root, as on two drives.
fn relative(from: &Path, to: &Path) -> Option<PathBuf> {
    let shared = from
        .components()
        .zip(to.components())
        .take_while(|(left, right)| left == right)
        .count();
    if shared == 0 {
        return None;
    }
    let mut toward: PathBuf = from.components().skip(shared).map(|_| "..").collect();
    toward.extend(to.components().skip(shared));
    Some(toward)
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
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");

    let mut host = launcher
        .launch(&agent(&registry, &peer), &control, &mut Plain, None)
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

/// A conversation whose commands are held to `limits`, run unconfined as
/// [`conversation`]'s are.
fn limited(sample: &Sample, limits: SandboxResourceLimits) -> SandboxPolicy {
    conversation(sample)
        .with_limits(limits)
        .expect("limits a policy holds")
}

/// A turn the peer answers, on an agent started under `policy`, after
/// `waited`.
async fn answered_after(sample: &Sample, policy: &SandboxPolicy, waited: Duration) -> Outcome {
    let peer = installed(
        sample,
        &Script::of([Reply::new().hear(1).say("say hello").say("end")]),
    );
    let consent = granted();
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");
    let mut host = launcher
        .launch(&agent(&registry, &peer), &control, &mut Plain, None)
        .await
        .expect("started");
    tokio::time::sleep(waited).await;
    let ended = host
        .turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &Kept::default(),
        )
        .await;
    let _ = host.stop(crucible_external_harnesses::STOP_GRACE).await;
    ended.unwrap_or_else(|failure| panic!("the turn failed: {}", failure.reason().as_str()))
}

/// An agent lives as long as the conversation, so the time one command is
/// allowed does not end it: an agent started under a one-second limit still
/// answers a turn taken after the limit has passed.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_outlives_the_time_one_command_is_allowed() {
    let sample = Sample::new("launch-command-time");
    let policy = limited(
        &sample,
        SandboxResourceLimits {
            command_time: Some(Duration::from_secs(1)),
            ..SandboxResourceLimits::default()
        },
    );

    let ended = answered_after(&sample, &policy, Duration::from_secs(2)).await;

    assert_eq!(ended, Outcome::Answered);
}

/// What an agent prints is a protocol the host reads a message at a time, so
/// the output one command may print does not end it either.
#[tokio::test(flavor = "multi_thread")]
async fn an_agent_outlives_the_output_one_command_may_print() {
    let sample = Sample::new("launch-output-bytes");
    let policy = limited(
        &sample,
        SandboxResourceLimits {
            output_bytes: Some(1),
            ..SandboxResourceLimits::default()
        },
    );

    let ended = answered_after(&sample, &policy, Duration::ZERO).await;

    assert_eq!(ended, Outcome::Answered);
}

/// An account step is one command, so the time a command is allowed still
/// ends one that does not: a version step held to one second comes back long
/// before the peer would have answered.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_step_is_still_held_to_the_time_one_command_is_allowed() {
    let sample = Sample::new("launch-step-time");
    let peer = installed(
        &sample,
        &Script::of([Reply::new().pause(Duration::from_secs(30)).say("1.2.3")]),
    );
    let consent = granted();
    let policy = limited(
        &sample,
        SandboxResourceLimits {
            command_time: Some(Duration::from_secs(1)),
            ..SandboxResourceLimits::default()
        },
    );
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");
    let began = std::time::Instant::now();

    let read = launcher
        .account(
            &agent(&registry, &peer),
            &control,
            &mut Plain,
            Account::Version,
            &|_| {},
        )
        .await
        .expect("started");

    assert!(
        began.elapsed() < Duration::from_secs(15),
        "{:?}",
        began.elapsed()
    );
    assert!(
        !matches!(&read, crucible_external_harnesses::Reading::Version(_)),
        "the version was read"
    );
}

/// A run that answers turns keeps every ceiling of the conversation's but
/// the three that measure one command's life, and the conversation keeps all
/// of them.
#[test]
fn a_lasting_run_drops_only_the_ceilings_of_one_command() {
    let sample = Sample::new("launch-ceilings");
    let conversation = limited(
        &sample,
        SandboxResourceLimits {
            command_time: Some(Duration::from_mins(20)),
            cpu_seconds: Some(3600),
            output_bytes: Some(10 * 1024 * 1024),
            concurrent_commands: Some(4),
            ..SandboxResourceLimits::default()
        },
    );

    let child = policy(
        &conversation,
        None,
        &[],
        Life::Conversation { resume: None }.limits(conversation.limits()),
    )
    .expect("a policy");

    assert_eq!(
        child.limits(),
        SandboxResourceLimits {
            command_time: None,
            cpu_seconds: None,
            output_bytes: None,
            ..conversation.limits()
        }
    );
    assert_eq!(
        policy(
            &conversation,
            None,
            &[],
            Life::Step.limits(conversation.limits())
        )
        .expect("a policy")
        .limits(),
        conversation.limits()
    );
}

/// The profile directory is the one place outside the workspace the child
/// may write, and what the workspace protects stays protected.
#[test]
fn the_childs_policy_opens_its_profile_and_keeps_protected_metadata() {
    let sample = Sample::new("launch-policy");
    fs::create_dir_all(sample.root().join(".git")).expect("a repository");
    // Resolved as the policy names it, so that what is asserted is the rule
    // and not the temporary directory's links.
    let profile = fs::canonicalize(std::env::temp_dir())
        .expect("resolved")
        .join("crucible-launch-policy-profile");
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");

    let child = policy(
        &conversation,
        Some(&profile),
        &["api.anthropic.com"],
        conversation.limits(),
    )
    .expect("a policy");

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

/// A conversation whose commands run unconfined has its child run so too: the
/// host's network is already open to it, and a grant of the row's hosts is
/// one an unconfined run cannot hold to, so the sandbox would refuse it.
#[test]
fn an_unconfined_conversation_grants_its_child_no_hosts_it_would_be_refused_for() {
    let sample = Sample::new("launch-unconfined");
    let conversation = SandboxPolicy::standard(&sample.workspace())
        .expect("a policy")
        .with_enabled(false);

    let child = policy(
        &conversation,
        None,
        &["api.anthropic.com"],
        conversation.limits(),
    )
    .expect("a policy");

    assert!(!child.enabled());
    assert!(
        matches!(child.network(), SandboxNetworkPolicy::Closed),
        "{:?}",
        child.network()
    );
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
    let profile = fs::canonicalize(profile).expect("resolved");
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");

    let child =
        policy(&conversation, Some(&profile), &[], conversation.limits()).expect("a policy");

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

    let child = policy(&conversation, Some(&inside), &[], conversation.limits());

    assert!(
        child.map_or(true, |child| !child.permits_path(
            &inside.join("auth.json"),
            SandboxFilesystemAccess::ReadWrite
        )),
        "a profile under .git was made writable"
    );
}

/// A profile that is a link leading nowhere yet is refused, since what it
/// leads to can be made later under protected metadata.
#[cfg(unix)]
#[test]
fn a_profile_that_is_a_dangling_link_is_not_opened() {
    let sample = Sample::new("launch-dangling-link");
    fs::create_dir_all(sample.root().join(".git")).expect("a repository");
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");
    let beside = Sample::new("launch-dangling-link-beside");
    let named = beside.root().join("profile");
    let target = fs::canonicalize(sample.root().join(".git"))
        .expect("resolved")
        .join("profile");
    std::os::unix::fs::symlink(&target, &named).expect("a link");

    let child = policy(&conversation, Some(&named), &[], conversation.limits());

    for path in [&named, &target] {
        assert!(
            child.as_ref().map_or(true, |child| !child
                .permits_path(&path.join("auth.json"), SandboxFilesystemAccess::ReadWrite)),
            "{} was made writable",
            path.display()
        );
    }
}

/// A profile named through a link into protected metadata is refused, as the
/// directory the link leads to would be.
#[cfg(unix)]
#[test]
fn a_profile_reached_through_a_link_into_protected_metadata_is_not_opened() {
    let sample = Sample::new("launch-protected-link");
    fs::create_dir_all(sample.root().join(".git")).expect("a repository");
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");
    let beside = Sample::new("launch-protected-link-beside");
    let link = beside.root().join("checkout");
    std::os::unix::fs::symlink(sample.root(), &link).expect("a link");
    let named = link.join(".git").join("profile");
    let real = fs::canonicalize(sample.root().join(".git"))
        .expect("resolved")
        .join("profile");

    let child = policy(&conversation, Some(&named), &[], conversation.limits());

    for path in [&named, &real] {
        assert!(
            child.as_ref().map_or(true, |child| !child
                .permits_path(&path.join("auth.json"), SandboxFilesystemAccess::ReadWrite)),
            "{} was made writable",
            path.display()
        );
    }
}

/// The row a signed catalog entry gives the agent keyed `peer`: it must answer
/// initializing as `fake-peer` at 1.0.0, signs in itself with `login`, is
/// started with `acp --stdio`, and reaches `api.example.com`.
pub(crate) fn cataloged() -> CatalogRow {
    cataloged_with(Some(serde_json::json!({
        "text": "The peer keeps nothing it is sent.",
        "source": "https://example.com/peer/data",
        "date": "24 Oct 2026",
        "origins": [],
    })))
}

/// [`cataloged`], its entry carrying `statement` as its statement of content
/// use where one is given.
fn cataloged_with(statement: Option<serde_json::Value>) -> CatalogRow {
    let file =
        serde_json::json!({"path": "peer", "size": 1, "sha256": "b".repeat(64), "exec": true});
    let mut entry = serde_json::json!({
        "key": "peer",
        "registry_id": "fake-peer",
        "version": "1.0.0",
        "kind": "binary",
        "license": "MIT",
        "review": "https://example.com/review/1",
        "admits": {"name": "fake-peer", "version": "1.0.0"},
        "reach": ["api.example.com"],
        "auth_methods": [{"id": "login", "name": "Log in", "kind": "agent"}],
        "targets": [{
            "platform": "any",
            "cmd": "peer",
            "args": ["acp", "--stdio"],
            "artifacts": [{
                "url": "https://example.com/peer.zip",
                "sha256": "a".repeat(64),
                "size": 1,
                "format": "zip",
                "unpacked": 1,
                "decompressed": 1,
                "files": [file],
            }],
        }],
    });
    if let Some(statement) = statement {
        entry
            .as_object_mut()
            .expect("an entry is an object")
            .insert("content_use".into(), statement);
    }
    let catalog = serde_json::json!({
        "schema": 1,
        "serial": 1,
        "revoked": [],
        "entries": [entry],
    });
    let signed = fixture::signed(catalog.to_string().as_bytes());
    let catalog = Catalog::verify(
        &signed.index,
        &signed.signature,
        &signed.keys,
        Held(0),
        std::time::SystemTime::now(),
    )
    .expect("the catalog is believed");
    let entry = catalog.entries().first().expect("an entry");
    CatalogRow::new(entry, entry.targets.first().expect("a target")).expect("a row")
}

/// A consent that has said yes to the catalog agent keyed `peer`, whose route
/// is the hold its entry's statement gives.
fn peer_granted() -> Consent {
    let hold = cataloged().warned().cloned().expect("a statement");
    let consent = Consent::new(Routes::production().cataloging([hold]));
    consent.give("agent:peer");
    consent
}

/// The launcher starts a catalog agent's row as its entry reviewed it: with
/// its target's arguments, under its own route's yes, and reads it signed in
/// only where it answers initializing as the agent its entry names.
#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_row_is_launched_as_its_entry_reviewed_it() {
    let key = AgentKey::new("peer").expect("a key");
    let registry = Registry::new(&[]).cataloging(Box::leak(Box::new([cataloged()])));
    for (calls_itself, signed_in) in [("fake-peer", true), ("another-agent", false)] {
        let sample = Sample::new(&format!("launch-catalog-{calls_itself}"));
        let peer = installed(
            &sample,
            &Script::of([Reply::new()
                .hear(1)
                .say(acp::initialized(calls_itself, "1.0.0", "login"))
                .hear(1)
                .say(acp::opened(1))]),
        );
        let instance = ExternalAgent::unwritten(
            "work",
            Driver::Catalog,
            &peer.to_string_lossy(),
            None,
            Some(&key),
        )
        .expect("a catalog instance");
        let agent = Agent::configured(&instance, &registry, |_| None).expect("its catalog row");
        let consent = peer_granted();
        let policy = conversation(&sample);
        let state = sample.state();
        let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
        let mut dialect = agent.row().dialect(Mode::Ask, None, None).expect("asking");
        let control = Control::new().expect("a control");

        let read = launcher
            .account(&agent, &control, dialect.as_mut(), Account::Status, &|_| {})
            .await
            .expect("started");

        assert_eq!(
            matches!(read, Reading::Signed(Authentication::SignedIn)),
            signed_in,
            "{calls_itself}: {read:?}"
        );
        assert_eq!(
            fake_peer::argued(peer.parent().expect("a directory")),
            vec![vec!["acp".to_owned(), "--stdio".to_owned()]],
            "{calls_itself}"
        );
    }
}

/// What the instance `work`, run as the catalog agent keyed `peer` by the
/// program at `executable`, is, over a registry listing `row`.
fn peer_agent(row: CatalogRow, executable: &Path) -> Agent {
    let key = AgentKey::new("peer").expect("a key");
    let registry: &'static Registry = Box::leak(Box::new(
        Registry::new(&[]).cataloging(Box::leak(Box::new([row]))),
    ));
    let instance = ExternalAgent::unwritten(
        "work",
        Driver::Catalog,
        &executable.to_string_lossy(),
        None,
        Some(&key),
    )
    .expect("a catalog instance");
    Agent::configured(&instance, registry, |_| None).expect("its catalog row")
}

/// An entry without a statement of content use is listed and never launched,
/// whatever yes its route has been given: the launch is refused with the
/// reason, for a turn and for an account step alike, and nothing is started.
#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_agent_without_a_statement_is_never_launched() {
    let sample = Sample::new("launch-catalog-unstated");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let agent = peer_agent(cataloged_with(None), &peer);
    let consent = Consent::new(Routes::production());
    consent.give("agent:peer");
    let policy = conversation(&sample);
    let state = sample.root();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let control = Control::new().expect("a control");
    let mut dialect = agent.row().dialect(Mode::Ask, None, None).expect("asking");

    let turn = launcher
        .launch(&agent, &control, dialect.as_mut(), None)
        .await;
    let status = launcher
        .account(&agent, &control, dialect.as_mut(), Account::Status, &|_| {})
        .await;

    for refused in [turn.err(), status.err()] {
        assert!(
            matches!(
                &refused,
                Some(Unlaunched::Unstated {
                    why: Unstated::Missing,
                    ..
                })
            ),
            "{refused:?}"
        );
        assert_eq!(
            refused.map(|why| why.to_string()),
            Some(
                "peer cannot be launched: its catalog entry gives no statement of how its \
                 vendor uses what is sent."
                    .to_owned()
            )
        );
    }
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 0);
}

/// An entry with a statement asks its own route before anything is started,
/// and another catalog agent's yes does not answer it.
#[tokio::test(flavor = "multi_thread")]
async fn a_catalog_agent_with_a_statement_asks_its_own_hold_before_anything_is_started() {
    let sample = Sample::new("launch-catalog-stated");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let row = cataloged();
    let hold = row.warned().cloned().expect("a statement");
    let agent = peer_agent(row, &peer);
    let consent = Consent::new(Routes::production().cataloging([hold]));
    consent.give("agent:another");
    let policy = conversation(&sample);
    let state = sample.root();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let control = Control::new().expect("a control");
    let mut dialect = agent.row().dialect(Mode::Ask, None, None).expect("asking");

    let turn = launcher
        .launch(&agent, &control, dialect.as_mut(), None)
        .await;
    let status = launcher
        .account(&agent, &control, dialect.as_mut(), Account::Status, &|_| {})
        .await;

    for refused in [turn.err(), status.err()] {
        assert!(
            matches!(
                &refused,
                Some(Unlaunched::Asks(asked))
                    if asked.route == "agent:peer"
                        && asked.warning.sentence.starts_with("The peer keeps nothing it is sent.")
            ),
            "{refused:?}"
        );
    }
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 0);
}

/// A catalog row reaches the hosts its entry's profile grants, and the child
/// of a conversation that confines its commands may reach those and no other.
#[test]
fn a_catalog_row_reaches_the_hosts_its_entry_grants() {
    let sample = Sample::new("launch-catalog-reach");
    let row = AgentRow::Catalog(cataloged());
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");

    let child =
        policy(&conversation, None, &row.reaches(), conversation.limits()).expect("a policy");

    assert_eq!(row.reaches(), ["api.example.com"]);
    let SandboxNetworkPolicy::Domains(domains) = child.network() else {
        panic!("{:?}", child.network());
    };
    assert!(domains.permits_host("api.example.com"));
    assert!(!domains.permits_host("elsewhere.example.com"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_or_a_probe_without_a_yes_starts_nothing() {
    let sample = Sample::new("launch-unheld");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let consent = Consent::new(Routes::production());
    let policy = conversation(&sample);
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let agent = agent(&registry, &peer);
    let control = Control::new().expect("a control");

    let turn = launcher.launch(&agent, &control, &mut Plain, None).await;
    let probe = launcher
        .account(&agent, &control, &mut Plain, Account::Version, &|_| {})
        .await;
    let status = launcher
        .account(&agent, &control, &mut Plain, Account::Status, &|_| {})
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
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");

    let read = launcher
        .account(
            &agent(&registry, &peer),
            &control,
            &mut Plain,
            Account::Version,
            &|_| {},
        )
        .await
        .expect("started");

    assert!(
        matches!(&read, crucible_external_harnesses::Reading::Version(version) if version.as_str() == "1.2.3"),
        "the version was not read"
    );
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_held_back_is_refused_with_its_documentation_and_nothing_internal() {
    let sample = Sample::new("launch-held-back");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let consent = granted();
    let policy = conversation(&sample);
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&HELD_BACK);
    let control = Control::new().expect("a control");

    let refused = launcher
        .launch(&agent(&registry, &peer), &control, &mut Plain, None)
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
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");
    // Looked for by name on a `PATH` with nothing on it.
    let missing = Agent::routed(&route(Path::new("plain"), None), &registry, |_| {
        Some(OsString::from(sample.root().join("empty")))
    })
    .expect("a row");
    assert!(matches!(missing.discovery(), Discovery::Missing));

    let refused = launcher
        .launch(&missing, &control, &mut Plain, None)
        .await
        .expect_err("missing");

    assert_eq!(
        refused.to_string(),
        "Not installed: no plain command on PATH and none at the configured executable. \
         Install Claude Code, then choose Check again."
    );
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 0);
}

/// The plain protocol with its account steps spoken: a step begins with its
/// own name, a `link` line is a sign-in link, and `in` ends it.
#[derive(Debug, Default)]
struct Speaking(Plain);

impl Dialect for Speaking {
    fn program(&self) -> &str {
        self.0.program()
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        self.0.request(shape)
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        self.0.open(turn)
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        self.0.heard(frame)
    }

    fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        self.0.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.0.cancel()
    }

    fn account(&mut self, step: Account, shape: Shape<'_>) -> Option<Request> {
        self.0.account(step, shape)
    }

    fn read(&self, step: Account, ran: &Ran) -> Reading {
        self.0.read(step, ran)
    }

    fn spoken(&mut self, step: Account) -> Option<Vec<String>> {
        Some(vec![format!("{step:?}")])
    }

    fn told(&mut self, _step: Account, frame: &str) -> Result<Told, Failure> {
        Ok(match (frame.split_once(' '), frame) {
            (Some(("link", link)), _) => Told::Link(Text::new(link)),
            (_, "in") => Told::Done(Reading::Signed(Authentication::SignedIn)),
            _ => Told::Nothing,
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_spoken_sign_in_hands_its_link_on_and_reads_its_end() {
    let sample = Sample::new("launch-spoken");
    let peer = installed(
        &sample,
        &Script::of([Reply::new()
            .hear(1)
            .say("link https://sign.in/here")
            .say("in")]),
    );
    let consent = granted();
    let policy = conversation(&sample);
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");
    let shown = Mutex::new(Vec::new());

    let read = launcher
        .account(
            &agent(&registry, &peer),
            &control,
            &mut Speaking::default(),
            Account::SignIn,
            &|link| {
                shown
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(link.as_str().to_owned());
            },
        )
        .await
        .expect("started");

    assert!(
        matches!(read, Reading::Signed(Authentication::SignedIn)),
        "the sign-in did not read as signed in"
    );
    assert_eq!(
        shown.into_inner().unwrap_or_else(PoisonError::into_inner),
        vec!["https://sign.in/here".to_owned()]
    );
    assert_eq!(
        fake_peer::heard(peer.parent().expect("a directory")),
        vec!["SignIn".to_owned()]
    );
}

/// A profile is where the agent keeps its sign-in, so it is made private to
/// the user before anything is started, whether it was missing or open.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn a_profile_is_made_private_to_the_user_before_the_agent_starts() {
    use std::os::unix::fs::PermissionsExt;

    let sample = Sample::new("launch-private-profile");
    let peer = installed(
        &sample,
        &Script::of([Reply::lines(["1.0.0"]), Reply::lines(["1.0.0"])]),
    );
    let consent = granted();
    let policy = conversation(&sample);
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");
    let beside = Sample::new("launch-private-profile-beside");
    let missing = beside.root().join("profiles").join("missing");
    let open = beside.root().join("open");
    fs::create_dir_all(&open).expect("made");
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).expect("opened");

    for profile in [&missing, &open] {
        let agent =
            Agent::routed(&route(&peer, Some(profile)), &registry, |_| None).expect("a row");
        launcher
            .account(&agent, &control, &mut Plain, Account::Version, &|_| {})
            .await
            .expect("started");
        let mode = fs::metadata(profile).expect("there").permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{}", profile.display());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_the_conversation_protects_is_refused_before_it_is_made() {
    let sample = Sample::new("launch-protected-profile");
    fs::create_dir_all(sample.root().join(".git")).expect("a repository");
    let peer = installed(&sample, &Script::of([Reply::lines(["1.0.0"])]));
    let consent = granted();
    let policy = conversation(&sample);
    let state = sample.state();
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent, &state);
    let registry = Registry::new(&SHIPPED);
    let control = Control::new().expect("a control");
    let inside = sample.root().join(".git").join("profile");
    let agent = Agent::routed(&route(&peer, Some(&inside)), &registry, |_| None).expect("a row");

    let refused = launcher
        .account(&agent, &control, &mut Plain, Account::Version, &|_| {})
        .await;

    assert!(
        matches!(refused, Err(Unlaunched::Policy(_))),
        "the profile under .git was not refused by the policy"
    );
    assert!(!inside.exists(), "a profile under .git was made");
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
    // where it names a peer's own directory from here. That peer is put beside
    // this test's executable, which a relative path can reach where a
    // temporary directory on another drive could not.
    let beside = Beside::new();
    let near = fake_peer::install(&beside.0, "plain", &Script::of([])).expect("installed");
    let here =
        fs::canonicalize(std::env::current_dir().expect("a working directory")).expect("resolved");
    let toward = relative(&here, &beside.0).expect("one root");
    assert!(toward.is_relative() && toward.join(near.file_name().expect("a name")).is_file());
    let relative = discover("plain", |_| Some(OsString::from(&toward)));

    assert!(matches!(by_name, Discovery::Found(found) if found.path() == peer));
    assert!(matches!(written, Discovery::Found(found) if found.path() == peer));
    assert!(matches!(relative, Discovery::Missing));
}

/// The model a route names is the one every start of its agent is given, and
/// a route naming none starts it on its provider's default, each at the
/// model's default rung.
#[test]
fn an_agent_is_started_with_the_model_its_route_names() {
    for (model, given) in [
        (Some("opus"), Some("opus")),
        (None, Some("claude-opus-5-5")),
    ] {
        let mut named = route(Path::new("/bin/claude"), None);
        if let Route::External { model: at, .. } = &mut named {
            *at = model.map(Into::into);
        }
        let agent =
            Agent::routed(&named, &Registry::new(&SHIPPED), |_| None).expect("Claude Code's row");
        let shape = agent.shape(None, Path::new("/work"));
        assert_eq!((shape.model, shape.effort), (given, Some("medium")));
    }
}

/// A session a catalog agent ran is bound to its key as well as its driver,
/// and the route read back from the binding is the driver it was bound with;
/// a built-in driver's binding is its word alone, as it always was.
#[test]
fn a_catalog_agent_s_session_is_bound_to_its_key() {
    let goose = AgentKey::new("goose").expect("an agent key");
    assert_eq!(bound(Driver::Catalog, Some(&goose)), "catalog:goose");
    assert_eq!(bound(Driver::Claude, None), "claude");
    assert_eq!(bound(Driver::Antigravity, None), "antigravity");
    for driver in Driver::ALL {
        let agent = (driver == Driver::Catalog).then_some(&goose);
        assert_eq!(
            bound_driver(&bound(driver, agent)),
            Some(driver),
            "{driver:?}"
        );
    }
    for unbound in [
        "catalog:",
        "catalog:Goose",
        "claude:goose",
        "native",
        "plain",
    ] {
        assert_eq!(bound_driver(unbound), None, "{unbound}");
    }
}

/// A catalog agent chooses its own model, so a route of one starts it on
/// none, whatever its entry names, and it is the catalog agent the route
/// serves rather than a provider.
#[test]
fn a_catalog_agent_is_started_on_no_model() {
    let goose = AgentKey::new("goose").expect("an agent key");
    let catalog = Serves::Agent(goose);
    assert_eq!(
        started(Driver::Catalog, &catalog, Some("opus"), Some(Effort::High)),
        (None, None)
    );
    assert_eq!(
        started(
            Driver::Claude,
            &Serves::Provider("anthropic"),
            Some("opus"),
            None
        )
        .0
        .as_deref(),
        Some("opus")
    );
}

/// An agent taken from its configuration entry, as a reopened session's is,
/// is started with the model the entry names.
#[test]
fn an_agent_from_its_entry_is_started_with_the_model_it_names() {
    let sample = Sample::new("launching-entry-model");
    let settings = sample.user(
        r#"{"externalAgents": {"work": {"driver": "claude", "executable": "claude", "model": "opus"}}}"#,
    );
    let entry = settings.external_agents().pop().expect("the entry");
    let agent = Agent::configured(&entry, &Registry::new(&SHIPPED), |_| None).expect("its row");

    assert_eq!(agent.model(), Some("opus"));
    assert_eq!(agent.shape(None, Path::new("/work")).model, Some("opus"));
}

/// What the account steps of the agents [`SHIPPED`] holds are taken against.
static STEPPED: Registry = Registry::new(&SHIPPED);

/// The `externalAgents` entry `work`, Claude Code's program at `executable`.
fn entry(sample: &Sample, executable: &Path) -> ExternalAgent {
    let document = serde_json::json!({
        "externalAgents": {
            "work": {"driver": "claude", "executable": executable.to_string_lossy()},
        },
    });
    let settings = sample.user(&document.to_string());
    settings.external_agents().pop().expect("the entry")
}

/// The steps of the agents [`STEPPED`] holds, each started under `sandbox`.
fn steps(
    sandbox: Arc<dyn SandboxService>,
    policy: &SandboxPolicy,
    consent: &Consent,
    sample: &Sample,
) -> Steps {
    Steps::new(
        Launcher::new(sandbox, Handle::current(), policy, consent, &sample.state()),
        Arc::new(SandboxEnablement::new(false, false)),
        &STEPPED,
        None,
    )
}

/// Waits until the peer installed beside `peer` has claimed `want` starts,
/// which a slow process start may not have done when its launch returns, and
/// fails the test if it has not within a bound. A start claims the next reply
/// as its process reads the counter, so a step started before then could take
/// the run's.
async fn played_reaches(peer: &Path, want: usize) {
    let here = peer.parent().expect("a directory");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while fake_peer::played(here) < want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the peer's counter never reached {want}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// A run that could not be confirmed stopped may still be running, so no
/// account step of its entry is started beside it either: the step reads as
/// unreadable, saying what a run launched beside it is refused with, and the
/// agent's program is not started again.
#[tokio::test(flavor = "multi_thread")]
async fn no_account_step_starts_beside_a_run_not_confirmed_stopped() {
    let sample = Sample::new("launch-step-beside-unstopped");
    let peer = installed(
        &sample,
        &Script::of([Reply::new().hear_out(), Reply::lines(["in"])]),
    );
    let consent = granted();
    let policy = conversation(&sample);
    let steps = steps(Arc::new(Stubborn(local())), &policy, &consent, &sample);
    let entry = entry(&sample, &peer);
    let (agent, _) = steps.onto(&entry, Mode::Ask).expect("startable");
    let runs = steps.runs(agent, entry.name());
    let run = runs
        .launch(Mode::Ask, None, None, None)
        .await
        .expect("started");
    played_reaches(&peer, 1).await;
    assert!(run.close().await.is_err(), "the stop was confirmed");
    let beside = match runs.launch(Mode::Ask, None, None, None).await {
        Ok(second) => {
            let _closed = second.close().await;
            None
        }
        Err(failure) => Some(failure.reason().as_str().to_owned()),
    };

    let read = steps.account(&entry, Account::Status).await;

    assert_eq!(beside, Some(Refusal::Unstopped.to_string()));
    assert_eq!(
        read,
        Reading::Unreadable(Failure::new(&Refusal::Unstopped.to_string()))
    );
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 1);
}

/// An account step takes no part in the one-at-a-time bound of its entry's
/// runs, so a status check beside a live run whose stop is not in doubt runs
/// and reads what the program says, rather than finding the agent busy.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_step_runs_beside_a_live_run_of_its_entry() {
    let sample = Sample::new("launch-step-beside-live");
    let peer = installed(
        &sample,
        &Script::of([Reply::new().hear(1), Reply::lines(["in"])]),
    );
    let consent = granted();
    let policy = conversation(&sample);
    let steps = steps(local(), &policy, &consent, &sample);
    let entry = entry(&sample, &peer);
    let (agent, _) = steps.onto(&entry, Mode::Ask).expect("startable");
    let run = steps
        .runs(agent, entry.name())
        .launch(Mode::Ask, None, None, None)
        .await
        .expect("started");
    played_reaches(&peer, 1).await;

    let read = steps.account(&entry, Account::Status).await;
    run.close().await.expect("closed");

    assert_eq!(read, Reading::Signed(Authentication::SignedIn));
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 2);
}

/// The runs of one route are one agent's: none is started beside another
/// still held, which is what keeps one that could not be confirmed stopped
/// from having another started beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_route_starts_no_run_beside_one_it_still_holds() {
    let sample = Sample::new("launch-runs-one");
    let peer = installed(
        &sample,
        &Script::of([Reply::new().hear(1), Reply::new().hear(1)]),
    );
    let consent = granted();
    let policy = conversation(&sample);
    let registry = Registry::new(&SHIPPED);
    let runs = Runs::new(
        Launcher::new(
            local(),
            Handle::current(),
            &policy,
            &consent,
            &sample.state(),
        ),
        Arc::new(SandboxEnablement::new(false, false)),
        agent(&registry, &peer),
        "work",
        &Controls::default(),
    );

    let first = runs
        .launch(Mode::Ask, None, None, None)
        .await
        .expect("started");
    let refused = match runs.launch(Mode::Ask, None, None, None).await {
        Ok(second) => {
            let _closed = second.close().await;
            None
        }
        Err(failure) => Some(failure.reason().as_str().to_owned()),
    };
    first.close().await.expect("closed");

    assert_eq!(refused.as_deref(), Some("this agent is already running"));
    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 1);
}

/// The route a conversation is opened on at startup and the steps beside it
/// are one instance's: a run of that route whose stop could not be confirmed
/// is held against a run launched later through [`Steps::runs`], as a resume
/// onto or back to the instance launches one, so no second process starts
/// beside the one that may still be running.
#[tokio::test(flavor = "multi_thread")]
async fn the_route_opened_at_startup_holds_against_a_later_run_of_its_instance() {
    let sample = Sample::new("launch-startup-route-shares");
    let peer = installed(
        &sample,
        &Script::of([Reply::new().hear_out(), Reply::lines(["in"])]),
    );
    let consent = granted();
    let policy = conversation(&sample);
    let registry = Registry::new(&SHIPPED);
    let entry = entry(&sample, &peer);
    let (runs, steps) = first_runs(
        Some((
            Launcher::new(
                Arc::new(Stubborn(local())),
                Handle::current(),
                &policy,
                &consent,
                &sample.state(),
            ),
            agent(&registry, &peer),
            entry.name(),
        )),
        Some((
            Launcher::new(
                Arc::new(Stubborn(local())),
                Handle::current(),
                &policy,
                &consent,
                &sample.state(),
            ),
            &STEPPED,
            None,
        )),
        Arc::new(SandboxEnablement::new(false, false)),
    );
    let runs = runs.expect("the route's runs");
    let steps = steps.expect("the steps");
    let run = runs
        .launch(Mode::Ask, None, None, None)
        .await
        .expect("started");
    played_reaches(&peer, 1).await;
    assert!(run.close().await.is_err(), "the stop was confirmed");
    let (agent, _) = steps.onto(&entry, Mode::Ask).expect("startable");

    let later = match steps
        .runs(agent, entry.name())
        .launch(Mode::Ask, None, None, None)
        .await
    {
        Ok(second) => {
            let _closed = second.close().await;
            None
        }
        Err(failure) => Some(failure.reason().as_str().to_owned()),
    };

    assert_eq!(fake_peer::played(peer.parent().expect("a directory")), 1);
    assert_eq!(later, Some(Refusal::Unstopped.to_string()));
}

/// A sandbox whose processes run as the local one's do, but which can never
/// confirm that one of them stopped.
pub(crate) struct Stubborn(pub(crate) Arc<dyn SandboxService>);

impl SandboxService for Stubborn {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.0.probe()
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let session: Box<dyn SandboxSession> =
                Box::new(StubbornSession(self.0.prepare(request).await?));
            Ok(session)
        })
    }
}

/// Its session.
struct StubbornSession(Box<dyn SandboxSession>);

impl SandboxSession for StubbornSession {
    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        self.0.materialize()
    }

    fn stage<'a>(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let launch: Box<dyn SandboxLaunch> =
                Box::new(StubbornLaunch(self.0.stage(command).await?));
            Ok(launch)
        })
    }
}

/// Its staged launch.
struct StubbornLaunch(Box<dyn SandboxLaunch>);

impl SandboxLaunch for StubbornLaunch {
    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let process: Box<dyn SandboxProcess> =
                Box::new(StubbornProcess(self.0.release().await?));
            Ok(process)
        })
    }
}

/// Its process, which never reads as ended and whose stop always fails.
struct StubbornProcess(Box<dyn SandboxProcess>);

impl SandboxProcess for StubbornProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.0.take_stdin()
    }

    fn take_async_stdin(&mut self) -> Option<Box<dyn SandboxInput>> {
        self.0.take_async_stdin()
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.0.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.0.take_stderr()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        Ok(None)
    }

    fn stop(&mut self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(async { Err(std::io::Error::other("the scope did not answer")) })
    }

    fn inspection(&self) -> &SandboxInspection {
        self.0.inspection()
    }

    fn usage(&self) -> SandboxUsage {
        self.0.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.0.violation()
    }
}

/// The built-in Antigravity row alone.
static ANTIGRAVITY_ONLY: [Row; 1] = [ANTIGRAVITY];

/// The registry holding it.
static ANTIGRAVITY_REGISTRY: Registry = Registry::new(&ANTIGRAVITY_ONLY);

/// The catalog's entry for `version` of Antigravity.
fn antigravity_release(version: &str) -> serde_json::Value {
    release_of(
        "antigravity",
        version,
        (
            &format!("https://dl.google.com/agy-extensions/releases/linux/agy-{version}.zip"),
            &"a".repeat(64),
            1000,
        ),
        [("agy_acp_server.par", 700), ("localharness_external", 300)],
        &[],
    )
}

/// What the fake peer says to Antigravity's status check once it is
/// initialized as `version`: it initializes, then opens a session.
fn antigravity_answering(version: &str) -> Reply {
    Reply::new()
        .hear(1)
        .say(acp::initialized(
            "antigravity-acp",
            version,
            "oauth-personal",
        ))
        .hear(1)
        .say(acp::opened(1))
}

/// The status of the Antigravity the peer playing `script` is, as the steps of
/// the registry holding its row read it, with the trust state `sample` holds.
async fn antigravity_status(sample: &Sample, script: &Script) -> (Reading, usize) {
    let here = sample.root().join("peer");
    let program = fake_peer::install(&here, "antigravity", script).expect("the fake peer");
    let consent = Consent::new(Routes::production());
    consent.give("agent:antigravity");
    let policy = conversation(sample);
    let state = sample.state();
    let steps = Steps::new(
        Launcher::new(local(), Handle::current(), &policy, &consent, &state),
        Arc::new(SandboxEnablement::new(false, false)),
        &ANTIGRAVITY_REGISTRY,
        None,
    );
    let entry = ExternalAgent::unwritten(
        "space",
        Driver::Antigravity,
        &program.to_string_lossy(),
        None,
        None,
    )
    .expect("an instance of a built-in driver");

    let read = steps.account(&entry, Account::Status).await;

    (read, fake_peer::played(&here))
}

/// The version the agent answers as is admitted when the trust state lists
/// it, and is sent nothing more when it does not: a release the catalog
/// reviewed since this build was made is run without a release of crucible.
#[tokio::test(flavor = "multi_thread")]
async fn antigravity_is_admitted_as_a_version_the_catalog_lists_and_no_other() {
    let unlisted = Sample::new("launch-antigravity-unlisted");
    let (refused, heard) = antigravity_status(
        &unlisted,
        &Script::of([antigravity_answering("1.4.1").hear_out()]),
    )
    .await;
    let listed = Sample::new("launch-antigravity-listed");
    trusting(&listed, 2, &[antigravity_release("1.4.1")]);
    let (admitted, _) =
        antigravity_status(&listed, &Script::of([antigravity_answering("1.4.1")])).await;
    let snapshot = Sample::new("launch-antigravity-snapshot");
    let (older, _) = antigravity_status(
        &snapshot,
        &Script::of([antigravity_answering(snapshot_version().unwrap_or("1.3.0"))]),
    )
    .await;

    let Reading::Unreadable(failure) = refused else {
        panic!("{refused:?}");
    };
    assert!(failure.reason().as_str().contains("1.4.1"), "{failure:?}");
    assert_eq!(heard, 1, "nothing more was sent");
    assert!(matches!(admitted, Reading::Signed(_)), "{admitted:?}");
    if snapshot_version().is_some() {
        assert!(matches!(older, Reading::Signed(_)), "{older:?}");
    }
}

/// What a missing Antigravity is told to do is what the trust state offers:
/// the command that installs the release, and where the state cannot be used,
/// that it is to be repaired.
#[test]
fn a_missing_antigravity_is_told_what_the_trust_state_offers() {
    let sample = Sample::new("launch-antigravity-missing");
    let consent = Consent::new(Routes::production());
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: "no-such-antigravity-program".into(),
        profile: None,
        model: None,
        effort: None,
        enabled: true,
    };
    let registry = &ANTIGRAVITY_REGISTRY;
    let missing = Agent::routed(&route, registry, |_| {
        Some(OsString::from(sample.root().join("empty")))
    })
    .expect("a row");
    let said = |trust: &Trust| match missing.program(&consent, trust) {
        Err(Unlaunched::NotInstalled([_, next])) => next,
        other => panic!("{other:?}"),
    };

    let offered = said(&trusting(&sample, 2, &[antigravity_release("1.4.1")]));
    let file = trust_of(&sample).file();
    std::fs::create_dir_all(file.parent().expect("a directory")).expect("a directory");
    std::fs::write(&file, "not a trust state").expect("the file");
    let unusable = said(&trust_of(&sample));

    assert!(
        offered.contains("crucible agents install antigravity"),
        "{offered}"
    );
    assert!(
        unusable.contains(&file.display().to_string()) && unusable.contains("Repair or remove"),
        "{unusable}"
    );
}

/// A run is admitted as the releases on offer when it is launched, not when
/// its agent was selected: a catalog taken in since is the one it is held to.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_admitted_as_the_releases_on_offer_when_it_is_launched() {
    let sample = Sample::new("launch-antigravity-fresh");
    let consent = Consent::new(Routes::production());
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: "antigravity".into(),
        profile: None,
        model: None,
        effort: None,
        enabled: true,
    };
    let agent = Agent::routed(&route, &ANTIGRAVITY_REGISTRY, |_| None).expect("a row");
    let policy = conversation(&sample);
    let state = sample.state();
    let runs = Runs::new(
        Launcher::new(local(), Handle::current(), &policy, &consent, &state),
        Arc::new(SandboxEnablement::new(false, false)),
        agent,
        "space",
        &Controls::default(),
    );
    let answers = |version: &str| {
        let mut dialect = runs.dialect(Mode::Ask).expect("asking");
        let _ = dialect.request(Shape {
            workspace: Some(Path::new("/work")),
            ..Shape::default()
        });
        let _ = dialect
            .open(&TurnRequest::new(TurnId::FIRST, "hello"))
            .expect("a first request");
        let frame = acp::initialized("antigravity-acp", version, "oauth-personal");
        dialect.heard(&frame).expect("an answer")
    };

    let before = answers("1.4.1");
    trusting(&sample, 2, &[antigravity_release("1.4.1")]);
    let after = answers("1.4.1");

    assert!(matches!(before.as_slice(), [Act::Fail(_)]), "{before:?}");
    assert!(matches!(after.as_slice(), [Act::Send(_)]), "{after:?}");
}

/// A start of an Antigravity whose trust state cannot be used says so, naming
/// the file to repair or remove, rather than that no release is offered.
#[tokio::test(flavor = "multi_thread")]
async fn a_found_antigravity_with_an_unusable_trust_state_is_refused_naming_the_file() {
    let sample = Sample::new("launch-antigravity-untrusted");
    let file = trust_of(&sample).file();
    std::fs::create_dir_all(file.parent().expect("a directory")).expect("a directory");
    std::fs::write(&file, "not a trust state").expect("the file");

    let (read, heard) =
        antigravity_status(&sample, &Script::of([antigravity_answering("1.3.0")])).await;

    let Reading::Unreadable(failure) = read else {
        panic!("{read:?}");
    };
    let said = failure.reason().as_str();
    assert!(said.contains(&file.display().to_string()), "{said}");
    assert!(said.contains("Repair or remove"), "{said}");
    assert_eq!(heard, 0, "nothing was started");
}

/// The wire a run is made with follows the release its program is: the
/// version directory it is kept in, not the newest one on offer.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_is_started_with_the_arguments_of_the_release_its_program_is() {
    let sample = Sample::new("launch-antigravity-directory");
    let release = |version: &str, args: &[&str]| {
        release_of(
            "antigravity",
            version,
            (
                &format!("https://dl.google.com/agy-extensions/releases/linux/agy-{version}.zip"),
                &"a".repeat(64),
                1000,
            ),
            [("agy_acp_server.par", 700), ("localharness_external", 300)],
            args,
        )
    };
    trusting(
        &sample,
        2,
        &[release("1.4.1", &["--new"]), release("1.3.0", &["--old"])],
    );
    let consent = Consent::new(Routes::production());
    let policy = conversation(&sample);
    let state = sample.state();
    let arguments = |version: &str| {
        let program = sample
            .root()
            .join("programs/antigravity")
            .join(version)
            .join("agy_acp_server.par");
        std::fs::create_dir_all(program.parent().expect("a directory")).expect("a directory");
        std::fs::write(&program, "#!/bin/sh\n").expect("the program");
        #[cfg(unix)]
        std::fs::set_permissions(
            &program,
            std::os::unix::fs::PermissionsExt::from_mode(0o755),
        )
        .expect("executable");
        let route = Route::External {
            serves: Serves::Provider("google"),
            instance: "space".into(),
            driver: Driver::Antigravity,
            executable: program.to_string_lossy().into(),
            profile: None,
            model: None,
            effort: None,
            enabled: true,
        };
        let agent = Agent::routed(&route, &ANTIGRAVITY_REGISTRY, |_| None).expect("a row");
        let runs = Runs::new(
            Launcher::new(local(), Handle::current(), &policy, &consent, &state),
            Arc::new(SandboxEnablement::new(false, false)),
            agent,
            "space",
            &Controls::default(),
        );
        runs.dialect(Mode::Ask)
            .expect("asking")
            .request(Shape::default())
            .arguments
    };

    assert_eq!(arguments("1.4.1"), ["--new"]);
    assert_eq!(arguments("1.3.0"), ["--old"]);
}

/// The variable an entry turns its agent's updater off with is set, in place
/// of any value of the same name the request already carried.
#[test]
fn the_updater_off_variable_replaces_any_value_of_its_name() {
    let request = Request {
        arguments: Vec::new(),
        environment: vec![
            ("GOOSE_NO_UPDATE".to_owned(), "0".to_owned()),
            ("PATH".to_owned(), "/usr/bin".to_owned()),
        ],
        shown: Vec::new(),
    };
    let off = Variable {
        name: "GOOSE_NO_UPDATE".into(),
        value: "1".into(),
    };

    let started = updating_off(request, Some(&off));

    assert_eq!(
        started.environment,
        [
            ("PATH".to_owned(), "/usr/bin".to_owned()),
            ("GOOSE_NO_UPDATE".to_owned(), "1".to_owned()),
        ]
    );
}

/// A version root inside a writable root is read-only at and beneath it, and
/// a rule that would let something beneath it be written is gone.
#[test]
fn a_version_root_is_read_only_inside_a_writable_root() {
    let sample = Sample::new("launch-read-only-root");
    let conversation = SandboxPolicy::standard(&sample.workspace()).expect("a policy");
    let root = sample
        .workspace()
        .root()
        .join("programs")
        .join("goose")
        .join("1.0.0");
    let profile = root.join("profile");
    let child =
        policy(&conversation, Some(&profile), &[], conversation.limits()).expect("a policy");

    let narrowed = read_only(&child, &root).expect("a policy");

    assert!(child.permits_path(&root.join("goose"), SandboxFilesystemAccess::ReadWrite));
    assert!(narrowed.permits_path(&root.join("goose"), SandboxFilesystemAccess::ReadOnly));
    assert!(!narrowed.permits_path(&root.join("goose"), SandboxFilesystemAccess::ReadWrite));
    assert!(!narrowed.permits_path(&profile, SandboxFilesystemAccess::ReadWrite));
    assert!(narrowed.permits_path(
        &sample.workspace().root().join("src"),
        SandboxFilesystemAccess::ReadWrite
    ));
}
