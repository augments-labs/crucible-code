//! A supervised agent, started as the fake peer over a real sandbox and spoken
//! to in a line protocol of this file's own.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_execution::{Confinement, Event, Outcome, SessionRef, Sink, TurnRequest};
use crucible_external_harnesses::conformance::{self, CASES, Set};
use crucible_external_harnesses::fake_peer::{
    self, EXHAUSTED, Plain, Reply, Script, Stream, UNWRITABLE,
};
use crucible_external_harnesses::{
    Account, Control, Dialect, Discovery, Held, Host, Launch, PREPARING, Ran, Refusal, Request,
    STOP_GRACE, Shape,
};
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxCleanup, SandboxCommand, SandboxError, SandboxFeature,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection,
    SandboxLaunch, SandboxNetworkPolicy, SandboxPolicy, SandboxRequest, SandboxResourceLimits,
    SandboxService, SandboxSession, inspection,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::TurnId;
use tokio::runtime::Handle;

/// A value long enough to be a secret, which nothing kept may carry.
const SENTINEL: &str = "sentinel-c0ffee-5ec7e7";

/// The route this file's launches are held under.
const ROUTE: &str = "agent:plain";

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
            "crucible-host-{name}-{unique}-{}",
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

/// `here` as the only root, with a sandbox required or not.
#[allow(clippy::expect_used)]
fn policy(here: &Path, required: bool) -> SandboxPolicy {
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
    .with_enabled(required)
}

fn local() -> Arc<dyn SandboxService> {
    Arc::new(LocalSandbox::new().watching_on(Handle::current()))
}

/// The fake peer installed in `here` as `program`, playing `script`, as it
/// was found.
#[allow(clippy::expect_used)]
fn found(here: &Path, program: &str, script: &Script) -> crucible_external_harnesses::Program {
    let path = fake_peer::install(here, program, script).expect("the fake peer is built");
    discovered(&path)
}

/// What discovery found at `path`, which a test installed there.
#[allow(clippy::expect_used)]
fn discovered(path: &Path) -> crucible_external_harnesses::Program {
    match Discovery::of(Some(path)) {
        Discovery::Found(program) => Some(program),
        Discovery::Missing | Discovery::NotExecutable(_) => None,
    }
    .expect("the installed peer is found")
}

/// A launch of `program` in `here` asking `request`, unconfined.
fn launch(program: crucible_external_harnesses::Program, here: &Path, request: Request) -> Launch {
    Launch::new(program, policy(here, false), Held::granted(ROUTE)).asking(request)
}

/// Runs the version step once over a fresh launch of `program`.
#[allow(clippy::expect_used)]
async fn version(
    program: &crucible_external_harnesses::Program,
    here: &Path,
    control: &Control,
) -> Ran {
    let request = Plain
        .account(Account::Version, Shape::default())
        .expect("a version step");
    Host::launch(
        local(),
        Handle::current(),
        control,
        launch(program.clone(), here, request),
    )
    .await
    .expect("started")
    .run()
    .await
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn every_conformance_case_ends_as_it_must() {
    let at = Owned::new("conformance");
    let set = Set {
        opening: 1,
        effect: "file src/lib.rs".to_owned(),
        ask: |native| format!("ask {native} read the notes"),
        answered: "end".to_owned(),
        cancelled: "cancelled".to_owned(),
    };
    let passed = conformance::run(local(), || Box::new(Plain), &set, &at.0)
        .await
        .expect("every case passes");
    assert_eq!(passed, CASES);
}

/// One install serves a session of launches, each consuming the next reply,
/// and a launch past the last reply fails with the peer's own status and
/// words rather than playing the first again.
#[tokio::test(flavor = "multi_thread")]
async fn one_install_plays_its_replies_in_order_and_refuses_past_the_last() {
    let at = Owned::new("in-order");
    let program = found(
        &at.0,
        "plain",
        &Script::of([Reply::lines(["one"]), Reply::lines(["two"])]),
    );
    let control = Control::new().expect("a control");

    let first = version(&program, &at.0, &control).await;
    let second = version(&program, &at.0, &control).await;
    let third = version(&program, &at.0, &control).await;

    assert_eq!((first.code, first.said), (Some(0), vec!["one".to_owned()]));
    assert_eq!(
        (second.code, second.said),
        (Some(0), vec!["two".to_owned()])
    );
    assert_eq!(third.code, Some(i32::from(EXHAUSTED)));
    assert!(third.said.is_empty());
    assert!(third.muttered.contains("fake peer: script exhausted"));
    assert_eq!(fake_peer::played(&at.0), 2);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_launch_whose_place_cannot_be_kept_fails_with_its_own_status() {
    let at = Owned::new("unwritable");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    // A directory where the counter goes can be neither read nor replaced.
    fs::create_dir(at.0.join("played")).expect("made");
    let control = Control::new().expect("a control");

    let ran = version(&program, &at.0, &control).await;

    assert_eq!(ran.code, Some(i32::from(UNWRITABLE)));
    assert!(ran.said.is_empty(), "reply one was not played");
    assert!(ran.muttered.contains("fake peer: counter unwritable"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_program_changed_since_it_was_selected_is_not_started() {
    let at = Owned::new("changed");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let mut bytes = fs::read(program.path()).expect("readable");
    bytes.push(0);
    fs::write(program.path(), bytes).expect("rewritten");
    let control = Control::new().expect("a control");

    let refused = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect_err("changed");

    assert!(matches!(refused, Refusal::Changed(_)), "{refused}");
    assert_eq!(fake_peer::played(&at.0), 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_second_run_of_one_agent_is_refused_while_the_first_lives() {
    let at = Owned::new("busy");
    let program = found(
        &at.0,
        "plain",
        &Script::of([Reply::new().hear_out(), Reply::lines(["two"])]),
    );
    let control = Control::new().expect("a control");
    let first = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program.clone(), &at.0, Request::default()),
    )
    .await
    .expect("started");

    let second = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect_err("one at a time");

    assert!(matches!(second, Refusal::Busy), "{second}");
    first.stop(STOP_GRACE).await;
    assert_eq!(fake_peer::played(&at.0), 1);
}

/// A sandbox that prepares sessions it can only watch.
struct Observing(SandboxBackendIdentity);

impl Observing {
    #[allow(clippy::expect_used)] // Every word the identity is given is valid.
    fn new() -> Self {
        Self(
            SandboxBackendIdentity::new(
                SandboxBackendId::new("observing").expect("a valid backend id"),
                "1",
                SandboxBackendProvenance::Compatibility,
                None,
            )
            .expect("a valid identity"),
        )
    }
}

impl SandboxService for Observing {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async move { Ok((self.0.clone(), observed())) })
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            if request.policy().enabled() {
                // Watching is not holding: a confined session is refused.
                return Err(SandboxError::Unsupported {
                    feature: SandboxFeature::Filesystem,
                });
            }
            let inspection = inspection(
                request.id(),
                self.0.clone(),
                observed(),
                request.policy(),
                request.manifest(),
                false,
                Some("this sandbox watches and does not hold"),
                SandboxCleanup::Pending,
            )?;
            let session: Box<dyn SandboxSession> = Box::new(Watched(inspection));
            Ok(session)
        })
    }
}

fn observed() -> SandboxCapabilities {
    SandboxFeature::ALL
        .into_iter()
        .fold(SandboxCapabilities::none(), |claims, feature| {
            claims.with(feature, SandboxCapability::Observed)
        })
}

/// A session nothing may start in: the launch is refused before it is.
struct Watched(SandboxInspection);

impl SandboxSession for Watched {
    fn inspection(&self) -> &SandboxInspection {
        &self.0
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async { Err(SandboxError::InvalidCommand) })
    }

    fn stage<'a>(
        self: Box<Self>,
        _command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async { Err(SandboxError::InvalidCommand) })
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_required_sandbox_that_can_only_observe_refuses_with_that_claim() {
    let at = Owned::new("observed");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let control = Control::new().expect("a control");
    let required = Launch::new(program, policy(&at.0, true), Held::granted(ROUTE));

    let refused = Host::launch(
        Arc::new(Observing::new()),
        Handle::current(),
        &control,
        required,
    )
    .await
    .expect_err("not hard-enforced");

    assert!(
        matches!(refused, Refusal::Unconfined(Confinement::Observed)),
        "{refused}"
    );
    assert_eq!(fake_peer::played(&at.0), 0);
}

/// A value of the agent's environment is a credential as far as the host
/// knows, wherever the agent echoes it.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_the_agent_echoes_reaches_no_event_and_no_failure() {
    let at = Owned::new("sentinel");
    let reply = Reply::new()
        .hear(1)
        .mutter(SENTINEL)
        .say(format!("say token {SENTINEL}"))
        .exit(3);
    let program = found(&at.0, "plain", &Script::of([reply]));
    let control = Control::new().expect("a control");
    let request = Request {
        arguments: Vec::new(),
        environment: vec![("PLAIN_TOKEN".to_owned(), SENTINEL.to_owned())],
    };
    let mut host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, request),
    )
    .await
    .expect("started");
    let sink = Kept::default();

    let failure = host
        .turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &sink,
        )
        .await
        .expect_err("it crashed");
    host.stop(STOP_GRACE).await;

    let kept = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::Delta(text) | Event::Background(text) => Some(text.as_str()),
            Event::Process(fault) => Some(fault.seen().as_str()),
            Event::Failed(failure) => Some(failure.reason().as_str()),
            _ => None,
        })
        .chain([failure.reason().as_str()])
        .collect::<Vec<_>>()
        .join("\n");
    assert!(kept.contains("[withheld]"), "{kept}");
    assert!(!kept.contains(SENTINEL), "{kept}");
}

/// The peer is installed under the name discovery looks for, with the
/// platform's suffix, and stopping the turn ends the process the peer itself
/// started.
#[tokio::test(flavor = "multi_thread")]
async fn stopping_an_unheeding_turn_ends_what_the_agent_started() {
    let at = Owned::new("grandchild");
    let reply = Reply::new().hear(1).linger().pause(Duration::from_mins(1));
    let path = fake_peer::install(&at.0, "claude", &Script::of([reply])).expect("built");
    #[cfg(windows)]
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("claude.exe")
    );
    #[cfg(not(windows))]
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("claude")
    );
    let program = discovered(&path);
    let control = Control::new().expect("a control");
    let mut host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect("started");
    let interrupting = control.clone();
    let turning = tokio::spawn(async move {
        let sink = Kept::default();
        let outcome = host
            .turn(
                &mut Plain,
                &SessionRef::new("one").expect("a name"),
                &TurnRequest::new(TurnId::FIRST, "go"),
                &sink,
            )
            .await;
        host.stop(STOP_GRACE).await;
        outcome
    });
    while fake_peer::lingering(&at.0).is_none() {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    interrupting.interrupt();
    let outcome = turning.await.expect("joined");

    assert_eq!(outcome, Ok(Outcome::Cancelled));
    let first = fake_peer::lingering(&at.0);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        fake_peer::lingering(&at.0),
        first,
        "the grandchild still beats"
    );
}

/// The peer is linked before this test is, which is what Cargo always does;
/// its stamp, not its age, is what is compared.
#[test]
fn a_peer_built_before_the_test_is_installed() {
    let at = Owned::new("older");
    let peer = Path::new(env!("CARGO_BIN_EXE_crucible-fake-peer"));
    let test = std::env::current_exe().expect("this test");
    let modified = |path: &Path| fs::metadata(path).and_then(|meta| meta.modified()).ok();
    assert!(modified(peer) <= modified(&test));

    let installed = fake_peer::install(&at.0, "plain", &Script::of([])).expect("installed");

    assert!(installed.starts_with(&at.0));
}

/// A sandbox that could hold the agent in fully, and did not prepare it.
struct Faltering(SandboxBackendIdentity);

impl SandboxService for Faltering {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async move {
            let enforced = SandboxFeature::ALL
                .into_iter()
                .fold(SandboxCapabilities::none(), |claims, feature| {
                    claims.with(feature, SandboxCapability::Enforced)
                });
            Ok((self.0.clone(), enforced))
        })
    }

    fn prepare(
        &self,
        _request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async {
            Err(SandboxError::BackendUnavailable {
                reason: "not now".into(),
            })
        })
    }
}

/// A sandbox able to hold the agent in that failed to prepare it is not one
/// whose most is enforced confinement: the refusal is its own failure.
#[tokio::test(flavor = "multi_thread")]
async fn a_sandbox_that_could_hold_it_in_and_did_not_prepare_refuses_with_its_own_failure() {
    let at = Owned::new("faltering");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let control = Control::new().expect("a control");
    let required = Launch::new(program, policy(&at.0, true), Held::granted(ROUTE));

    let refused = Host::launch(
        Arc::new(Faltering(Observing::new().0)),
        Handle::current(),
        &control,
        required,
    )
    .await
    .expect_err("not prepared");

    assert!(
        matches!(
            refused,
            Refusal::Sandbox(SandboxError::BackendUnavailable { .. })
        ),
        "{refused}"
    );
    assert_eq!(fake_peer::played(&at.0), 0);
}

/// The session name an agent reports is put on its next command line, so one
/// that reads as an option ends the turn rather than being kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_session_name_that_reads_as_an_option_is_not_kept() {
    let at = Owned::new("option");
    let reply = Reply::new()
        .hear(1)
        .say("session --dangerously-skip-permissions")
        .say("end");
    let program = found(&at.0, "plain", &Script::of([reply]));
    let control = Control::new().expect("a control");
    let mut host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect("started");
    let sink = Kept::default();

    let ended = host
        .turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &sink,
        )
        .await;
    let kept = host.session().map(str::to_owned);
    host.stop(STOP_GRACE).await;

    assert!(ended.is_err(), "{ended:?}");
    assert_eq!(kept, None);
    assert!(
        matches!(sink.events().last(), Some(Event::Failed(_))),
        "{:?}",
        sink.events()
    );
}

/// Standard error is kept to its first bytes, so a value the agent writes
/// across that bound is cut there; the part before the cut is withheld too.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_cut_where_standard_error_is_bounded_shows_none_of_itself() {
    let at = Owned::new("muttered-cut");
    // Eight bytes of the value fit beneath the bound the host keeps.
    let reply = Reply::new()
        .hear(1)
        .flood(Stream::Err, 8 * 1024 - 8)
        .mutter(SENTINEL)
        .pause(Duration::from_millis(500))
        .exit(3);
    let program = found(&at.0, "plain", &Script::of([reply]));
    let control = Control::new().expect("a control");
    let request = Request {
        arguments: Vec::new(),
        environment: vec![("PLAIN_TOKEN".to_owned(), SENTINEL.to_owned())],
    };
    let mut host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, request),
    )
    .await
    .expect("started");
    let sink = Kept::default();

    let failure = host
        .turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &sink,
        )
        .await
        .expect_err("it crashed");
    host.stop(STOP_GRACE).await;

    let kept = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::Process(fault) => Some(fault.seen().as_str()),
            Event::Failed(failure) => Some(failure.reason().as_str()),
            _ => None,
        })
        .chain([failure.reason().as_str()])
        .collect::<Vec<_>>()
        .join("\n");
    assert!(kept.contains("further bytes were dropped"), "{kept}");
    assert!(!kept.contains("sentinel"), "{kept}");
}

/// A sandbox that rewrites the program while it prepares, the window the
/// check made at selection cannot see.
struct Tampering {
    inner: Arc<dyn SandboxService>,
    program: PathBuf,
}

impl SandboxService for Tampering {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        self.inner.probe()
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            #[allow(clippy::expect_used)]
            {
                let mut bytes = fs::read(&self.program).expect("readable");
                bytes.push(0);
                fs::write(&self.program, bytes).expect("rewritten");
            }
            self.inner.prepare(request).await
        })
    }
}

/// The program is run as it was found or not at all, however long the sandbox
/// took to prepare it.
#[tokio::test(flavor = "multi_thread")]
async fn a_program_changed_while_its_sandbox_was_prepared_is_not_started() {
    let at = Owned::new("tampered");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let path = program.path().to_path_buf();
    let control = Control::new().expect("a control");

    let refused = Host::launch(
        Arc::new(Tampering {
            inner: local(),
            program: path,
        }),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect_err("changed");

    assert!(matches!(refused, Refusal::Changed(_)), "{refused}");
    assert_eq!(fake_peer::played(&at.0), 0);
}

/// A stop asked for before a turn begins is the turn's to heed.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_asked_for_as_a_turn_begins_stops_it() {
    let at = Owned::new("early-stop");
    let reply = Reply::new().hear(2).say("cancelled");
    let program = found(&at.0, "plain", &Script::of([reply]));
    let control = Control::new().expect("a control");
    let mut host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect("started");
    let sink = Kept::default();

    control.interrupt();
    let outcome = tokio::time::timeout(
        Duration::from_secs(20),
        host.turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &sink,
        ),
    )
    .await;
    host.stop(STOP_GRACE).await;

    assert!(matches!(outcome, Ok(Ok(Outcome::Cancelled))), "{outcome:?}");
}

/// A turn whose caller stopped waiting leaves no question open that an
/// answer could still be given to.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_given_up_on_leaves_no_question_open() {
    let at = Owned::new("given-up");
    let reply = Reply::new().hear(1).say("ask q read the notes").hear_out();
    let program = found(&at.0, "plain", &Script::of([reply]));
    let control = Control::new().expect("a control");
    let mut host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect("started");
    let sink = Kept::default();
    let session = SessionRef::new("one").expect("a name");
    let turn = TurnRequest::new(TurnId::FIRST, "go");
    let mut dialect = Plain;
    {
        let turning = host.turn(&mut dialect, &session, &turn, &sink);
        tokio::pin!(turning);
        let asked = tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                tokio::select! {
                    outcome = &mut turning => panic!("the turn ended first: {outcome:?}"),
                    () = tokio::time::sleep(Duration::from_millis(20)) => {
                        if !control.open().is_empty() {
                            break;
                        }
                    }
                }
            }
        })
        .await;
        assert!(asked.is_ok(), "the question was raised");
    }

    let open = control.open();
    host.stop(STOP_GRACE).await;

    assert!(open.is_empty(), "{open:?}");
}

/// A sandbox that does not prepare the agent and never says what it could
/// have held it in.
struct Mute;

impl SandboxService for Mute {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(std::future::pending())
    }

    fn prepare(
        &self,
        _request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async {
            Err(SandboxError::BackendUnavailable {
                reason: "not now".into(),
            })
        })
    }
}

/// Asking a sandbox that failed what it could have done is bounded like
/// preparing was: a launch refuses rather than waiting on it for ever.
#[tokio::test(start_paused = true)]
async fn a_sandbox_that_never_says_what_it_could_hold_refuses_in_time() {
    let at = Owned::new("mute");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let control = Control::new().expect("a control");
    let required = Launch::new(program, policy(&at.0, true), Held::granted(ROUTE));

    let refused = tokio::time::timeout(
        PREPARING.saturating_mul(3),
        Host::launch(Arc::new(Mute), Handle::current(), &control, required),
    )
    .await
    .expect("refused in time")
    .expect_err("not prepared");

    assert!(
        matches!(refused, Refusal::Unconfined(Confinement::Unsupported)),
        "{refused}"
    );
    assert_eq!(fake_peer::played(&at.0), 0);
}
