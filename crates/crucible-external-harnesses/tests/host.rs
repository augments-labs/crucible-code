//! A supervised agent, started as the fake peer over a real sandbox and spoken
//! to in a line protocol of this file's own.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_execution::{
    Authentication, Confinement, Decision, Event, Failure, Outcome, SessionRef, Sink, Text,
    TurnRequest,
};
use crucible_external_harnesses::conformance::{self, CASES, Set};
use crucible_external_harnesses::fake_peer::{
    self, EXHAUSTED, Plain, Reply, Script, Stream, UNWRITABLE,
};
use crucible_external_harnesses::{
    Account, Act, Control, Dialect, Discovery, Held, Host, Launch, PREPARING, Ran, Reading,
    Refusal, Request, STOP_GRACE, Shape, Told, Unstopped,
};
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxCleanup, SandboxCommand, SandboxError, SandboxFeature,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInput,
    SandboxInspection, SandboxLaunch, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy,
    SandboxProcess, SandboxRequest, SandboxResourceLimits, SandboxService, SandboxSession,
    SandboxUsage, SandboxViolation, inspection,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::TurnId;
use tokio::runtime::Handle;

/// A value long enough to be a secret, which nothing kept may carry.
const SENTINEL: &str = "sentinel-c0ffee-5ec7e7";

/// The route this file's launches are held under.
const ROUTE: &str = "agent:plain";

/// Puts a changed copy of the program at `path` in its place.
#[allow(clippy::expect_used)]
fn rewritten(path: &Path) {
    let mut bytes = fs::read(path).expect("readable");
    bytes.push(0);
    let beside = path.with_extension("changed");
    fs::write(&beside, bytes).expect("written");
    fs::rename(&beside, path).expect("replaced");
}

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
        opening: vec![(1, Vec::new())],
        effect: "file src/lib.rs".to_owned(),
        ask: |native| format!("ask {native} read the notes"),
        answer: |native, decision| match decision {
            Decision::Allow(_) => format!("allow {native}"),
            Decision::Deny => format!("deny {native}"),
            Decision::Answer(text) => format!("answer {native} {}", text.as_str()),
        },
        answered: "end".to_owned(),
        cancelled: "cancelled".to_owned(),
        stop: Some("cancel".to_owned()),
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
    rewritten(program.path());
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

/// Waits until the peer installed in `here` has kept its counter at `want`,
/// which a slow process start may not have done when the host's launch
/// returns, and fails the test if it has not within a bound.
async fn played_reaches(here: &Path, want: usize) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while fake_peer::played(here) < want {
        assert!(
            tokio::time::Instant::now() < deadline,
            "the peer's counter never reached {want}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
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
    played_reaches(&at.0, 1).await;
    first.stop(STOP_GRACE).await.expect("stopped");
    assert_eq!(fake_peer::played(&at.0), 1);
}

/// A stop given up on inside its grace has not confirmed the process ended,
/// so the agent's place stays taken and no second run is started beside what
/// may still be running.
#[tokio::test(flavor = "multi_thread")]
async fn a_stop_abandoned_in_its_grace_leaves_a_relaunch_refused() {
    let at = Owned::new("abandoned-stop");
    let program = found(
        &at.0,
        "plain",
        &Script::of([
            Reply::new().pause(Duration::from_mins(1)),
            Reply::lines(["two"]),
        ]),
    );
    let control = Control::new().expect("a control");
    let host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program.clone(), &at.0, Request::default()),
    )
    .await
    .expect("started");

    // The stop is given up on only once the child has started and kept its
    // counter, so that counter reads the one start whatever the start cost.
    played_reaches(&at.0, 1).await;
    let abandoned = tokio::time::timeout(Duration::from_millis(200), host.stop(STOP_GRACE)).await;
    let relaunched = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await;

    assert!(abandoned.is_err(), "the stop finished inside its grace");
    assert!(
        matches!(relaunched, Err(Refusal::Unstopped)),
        "{relaunched:?}"
    );
    assert_eq!(fake_peer::played(&at.0), 1);
}

/// How a stubborn sandbox hands its processes over, and what stopping one does.
#[derive(Debug, Clone, Copy)]
enum Mode {
    /// With its pipes, and a stop that always fails.
    Stuck,
    /// Without its input, and a stop that always fails.
    DeafAndStuck,
    /// Without its input, and a stop that works as the local one's does.
    Deaf,
}

/// A sandbox whose processes run as the local one's do, but are handed over as
/// its [`Mode`] says, so that a stop can be made to fail and so that no stop
/// can be confirmed.
struct Stubborn(Arc<dyn SandboxService>, Mode);

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
                Box::new(StubbornSession(self.0.prepare(request).await?, self.1));
            Ok(session)
        })
    }
}

/// Its session.
struct StubbornSession(Box<dyn SandboxSession>, Mode);

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
                Box::new(StubbornLaunch(self.0.stage(command).await?, self.1));
            Ok(launch)
        })
    }
}

/// Its staged launch.
struct StubbornLaunch(Box<dyn SandboxLaunch>, Mode);

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
                Box::new(StubbornProcess(self.0.release().await?, self.1));
            Ok(process)
        })
    }
}

/// Its process, which never reads as ended and, unless its mode lets the stop
/// work, whose stop always fails.
struct StubbornProcess(Box<dyn SandboxProcess>, Mode);

impl SandboxProcess for StubbornProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.0.take_stdin()
    }

    fn take_async_stdin(&mut self) -> Option<Box<dyn SandboxInput>> {
        match self.1 {
            Mode::Stuck => self.0.take_async_stdin(),
            Mode::DeafAndStuck | Mode::Deaf => None,
        }
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
        match self.1 {
            Mode::Stuck | Mode::DeafAndStuck => {
                Box::pin(async { Err(std::io::Error::other(STUCK)) })
            }
            Mode::Deaf => self.0.stop(),
        }
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

/// What the stubborn sandbox's failed stop says.
const STUCK: &str = "the scope did not answer";

/// A stop the sandbox could not confirm fails carrying the error that left it
/// unconfirmed, so that whoever reads the failure can say why.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn an_unstopped_run_keeps_the_error_that_left_it_unconfirmed() {
    let at = Owned::new("unstopped-source");
    let program = found(&at.0, "plain", &Script::of([Reply::new().hear_out()]));
    let control = Control::new().expect("a control");
    let host = Host::launch(
        Arc::new(Stubborn(local(), Mode::Stuck)),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await
    .expect("started");

    let unstopped = host
        .stop(Duration::ZERO)
        .await
        .expect_err("a stop that cannot be confirmed fails");

    let cause = std::error::Error::source(&unstopped).expect("the error that left it unconfirmed");
    let cause = cause
        .downcast_ref::<std::io::Error>()
        .expect("an I/O error");
    assert_eq!(cause.to_string(), STUCK);
}

/// A process handed over without a pipe to speak to it over, whose stop could
/// not be confirmed, has not been shown to have ended, so the launch is
/// refused with the error that left it unconfirmed and no second run is
/// started beside what may still be running.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn an_unspoken_agent_whose_stop_failed_holds_the_next_launch() {
    let at = Owned::new("unspoken-unconfirmed");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let control = Control::new().expect("a control");

    let first = Host::launch(
        Arc::new(Stubborn(local(), Mode::DeafAndStuck)),
        Handle::current(),
        &control,
        launch(program.clone(), &at.0, Request::default()),
    )
    .await
    .expect_err("an agent with no pipe to speak to it over is refused");
    let next = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await;

    assert!(matches!(first, Refusal::Unspoken { .. }), "{first:?}");
    assert!(matches!(next, Err(Refusal::Unstopped)), "{next:?}");
    let cause = std::error::Error::source(&first).expect("the error that left it unconfirmed");
    let cause = cause
        .downcast_ref::<std::io::Error>()
        .expect("an I/O error");
    assert_eq!(cause.to_string(), STUCK);
    assert!(
        !first.to_string().contains(STUCK),
        "the error is reached through its source, not repeated in the words: {first}"
    );
}

/// A process handed over without a pipe whose stop was confirmed is gone, so
/// the refusal names no error and the next launch runs.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn an_unspoken_agent_whose_stop_was_confirmed_leaves_the_next_launch_free() {
    let at = Owned::new("unspoken-confirmed");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let control = Control::new().expect("a control");

    let first = Host::launch(
        Arc::new(Stubborn(local(), Mode::Deaf)),
        Handle::current(),
        &control,
        launch(program.clone(), &at.0, Request::default()),
    )
    .await
    .expect_err("an agent with no pipe to speak to it over is refused");
    let next = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await;

    assert!(matches!(first, Refusal::Unspoken { .. }), "{first:?}");
    assert!(std::error::Error::source(&first).is_none());
    let host = next.expect("the agent's place is free");
    host.stop(STOP_GRACE).await.expect("stopped");
}

/// A stop that never finished says so, rather than naming an error it never
/// had.
#[test]
fn an_unstopped_run_whose_stop_never_finished_says_so_and_names_no_error() {
    let never = Unstopped::Unfinished;
    let failed = Unstopped::Unreaped(std::io::Error::other(STUCK));

    assert!(std::error::Error::source(&never).is_none());
    assert_ne!(never.to_string(), failed.to_string());
}

/// The plain protocol with its account steps spoken: a step begins with its
/// own name, a `link` line is a sign-in link, and `in` or `out` ends it.
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
        Ok(match frame.split_once(' ') {
            Some(("link", link)) => Told::Link(Text::new(link)),
            _ => match frame {
                "in" => Told::Done(Reading::Signed(Authentication::SignedIn)),
                "out" => Told::Done(Reading::Signed(Authentication::SignedOut)),
                _ => Told::Nothing,
            },
        })
    }
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_sign_in_shows_its_link_while_it_waits_and_is_stopped_past_its_bound() {
    let at = Owned::new("spoken-sign-in");
    let program = found(
        &at.0,
        "plain",
        &Script::of([Reply::new()
            .hear(1)
            .say("link https://sign.in/here")
            .pause(Duration::from_mins(1))
            .say("in")]),
    );
    let control = Control::new().expect("a control");
    let mut dialect = Speaking::default();
    let request = dialect
        .account(Account::SignIn, Shape::default())
        .expect("a sign-in step");
    let frames = dialect.spoken(Account::SignIn).expect("spoken");
    let host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, request),
    )
    .await
    .expect("started");
    let shown = Mutex::new(Vec::new());

    let read = host
        .speak(
            &mut dialect,
            Account::SignIn,
            &frames,
            &|link| {
                shown
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .push(link.as_str().to_owned());
            },
            Duration::from_millis(500),
        )
        .await;

    assert_eq!(
        shown.into_inner().unwrap_or_else(PoisonError::into_inner),
        vec!["https://sign.in/here".to_owned()]
    );
    assert!(
        matches!(&read, Reading::Unreadable(failure) if failure.reason().as_str().contains("in time")),
        "{read:?}"
    );
    assert_eq!(fake_peer::heard(&at.0), vec!["SignIn".to_owned()]);
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_spoken_status_reads_what_the_agent_said_and_frees_its_place() {
    let at = Owned::new("spoken-status");
    let program = found(
        &at.0,
        "plain",
        &Script::of([
            Reply::new().hear(1).say("noise").say("out"),
            Reply::lines(["1.0.0"]),
        ]),
    );
    let control = Control::new().expect("a control");
    let mut dialect = Speaking::default();
    let request = dialect
        .account(Account::Status, Shape::default())
        .expect("a status step");
    let frames = dialect.spoken(Account::Status).expect("spoken");
    let host = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program.clone(), &at.0, request),
    )
    .await
    .expect("started");

    let read = host
        .speak(&mut dialect, Account::Status, &frames, &|_| {}, PREPARING)
        .await;
    let after = version(&program, &at.0, &control).await;

    assert!(
        matches!(read, Reading::Signed(Authentication::SignedOut)),
        "{read:?}"
    );
    assert_eq!(after.said, vec!["1.0.0".to_owned()]);
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
        shown: Vec::new(),
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
    host.stop(STOP_GRACE).await.expect("stopped");

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

/// A value the launch names shown says nothing about an account, so what the
/// agent says holding it is kept as the agent said it, a path under the home
/// reading as that path; a value it does not name is still withheld.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_the_launch_names_shown_is_kept_as_the_agent_said_it() {
    let at = Owned::new("shown");
    let reply = Reply::new()
        .hear(1)
        .say(format!(
            "say edited /home/someone/src/lib.rs with {SENTINEL}"
        ))
        .say("end");
    let program = found(&at.0, "plain", &Script::of([reply]));
    let control = Control::new().expect("a control");
    let request = Request {
        arguments: Vec::new(),
        environment: vec![
            ("HOME".to_owned(), "/home/someone".to_owned()),
            ("PLAIN_TOKEN".to_owned(), SENTINEL.to_owned()),
        ],
        shown: vec!["HOME"],
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

    let ended = host
        .turn(
            &mut Plain,
            &SessionRef::new("one").expect("a name"),
            &TurnRequest::new(TurnId::FIRST, "go"),
            &sink,
        )
        .await;
    host.stop(STOP_GRACE).await.expect("stopped");

    assert!(ended.is_ok(), "{ended:?}");
    let said = sink
        .events()
        .iter()
        .filter_map(|event| match event {
            Event::Delta(text) => Some(text.as_str().to_owned()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(said, "edited /home/someone/src/lib.rs with [withheld]");
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
        host.stop(STOP_GRACE).await.expect("stopped");
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

/// Cargo links the peer and this test in either order, so its stamp, not its
/// age beside the test, is what is compared.
#[test]
fn a_peer_is_installed_by_its_stamp_whichever_was_linked_first() {
    let at = Owned::new("older");

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
    host.stop(STOP_GRACE).await.expect("stopped");

    assert!(ended.is_err(), "{ended:?}");
    assert_eq!(kept, None);
    assert!(
        matches!(sink.events().last(), Some(Event::Failed(_))),
        "{:?}",
        sink.events()
    );
}

/// What a turn whose program wrote `value` to standard error after `flood`
/// bytes, then crashed, left in its events and its failure.
// Each step is one the tests it serves would fail on.
#[allow(clippy::expect_used)]
async fn crashed_muttering(name: &str, flood: u64, value: &str) -> String {
    let at = Owned::new(name);
    let reply = Reply::new()
        .hear(1)
        .flood(Stream::Err, flood)
        .mutter(value)
        .pause(Duration::from_millis(500))
        .exit(3);
    let program = found(&at.0, "plain", &Script::of([reply]));
    let control = Control::new().expect("a control");
    let request = Request {
        arguments: Vec::new(),
        environment: vec![("PLAIN_TOKEN".to_owned(), value.to_owned())],
        shown: Vec::new(),
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
    host.stop(STOP_GRACE).await.expect("stopped");

    sink.events()
        .iter()
        .filter_map(|event| match event {
            Event::Process(fault) => Some(fault.seen().as_str()),
            Event::Failed(failure) => Some(failure.reason().as_str()),
            _ => None,
        })
        .chain([failure.reason().as_str()])
        .collect::<Vec<_>>()
        .join("\n")
}

/// Standard error is kept to its first bytes, so a value the agent writes
/// across that bound is cut there; the part before the cut is withheld too.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_cut_where_standard_error_is_bounded_shows_none_of_itself() {
    // Eight bytes of the value fit beneath the bound the host keeps.
    let kept = crashed_muttering("muttered-cut", 8 * 1024 - 8, SENTINEL).await;

    assert!(kept.contains("further bytes were dropped"), "{kept}");
    assert!(!kept.contains("sentinel"), "{kept}");
}

/// A cut inside one of a value's characters leaves half a character, which
/// is shown as a replacement and hides nothing of what came before it.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_cut_inside_a_character_shows_none_of_itself() {
    // Ten bytes fit: `sentinel-` and the first of the two bytes of `é`.
    let kept = crashed_muttering("muttered-split", 8 * 1024 - 10, "sentinel-é-5ec7e7").await;

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
            rewritten(&self.program);
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
    host.stop(STOP_GRACE).await.expect("stopped");

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
    host.stop(STOP_GRACE).await.expect("stopped");

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

/// Where a sandbox that starts its process holds the launch.
#[derive(Clone, Copy)]
enum Hold {
    /// After the spawn, before the process is handed over.
    Release,
    /// While the stop of a process handed over without its pipes is waited on.
    Stop,
}

/// A sandbox that starts its process and then holds the launch, so that a
/// launch can be dropped at a known point before the host holds a run.
struct Holding {
    inner: Arc<dyn SandboxService>,
    at: Hold,
    /// Told once the launch is held.
    reached: Arc<tokio::sync::Notify>,
}

impl SandboxService for Holding {
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
            let session: Box<dyn SandboxSession> = Box::new(HoldingSession {
                inner: self.inner.prepare(request).await?,
                at: self.at,
                reached: Arc::clone(&self.reached),
            });
            Ok(session)
        })
    }
}

/// Its session.
struct HoldingSession {
    inner: Box<dyn SandboxSession>,
    at: Hold,
    reached: Arc<tokio::sync::Notify>,
}

impl SandboxSession for HoldingSession {
    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        self.inner.materialize()
    }

    fn stage<'a>(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let launch: Box<dyn SandboxLaunch> = Box::new(HoldingLaunch {
                inner: self.inner.stage(command).await?,
                at: self.at,
                reached: self.reached,
            });
            Ok(launch)
        })
    }
}

/// Its staged launch.
struct HoldingLaunch {
    inner: Box<dyn SandboxLaunch>,
    at: Hold,
    reached: Arc<tokio::sync::Notify>,
}

impl SandboxLaunch for HoldingLaunch {
    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let spawned = self.inner.release().await?;
            match self.at {
                Hold::Release => {
                    let _spawned = spawned;
                    self.reached.notify_one();
                    std::future::pending().await
                }
                Hold::Stop => {
                    let process: Box<dyn SandboxProcess> = Box::new(Pipeless {
                        inner: spawned,
                        reached: self.reached,
                    });
                    Ok(process)
                }
            }
        })
    }
}

/// A process handed over without its input, whose stop does not answer.
struct Pipeless {
    inner: Box<dyn SandboxProcess>,
    reached: Arc<tokio::sync::Notify>,
}

impl SandboxProcess for Pipeless {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        None
    }

    fn take_async_stdin(&mut self) -> Option<Box<dyn SandboxInput>> {
        None
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.inner.take_stdout()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.inner.take_stderr()
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.inner.try_wait()
    }

    fn stop(&mut self) -> BoxFuture<'_, std::io::Result<()>> {
        Box::pin(async {
            self.reached.notify_one();
            std::future::pending().await
        })
    }

    fn inspection(&self) -> &SandboxInspection {
        self.inner.inspection()
    }

    fn usage(&self) -> SandboxUsage {
        self.inner.usage()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.inner.violation()
    }
}

/// A launch given up on after its process was spawned and before the host held
/// a run leaves the agent's place free: the next launch is not refused as
/// though a run were still live.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_dropped_after_its_spawn_leaves_the_next_launch_unrefused() {
    let at = Owned::new("dropped-launch");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let control = Control::new().expect("a control");
    let reached = Arc::new(tokio::sync::Notify::new());
    let held = Holding {
        inner: local(),
        at: Hold::Release,
        reached: Arc::clone(&reached),
    };
    let mut launching = Box::pin(Host::launch(
        Arc::new(held),
        Handle::current(),
        &control,
        launch(program.clone(), &at.0, Request::default()),
    ));

    tokio::select! {
        started = &mut launching => panic!("the held launch finished: {started:?}"),
        () = reached.notified() => {}
    }
    drop(launching);
    let next = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await;

    let host = next.expect("the agent's place is free");
    host.stop(STOP_GRACE).await.expect("stopped");
}

/// A launch given up on while the stop of a process it could not speak to was
/// still being waited on has not confirmed that process ended, so the agent's
/// place stays taken, as a run whose stop was abandoned keeps it.
#[tokio::test(flavor = "multi_thread")]
async fn a_launch_dropped_while_a_pipeless_process_is_stopped_leaves_a_relaunch_refused() {
    let at = Owned::new("dropped-stop");
    let program = found(&at.0, "plain", &Script::of([Reply::lines(["one"])]));
    let control = Control::new().expect("a control");
    let reached = Arc::new(tokio::sync::Notify::new());
    let held = Holding {
        inner: local(),
        at: Hold::Stop,
        reached: Arc::clone(&reached),
    };
    let mut launching = Box::pin(Host::launch(
        Arc::new(held),
        Handle::current(),
        &control,
        launch(program.clone(), &at.0, Request::default()),
    ));

    tokio::select! {
        started = &mut launching => panic!("the held launch finished: {started:?}"),
        () = reached.notified() => {}
    }
    drop(launching);
    let next = Host::launch(
        local(),
        Handle::current(),
        &control,
        launch(program, &at.0, Request::default()),
    )
    .await;

    assert!(matches!(next, Err(Refusal::Unstopped)), "{next:?}");
}
