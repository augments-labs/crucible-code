//! One run of another vendor's agent, started under its sandbox and spoken to
//! over its pipes.
//!
//! [`Host::launch`] is the one place a supervised agent is started. It takes a
//! [`Launch`], which carries the route's content-use hold by value, so a start
//! with no hold does not compile and one hold starts one process. The program
//! is compared with what was selected before anything is prepared, the sandbox
//! prepares it under the launch's policy, and a policy that requires a sandbox
//! is refused, with the claim that could be made, wherever the session that
//! came back is not hard-enforced. Every byte after that goes through
//! `crucible-transport`: frames are bounded at its [`FRAME_BYTES`], standard
//! error is drained and its beginning kept, and every wait is bounded.
//!
//! What the program says is read by a [`Dialect`], which parses its wire and
//! answers in [`Act`]s; this module is what holds the dialect to the contract.
//! The host alone reports a turn's beginning and its end, so a dialect cannot
//! claim an outcome its program never reached: a stream that ends early, a
//! frame that will not parse, a carriage return, an oversized frame or a
//! dialect reporting what only the host reports all end the turn incomplete,
//! never guessed into an answer. Terminal control sequences are stripped from
//! every text before it reaches an event, and every value of the program's
//! environment long enough to be a secret is hidden from what is kept.
//!
//! A question the program asks is raised in the [`Control`]'s table under the
//! generation of this run, so a decision for a run that has since been started
//! again, for a turn that has ended, a second answer or a wrong kind of answer
//! is refused there before the program's own name for the question is looked
//! at. A pending question is never dropped: when the table is full, the turn
//! stops incomplete instead.
//!
//! Stopping a turn asks first, in the dialect's own words, waits out a grace,
//! and only then stops the sandbox's process, which ends the scope it started
//! and nothing else: a host never stops a process it did not start. A run
//! that could not be confirmed to have stopped keeps its agent's one place, so
//! no other run of that agent is started beside it.

use std::borrow::Cow;
use std::fmt;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use crucible_execution::{
    Authentication, Confinement, Decision, Detail, Effects, Event, Failure, Fault, Outcome,
    PENDINGS, Pending, PendingRef, SessionRef, Sink, Stale, Text, TurnRequest, Unraised,
};
use crucible_sandbox::{
    SandboxCapabilities, SandboxCapability, SandboxCommand, SandboxEnvironment, SandboxError,
    SandboxFeature, SandboxManifest, SandboxOutput, SandboxProcess, SandboxRequest, SandboxService,
};
use crucible_transport::{FRAME_BYTES, Finish, Frames, Heard, Muttered, Pipes, Said, Written};
use crucible_types::{Ancestry, SandboxId, ToolId};
use tokio::runtime::Handle;
use tokio::sync::{Notify, mpsc};
use tokio::time::Instant;

use crate::launch::{Launch, Refusal, Request};
use crate::pending::Asked;

/// The longest a working agent may say nothing before its turn is given up
/// on. A model thinking through a hard step can be quiet for minutes; one
/// quiet for longer than this has stopped, whatever it is waiting for.
pub const SILENCE: Duration = Duration::from_mins(10);

/// The longest a turn waits while a question it asked is open, which is the
/// user's time to answer and not the agent's to spend.
pub const WAITING: Duration = Duration::from_hours(24);

/// How long an agent asked to stop in its own words has to do so before its
/// process is stopped.
pub const CANCEL_GRACE: Duration = Duration::from_secs(5);

/// How long a process whose input has been closed has to exit on its own
/// before it is stopped.
pub const STOP_GRACE: Duration = Duration::from_secs(2);

/// How long the sandbox has to prepare a launch.
pub const PREPARING: Duration = Duration::from_secs(30);

/// The most bytes an account step's output keeps: a version line or a status
/// document, never a stream.
pub const RAN_BYTES: usize = 64 * 1024;

/// The shortest environment value hidden from kept text. A shorter one occurs
/// by chance in ordinary output, and hiding it would hide that output instead.
pub const WITHHELD_BYTES: usize = 8;

/// Which account step a dialect is asked to start.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Account {
    /// What version the program is.
    Version,
    /// Whether it is signed in.
    Status,
    /// Signing it in.
    SignIn,
    /// Signing it out.
    SignOut,
}

/// What a launch is shaped by besides its program.
#[derive(Debug, Clone, Copy, Default)]
pub struct Shape<'a> {
    /// The agent's own name for the session to resume, where there is one.
    pub resume: Option<&'a str>,
    /// The profile directory, where the route has one.
    pub profile: Option<&'a Path>,
}

/// What an account step's run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ran {
    /// Its exit code, where it exited with one.
    pub code: Option<i32>,
    /// The lines it wrote to standard output, at most [`RAN_BYTES`] of them.
    pub said: Vec<String>,
    /// Whether anything it wrote was left out or could not be read.
    pub cut: bool,
    /// The beginning of its standard error, with every withheld value hidden.
    pub muttered: String,
}

/// What a dialect read out of an account step's run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reading {
    /// The version it reported.
    Version(Text),
    /// The account state it reported.
    Signed(Authentication),
    /// It said nothing the dialect could read.
    Unreadable(Failure),
}

/// What a dialect makes of one frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Act {
    /// Report this. Only what the program did during the turn: a delta, a
    /// tool's activity or report, a changed file, usage or background work.
    Say(Event),
    /// The program's own name for the session, by which it is resumed.
    Session(String),
    /// The program asks this, under its own name `native`.
    Ask {
        /// The program's name for the question, which its answer carries.
        native: String,
        /// What it asks.
        pending: Pending,
    },
    /// Send the program this frame.
    Send(String),
    /// The program reports that the turn failed.
    Fail(Failure),
    /// The program ended the turn this way.
    End(Outcome),
}

/// One vendor's wire, read and written.
///
/// A dialect parses what its program says and composes what is said to it,
/// and does nothing else: it starts nothing, keeps nothing past its turn and
/// decides nothing about outcomes. Every step that would start a process
/// returns a [`Request`] for the caller to start under a hold of its own.
pub trait Dialect: Send {
    /// The command the program is looked up as, without any suffix the
    /// platform adds.
    fn program(&self) -> &'static str;

    /// What a turn's process is started with.
    fn request(&self, shape: Shape<'_>) -> Request;

    /// The frames that begin `turn`.
    ///
    /// # Errors
    ///
    /// A [`Failure`] where the turn cannot be said on this wire.
    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure>;

    /// What `frame` means.
    ///
    /// # Errors
    ///
    /// A [`Failure`] where it is not a frame of this wire, which ends the turn
    /// incomplete.
    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure>;

    /// The frame that answers the question named `native` with `decision`.
    ///
    /// # Errors
    ///
    /// A [`Failure`] where this wire cannot carry that answer.
    fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure>;

    /// The frame that asks the program to stop the turn, where its wire has
    /// one.
    fn cancel(&mut self) -> Option<String>;

    /// What the account step `step` is started with, where the program has
    /// one.
    fn account(&self, step: Account, shape: Shape<'_>) -> Option<Request>;

    /// What `step`'s run reported.
    fn read(&self, step: Account, ran: &Ran) -> Reading;
}

/// A decision settled in the table and on its way to the program.
#[derive(Debug)]
struct Decided {
    /// The program's name for the question.
    native: Box<str>,
    /// What was decided.
    decision: Decision,
}

/// What every run of one agent shares: its question table, the decisions on
/// their way to the running turn, and the request to stop it.
struct Shared {
    /// The questions open, under both their names.
    asked: Mutex<Asked>,
    /// Where a settled decision is put for the running turn.
    sender: mpsc::Sender<Decided>,
    /// Where the running turn takes them from, held by that turn alone.
    receiver: tokio::sync::Mutex<mpsc::Receiver<Decided>>,
    /// Whether stopping the running turn has been asked for.
    interrupted: AtomicBool,
    /// What a stop request rings.
    bell: Notify,
    /// Whether a run is live, which is the bound of one process at a time.
    running: AtomicBool,
    /// Whether a run could not be confirmed to have stopped, which keeps
    /// `running` held for good.
    unstopped: AtomicBool,
}

/// The handle a backend decides and interrupts through, shared by every run
/// of one agent.
#[derive(Clone)]
pub struct Control {
    /// What every run shares.
    shared: Arc<Shared>,
}

impl fmt::Debug for Control {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Control")
            .field("running", &self.shared.running.load(Ordering::SeqCst))
            .finish_non_exhaustive()
    }
}

/// Why a decision did not reach the program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Undecided {
    /// It settles nothing open.
    #[error(transparent)]
    Stale(#[from] Stale),
    /// Decisions are arriving faster than the running turn takes them; the
    /// question stays open and can be answered again.
    #[error("too many decisions are on their way to this agent; try again")]
    Full,
}

impl Control {
    /// A handle with nothing open, or nothing once this process has numbered
    /// every instance it can.
    #[must_use]
    pub fn new() -> Option<Self> {
        let (sender, receiver) = mpsc::channel(PENDINGS);
        Some(Self {
            shared: Arc::new(Shared {
                asked: Mutex::new(Asked::new()?),
                sender,
                receiver: tokio::sync::Mutex::new(receiver),
                interrupted: AtomicBool::new(false),
                bell: Notify::new(),
                running: AtomicBool::new(false),
                unstopped: AtomicBool::new(false),
            }),
        })
    }

    /// Settles what `reference` names with `decision`, and hands the answer to
    /// the running turn.
    ///
    /// The place in the queue is taken before the table is touched, so a
    /// decision that cannot be delivered leaves its question open rather than
    /// settled and lost; and it is sent while the table is still held, so a
    /// turn that begins after it finds it queued and drops it, and none can
    /// begin between the two and take it for its own question.
    ///
    /// # Errors
    ///
    /// [`Undecided::Stale`] for a decision the table refuses, and
    /// [`Undecided::Full`] where no place is left in the queue.
    pub fn decide(&self, reference: &PendingRef, decision: Decision) -> Result<(), Undecided> {
        // A closed queue cannot happen while this holds its receiver, and is
        // refused like a full one rather than assumed away.
        let permit = self
            .shared
            .sender
            .try_reserve()
            .map_err(|_| Undecided::Full)?;
        let mut asked = self.asked();
        let (native, _) = asked.settle(reference, &decision)?;
        permit.send(Decided { native, decision });
        drop(asked);
        Ok(())
    }

    /// Asks the running turn to stop, or the next to run where none is yet
    /// reading: a stop asked for just as a turn begins is not lost to it.
    /// [`Host::rest`] forgets one nothing is left to heed.
    pub fn interrupt(&self) {
        self.shared.interrupted.store(true, Ordering::SeqCst);
        self.shared.bell.notify_one();
    }

    /// Forgets a stop asked for after the turn it was meant for had ended, so
    /// that it does not stop the next one. A turn rests as it ends and a run
    /// as it starts; a holder of runs rests one it is given back, through
    /// [`Host::rest`].
    pub(crate) fn rest(&self) {
        self.shared.interrupted.store(false, Ordering::SeqCst);
    }

    /// What is open, oldest first.
    #[must_use]
    pub fn open(&self) -> Vec<PendingRef> {
        self.asked().open().cloned().collect()
    }

    /// The table, whether or not a holder panicked: every change to it is a
    /// whole step, so what a panic left behind is still a table.
    fn asked(&self) -> MutexGuard<'_, Asked> {
        self.shared
            .asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

/// The questions of a running turn, closed however the turn ends: a turn
/// whose caller stopped waiting leaves nothing open that could be answered
/// and never sent.
struct Asking<'s>(&'s Shared);

impl Drop for Asking<'_> {
    fn drop(&mut self) {
        self.0
            .asked
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .end();
    }
}

/// How one exchange with the program ended, before the host reports it.
enum Ending {
    /// The program ended the turn.
    Ended(Outcome),
    /// The program reported that the turn failed.
    Failed(Failure),
    /// It was asked to stop and did not within its grace.
    Unheeded,
    /// The stream stopped being one the host could read, and this is what was
    /// seen.
    Lost(String),
}

/// What one turn carries through its exchange.
struct Turning<'a> {
    /// What reads and writes the program's wire.
    dialect: &'a mut dyn Dialect,
    /// Where what it does is posted.
    sink: &'a dyn Sink,
    /// The last effect it reported starting, which a turn cut short may have
    /// left half done.
    under_way: Option<Text>,
    /// When a turn asked to stop is stopped regardless, once it has been
    /// asked.
    cancelling: Option<Instant>,
}

/// One run of an agent's program.
pub struct Host {
    /// The process, kept to be watched and stopped.
    process: Box<dyn SandboxProcess>,
    /// What it says.
    frames: Frames<Heard<Box<dyn SandboxOutput>>>,
    /// What is said to it, until its input is closed.
    written: Option<Written<Said>>,
    /// What it says beside the conversation.
    muttered: Muttered,
    /// What it is decided and interrupted through.
    control: Control,
    /// How far the sandbox holds it in.
    claim: Confinement,
    /// Every value of its environment long enough to be a secret.
    withheld: Vec<Box<str>>,
    /// Its own name for the session, once it has said one.
    session: Option<Box<str>>,
    /// The silence a working turn sits through.
    patience: Duration,
    /// How it finished, once it has.
    finished: Option<Finish>,
}

impl fmt::Debug for Host {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Host")
            .field("claim", &self.claim)
            .field("withheld", &self.withheld.len())
            .field("finished", &self.finished.is_some())
            .finish_non_exhaustive()
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        // A process that may still be running still takes the agent's place.
        if matches!(self.finished, Some(Finish::Unreaped(_))) {
            self.control.shared.unstopped.store(true, Ordering::SeqCst);
        } else {
            self.control.shared.running.store(false, Ordering::SeqCst);
        }
    }
}

impl Host {
    /// Starts what `launch` describes under `service`, with its pipes read and
    /// written on `runtime`, as a new run of the agent `control` speaks to.
    ///
    /// Every question the agent's previous run asked is closed first, because
    /// nothing can answer it now. Nothing is prepared for a program that is no
    /// longer the one selected, and nothing is started under a policy that
    /// requires a sandbox when what the sandbox prepared is not
    /// hard-enforced.
    ///
    /// # Errors
    ///
    /// [`Refusal`] for each reason the launch did not start a process, or
    /// started one and stopped it again.
    pub async fn launch(
        service: Arc<dyn SandboxService>,
        runtime: Handle,
        control: &Control,
        launch: Launch,
    ) -> Result<Self, Refusal> {
        launch.program().unchanged()?;
        if control.shared.unstopped.load(Ordering::SeqCst) {
            return Err(Refusal::Unstopped);
        }
        if control
            .shared
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(Refusal::Busy);
        }
        // Released again on every refusal below; from the moment a host
        // exists, its drop releases it, unless the run could not be confirmed
        // to have stopped.
        control.rest();
        let started = Self::start(service, &runtime, control, launch).await;
        if started.is_err() {
            control.shared.running.store(false, Ordering::SeqCst);
        }
        started
    }

    /// The launch itself, once this agent is known to have no other run.
    async fn start(
        service: Arc<dyn SandboxService>,
        runtime: &Handle,
        control: &Control,
        launch: Launch,
    ) -> Result<Self, Refusal> {
        if !control.asked().relaunch() {
            return Err(Refusal::Spent);
        }
        let required = launch.policy().enabled();
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("external-agent"),
            launch.policy().clone(),
            SandboxManifest::empty(),
        );
        let prepared = tokio::time::timeout(PREPARING, service.prepare(request))
            .await
            .unwrap_or_else(|_| {
                Err(SandboxError::BackendUnavailable {
                    reason: "the sandbox did not prepare the agent in time".into(),
                })
            });
        let mut session = match prepared {
            Ok(session) => session,
            Err(
                problem @ (SandboxError::Unsupported { .. }
                | SandboxError::BackendUnavailable { .. }),
            ) if required => {
                // A sandbox that could hold the agent in fully and did not
                // prepare it failed; it is not the most this agent can have.
                // Bounded as preparing was: a sandbox that does not say is
                // taken to hold nothing.
                return Err(
                    match tokio::time::timeout(PREPARING, service.probe()).await {
                        Ok(Ok((_, capabilities))) => match least(&capabilities) {
                            Confinement::HardEnforced => problem.into(),
                            claim => Refusal::Unconfined(claim),
                        },
                        Ok(Err(_)) | Err(_) => Refusal::Unconfined(Confinement::Unsupported),
                    },
                );
            }
            Err(problem) => return Err(problem.into()),
        };
        let inspection = session.inspection();
        let claim = if inspection.confined() {
            Confinement::HardEnforced
        } else {
            least(inspection.capabilities())
        };
        if required && claim != Confinement::HardEnforced {
            return Err(Refusal::Unconfined(claim));
        }
        session.materialize().await?;
        let Request {
            arguments,
            environment,
        } = launch.request();
        let pairs = environment
            .iter()
            .map(|(name, value)| (name.as_str(), std::ffi::OsStr::new(value.as_str())));
        let command = SandboxCommand::new(
            launch.program().path(),
            arguments.iter().map(std::ffi::OsString::from),
            SandboxEnvironment::new(pairs)?,
        )?
        .spoken_to();
        let withheld = environment
            .iter()
            .map(|(_, value)| value)
            .filter(|value| value.len() >= WITHHELD_BYTES)
            .map(|value| value.as_str().into())
            .collect();
        let patience = launch.patience();
        // Preparing can take a while, and the program is run as it was found
        // or not at all.
        launch.program().unchanged()?;
        let mut process = session.start(command).await?;
        let pipes = Pipes::taken(process.as_mut(), patience, runtime)
            .await
            .map_err(|_| Refusal::Unspoken)?;
        Ok(Self {
            process,
            frames: Frames::new(pipes.heard),
            written: Some(Written::new(pipes.said)),
            muttered: pipes.muttered,
            control: control.clone(),
            claim,
            withheld,
            session: None,
            patience,
            finished: None,
        })
    }

    /// How far the sandbox holds this run in, as its session was inspected.
    #[must_use]
    pub const fn claim(&self) -> Confinement {
        self.claim
    }

    /// What this run is decided and interrupted through.
    #[must_use]
    pub const fn control(&self) -> &Control {
        &self.control
    }

    /// Whether the run has ended, so that it takes no further turn.
    #[must_use]
    pub const fn ended(&self) -> bool {
        self.finished.is_some()
    }

    /// Forgets a stop asked for after the turn it was meant for had ended,
    /// for a holder this run was given back to between turns.
    pub fn rest(&self) {
        self.control.rest();
    }

    /// The program's own name for the session, once it has said one.
    #[must_use]
    pub fn session(&self) -> Option<&str> {
        self.session.as_deref()
    }

    /// Runs `turn` of `session` over this run, read through `dialect`, and
    /// posts what it does to `sink`, from its start to its end.
    ///
    /// # Errors
    ///
    /// The [`Failure`] the turn ended in, after it has been posted: the
    /// program's own, or the host's account of a stream it stopped being able
    /// to read, which ends this run.
    pub async fn turn(
        &mut self,
        dialect: &mut dyn Dialect,
        session: &SessionRef,
        turn: &TurnRequest,
        sink: &dyn Sink,
    ) -> Result<Outcome, Failure> {
        let shared = Arc::clone(&self.control.shared);
        let mut queue = shared.receiver.lock().await;
        // The table is begun before the queue is emptied: what is queued was
        // sent under the table's lock for a turn that has ended, and nothing
        // settled after this can be for any turn but this one.
        self.control.asked().begin(session.clone(), turn.turn());
        let asking = Asking(&shared);
        while queue.try_recv().is_ok() {}
        sink.post(Event::Started { turn: turn.turn() });
        let mut turning = Turning {
            dialect,
            sink,
            under_way: None,
            cancelling: None,
        };
        let ending = match turning.dialect.open(turn) {
            Ok(frames) => match self.say(&frames).await {
                Ok(()) => self.exchange(&mut turning, &mut queue, &shared).await,
                Err(lost) => Ending::Lost(lost),
            },
            Err(failure) => Ending::Failed(failure),
        };
        let under_way = turning.under_way;
        drop(asking);
        self.control.rest();
        drop(queue);
        match ending {
            Ending::Ended(outcome) => {
                sink.post(Event::Finished(outcome));
                Ok(outcome)
            }
            Ending::Failed(failure) => {
                let failure = Failure::new(self.shown(failure.reason()).as_str());
                sink.post(Event::Failed(failure.clone()));
                Err(failure)
            }
            Ending::Unheeded => {
                self.finish(Duration::ZERO).await;
                if let Some(effect) = under_way {
                    sink.post(Event::Uncertain(Effects::new(effect)));
                }
                sink.post(Event::Finished(Outcome::Cancelled));
                Ok(Outcome::Cancelled)
            }
            Ending::Lost(seen) => {
                let ended = self.finish(STOP_GRACE).await;
                let tail = self.aside();
                let mut account = format!("{seen}; {ended}");
                if !tail.trim().is_empty() {
                    account.push_str("; it said: ");
                    account.push_str(tail.trim());
                }
                let seen = self.shown(&Text::new(&account));
                if let Some(effect) = under_way {
                    sink.post(Event::Uncertain(Effects::new(effect)));
                }
                sink.post(Event::Process(Fault::new(seen.clone())));
                let failure = Failure::new(seen.as_str());
                sink.post(Event::Failed(failure.clone()));
                Err(failure)
            }
        }
    }

    /// Reads and answers until the turn ends.
    async fn exchange(
        &mut self,
        turning: &mut Turning<'_>,
        queue: &mut mpsc::Receiver<Decided>,
        shared: &Shared,
    ) -> Ending {
        loop {
            let deadline = turning.cancelling.unwrap_or_else(Instant::now);
            tokio::select! {
                biased;
                Some(decided) = queue.recv() => {
                    let answered = turning.dialect.answer(&decided.native, &decided.decision);
                    let frame = match answered {
                        Ok(frame) => frame,
                        Err(failure) => return Ending::Lost(failure.reason().to_string()),
                    };
                    if let Err(lost) = self.say(&[frame]).await {
                        return Ending::Lost(lost);
                    }
                    self.waited();
                }
                () = shared.bell.notified() => {
                    if shared.interrupted.swap(false, Ordering::SeqCst)
                        && turning.cancelling.is_none()
                    {
                        if let Some(frame) = turning.dialect.cancel()
                            && let Err(lost) = self.say(&[frame]).await
                        {
                            return Ending::Lost(lost);
                        }
                        turning.cancelling = Instant::now().checked_add(CANCEL_GRACE);
                    }
                }
                () = tokio::time::sleep_until(deadline), if turning.cancelling.is_some() => {
                    return Ending::Unheeded;
                }
                frame = self.frames.next_frame_async() => {
                    let line = match frame {
                        None => {
                            return Ending::Lost(
                                "the agent stopped before it finished the turn".to_owned(),
                            );
                        }
                        Some(Err(problem)) => return Ending::Lost(problem.to_string()),
                        Some(Ok(line)) => line,
                    };
                    if line.contains('\r') {
                        return Ending::Lost(
                            "the agent ended a line with a carriage return".to_owned(),
                        );
                    }
                    let acts = match turning.dialect.heard(&line) {
                        Ok(acts) => acts,
                        Err(failure) => return Ending::Lost(failure.reason().to_string()),
                    };
                    for act in acts {
                        if let Some(ending) = self.act(act, turning).await {
                            return ending;
                        }
                    }
                }
            }
        }
    }

    /// Does what one act asks, or says how the turn ended.
    async fn act(&mut self, act: Act, turning: &mut Turning<'_>) -> Option<Ending> {
        match act {
            Act::Say(event) => match self.reported(event) {
                Ok(event) => {
                    match &event {
                        Event::ToolActivity { summary, .. } => {
                            turning.under_way = Some(summary.clone());
                        }
                        Event::FileChanged { path } => turning.under_way = Some(path.clone()),
                        _ => {}
                    }
                    turning.sink.post(event);
                    None
                }
                Err(lost) => Some(Ending::Lost(lost)),
            },
            Act::Session(native) => {
                if !crate::launch::nameable(&native) {
                    return Some(Ending::Lost(
                        "the agent named its session with a name crucible cannot keep".to_owned(),
                    ));
                }
                self.session = Some(native.into());
                None
            }
            Act::Ask { native, pending } => {
                if turning.cancelling.is_some() {
                    // A question asked after the turn was told to stop is
                    // answered no, here, and never raised: nobody is left to
                    // answer it, and a yes nobody gave is not one to send.
                    let frame = match turning.dialect.answer(&native, &Decision::Deny) {
                        Ok(frame) => frame,
                        Err(failure) => return Some(Ending::Lost(failure.reason().to_string())),
                    };
                    return self.say(&[frame]).await.err().map(Ending::Lost);
                }
                self.ask(&native, pending, turning.sink)
                    .err()
                    .map(Ending::Lost)
            }
            Act::Send(frame) => self.say(&[frame]).await.err().map(Ending::Lost),
            Act::Fail(failure) => Some(Ending::Failed(failure)),
            Act::End(outcome) => Some(Ending::Ended(outcome)),
        }
    }

    /// Raises what the program asked, or says why the stream cannot go on.
    fn ask(&mut self, native: &str, pending: Pending, sink: &dyn Sink) -> Result<(), String> {
        let Some(pending) = self.asked_as_shown(pending) else {
            return Err(
                "the agent asked about text carrying terminal control characters, which \
                 cannot be shown as it would run"
                    .to_owned(),
            );
        };
        let raised = {
            let mut asked = self.control.asked();
            if asked.holds(native) {
                return Err("the agent asked a second question under one name".to_owned());
            }
            asked.raise(native, pending.clone())
        };
        match raised {
            Ok(reference) => {
                sink.post(Event::Pending(reference, pending));
                self.frames.stream_mut().patient_for(WAITING);
                Ok(())
            }
            Err(Unraised::Full) => Err(
                "the agent asked more questions than crucible keeps open; the turn is \
                 stopped rather than one of them dropped"
                    .to_owned(),
            ),
            Err(problem @ (Unraised::Idle | Unraised::Exhausted | Unraised::Cut)) => {
                Err(problem.to_string())
            }
        }
    }

    /// An event the dialect reported, as it may be shown, or why it may not.
    fn reported(&self, event: Event) -> Result<Event, String> {
        match event {
            Event::Delta(text) => Ok(Event::Delta(self.shown(&text))),
            Event::ToolActivity { tool, summary } => Ok(Event::ToolActivity {
                tool: self.shown(&tool),
                summary: self.shown(&summary),
            }),
            Event::ToolReport { tool, report } => Ok(Event::ToolReport {
                tool: self.shown(&tool),
                report: self.shown(&report),
            }),
            Event::FileChanged { path } => Ok(Event::FileChanged {
                path: self.shown(&path),
            }),
            Event::Background(text) => Ok(Event::Background(self.shown(&text))),
            Event::Usage(usage) => Ok(Event::Usage(usage)),
            Event::Opened(_)
            | Event::Started { .. }
            | Event::Pending(..)
            | Event::Finished(_)
            | Event::Failed(_)
            | Event::Uncertain(_)
            | Event::Process(_) => Err(
                "the agent's reader reported what only its host reports, and the stream \
                 cannot be trusted past it"
                    .to_owned(),
            ),
        }
    }

    /// `pending` with every text hidden as kept text is, or nothing where
    /// stripping a control sequence would change what is asked: a question
    /// shown other than as it would run asks about something else.
    fn asked_as_shown(&self, pending: Pending) -> Option<Pending> {
        let clean = |text: &Text| (stripped(text.as_str()) == text.as_str()).then_some(());
        match &pending {
            Pending::Permission {
                tool,
                summary,
                detail,
            } => {
                clean(tool)?;
                clean(summary)?;
                match detail {
                    Detail::Command { sent, .. } | Detail::Network { sent } => clean(sent)?,
                    Detail::Reads | Detail::ReadsOutside | Detail::MutatesFile => {}
                }
            }
            Pending::Question { prompt } => clean(prompt)?,
        }
        Some(match pending {
            Pending::Permission {
                tool,
                summary,
                detail,
            } => Pending::Permission {
                tool: self.shown(&tool),
                summary: self.shown(&summary),
                detail: match detail {
                    Detail::Command { sent, left_running } => Detail::Command {
                        sent: self.shown(&sent),
                        left_running,
                    },
                    Detail::Network { sent } => Detail::Network {
                        sent: self.shown(&sent),
                    },
                    kind @ (Detail::Reads | Detail::ReadsOutside | Detail::MutatesFile) => kind,
                },
            },
            Pending::Question { prompt } => Pending::Question {
                prompt: self.shown(&prompt),
            },
        })
    }

    /// Back to a working agent's patience once nothing is waiting on the user.
    fn waited(&mut self) {
        let open = self.control.asked().open().next().is_some();
        let heard = self.frames.stream_mut();
        heard.patient_for(if open { WAITING } else { self.patience });
        // An answer sent is an exchange's edge: the silence that follows is the
        // agent's, measured from here.
        heard.abandoned_when(None);
    }

    /// Writes each of `frames`, or says why the stream cannot go on.
    async fn say(&mut self, frames: &[String]) -> Result<(), String> {
        for frame in frames {
            if frame.len() > FRAME_BYTES {
                return Err("a frame for the agent was longer than a frame may be".to_owned());
            }
            let Some(written) = self.written.as_mut() else {
                return Err("the agent's input was already closed".to_owned());
            };
            written
                .send_async(frame)
                .await
                .map_err(|problem| problem.to_string())?;
        }
        Ok(())
    }

    /// `text` as it may be kept.
    fn shown(&self, text: &Text) -> Text {
        hidden(text, &self.withheld)
    }

    /// What the program wrote to standard error, as it may be kept.
    ///
    /// Only its beginning is kept, and a footnote says how much followed; the
    /// text before that footnote was cut, and can end partway through a
    /// withheld value. A footnote the program wrote itself only withholds
    /// more.
    fn aside(&self) -> String {
        let said = self.muttered.text();
        let Some((kept, footnote)) = said
            .rsplit_once("\n[")
            .filter(|(_, footnote)| footnote.contains(" further bytes were dropped: "))
        else {
            return self.shown(&Text::new(&said)).as_str().to_owned();
        };
        // A cut inside a character left half of it, read as a replacement
        // that would hide the part of a value before it.
        let kept = kept.trim_end_matches(char::REPLACEMENT_CHARACTER);
        // Clipped one byte short of a character past what is kept, which
        // keeps all of it and says it was cut.
        let mut over = kept.to_owned();
        over.push(' ');
        let shown = self.shown(&Text::clipped(&over, kept.len()));
        let footnote = self.shown(&Text::new(footnote));
        format!("{}\n[{}", shown.as_str(), footnote.as_str())
    }

    /// Closes the process's input, waits `grace` for it to end and stops it
    /// where it does not, once; and says how it ended.
    async fn finish(&mut self, grace: Duration) -> String {
        // An agent reading its input to its end is told there is no more.
        self.written = None;
        if self.finished.is_none() {
            self.finished = Some(Finish::after_async(self.process.as_mut(), grace).await);
        }
        match &self.finished {
            Some(Finish::Exited(status)) => format!("it ended with {status}"),
            Some(Finish::Stopped) => "it was stopped".to_owned(),
            Some(Finish::Unpublished(_)) => "it ended without its work confirmed".to_owned(),
            Some(Finish::Unreaped(_)) | None => {
                "it could not be confirmed to have stopped".to_owned()
            }
        }
    }

    /// Reads an account step's run to its end and reaps it.
    ///
    /// Its input is closed first: a step is told what to do by its arguments,
    /// and one that waits to be typed to reads the end of its input instead.
    /// Standard output is kept a line at a time up to [`RAN_BYTES`]; past
    /// that, and on anything unreadable, the run is marked cut and its process
    /// stopped rather than read further.
    pub async fn run(mut self) -> Ran {
        self.written = None;
        let mut said = Vec::new();
        let mut kept = 0_usize;
        let mut cut = false;
        loop {
            match self.frames.next_frame_async().await {
                None => break,
                Some(Ok(line)) => {
                    kept = kept.saturating_add(line.len());
                    if kept > RAN_BYTES {
                        cut = true;
                        break;
                    }
                    said.push(self.shown(&Text::new(&line)).as_str().to_owned());
                }
                Some(Err(_)) => {
                    cut = true;
                    break;
                }
            }
        }
        let grace = if cut { Duration::ZERO } else { STOP_GRACE };
        self.finish(grace).await;
        let code = match &self.finished {
            Some(Finish::Exited(status)) => status.code(),
            _ => None,
        };
        let muttered = self.aside();
        Ran {
            code,
            said,
            cut,
            muttered,
        }
    }

    /// Ends the run: its input closes, it has `grace` to exit, and it is
    /// stopped where it does not.
    ///
    /// # Errors
    ///
    /// [`Unstopped`] where it could not be confirmed to have stopped, so that
    /// its process may still be running.
    pub async fn stop(mut self, grace: Duration) -> Result<(), Unstopped> {
        self.finish(grace).await;
        let unconfirmed = matches!(self.finished, Some(Finish::Unreaped(_)) | None);
        drop(self);
        if unconfirmed { Err(Unstopped) } else { Ok(()) }
    }
}

/// A run that could not be confirmed to have stopped: its process may still
/// be running.
#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("this agent's run could not be confirmed to have stopped, so it may still be running")]
pub struct Unstopped;

/// The weakest claim among what confines a process at all: its filesystem,
/// its descriptors, its descendants, the kernel's surface and its privileges.
fn least(capabilities: &SandboxCapabilities) -> Confinement {
    let weakest = [
        SandboxFeature::Filesystem,
        SandboxFeature::DescriptorIsolation,
        SandboxFeature::ProcessIsolation,
        SandboxFeature::KernelSurface,
        SandboxFeature::PrivilegeIsolation,
    ]
    .into_iter()
    .map(|feature| capabilities.claim(feature))
    .min()
    .unwrap_or(SandboxCapability::Unsupported);
    match weakest {
        SandboxCapability::Enforced => Confinement::HardEnforced,
        SandboxCapability::Observed => Confinement::Observed,
        SandboxCapability::Unsupported => Confinement::Unsupported,
    }
}

/// `text` as it may be kept: no terminal control sequence, and none of the
/// `withheld` values. Still incomplete where it was.
pub(crate) fn hidden(text: &Text, withheld: &[Box<str>]) -> Text {
    let mut kept = stripped(text.as_str());
    for value in withheld {
        if kept.contains(&**value) {
            kept = Cow::Owned(kept.replace(&**value, "[withheld]"));
        }
    }
    if text.incomplete() {
        // Clipped text can end partway through a value, which shows the part
        // before the cut.
        let cut = withheld
            .iter()
            .filter_map(|value| {
                (1..value.len())
                    .rev()
                    .filter_map(|end| value.get(..end))
                    .find(|part| kept.ends_with(part))
                    .map(str::len)
            })
            .max();
        if let Some(cut) = cut {
            let end = kept.len().saturating_sub(cut);
            kept = Cow::Owned(format!("{}[withheld]", kept.get(..end).unwrap_or_default()));
        }
    }
    match kept {
        Cow::Borrowed(_) => text.clone(),
        Cow::Owned(kept) if text.incomplete() => {
            // Clipped one byte short of a character past what is kept,
            // which keeps all of it and still says something was left out.
            let mut over = kept.clone();
            over.push(' ');
            Text::clipped(&over, kept.len())
        }
        Cow::Owned(kept) => Text::new(&kept),
    }
}

/// `text` without terminal control: escape sequences, the C0 controls but a
/// newline and a tab, delete and the C1 controls are dropped.
#[must_use]
pub fn stripped(text: &str) -> Cow<'_, str> {
    let control = |c: char| (c.is_control() && c != '\n' && c != '\t') || c == '\u{7f}';
    if !text.chars().any(control) {
        return Cow::Borrowed(text);
    }
    let mut kept = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => match chars.next() {
                // A control sequence runs to its final byte.
                Some('[') => {
                    for c in chars.by_ref() {
                        if ('\u{40}'..='\u{7e}').contains(&c) {
                            break;
                        }
                    }
                }
                // An operating system command runs to a bell or a string
                // terminator.
                Some(']') => {
                    while let Some(c) = chars.next() {
                        if c == '\u{7}' {
                            break;
                        }
                        if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                            chars.next();
                            break;
                        }
                    }
                }
                // Any other escape takes one character with it.
                Some(_) | None => {}
            },
            // The eight-bit introducers carry sequences of their own.
            '\u{9b}' => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            c if control(c) => {}
            c => kept.push(c),
        }
    }
    Cow::Owned(kept)
}
