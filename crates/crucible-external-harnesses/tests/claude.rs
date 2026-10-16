//! Claude Code, started as the fake peer speaking its stream protocol over a
//! real sandbox: every conformance case, a connect and the turns of one run,
//! the figures each turn reports, and what a question asked as its turn is
//! stopped comes to.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crucible_execution::{
    Authentication, Cost, Currency, Decision, Event, Failure, Lasting, Outcome, PendingRef,
    SessionRef, Sink, Stale, TurnRequest, Usage,
};
use crucible_external_harnesses::claude::Claude;
use crucible_external_harnesses::conformance::{self, CASES};
use crucible_external_harnesses::fake_peer::scripts::claude as scripts;
use crucible_external_harnesses::fake_peer::{self, Reply, Script};
use crucible_external_harnesses::{
    Account, Control, Dialect, Discovery, Held, Host, Launch, Program, Reading, STOP_GRACE, Shape,
    Undecided,
};
use crucible_sandbox::{
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits, SandboxService,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::TurnId;
use tokio::runtime::Handle;

/// The route this file's launches are held under.
const ROUTE: &str = "agent:claude";

/// The longest a test waits for the peer to hear something.
const HEARING: Duration = Duration::from_secs(20);

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
            "crucible-claude-{name}-{unique}-{}",
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

/// The fake peer installed in `here` as `claude`, playing `script`.
#[allow(clippy::expect_used)]
fn installed(here: &Path, script: &Script) -> Program {
    let path = fake_peer::install(here, "claude", script).expect("the fake peer is built");
    match Discovery::of(Some(&path)) {
        Discovery::Found(program) => Some(program),
        Discovery::Missing | Discovery::NotExecutable(_) => None,
    }
    .expect("the installed peer is found")
}

/// What the account step `step` read, over a launch of its own.
#[allow(clippy::expect_used)]
async fn step(program: &Program, here: &Path, control: &Control, step: Account) -> Reading {
    let mut dialect = Claude::default();
    let shape = Shape {
        profile: Some(here),
        ..Shape::default()
    };
    let request = dialect.account(step, shape).expect("an account step");
    let launch = Launch::new(program.clone(), policy(here), Held::granted(ROUTE)).asking(request);
    let ran = Host::launch(local(), Handle::current(), control, launch)
        .await
        .expect("started")
        .run()
        .await;
    dialect.read(step, &ran)
}

/// A run of `program` in `here`, resuming `resume` where it is given.
#[allow(clippy::expect_used)]
async fn run(
    program: &Program,
    here: &Path,
    control: &Control,
    dialect: &mut Claude,
    resume: Option<&str>,
) -> Host {
    let shape = Shape {
        resume,
        profile: Some(here),
        workspace: Some(here),
        model: None,
        effort: None,
    };
    let launch = Launch::new(program.clone(), policy(here), Held::granted(ROUTE))
        .asking(dialect.request(shape));
    Host::launch(local(), Handle::current(), control, launch)
        .await
        .expect("started")
}

/// What a turn's sink does besides keeping what it was posted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum React {
    /// Nothing.
    Nothing,
    /// Allows every question as it is raised.
    Allow,
    /// Asks the turn to stop once it reports an effect under way.
    InterruptOnEffect,
    /// Asks the turn to stop once it raises a question, and decides nothing.
    InterruptOnQuestion,
}

/// Keeps what turns posted, and reacts as a test needs.
struct Recorder {
    /// The handle decisions and stops go through.
    control: Control,
    /// What it does besides keeping.
    react: React,
    /// What was posted.
    events: Mutex<Vec<Event>>,
}

impl Recorder {
    const fn new(control: Control, react: React) -> Self {
        Self {
            control,
            react,
            events: Mutex::new(Vec::new()),
        }
    }

    fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The figures each turn reported, in order.
    fn usage(&self) -> Vec<Usage> {
        self.events()
            .into_iter()
            .filter_map(|event| match event {
                Event::Usage(observed) => Some(*observed.value()),
                _ => None,
            })
            .collect()
    }

    /// The question the turns raised last.
    fn asked(&self) -> Option<PendingRef> {
        self.events()
            .into_iter()
            .rev()
            .find_map(|event| match event {
                Event::Pending(reference, _) => Some(reference),
                _ => None,
            })
    }
}

impl Sink for Recorder {
    fn post(&self, event: Event) {
        match (self.react, &event) {
            (React::Allow, Event::Pending(reference, _)) => {
                let _ = self
                    .control
                    .decide(reference, Decision::Allow(Lasting::Once));
            }
            (React::InterruptOnEffect, Event::ToolActivity { .. })
            | (React::InterruptOnQuestion, Event::Pending(..)) => self.control.interrupt(),
            _ => {}
        }
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

/// Plays the turn numbered `turn` of the session over `host`.
#[allow(clippy::expect_used)]
async fn turn(
    host: &mut Host,
    dialect: &mut Claude,
    turn: TurnId,
    sink: &Recorder,
) -> Result<Outcome, Failure> {
    let session = SessionRef::new("claude").expect("a session reference");
    host.turn(dialect, &session, &TurnRequest::new(turn, "Go on."), sink)
        .await
}

/// Connects as a conversation does: the version probe, then the status
/// check, each a launch of its own.
async fn connect(program: &Program, here: &Path, control: &Control) {
    assert!(
        matches!(
            step(program, here, control, Account::Version).await,
            Reading::Version(version) if version.as_str() == "2.1.296"
        ),
        "the probe reads the reviewed release"
    );
    assert!(
        matches!(
            step(program, here, control, Account::Status).await,
            Reading::Signed(Authentication::SignedIn)
        ),
        "the status check reads signed in"
    );
}

/// The cost of `millionths` of a dollar.
#[allow(clippy::expect_used)]
fn dollars(millionths: u64) -> Cost {
    Cost::new(millionths, Currency::new("USD").expect("a currency"))
}

/// How many `user` frames the peer heard.
fn prompts(here: &Path) -> usize {
    fake_peer::heard(here)
        .iter()
        .filter(|line| {
            serde_json::from_str::<serde_json::Value>(line).is_ok_and(|frame| {
                frame.get("type").and_then(serde_json::Value::as_str) == Some("user")
            })
        })
        .count()
}

/// Waits until the peer in `here` has heard `line`.
async fn hears(here: &Path, line: &str) -> bool {
    let until = tokio::time::Instant::now() + HEARING;
    while tokio::time::Instant::now() < until {
        if fake_peer::heard(here).iter().any(|heard| heard == line) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn every_conformance_case_ends_as_it_must_over_the_stream_protocol() {
    let at = Owned::new("conformance");

    let passed = conformance::run(
        local(),
        || Box::new(Claude::default()),
        &scripts::set(),
        &at.0,
    )
    .await
    .expect("every case passes");

    assert_eq!(passed, CASES);
}

/// A question the user allows once its turn was told to stop is answered
/// no: the agent was told to stop, and a yes reaching it then would run what
/// nobody is left to watch.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_question_allowed_after_its_turn_was_told_to_stop_is_answered_no() {
    let at = Owned::new("allowed-after-stop");
    let reply = Reply::new()
        .hear(1)
        .say(scripts::init())
        .say(scripts::asked("q"))
        .hear(2)
        .say(scripts::stopped())
        .say(scripts::interrupted());
    let program = installed(&at.0, &Script::of([reply]));
    let control = Control::new().expect("a control");
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let sink = Recorder::new(control.clone(), React::InterruptOnQuestion);

    let deciding = {
        let (control, here) = (control.clone(), at.0.clone());
        tokio::spawn(async move {
            if !hears(&here, &scripts::stop()).await {
                return None;
            }
            let question = control.open().into_iter().next()?;
            Some(control.decide(&question, Decision::Allow(Lasting::Once)))
        })
    };
    let ended = turn(&mut host, &mut dialect, TurnId::FIRST, &sink).await;
    let decided = deciding.await.expect("the decision was made");
    host.stop(STOP_GRACE).await.expect("stopped");

    assert_eq!(ended, Ok(Outcome::Cancelled));
    assert!(decided.is_some(), "the question was decided after the stop");
    let heard = fake_peer::heard(&at.0);
    let allowed = scripts::answer("q", &Decision::Allow(Lasting::Once));
    assert!(
        !heard.contains(&allowed),
        "the agent heard a yes: {heard:?}"
    );
    assert_eq!(
        heard.last(),
        Some(&scripts::answer("q", &Decision::Deny)),
        "the agent heard no answer of no"
    );
}

/// A question still open when its turn ended stopped cannot be decided: the
/// turn it belongs to is over.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_question_left_open_by_a_stopped_turn_is_refused_as_stale() {
    let at = Owned::new("open-after-stop");
    let reply = Reply::new()
        .hear(1)
        .say(scripts::init())
        .say(scripts::asked("q"))
        .hear(1)
        .say(scripts::withdrawn("q"))
        .say(scripts::stopped())
        .say(scripts::interrupted())
        .hear(1)
        .say(scripts::init())
        .say(scripts::answered(Some(scripts::PRICE)));
    let program = installed(&at.0, &Script::of([reply]));
    let control = Control::new().expect("a control");
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let stopping = Recorder::new(control.clone(), React::InterruptOnQuestion);

    let first = turn(&mut host, &mut dialect, TurnId::FIRST, &stopping).await;
    let question = stopping.asked().expect("the turn raised its question");
    let after = control.decide(&question, Decision::Allow(Lasting::Once));
    let next = Recorder::new(control.clone(), React::Nothing);
    let second = turn(&mut host, &mut dialect, TurnId::FIRST.next(), &next).await;
    let during = control.decide(&question, Decision::Allow(Lasting::Once));
    host.stop(STOP_GRACE).await.expect("stopped");

    assert_eq!(first, Ok(Outcome::Cancelled));
    assert_eq!(second, Ok(Outcome::Answered));
    assert!(
        matches!(after, Err(Undecided::Stale(Stale::Turn))),
        "decided after its turn: {after:?}"
    );
    assert!(
        matches!(during, Err(Undecided::Stale(Stale::Turn))),
        "decided after the next turn: {during:?}"
    );
    let allowed = scripts::answer("q", &Decision::Allow(Lasting::Once));
    assert!(!fake_peer::heard(&at.0).contains(&allowed));
}

/// A connect and two turns start the agent three times: the probe, the
/// status check and one run that hears both prompts; each turn reports its
/// own figures, the second its share of the session's running cost.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_connect_and_two_turns_start_one_probe_one_status_check_and_one_run() {
    let at = Owned::new("two-turns");
    let program = installed(&at.0, &scripts::two_priced());
    let control = Control::new().expect("a control");

    connect(&program, &at.0, &control).await;
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let sink = Recorder::new(control.clone(), React::Nothing);
    let first = turn(&mut host, &mut dialect, TurnId::FIRST, &sink).await;
    let second = turn(&mut host, &mut dialect, TurnId::FIRST.next(), &sink).await;
    host.stop(STOP_GRACE).await.expect("stopped");

    assert_eq!(
        (first, second),
        (Ok(Outcome::Answered), Ok(Outcome::Answered))
    );
    assert_eq!(fake_peer::played(&at.0), 3, "no second launch for a turn");
    assert_eq!(prompts(&at.0), 2, "one run heard both prompts");
    let used = sink.usage();
    let input = scripts::UNCACHED + scripts::CACHE_READ + scripts::CACHE_WRITE;
    let expected = |turn| {
        Usage::of(turn)
            .input(input)
            .cache_read(scripts::CACHE_READ)
            .cache_write(scripts::CACHE_WRITE)
            .output(scripts::OUTPUT)
            .cost(dollars(scripts::PRICE))
    };
    assert_eq!(
        used,
        [expected(TurnId::FIRST), expected(TurnId::FIRST.next())]
    );
}

/// A turn whose `result` frame gives no cost reports its tokens and no cost.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_turn_that_reports_no_cost_carries_its_tokens_and_none() {
    let at = Owned::new("unpriced");
    let program = installed(&at.0, &scripts::priced_then_unreported());
    let control = Control::new().expect("a control");

    connect(&program, &at.0, &control).await;
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let sink = Recorder::new(control.clone(), React::Nothing);
    let first = turn(&mut host, &mut dialect, TurnId::FIRST, &sink).await;
    let second = turn(&mut host, &mut dialect, TurnId::FIRST.next(), &sink).await;
    host.stop(STOP_GRACE).await.expect("stopped");

    assert_eq!(
        (first, second),
        (Ok(Outcome::Answered), Ok(Outcome::Answered))
    );
    let costs: Vec<_> = sink.usage().iter().map(Usage::spent).collect();
    assert_eq!(costs, [Some(dollars(scripts::PRICE)), None]);
    assert!(
        sink.usage()
            .iter()
            .all(|used| used.output_tokens() == Some(scripts::OUTPUT))
    );
}

/// A stopped turn reports no figures, and the turn after it reports its
/// share of the running cost the stopped one left unreported.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_stopped_turn_between_two_priced_ones_reports_nothing_and_breaks_no_difference() {
    let at = Owned::new("interrupt-between");
    let program = installed(&at.0, &scripts::priced_interrupt_priced());
    let control = Control::new().expect("a control");

    connect(&program, &at.0, &control).await;
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let plain = Recorder::new(control.clone(), React::Nothing);
    let stopping = Recorder::new(control.clone(), React::InterruptOnEffect);
    let first = turn(&mut host, &mut dialect, TurnId::FIRST, &plain).await;
    let second = turn(&mut host, &mut dialect, TurnId::FIRST.next(), &stopping).await;
    let third = turn(&mut host, &mut dialect, TurnId::FIRST.next().next(), &plain).await;
    host.stop(STOP_GRACE).await.expect("stopped");

    assert_eq!(
        [first, second, third],
        [
            Ok(Outcome::Answered),
            Ok(Outcome::Cancelled),
            Ok(Outcome::Answered)
        ]
    );
    assert!(
        stopping.usage().is_empty(),
        "the stopped turn reported figures"
    );
    let costs: Vec<_> = plain.usage().iter().map(Usage::spent).collect();
    assert_eq!(
        costs,
        [Some(dollars(scripts::PRICE)), Some(dollars(scripts::PRICE))]
    );
    assert!(fake_peer::heard(&at.0).contains(&scripts::stop()));
}

/// Over the cumulative pair, the fresh run's turn reports its cost, and the
/// resumed run's turn, with no figure remembered from before it, reports its
/// tokens and no cost.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_resumed_run_with_no_remembered_cost_reports_none_for_its_turn() {
    let at = Owned::new("cumulative");
    let program = installed(&at.0, &scripts::cumulative_pair());
    let control = Control::new().expect("a control");

    connect(&program, &at.0, &control).await;
    let sink = Recorder::new(control.clone(), React::Nothing);
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let fresh = turn(&mut host, &mut dialect, TurnId::FIRST, &sink).await;
    let session = host
        .session()
        .map(str::to_owned)
        .expect("the session was named");
    host.stop(STOP_GRACE).await.expect("stopped");
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, Some(&session)).await;
    let resumed = turn(&mut host, &mut dialect, TurnId::FIRST.next(), &sink).await;
    host.stop(STOP_GRACE).await.expect("stopped");

    assert_eq!(session, scripts::SESSION);
    assert_eq!(
        (fresh, resumed),
        (Ok(Outcome::Answered), Ok(Outcome::Answered))
    );
    let used = sink.usage();
    let costs: Vec<_> = used.iter().map(Usage::spent).collect();
    assert_eq!(costs, [Some(dollars(scripts::PRICE)), None]);
    assert!(used.iter().all(|used| used.input_tokens().is_some()));
}

/// A frame of a kind the reviewed release does not write ends the turn
/// incomplete, never guessed into an answer.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_frame_the_reviewed_release_does_not_write_ends_the_turn_incomplete() {
    let at = Owned::new("unreviewed");
    let reply = Reply::new()
        .hear(1)
        .say(scripts::init())
        .say(r#"{"type":"something_new","session_id":"s"}"#)
        .hear_out();
    let program = installed(&at.0, &Script::of([reply]));
    let control = Control::new().expect("a control");
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let sink = Recorder::new(control.clone(), React::Nothing);

    let ended = turn(&mut host, &mut dialect, TurnId::FIRST, &sink).await;
    let _ = host.stop(STOP_GRACE).await;

    assert!(
        matches!(&ended, Err(failure) if failure.reason().as_str().contains("reviewed release")),
        "the turn ended {ended:?}"
    );
    assert!(
        sink.events()
            .iter()
            .any(|event| matches!(event, Event::Process(_)))
    );
}

/// A signed-out profile's status check reads signed out.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn a_signed_out_profile_reads_signed_out() {
    let at = Owned::new("signed-out");
    let program = installed(&at.0, &Script::of([scripts::signed_out()]));
    let control = Control::new().expect("a control");

    let read = step(&program, &at.0, &control, Account::Status).await;

    assert!(matches!(read, Reading::Signed(Authentication::SignedOut)));
}

/// The headless sequence plays as a front end with no terminal drives it: a
/// connect, a turn whose question is allowed, a turn that is stopped, and a
/// disconnect that starts nothing.
#[tokio::test(flavor = "multi_thread")]
#[allow(clippy::expect_used)]
async fn the_headless_sequence_plays_a_question_a_stop_and_a_disconnect() {
    let at = Owned::new("headless");
    let program = installed(&at.0, &scripts::headless());
    let control = Control::new().expect("a control");

    connect(&program, &at.0, &control).await;
    let mut dialect = Claude::default();
    let mut host = run(&program, &at.0, &control, &mut dialect, None).await;
    let allowing = Recorder::new(control.clone(), React::Allow);
    let stopping = Recorder::new(control.clone(), React::InterruptOnEffect);
    let asked = turn(&mut host, &mut dialect, TurnId::FIRST, &allowing).await;
    let stopped = turn(&mut host, &mut dialect, TurnId::FIRST.next(), &stopping).await;
    host.stop(STOP_GRACE).await.expect("stopped");

    assert_eq!(
        (asked, stopped),
        (Ok(Outcome::Answered), Ok(Outcome::Cancelled))
    );
    assert_eq!(fake_peer::played(&at.0), 3);
    let allowed = scripts::answer("q", &Decision::Allow(Lasting::Once));
    assert!(fake_peer::heard(&at.0).contains(&allowed));
}
