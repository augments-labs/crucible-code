//! What every dialect must survive, run against its own fake peer.
//!
//! [`run`] plays each case of [`CASES`] through [`Host`] with the fake peer
//! installed under the dialect's program name, one install per case in a
//! directory of its own, and holds the turn to the outcome the case names. A
//! dialect brings only what is its own: a [`Set`] of the lines its program
//! would write, and how many frames it opens a turn with. The suite composes
//! every reply from those, so no dialect's tests repeat it.
//!
//! The suite builds a [`Held`] for a route of its own and launches the fake
//! peer itself. It is compiled under `proof` alone, never ships, and is the
//! one file besides the application's that the repository check lets do
//! either.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crucible_execution::{
    Decision, Event, Failure, Lasting, Outcome, PENDINGS, PendingRef, SessionRef, Sink, TurnRequest,
};
use crucible_sandbox::{
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits, SandboxService,
};
use crucible_transport::FRAME_BYTES;
use crucible_types::TurnId;
use tokio::runtime::Handle;

use crate::fake_peer::{self, Reply, Script, Stream};
use crate::host::{Control, Dialect, Host, STOP_GRACE, Shape, Undecided};
use crate::launch::{Discovery, Held, Launch};

/// The route every launch of the suite is held under.
const ROUTE: &str = "agent:conformance";

/// How long a peer waiting on its input is left before its turn is given up
/// on.
const IMPATIENT: Duration = Duration::from_millis(500);

/// How long a peer that ignores a stop is left asleep: far past any grace.
const ASLEEP: Duration = Duration::from_mins(1);

/// How long the beat of a grandchild that should have been stopped is
/// watched.
const WATCHED: Duration = Duration::from_millis(400);

/// Every case, in the order [`run`] plays them.
pub const CASES: &[&str] = &[
    "a clean turn ends answered",
    "a question answered reaches the agent",
    "a crash after an effect ends incomplete with the effect uncertain",
    "an oversized frame ends incomplete",
    "a second question under one name ends incomplete",
    "a decision after its turn ended is stale",
    "a decision for an earlier run is stale",
    "more questions than are kept end incomplete",
    "a flood on standard error still ends answered",
    "an agent waiting on its input is given up on",
    "a question after a stop was asked is denied unseen",
    "an unheeded stop ends the agent and what it started",
];

/// What a dialect's program writes, line by line, for the suite to compose
/// replies from.
#[derive(Debug, Clone)]
pub struct Set {
    /// How many frames the dialect writes to begin a turn.
    pub opening: u32,
    /// A line reporting an effect under way: a tool started or a file
    /// changed.
    pub effect: String,
    /// The line that asks permission under the program's name `native`.
    pub ask: fn(native: &str) -> String,
    /// The line that ends a turn answered.
    pub answered: String,
    /// The line that ends a turn cancelled.
    pub cancelled: String,
}

/// A case that did not end as it must.
#[derive(Debug, thiserror::Error)]
#[error("{case}: {saw}")]
pub struct Broken {
    /// The case, as [`CASES`] names it.
    pub case: &'static str,
    /// What happened instead.
    pub saw: String,
}

/// Plays every case of [`CASES`] with the dialect `dialect` makes, under
/// `service`, each in a directory of its own beneath `directory`, and says
/// which passed.
///
/// # Errors
///
/// The first case that did not end as it must, and what it ended in.
pub async fn run(
    service: Arc<dyn SandboxService>,
    dialect: fn() -> Box<dyn Dialect>,
    set: &Set,
    directory: &Path,
) -> Result<Vec<&'static str>, Broken> {
    let suite = Suite {
        service,
        dialect,
        set,
        directory,
    };
    let mut passed = Vec::with_capacity(CASES.len());
    for (index, case) in CASES.iter().enumerate() {
        suite
            .case(index)
            .await
            .map_err(|saw| Broken { case, saw })?;
        passed.push(*case);
    }
    Ok(passed)
}

/// What every case is played with.
struct Suite<'a> {
    /// The sandbox every launch is started under.
    service: Arc<dyn SandboxService>,
    /// A fresh dialect, one per run.
    dialect: fn() -> Box<dyn Dialect>,
    /// The dialect's lines.
    set: &'a Set,
    /// Where each case gets a directory.
    directory: &'a Path,
}

/// What a case's sink does besides keeping what it was posted.
#[derive(Debug, Clone)]
enum React {
    /// Nothing.
    Nothing,
    /// Asks the turn to stop as soon as it starts.
    Interrupt,
    /// Allows every question as it is raised.
    Allow,
    /// Decides on `reference` as soon as the turn starts, and keeps how that
    /// went.
    Decide(PendingRef),
}

/// Keeps what a turn posted, and reacts as its case needs.
struct Recorder {
    /// The handle the case decides and interrupts through.
    control: Control,
    /// What it does besides keeping.
    react: React,
    /// What was posted.
    events: Mutex<Vec<Event>>,
    /// How a decision made on start went.
    decided: Mutex<Option<Result<(), Undecided>>>,
}

impl Recorder {
    /// A sink for a turn on `control`.
    const fn new(control: Control, react: React) -> Self {
        Self {
            control,
            react,
            events: Mutex::new(Vec::new()),
            decided: Mutex::new(None),
        }
    }

    /// What was posted.
    fn events(&self) -> Vec<Event> {
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How the decision made on start went, where one was made.
    fn decided(&self) -> Option<Result<(), Undecided>> {
        *self.decided.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Sink for Recorder {
    fn post(&self, event: Event) {
        match (&self.react, &event) {
            (React::Interrupt, Event::Started { .. }) => self.control.interrupt(),
            (React::Allow, Event::Pending(reference, _)) => {
                // A refusal here leaves the question open, which the case's
                // own check then sees as a turn that never ended answered.
                let _ = self
                    .control
                    .decide(reference, Decision::Allow(Lasting::Once));
            }
            (React::Decide(reference), Event::Started { .. }) => {
                let went = self
                    .control
                    .decide(reference, Decision::Allow(Lasting::Once));
                *self.decided.lock().unwrap_or_else(PoisonError::into_inner) = Some(went);
            }
            _ => {}
        }
        self.events
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

/// How one turn went.
struct Played {
    /// What it returned.
    result: Result<Outcome, Failure>,
    /// What it posted, and how a decision on start went.
    sink: Recorder,
}

impl Played {
    /// Whether it posted an event `matches` accepts.
    fn posted(&self, matches: fn(&Event) -> bool) -> bool {
        self.sink.events().iter().any(matches)
    }
}

impl Suite<'_> {
    /// The case at `index` of [`CASES`], or what it saw instead of passing.
    async fn case(&self, index: usize) -> Result<(), String> {
        let here = self.directory.join(format!("case-{index}"));
        let set = self.set;
        let opened = || Reply::new().hear(set.opening);
        match index {
            0 => {
                let played = self
                    .one(&here, opened().say(&set.answered), React::Nothing, None)
                    .await?;
                expect(&played.result, Outcome::Answered)?;
                let heard = fake_peer::heard(&here);
                if heard.len() != usize::try_from(set.opening).unwrap_or(usize::MAX) {
                    return Err(format!("the agent heard {heard:?} where the turn opened"));
                }
                Ok(())
            }
            1 => {
                let reply = opened().say((set.ask)("q")).hear(1).say(&set.answered);
                let played = self.one(&here, reply, React::Allow, None).await?;
                expect(&played.result, Outcome::Answered)?;
                let answer = (self.dialect)()
                    .answer("q", &Decision::Allow(Lasting::Once))
                    .map_err(|failure| failure.reason().to_string())?;
                match fake_peer::heard(&here).last() {
                    Some(last) if *last == answer => Ok(()),
                    last => Err(format!("the agent last heard {last:?}, not its answer")),
                }
            }
            2 => {
                let reply = opened().say(&set.effect).exit(9);
                let played = self.one(&here, reply, React::Nothing, None).await?;
                refused(&played.result)?;
                if !played.posted(|event| matches!(event, Event::Uncertain(_))) {
                    return Err("no effect was reported uncertain".to_owned());
                }
                if !played.posted(|event| matches!(event, Event::Process(_))) {
                    return Err("the process's end was not reported".to_owned());
                }
                Ok(())
            }
            3 => {
                let reply = opened().flood(Stream::Out, FRAME_BYTES as u64 + 1);
                let played = self.one(&here, reply, React::Nothing, None).await?;
                refused(&played.result)
            }
            4 => {
                let reply = opened().say((set.ask)("q")).say((set.ask)("q")).hear_out();
                let played = self.one(&here, reply, React::Nothing, None).await?;
                refused(&played.result)
            }
            5 => {
                let reply = opened().say((set.ask)("q")).say(&set.answered);
                let played = self.one(&here, reply, React::Nothing, None).await?;
                expect(&played.result, Outcome::Answered)?;
                let reference = asked(&played)?;
                let control = played.sink.control;
                match control.decide(&reference, Decision::Allow(Lasting::Once)) {
                    Err(Undecided::Stale(_)) => Ok(()),
                    went => Err(format!("a decision after the turn went {went:?}")),
                }
            }
            6 => self.earlier_run(&here).await,
            7 => {
                let mut reply = opened();
                for native in 0..=PENDINGS {
                    reply = reply.say((set.ask)(&format!("q{native}")));
                }
                let played = self
                    .one(&here, reply.hear_out(), React::Nothing, None)
                    .await?;
                match &played.result {
                    Err(failure) if failure.reason().as_str().contains("more questions") => Ok(()),
                    other => Err(format!("the turn ended {other:?}")),
                }
            }
            8 => {
                let reply = opened()
                    .flood(Stream::Err, 4 * FRAME_BYTES as u64)
                    .say(&set.answered);
                let played = self.one(&here, reply, React::Nothing, None).await?;
                expect(&played.result, Outcome::Answered)
            }
            9 => {
                let played = self
                    .one(&here, opened().hear_out(), React::Nothing, Some(IMPATIENT))
                    .await?;
                refused(&played.result)
            }
            10 => {
                let mut reply = opened();
                reply = if (self.dialect)().cancel().is_some() {
                    reply.hear(1)
                } else {
                    reply.pause(Duration::from_millis(100))
                };
                let reply = reply.say((set.ask)("late")).hear(1).say(&set.cancelled);
                let played = self.one(&here, reply, React::Interrupt, None).await?;
                expect(&played.result, Outcome::Cancelled)?;
                if played.posted(|event| matches!(event, Event::Pending(..))) {
                    return Err("a question asked after the stop was shown".to_owned());
                }
                let denial = (self.dialect)()
                    .answer("late", &Decision::Deny)
                    .map_err(|failure| failure.reason().to_string())?;
                match fake_peer::heard(&here).last() {
                    Some(last) if *last == denial => Ok(()),
                    last => Err(format!("the agent last heard {last:?}, not a denial")),
                }
            }
            11 => {
                let reply = opened().linger().pause(ASLEEP);
                let played = self.one(&here, reply, React::Interrupt, None).await?;
                expect(&played.result, Outcome::Cancelled)?;
                let Some(first) = fake_peer::lingering(&here) else {
                    return Err("the agent's own process never began".to_owned());
                };
                tokio::time::sleep(WATCHED).await;
                match fake_peer::lingering(&here) {
                    Some(last) if last == first => Ok(()),
                    last => Err(format!(
                        "the agent's own process still beats ({first} then {last:?})"
                    )),
                }
            }
            _ => Err("no such case".to_owned()),
        }
    }

    /// A question raised in one run is decided while the next run's turn is
    /// running.
    async fn earlier_run(&self, here: &Path) -> Result<(), String> {
        let set = self.set;
        let first = Reply::new()
            .hear(set.opening)
            .say((set.ask)("q"))
            .say(&set.answered);
        let second = Reply::new().hear(set.opening).say(&set.answered);
        let program = installed(here, self.dialect, &Script::of([first, second]))?;
        let control = Control::new().ok_or("no control could be numbered")?;
        let played = self.turn(&program, &control, React::Nothing, None).await?;
        expect(&played.result, Outcome::Answered)?;
        let reference = asked(&played)?;
        let played = self
            .turn(&program, &control, React::Decide(reference), None)
            .await?;
        expect(&played.result, Outcome::Answered)?;
        match played.sink.decided() {
            Some(Err(Undecided::Stale(_))) => Ok(()),
            went => Err(format!("a decision for the earlier run went {went:?}")),
        }
    }

    /// One turn over a fresh install playing `reply`.
    async fn one(
        &self,
        here: &Path,
        reply: Reply,
        react: React,
        patience: Option<Duration>,
    ) -> Result<Played, String> {
        let program = installed(here, self.dialect, &Script::of([reply]))?;
        let control = Control::new().ok_or("no control could be numbered")?;
        self.turn(&program, &control, react, patience).await
    }

    /// Launches `program`, in the directory it was installed in, on
    /// `control`, and plays one turn.
    async fn turn(
        &self,
        program: &Path,
        control: &Control,
        react: React,
        patience: Option<Duration>,
    ) -> Result<Played, String> {
        let Discovery::Found(found) = Discovery::of(Some(program)) else {
            return Err(format!(
                "the fake peer at {} was not found",
                program.display()
            ));
        };
        let mut dialect = (self.dialect)();
        let here = program.parent().unwrap_or(program);
        let mut launch = Launch::new(found, policy(here)?, Held::granted(ROUTE))
            .asking(dialect.request(Shape::default()));
        if let Some(patience) = patience {
            launch = launch.waiting(patience);
        }
        let mut host = Host::launch(
            Arc::clone(&self.service),
            Handle::current(),
            control,
            launch,
        )
        .await
        .map_err(|refusal| refusal.to_string())?;
        let session = SessionRef::new("conformance").ok_or("no session reference")?;
        let sink = Recorder::new(control.clone(), react);
        let result = host
            .turn(
                dialect.as_mut(),
                &session,
                &TurnRequest::new(TurnId::FIRST, "conformance"),
                &sink,
            )
            .await;
        host.stop(STOP_GRACE).await;
        Ok(Played { result, sink })
    }
}

/// The fake peer installed in `here` as `dialect`'s program, playing
/// `script`.
fn installed(
    here: &Path,
    dialect: fn() -> Box<dyn Dialect>,
    script: &Script,
) -> Result<std::path::PathBuf, String> {
    let program = dialect().program();
    fake_peer::install(here, program, script).map_err(|unbuilt| unbuilt.to_string())
}

/// No confinement, the case's directory its only root: the suite is about
/// the wire, and the sandbox's own claims have suites of their own.
fn policy(here: &Path) -> Result<SandboxPolicy, String> {
    let here = std::fs::canonicalize(here).map_err(|problem| problem.to_string())?;
    let root = SandboxFilesystemRule::new(
        &here,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .map_err(|problem| problem.to_string())?;
    SandboxPolicy::new(
        true,
        [root],
        &here,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::confining(),
    )
    .map(|policy| policy.with_enabled(false))
    .map_err(|problem| problem.to_string())
}

/// The question the turn raised.
fn asked(played: &Played) -> Result<PendingRef, String> {
    played
        .sink
        .events()
        .into_iter()
        .find_map(|event| match event {
            Event::Pending(reference, _) => Some(reference),
            _ => None,
        })
        .ok_or_else(|| "the agent's question was never raised".to_owned())
}

/// Whether the turn ended `wanted`.
fn expect(result: &Result<Outcome, Failure>, wanted: Outcome) -> Result<(), String> {
    match result {
        Ok(outcome) if *outcome == wanted => Ok(()),
        other => Err(format!("the turn ended {other:?}")),
    }
}

/// Whether the turn ended in a failure.
fn refused(result: &Result<Outcome, Failure>) -> Result<(), String> {
    match result {
        Err(_) => Ok(()),
        Ok(outcome) => Err(format!("the turn ended {outcome:?}")),
    }
}
