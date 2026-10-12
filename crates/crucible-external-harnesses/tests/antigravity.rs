//! Antigravity, started as the fake peer speaking the Agent Client Protocol
//! over a real sandbox: every conformance case, and each account operation in
//! the words the user is shown.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{SystemTime, UNIX_EPOCH};

use crucible_execution::{Authentication, Text};
use crucible_external_harnesses::antigravity::{Antigravity, Ended, Manifest};
use crucible_external_harnesses::conformance::{self, CASES};
use crucible_external_harnesses::fake_peer::scripts::acp;
use crucible_external_harnesses::fake_peer::{self, Reply, Script};
use crucible_external_harnesses::{
    Account, Control, Dialect, Discovery, Held, Host, Launch, PREPARING, Reading, Shape,
};
use crucible_sandbox::{
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits, SandboxService,
};
use crucible_sandbox_local::LocalSandbox;
use tokio::runtime::Handle;

/// The route this file's launches are held under.
const ROUTE: &str = "agent:antigravity";

/// A sign-in link as the agent prints it.
const SIGN_IN: &str = "https://accounts.google.com/o/oauth2/v2/auth?client_id=c&redirect_uri=http%3A%2F%2F127.0.0.1%3A40123%2F&response_type=code&state=s";

/// A directory of the test's own, removed when it is dropped.
struct Owned(PathBuf);

impl Owned {
    // A test with nowhere to write has nothing to report.
    #[allow(clippy::expect_used)]
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let at = std::env::temp_dir().join(format!(
            "crucible-antigravity-{name}-{unique}-{}",
            std::process::id()
        ));
        fs::create_dir_all(&at).expect("a directory");
        Self(fs::canonicalize(&at).expect("a canonical directory"))
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn local() -> Arc<dyn SandboxService> {
    Arc::new(LocalSandbox::new().watching_on(Handle::current()))
}

/// The reviewed release's answer to initializing.
fn reviewed() -> String {
    acp::initialized(
        Manifest::REVIEWED.name,
        Manifest::REVIEWED.versions.first().copied().unwrap_or_default(),
        Manifest::REVIEWED.sign_in,
    )
}

/// `here` as the only root, unconfined.
#[allow(clippy::expect_used)]
fn policy(here: &Path) -> SandboxPolicy {
    let root = SandboxFilesystemRule::new(
        here,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .expect("an absolute root");
    SandboxPolicy::new(
        true,
        [root],
        here,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::confining(),
    )
    .expect("a valid policy")
    .with_enabled(false)
}

/// Speaks each of `steps` in turn over the fake peer installed in `here`
/// playing `script`, one launch each, in `profile`; returns what each read
/// and every link shown.
#[allow(clippy::expect_used)]
async fn spoken(
    here: &Path,
    profile: &Path,
    script: &Script,
    steps: &[Account],
) -> (Vec<Reading>, Vec<String>) {
    let path = fake_peer::install(here, "antigravity", script).expect("the fake peer is built");
    let program = match Discovery::of(Some(&path)) {
        Discovery::Found(program) => Some(program),
        Discovery::Missing | Discovery::NotExecutable(_) => None,
    }
    .expect("the installed peer is found");
    let control = Control::new().expect("a control");
    let shown = Mutex::new(Vec::new());
    let mut read = Vec::new();
    for step in steps {
        let mut dialect = Antigravity::default();
        let shape = Shape {
            profile: Some(profile),
            workspace: Some(here),
            ..Shape::default()
        };
        let request = dialect.account(*step, shape).expect("an account step");
        let frames = dialect.spoken(*step).expect("spoken");
        let launch = Launch::new(program.clone(), policy(here), Held::granted(ROUTE)).asking(request);
        let host = Host::launch(local(), Handle::current(), &control, launch)
            .await
            .expect("started");
        let show = |link: Text| {
            shown
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(link.as_str().to_owned());
        };
        read.push(host.speak(&mut dialect, *step, &frames, &show, PREPARING).await);
    }
    let shown = shown.into_inner().unwrap_or_else(PoisonError::into_inner);
    (read, shown)
}

/// A reply that answers initializing as the reviewed release, then answers
/// the step's one request with `answer`.
fn answering(answer: String) -> Reply {
    Reply::new().hear(1).say(reviewed()).hear(1).say(answer)
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn every_conformance_case_ends_as_it_must_over_the_protocol() {
    let at = Owned::new("conformance");
    let set = acp::set(reviewed());

    let passed = conformance::run(local(), || Box::new(Antigravity::default()), &set, &at.0)
        .await
        .expect("every case passes");

    assert_eq!(passed, CASES);
}

/// The link is shown while the agent waits for the browser, and the sign-in
/// is told as done only once a status step after it reads signed in.
#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_a_status_confirms_shows_its_link_and_says_signed_in() {
    let at = Owned::new("signed-in");
    let profile = at.0.join("profile");
    let sign_in = Reply::new()
        .hear(1)
        .say(reviewed())
        .hear(1)
        .say(format!(
            "Open the following link to authenticate the ACP server: {SIGN_IN}"
        ))
        .say(acp::response(1, &serde_json::json!({})));
    let script = Script::of([sign_in, answering(acp::opened(1))]);

    let (read, shown) = spoken(&at.0, &profile, &script, &[Account::SignIn, Account::Status]).await;

    assert_eq!(shown, vec![SIGN_IN.to_owned()]);
    let [signed, status] = read.as_slice() else { panic!("{read:?}") };
    assert_eq!(signed, &Reading::Signed(Authentication::SignedIn));
    assert_eq!(Ended::signed_in(status).says(), "Signed in");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_the_status_after_it_refuses_says_could_not_verify_sign_in() {
    let at = Owned::new("unverified");
    let profile = at.0.join("profile");
    let script = Script::of([
        answering(acp::response(1, &serde_json::json!({}))),
        answering(acp::sign_in_required(1)),
    ]);

    let (read, _) = spoken(&at.0, &profile, &script, &[Account::SignIn, Account::Status]).await;

    let [_, status] = read.as_slice() else { panic!("{read:?}") };
    assert_eq!(Ended::signed_in(status).says(), "Could not verify sign-in");
}

/// Status is read from what the agent answers in its profile alone, and an
/// agent that answers opening a session by asking for a sign-in is signed
/// out, whatever another Google product's sign-in in the usual home says.
#[tokio::test(flavor = "multi_thread")]
async fn a_sign_in_kept_outside_the_profile_does_not_read_as_signed_in() {
    let at = Owned::new("ide-marker");
    let profile = at.0.join("profile");
    let script = Script::of([answering(acp::sign_in_required(1))]);

    let request = Antigravity::default().request(Shape {
        profile: Some(&profile),
        ..Shape::default()
    });
    let (read, _) = spoken(&at.0, &profile, &script, &[Account::Status]).await;

    let home = request
        .environment
        .iter()
        .find(|(name, _)| name == "GEMINI_HOME")
        .map(|(_, value)| PathBuf::from(value));
    assert_eq!(home, Some(profile));
    assert_eq!(read, vec![Reading::Signed(Authentication::SignedOut)]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_logout_the_agent_confirms_says_signed_out_of_this_agent_profile() {
    let at = Owned::new("signed-out");
    let profile = at.0.join("profile");
    let script = Script::of([answering(acp::response(1, &serde_json::json!({})))]);

    let (read, _) = spoken(&at.0, &profile, &script, &[Account::SignOut]).await;

    let [logout] = read.as_slice() else { panic!("{read:?}") };
    assert_eq!(Ended::signed_out(logout).says(), "Signed out of this agent profile");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_logout_the_agent_refuses_says_external_sign_out_unverified() {
    let at = Owned::new("sign-out-unverified");
    let profile = at.0.join("profile");
    let script = Script::of([answering(acp::refused(1, -32603, "Internal error"))]);

    let (read, _) = spoken(&at.0, &profile, &script, &[Account::SignOut]).await;

    let [logout] = read.as_slice() else { panic!("{read:?}") };
    assert_eq!(
        Ended::signed_out(logout).says(),
        "Disconnected; external sign-out unverified"
    );
}

/// A release other than the reviewed one is sent nothing past initializing.
#[tokio::test(flavor = "multi_thread")]
async fn a_release_outside_the_manifest_is_sent_nothing_more() {
    let at = Owned::new("unreviewed");
    let profile = at.0.join("profile");
    let newer = acp::initialized(Manifest::REVIEWED.name, "9.9.9", Manifest::REVIEWED.sign_in);
    let script = Script::of([Reply::new().hear(1).say(newer).hear_out()]);

    let (read, _) = spoken(&at.0, &profile, &script, &[Account::Status]).await;

    let [Reading::Unreadable(refused)] = read.as_slice() else { panic!("{read:?}") };
    assert!(refused.reason().as_str().contains("9.9.9"), "{refused:?}");
    let heard = fake_peer::heard(&at.0);
    assert_eq!(heard.len(), 1, "{heard:?}");
}
