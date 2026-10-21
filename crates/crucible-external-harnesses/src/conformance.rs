//! What every dialect must survive, run against its own fake peer.
//!
//! [`run`] plays each case of [`CASES`] through [`Host`] with the fake peer
//! installed under the dialect's program name, one install per case in a
//! directory of its own, and holds the turn to the outcome the case names. A
//! dialect brings only what is its own: a [`Set`] of the lines its program
//! would write, the exchanges that open a turn, and the lines its program
//! must hear back. The suite composes every reply from those, so no dialect's
//! tests repeat it, and it judges what the program heard against the set,
//! never against the dialect under test.
//!
//! The suite builds a [`Held`] for a route of its own and launches the fake
//! peer itself. It is compiled under `proof` alone, never ships, and is the
//! one file besides the application's that the repository check lets do
//! either.

use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crucible_execution::{
    Decision, Event, Failure, Lasting, Outcome, PENDINGS, PendingRef, SessionRef, Sink, Stale,
    TurnRequest,
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

/// How often the beat of a grandchild is looked for while its turn runs.
const LOOKED_FOR: Duration = Duration::from_millis(20);

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
/// replies from, and what it must hear.
#[derive(Debug, Clone)]
pub struct Set {
    /// The exchanges that begin a turn, in order: how many frames the dialect
    /// writes, then the lines its program says back before the next.
    pub opening: Vec<(u32, Vec<String>)>,
    /// A line reporting an effect under way: a tool started or a file
    /// changed.
    pub effect: String,
    /// The line that asks permission under the program's name `native`.
    pub ask: fn(native: &str) -> String,
    /// The line the program must hear when the question `native` is decided
    /// `decision`.
    pub answer: fn(native: &str, decision: &Decision) -> String,
    /// The line that ends a turn answered.
    pub answered: String,
    /// The line that ends a turn cancelled.
    pub cancelled: String,
    /// The line the program hears when its open turn is stopped, where the
    /// dialect sends one.
    pub stop: Option<String>,
}

impl Set {
    /// A reply that plays the exchanges that open a turn.
    fn opened(&self) -> Reply {
        self.opening
            .iter()
            .fold(Reply::new(), |reply, (heard, said)| {
                said.iter().fold(reply.hear(*heard), Reply::say)
            })
    }

    /// How many frames the dialect writes to open a turn.
    fn opening_frames(&self) -> u32 {
        self.opening.iter().map(|(heard, _)| heard).sum()
    }
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
    /// Asks the turn to stop as soon as it reports an effect under way.
    InterruptOnEffect,
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
            (React::Interrupt, Event::Started { .. })
            | (React::InterruptOnEffect, Event::ToolActivity { .. } | Event::FileChanged { .. }) => {
                self.control.interrupt();
            }
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
        let opened = || set.opened();
        match index {
            0 => {
                // The program reads on to the end of its input, so a frame the
                // dialect writes past its opening is counted.
                let reply = opened().say(&set.answered).hear_out();
                let played = self.one(&here, reply, React::Nothing, None).await?;
                expect(&played.result, Outcome::Answered)?;
                let heard = fake_peer::heard(&here);
                if heard.len() != usize::try_from(set.opening_frames()).unwrap_or(usize::MAX) {
                    return Err(format!("the agent heard {heard:?} where the turn opened"));
                }
                Ok(())
            }
            1 => {
                let reply = opened().say((set.ask)("q")).hear(1).say(&set.answered);
                let played = self.one(&here, reply, React::Allow, None).await?;
                expect(&played.result, Outcome::Answered)?;
                let answer = (set.answer)("q", &Decision::Allow(Lasting::Once));
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
                let reply = opened().say(&set.effect);
                let reply = match set.stop {
                    Some(_) => reply.hear(1),
                    None => reply.pause(Duration::from_millis(100)),
                };
                let reply = reply.say((set.ask)("late")).hear(1).say(&set.cancelled);
                let played = self
                    .one(&here, reply, React::InterruptOnEffect, None)
                    .await?;
                expect(&played.result, Outcome::Cancelled)?;
                if played.posted(|event| matches!(event, Event::Pending(..))) {
                    return Err("a question asked after the stop was shown".to_owned());
                }
                let heard = fake_peer::heard(&here);
                if let Some(stop) = &set.stop
                    && !heard.contains(stop)
                {
                    return Err(format!("the agent heard {heard:?}, and no stop"));
                }
                let denial = (set.answer)("late", &Decision::Deny);
                match heard.last() {
                    Some(last) if *last == denial => Ok(()),
                    last => Err(format!("the agent last heard {last:?}, not a denial")),
                }
            }
            11 => {
                // Stopped as it starts, the dialect may not have written past
                // its first exchange, so the program waits on no more.
                let first = set.opening.first().map_or(0, |(heard, _)| *heard);
                let reply = Reply::new().hear(first).linger().pause(ASLEEP);
                // A beat is a truncate and then a write, so a stop landing
                // between the two leaves nothing to read afterwards: whether
                // the grandchild began is seen while the turn runs.
                let playing = self.one(&here, reply, React::Interrupt, None);
                tokio::pin!(playing);
                let mut began = false;
                let played = loop {
                    tokio::select! {
                        played = &mut playing => break played?,
                        () = tokio::time::sleep(LOOKED_FOR), if !began => {
                            began = fake_peer::lingering(&here).is_some();
                        }
                    }
                };
                expect(&played.result, Outcome::Cancelled)?;
                if !began {
                    return Err("the agent's own process never began".to_owned());
                }
                let first = fake_peer::lingering(&here);
                tokio::time::sleep(WATCHED).await;
                match fake_peer::lingering(&here) {
                    last if last == first => Ok(()),
                    last => Err(format!(
                        "the agent's own process still beats ({first:?} then {last:?})"
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
        let first = set.opened().say((set.ask)("q")).say(&set.answered);
        let second = set.opened().say(&set.answered);
        let program = installed(here, self.dialect, &Script::of([first, second]))?;
        let control = Control::new().ok_or("no control could be numbered")?;
        let played = self.turn(&program, &control, React::Nothing, None).await?;
        expect(&played.result, Outcome::Answered)?;
        let reference = asked(&played)?;
        let played = self
            .turn(&program, &control, React::Decide(reference), None)
            .await?;
        expect(&played.result, Outcome::Answered)?;
        // Refused for its run, which is checked before its turn: a run that
        // was not numbered again would refuse it only as settled.
        match played.sink.decided() {
            Some(Err(Undecided::Stale(Stale::Generation))) => Ok(()),
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
        let shape = Shape {
            workspace: Some(here),
            ..Shape::default()
        };
        let mut launch =
            Launch::new(found, policy(here)?, Held::granted(ROUTE)).asking(dialect.request(shape));
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
        host.stop(STOP_GRACE)
            .await
            .map_err(|unstopped| unstopped.to_string())?;
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

#[cfg(test)]
mod tests {
    //! The suite's cases, held to what they say they judge.

    use std::fs::OpenOptions;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::ExitStatus;
    use std::sync::Arc;

    use crucible_execution::Decision;
    use crucible_runtime::BoxFuture;
    use crucible_sandbox::{
        SandboxBackendIdentity, SandboxCapabilities, SandboxCommand, SandboxError, SandboxInput,
        SandboxInspection, SandboxLaunch, SandboxLifecycle, SandboxOutput, SandboxProcess,
        SandboxRequest, SandboxService, SandboxSession, SandboxUsage, SandboxViolation,
    };
    use crucible_sandbox_local::LocalSandbox;
    use tokio::runtime::Handle;

    use super::{CASES, Set, Suite};
    use crate::fake_peer::{self, Plain};
    use crate::tests::Owned;

    /// The local sandbox, except that a stop leaves the grandchild's beat as
    /// a stop landing between a beat's truncating open and its write does:
    /// empty, for good.
    struct Torn(Arc<dyn SandboxService>);

    impl SandboxService for Torn {
        fn probe(
            &self,
        ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>>
        {
            self.0.probe()
        }

        fn prepare(
            &self,
            request: SandboxRequest,
        ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
            // The fake peer's grandchild beats into the file of this name in
            // the directory it was started in; the test checks it was torn.
            let beat = request.policy().working_directory().join("lingering");
            Box::pin(async move {
                let inner = self.0.prepare(request).await?;
                let session: Box<dyn SandboxSession> = Box::new(TornSession { inner, beat });
                Ok(session)
            })
        }
    }

    /// A session whose started process tears the beat at `beat` when stopped.
    struct TornSession {
        inner: Box<dyn SandboxSession>,
        beat: PathBuf,
    }

    impl SandboxSession for TornSession {
        fn inspection(&self) -> &SandboxInspection {
            self.inner.inspection()
        }

        fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
            self.inner.materialize()
        }

        // The host starts its agent and never stages one apart.
        fn stage<'a>(
            self: Box<Self>,
            command: SandboxCommand,
        ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
        where
            Self: 'a,
        {
            self.inner.stage(command)
        }

        fn start<'a>(
            self: Box<Self>,
            command: SandboxCommand,
        ) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
        where
            Self: 'a,
        {
            Box::pin(async move {
                let inner = self.inner.start(command).await?;
                let process: Box<dyn SandboxProcess> = Box::new(TornProcess {
                    inner,
                    beat: self.beat,
                });
                Ok(process)
            })
        }
    }

    /// The local process, which truncates the beat at `beat` once stopped.
    struct TornProcess {
        inner: Box<dyn SandboxProcess>,
        beat: PathBuf,
    }

    impl SandboxProcess for TornProcess {
        fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
            self.inner.take_stdin()
        }

        fn take_async_stdin(&mut self) -> Option<Box<dyn SandboxInput>> {
            self.inner.take_async_stdin()
        }

        fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
            self.inner.take_stdout()
        }

        fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
            self.inner.take_stderr()
        }

        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            self.inner.try_wait()
        }

        fn ended(&mut self) -> bool {
            self.inner.ended()
        }

        fn publication_outcome(&self) -> Option<SandboxLifecycle> {
            self.inner.publication_outcome()
        }

        fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
            Box::pin(async move {
                let stopped = self.inner.stop().await;
                torn(&self.beat);
                stopped
            })
        }

        fn stop_sync(&mut self) -> io::Result<()> {
            let stopped = self.inner.stop_sync();
            torn(&self.beat);
            stopped
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

    /// Opens `beat` truncating, as a beat does, and writes nothing, as a beat
    /// stopped before its write does. Where there is no beat, makes none.
    fn torn(beat: &Path) {
        let _ = OpenOptions::new().write(true).truncate(true).open(beat);
    }

    /// The lines of the fake peer's own plain dialect.
    fn plain() -> Set {
        Set {
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
        }
    }

    /// A stop that lands between a beat's truncating open and its write leaves
    /// nothing to read afterwards, and the agent it ended had still begun.
    #[tokio::test(flavor = "multi_thread")]
    #[allow(clippy::expect_used)]
    async fn a_stop_that_tears_the_last_beat_still_finds_the_agent_began() {
        let at = Owned::new("torn-beat");
        let set = plain();
        let local: Arc<dyn SandboxService> =
            Arc::new(LocalSandbox::new().watching_on(Handle::current()));
        let suite = Suite {
            service: Arc::new(Torn(local)),
            dialect: || Box::new(Plain),
            set: &set,
            directory: &at.0,
        };
        let index = CASES
            .iter()
            .position(|case| *case == "an unheeded stop ends the agent and what it started")
            .expect("the case is played");

        let judged = suite.case(index).await;

        let here = at.0.join(format!("case-{index}"));
        assert!(
            here.join("lingering").is_file() && fake_peer::lingering(&here).is_none(),
            "the stop tore the beat"
        );
        assert_eq!(judged, Ok(()));
    }
}
