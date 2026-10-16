use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crucible_config::Driver;
use crucible_execution::{
    Authentication, Decision, Event, Failure, Outcome, SessionRef, Sink, Text, TurnRequest,
};
use crucible_external_harnesses::fake_peer::{self, Plain, Reply, Script};
use crucible_external_harnesses::{
    Account, Act, Control, Dialect, Discovery, Ran, Reading, Request, Shape, Told,
};
use crucible_sandbox::{
    SandboxFilesystemAccess, SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits,
    SandboxService,
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
    dialect: |_| Ok(Box::new(Plain)),
    profile_key: "configDir",
    route: "agent:claude",
    docs_section: "claude-code",
    reaches: &[],
    sign_in: |_| None,
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
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
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
    let launcher = Launcher::new(local(), Handle::current(), policy, &consent);
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
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
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
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
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
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
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
    fn program(&self) -> &'static str {
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
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
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
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
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
    let launcher = Launcher::new(local(), Handle::current(), &policy, &consent);
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
