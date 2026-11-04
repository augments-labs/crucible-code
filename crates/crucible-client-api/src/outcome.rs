//! What came of a command: the authoritative answer to one request.
//!
//! An [`Outcome`] says what the application settled, once, after it settled
//! it. It mirrors the outcomes the application already distinguishes for a
//! terminal — a switch that was taken but could not be written down is not the
//! same answer as one that was refused — and carries none of the values those
//! were made from: a failure is a [`Problem`], which is a code and the sentence
//! the failure's owner would have shown a person.

use serde_json::Value;

use crate::bounds::{Name, Text};
use crate::command::{Command, Mode};
use crate::context::Context;
use crate::error::{ErrorCode, Refusal};
use crate::request::Correlation;
use crate::usage::Usage;
use crate::wire::{Fields, Writing, frame, parsed};

/// Something the application tried and could not do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// The stable part.
    pub code: ErrorCode,
    /// What its owner would have shown a person, cut to the ceiling.
    pub message: Text,
}

impl Problem {
    /// A failure that reads as `shown`.
    ///
    /// Takes what a person would have been shown and nothing else: a `Display`
    /// is a sentence written to be read, where a `Debug` is a type's insides.
    #[must_use]
    pub fn failed(shown: &dyn std::fmt::Display) -> Self {
        Self {
            code: ErrorCode::Failed,
            message: Text::cut(&shown.to_string()),
        }
    }

    pub(crate) fn written(&self) -> Value {
        Writing::new()
            .with("code", self.code.as_str())
            .text("message", &self.message)
            .finish()
    }

    pub(crate) fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let problem = Self {
            code: code(&fields.string("code")?)?,
            message: fields.text("message")?,
        };
        fields.done()?;
        Ok(problem)
    }
}

fn code(word: &str) -> Result<ErrorCode, Refusal> {
    ErrorCode::named(word).ok_or_else(|| ErrorCode::Malformed.into())
}

/// What retiring prompt-cache resources ahead of a switch left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Retained {
    /// Resources whose deletion could not be confirmed.
    pub ambiguous: u64,
    /// Resources nothing owns any more.
    pub orphaned: u64,
}

impl Retained {
    fn onto(self, object: Writing) -> Writing {
        object
            .with("ambiguous", self.ambiguous)
            .with("orphaned", self.orphaned)
    }

    fn from(fields: &mut Fields) -> Result<Self, Refusal> {
        Ok(Self {
            ambiguous: fields.number("ambiguous")?,
            orphaned: fields.number("orphaned")?,
        })
    }
}

/// Why a model stopped, as this protocol spells it, and one stop that is
/// no model's: [`Stop::Uncertain`], which only a turn's outcome carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stop {
    /// It finished and yielded.
    Yielded,
    /// It is waiting for tool results.
    WantsTools,
    /// It ran out of room to answer in.
    OutOfTokens,
    /// The request did not fit its window.
    WindowExceeded,
    /// The provider's filter cut it short.
    Filtered,
    /// The provider paused it.
    Paused,
    /// Somebody cancelled it.
    Cancelled,
    /// Somebody stopped it while an installed agent was acting, so what the
    /// agent was doing may or may not have happened. Only a turn's outcome
    /// says this; the turn's own `finished` report calls it cancelled.
    Uncertain,
    /// The provider gave a reason nobody here has heard of.
    Unknown,
}

impl Stop {
    /// Every reason.
    pub const EVERY: [Self; 9] = [
        Self::Yielded,
        Self::WantsTools,
        Self::OutOfTokens,
        Self::WindowExceeded,
        Self::Filtered,
        Self::Paused,
        Self::Cancelled,
        Self::Uncertain,
        Self::Unknown,
    ];

    /// The word it crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Yielded => "yielded",
            Self::WantsTools => "wants_tools",
            Self::OutOfTokens => "out_of_tokens",
            Self::WindowExceeded => "window_exceeded",
            Self::Filtered => "filtered",
            Self::Paused => "paused",
            Self::Cancelled => "cancelled",
            Self::Uncertain => "uncertain",
            Self::Unknown => "unknown",
        }
    }

    pub(crate) fn named(word: &str) -> Result<Self, Refusal> {
        Self::EVERY
            .into_iter()
            .find(|stop| stop.as_str() == word)
            .ok_or_else(|| ErrorCode::Malformed.into())
    }

    fn maybe(fields: &mut Fields) -> Result<Option<Self>, Refusal> {
        fields
            .maybe("stop")
            .map(|value| Self::named(value.as_str().unwrap_or_default()))
            .transpose()
    }
}

/// What a session that cannot take a turn is missing, as this protocol
/// spells it.
///
/// Which of the three it is decides what somebody does next: set a credential
/// up, choose a provider, or choose a model of the provider chosen. A front end
/// with a screen says each as its own sentence, and so can a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Missing {
    /// The host holds no usable credential for any provider.
    Credential,
    /// A provider can be reached and none was chosen.
    Provider,
    /// A provider was chosen and no model of it.
    Model,
}

impl Missing {
    /// Every one of them.
    pub const EVERY: [Self; 3] = [Self::Credential, Self::Provider, Self::Model];

    /// The word it crosses as.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Credential => "credential",
            Self::Provider => "provider",
            Self::Model => "model",
        }
    }

    fn named(word: &str) -> Result<Self, Refusal> {
        Self::EVERY
            .into_iter()
            .find(|missing| missing.as_str() == word)
            .ok_or_else(|| ErrorCode::Malformed.into())
    }
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    /// The model was asked and the turn ran to a stop.
    Ran {
        /// Why it stopped.
        stop: Stop,
    },
    /// A guardrail turned the turn away.
    Rejected {
        /// The guardrail, by name.
        guard: Text,
        /// Its reason.
        why: Text,
        /// Why the model had stopped, where it had been asked at all.
        stop: Option<Stop>,
    },
    /// A guardrail could not decide, so the turn did not go on.
    Undecided {
        /// What stopped it deciding.
        problem: Problem,
        /// Why the model had stopped, where it had been asked at all.
        stop: Option<Stop>,
    },
    /// The turn could not be taken, or was not finished, for the reason the
    /// problem gives. Where a step would have had to wait and was dropped,
    /// what it began is unconfirmed rather than undone.
    Failed(Problem),
    /// Nothing was sent: the route the turn would go on is one whose vendor
    /// says it uses what is sent, or the installed agent's the turn would
    /// start, and no yes to it was given. The question was declined, went
    /// unanswered, or could not be put to this client; a yes that could not
    /// be written down is [`TurnOutcome::Failed`].
    Warned {
        /// The route, as a yes to it is written down.
        route: Text,
        /// What the vendor says, in English.
        sentence: Text,
        /// The page it says it on, and the day that was read.
        source: Text,
    },
}

/// How making room ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoomOutcome {
    /// A recap replaced this many messages.
    Made {
        /// How many.
        replaced: u64,
    },
    /// There was nothing worth replacing.
    Nothing,
    /// It was cancelled, and nothing was replaced, though old tool output
    /// pruned before it stays pruned.
    Stopped,
    /// Making room failed, for the reason the problem gives. Where the recap
    /// could not be had, nothing was replaced, though old tool output pruned
    /// before it stays pruned. Where a step would have had to wait and was
    /// dropped, what it began is unconfirmed and what was done before it
    /// stands, even a replacement already made.
    Failed(Problem),
}

/// How starting a new session ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClearOutcome {
    /// Nothing had been said, so the session in hand is the empty one.
    Nothing,
    /// A new session is being recorded into.
    Started {
        /// What went wrong closing the log of the one left, where anything did.
        unclosed: Option<Problem>,
    },
    /// The new log could not be started; the session in hand is untouched.
    Failed(Problem),
}

/// How picking a session back up ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResumeOutcome {
    /// The session named is the one already in hand.
    Same,
    /// It was picked up, with everything it held.
    Picked {
        /// What went wrong closing the log of the one left, where anything did.
        unclosed: Option<Problem>,
    },
    /// No session of this workspace answers to that identity.
    Unknown,
    /// A provider's model ran it, and the conversation is on an installed
    /// agent's route with no provider set up beside it to ask, since it
    /// started on the route or was logged out there of the one it had: it can
    /// be picked up on the next launch that reaches a provider directly.
    /// Nothing was opened, and the session in hand is untouched.
    NextLaunch,
    /// It could not be picked up; the session in hand is untouched.
    Failed(Problem),
}

/// How asking for a model ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelOutcome {
    /// The model does not serve the effort asked for with it.
    Unsupported,
    /// The provider could not be reached with what the host holds for it.
    Unreachable(Problem),
    /// The prompt cache could not be retired, so nothing was switched.
    CacheHeld(Problem),
    /// It is the model in force.
    Taken {
        /// What retiring the cache left behind.
        retained: Retained,
        /// Why the choice will not outlive the host process, where it will not.
        unwritten: Option<Problem>,
    },
    /// The conversation was on an installed agent's route, which does not
    /// reach the provider, and moved off it onto the provider and this model,
    /// in a new session; the session it left stays listed and can be resumed.
    MovedOff {
        /// The provider the model is asked of.
        provider: Text,
        /// What retiring the cache left behind.
        retained: Retained,
        /// Why the choice will not outlive the host process, where it will not.
        unwritten: Option<Problem>,
    },
    /// The provider is reached through an installed agent's instance the
    /// conversation was not on, and it moved onto that instance, which is
    /// started on this model, in a new session; the session it left stays
    /// listed and can be resumed.
    MovedOnto {
        /// The instance the conversation moved onto.
        onto: Text,
        /// Why the choice will not outlive the host process, where it will not.
        unwritten: Option<Problem>,
    },
}

/// How asking for an effort ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffortOutcome {
    /// There is no model in force to ask it of.
    Unasked,
    /// The model in force does not serve it.
    Unsupported,
    /// It is the effort in force.
    Taken {
        /// Why the choice will not outlive the host process, where it will not.
        unwritten: Option<Problem>,
    },
}

/// How asking for a speed ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpeedOutcome {
    /// There is no model in force to ask it of.
    Unasked,
    /// The model in force has no fast form.
    Unsupported,
    /// The model in force is itself a fast model, with no standard form to
    /// switch to; the other models are chosen with the model.
    Own,
    /// It is the speed asked for from the next turn on.
    Taken {
        /// Why the choice will not outlive the host process, where it will not.
        unwritten: Option<Problem>,
    },
}

/// How adopting a stored credential, or connecting through an installed
/// agent, ended; or the move on an agent a client that takes part in installs
/// chose in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoginOutcome {
    /// The credential is stored and cannot be used.
    Unusable(Problem),
    /// Another provider is answering and keeps the session.
    Elsewhere,
    /// The prompt cache could not be retired, so nothing was switched.
    CacheHeld(Problem),
    /// The provider is now the one answering.
    Serving {
        /// What retiring the cache left behind.
        retained: Retained,
        /// Why the choice will not outlive the host process, where it will not.
        unwritten: Option<Problem>,
    },
    /// The provider is reached through an installed agent's instance from
    /// now on, which is written down, the agent read signed in first.
    AgentConnected {
        /// What the person is told.
        said: Text,
        /// The instance the conversation moved onto, in a new session; the
        /// session it left stays listed and can be resumed. Absent where it
        /// stayed where it was.
        onto: Option<Name>,
    },
    /// The agent read signed in, and the configuration that would reach the
    /// provider through it could not be written, so nothing changed.
    ConnectUnsaved {
        /// What the person is told.
        said: Text,
        /// Why it could not be written.
        problem: Problem,
    },
    /// Nobody chose a way to go on, so nothing changed: the client could not
    /// be asked, or it cancelled or went back.
    Undecided,
    /// The conversation is on an installed agent's route for the provider,
    /// and keeps it: the provider is reached directly, with the credential
    /// stored, from the next launch on.
    NextLaunch {
        /// The provider.
        provider: Text,
        /// Why the next launch will still reach it through the agent, where
        /// that could not be written down.
        unwritten: Option<Problem>,
    },
    /// The conversation is on an installed agent's route, and the credential
    /// is stored for another provider: nothing else changed. A provider this
    /// run reaches through another agent, or one whose new session could not
    /// be started, comes to this.
    Stored {
        /// The provider the credential is for.
        provider: Text,
    },
    /// The conversation was on an installed agent's route, and the credential
    /// is for another provider, which is now the one answering, in a new
    /// session; the session it left stays listed and can be resumed.
    MovedOff {
        /// The provider the credential is for.
        provider: Text,
        /// What retiring the cache left behind.
        retained: Retained,
        /// Why the provider will not be the next run's, where it will not.
        unwritten: Option<Problem>,
    },
    /// A move on an agent was chosen in place of a connect, and the choice
    /// itself changed nothing. One chosen where the agent cannot be started,
    /// or from the catalog's agents, ended the connect before anything was
    /// started, asked of its route or written. A sign-in chosen once the
    /// agent read signed out came after its route's yes, written where it was
    /// asked, and after the agent was started to read its status. It is
    /// offered only to a client that claimed
    /// [`Capability::Install`](crate::Capability::Install), which asks for the
    /// move with the command [`AgentAction::command`] gives.
    AgentChosen {
        /// The move chosen.
        action: AgentAction,
    },
}

/// A move on an installed agent a client can ask the host for: the four the
/// agents door carries out, each named as the catalog or the configuration
/// names what it moves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAction {
    /// Install this version of this catalog agent.
    Install {
        /// The agent, by the name the catalog gives it.
        agent: Name,
        /// The version, as the catalog spells it.
        version: Name,
    },
    /// Move this installed catalog agent to the version the catalog now
    /// offers for it.
    Update {
        /// The agent, by the name the catalog gives it.
        agent: Name,
    },
    /// Move this installed catalog agent back to the version it was on before.
    Rollback {
        /// The agent, by the name the catalog gives it.
        agent: Name,
    },
    /// Adopt a sign-in stored for this instance by this method.
    SignIn {
        /// The instance, by the name the configuration gives it.
        instance: Name,
        /// The method, by the name the agent gives it.
        method: Name,
    },
}

impl AgentAction {
    /// The command that asks for this move.
    #[must_use]
    pub fn command(&self) -> Command {
        match self {
            Self::Install { agent, version } => Command::InstallAgent {
                agent: agent.clone(),
                version: version.clone(),
            },
            Self::Update { agent } => Command::UpdateAgent {
                agent: agent.clone(),
            },
            Self::Rollback { agent } => Command::RollbackAgent {
                agent: agent.clone(),
            },
            Self::SignIn { instance, method } => Command::SignInAgent {
                instance: instance.clone(),
                method: method.clone(),
            },
        }
    }
}

/// What still authenticates a provider once its stored credential is gone.
///
/// Where a credential lives, never the credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Standing {
    /// An environment variable, by name.
    Environment(Text),
    /// Another stored key.
    StoredKey,
    /// A stored account.
    Subscription,
    /// The installed agent the conversation reaches it through.
    External {
        /// The configured instance.
        instance: Text,
    },
}

/// How forgetting a stored credential ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogoutOutcome {
    /// The prompt cache could not be retired, so nothing was forgotten.
    CacheHeld(Problem),
    /// The credential could not be removed.
    Unforgotten {
        /// What retiring the cache left behind.
        retained: Retained,
        /// Why not.
        problem: Problem,
    },
    /// It was removed, and the session is asking somebody else anyway.
    Kept {
        /// What retiring the cache of the provider kept beside an installed
        /// agent's route left behind, where that provider was this one.
        retained: Retained,
    },
    /// It was removed, and the provider is still authenticated another way.
    StillServed {
        /// What retiring the cache left behind.
        retained: Retained,
        /// The other way.
        standing: Standing,
    },
    /// It was removed, and the session is signed out.
    SignedOut {
        /// What retiring the cache left behind.
        retained: Retained,
    },
    /// The instance is out of use from Crucible, durably, and the work it had
    /// running has stopped. The agent's own sign-in was not touched.
    Disconnected {
        /// What the person is told, which says what is known of that sign-in.
        said: Text,
    },
    /// The instance takes no new work and that is written down, but the work
    /// it had running could not be confirmed stopped.
    NewWorkDisabledWorkNotConfirmedStopped {
        /// What the person is told.
        said: Text,
    },
    /// The instance takes no new work in this run, and that could not be
    /// written down, so the next run may use it again.
    StoppedForThisRunOnly {
        /// What the person is told.
        said: Text,
        /// Why it could not be written down.
        problem: Problem,
    },
    /// The instance's settings are gone from the configuration. The agent's
    /// sign-in, its program and the conversations held were not touched.
    IntegrationRemoved {
        /// What the person is told, which says what was kept.
        said: Text,
    },
    /// The instance, and every one sharing its sign-in, are out of use from
    /// Crucible, durably, and the agent's own program says that sign-in is
    /// signed out.
    AgentSignedOut {
        /// What the person is told.
        said: Text,
    },
    /// The instance, and every one sharing its sign-in, are out of use from
    /// Crucible, durably, but nothing showed that the agent's own sign-in was
    /// signed out.
    SignOutUnverified {
        /// What the person is told, which says the sign-out is unverified.
        said: Text,
    },
    /// Nothing was removed, because a provider is still reached through the
    /// instance.
    StillReferenced {
        /// What the person is told, which names the provider.
        said: Text,
    },
    /// The yes to the agent's content-use route could not be written down, so
    /// the agent's program was not started and nothing changed.
    HoldUnkept {
        /// What the person is told.
        said: Text,
        /// Why it could not be written down.
        problem: Problem,
    },
    /// Nobody chose a way to go on, so nothing changed: the client could not
    /// be asked, or it cancelled, or the operation was refused before it
    /// started, and the pending action put to the client just before it said
    /// why.
    Undecided,
}

/// One persistent prompt-cache resource, as a listing shows it.
///
/// The vendor's handle for it is not here: that is what deletes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resource {
    /// What state it is in.
    pub state: Text,
    /// When it expires, in seconds since the epoch, where that is known.
    pub expires_at: Option<u64>,
    /// How its owner is isolated.
    pub isolation: Text,
    /// Whether one owner holds it alone.
    pub exclusive: bool,
    /// The provider protocol it was made over.
    pub protocol: Text,
}

/// How listing the prompt cache ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheOutcome {
    /// What is held, up to the list ceiling.
    Listed {
        /// The resources.
        resources: Vec<Resource>,
        /// Whether there were more than the ceiling lets through.
        truncated: bool,
    },
    /// The private store could not be read, or reading it would have had to
    /// wait and was dropped.
    Failed(Problem),
}

/// How deleting the prompt cache ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanOutcome {
    /// What was looked at and what became of it.
    Counted {
        /// How many were looked at.
        inspected: u64,
        /// How many were deleted.
        deleted: u64,
        /// How many could not be confirmed deleted.
        ambiguous: u64,
        /// How many nothing owns any more.
        orphaned: u64,
    },
    /// The private store could not be read or updated, or a step on it would
    /// have had to wait and was dropped, which may or may not have acted. It is
    /// also how the cleanup ends when the provider has no way to delete what
    /// is recorded, or the cleanup was cancelled.
    Failed(Problem),
}

/// How changing the sandbox requirement ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxOutcome {
    /// New processes are started this way from now on.
    Set {
        /// Whether the sandbox is required.
        enabled: bool,
    },
    /// Nothing changed.
    Unchanged(Problem),
}

/// How remembering a theme ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThemeOutcome {
    /// It is written down for the next run.
    Remembered,
    /// It could not be written down.
    Unwritten(Problem),
}

/// Who decided a setting, where it was not the user's own configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forced {
    /// The environment the host was started in.
    Environment,
    /// A configuration file of the project.
    Project,
}

/// How writing a setting ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingOutcome {
    /// It is written down for this run and the next.
    Remembered,
    /// Something nearer than the user's own configuration decides it, so
    /// nothing was written.
    Forced(Forced),
    /// It could not be written down.
    Unwritten(Problem),
}

/// How many entries of one kind a release held.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    /// The kind, as the changelog heads it, in lower case: `fixed`. Entries
    /// above any heading are counted as `changed`.
    pub kind: Name,
    /// How many.
    pub count: u64,
}

/// One release, as the notes tell it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Its number: `0.43.3`.
    pub version: Name,
    /// The day it was cut.
    pub date: Name,
    /// How many entries of each kind it held, in the order they are told.
    pub groups: Vec<Group>,
    /// Its words in markdown, where it is told in full, cut to the ceiling.
    pub text: Option<Text>,
}

/// How asking for the release notes ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotesOutcome {
    /// Every release, oldest first, the newest told in full.
    Listed {
        /// The releases, the newest up to the list ceiling.
        releases: Vec<Release>,
        /// The version the host is running.
        running: Name,
        /// Whether older releases were left out to keep under the ceiling.
        truncated: bool,
    },
    /// The release asked for, told in full.
    One(Release),
    /// No release has that number.
    Unknown {
        /// The newest release there is.
        newest: Name,
    },
    /// What was asked for is not written as a version.
    NotAVersion {
        /// The newest release there is, which is how one is written.
        newest: Name,
    },
}

/// What came of one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It was turned away before anything changed.
    Refused(Refusal),
    /// A turn ended.
    Turn(TurnOutcome),
    /// Making room ended.
    Room(RoomOutcome),
    /// A prompt or a request for room was not taken, because there is no
    /// model to ask: nothing was recorded and nothing was sent.
    Unasked(Missing),
    /// The running turn has been asked to stop.
    Cancelling,
    /// Starting a new session ended.
    Cleared(ClearOutcome),
    /// Picking a session up ended.
    Resumed(ResumeOutcome),
    /// Asking for a model ended.
    Model(ModelOutcome),
    /// Asking for an effort ended.
    Effort(EffortOutcome),
    /// Asking for a speed ended.
    Speed(SpeedOutcome),
    /// This is the mode now in force.
    Mode(Mode),
    /// Adopting a credential ended.
    Login(LoginOutcome),
    /// Forgetting a credential ended.
    Logout(LogoutOutcome),
    /// Listing the prompt cache ended.
    Cache(CacheOutcome),
    /// Deleting the prompt cache ended.
    Cleaned(CleanOutcome),
    /// Changing the sandbox requirement ended.
    Sandbox(SandboxOutcome),
    /// Remembering a theme ended.
    Theme(ThemeOutcome),
    /// Writing a setting ended.
    Setting(SettingOutcome),
    /// The commands that ship, by the word each crosses as.
    Help(Vec<Name>),
    /// Asking for the release notes ended.
    Notes(NotesOutcome),
    /// How the window of the next request is spent.
    Context(Context),
    /// What the session has used, and the plan windows a vendor reported.
    Usage(Usage),
    /// The client is leaving; the host closes what it owns.
    Leaving,
}

/// An [`Outcome`], paired with the request it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    /// The request answered, where the frame it came in could be paired.
    pub correlation: Option<Correlation>,
    /// What came of it.
    pub outcome: Outcome,
}

impl Response {
    /// The frame this response travels as.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] where the frame would be over the ceiling.
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        frame(
            &Writing::new()
                .with("version", crate::request::Version::CURRENT.number())
                .maybe("correlation", self.correlation.map(Correlation::number))
                .with("outcome", self.outcome.written())
                .finish(),
        )
    }

    /// The response `bytes` spell.
    ///
    /// # Errors
    ///
    /// [`Refusal`] for anything but one whole, bounded response in a version
    /// this build speaks.
    pub fn decode(bytes: &[u8]) -> Result<Self, Refusal> {
        let mut fields = Fields::of(parsed(bytes)?)?;
        let version = fields.number("version")?;
        if version != u64::from(crate::request::Version::CURRENT.number()) {
            return Err(ErrorCode::UnsupportedVersion.into());
        }
        let response = Self {
            correlation: fields.maybe_number("correlation")?.map(Correlation::new),
            outcome: Outcome::read(fields.take("outcome")?)?,
        };
        fields.done()?;
        Ok(response)
    }
}

/// An optional problem under `key`.
fn maybe_problem(fields: &mut Fields, key: &str) -> Result<Option<Problem>, Refusal> {
    fields.maybe(key).map(Problem::read).transpose()
}

impl Outcome {
    /// The commands that ship, as [`Outcome::Help`] lists them.
    #[must_use]
    pub fn help() -> Self {
        Self::Help(
            Command::KINDS
                .iter()
                .filter_map(|kind| Name::new(kind).ok())
                .collect(),
        )
    }

    /// Every kind of outcome, by the word it crosses as.
    pub const KINDS: [&'static str; 23] = [
        "refused",
        "turn",
        "room",
        "unasked",
        "cancelling",
        "cleared",
        "resumed",
        "model",
        "effort",
        "speed",
        "mode",
        "login",
        "logout",
        "cache",
        "cleaned",
        "sandbox",
        "theme",
        "setting",
        "help",
        "notes",
        "context",
        "usage",
        "leaving",
    ];

    /// The word this kind crosses as.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Refused(_) => "refused",
            Self::Turn(_) => "turn",
            Self::Room(_) => "room",
            Self::Unasked(_) => "unasked",
            Self::Cancelling => "cancelling",
            Self::Cleared(_) => "cleared",
            Self::Resumed(_) => "resumed",
            Self::Model(_) => "model",
            Self::Effort(_) => "effort",
            Self::Speed(_) => "speed",
            Self::Mode(_) => "mode",
            Self::Login(_) => "login",
            Self::Logout(_) => "logout",
            Self::Cache(_) => "cache",
            Self::Cleaned(_) => "cleaned",
            Self::Sandbox(_) => "sandbox",
            Self::Theme(_) => "theme",
            Self::Setting(_) => "setting",
            Self::Help(_) => "help",
            Self::Notes(_) => "notes",
            Self::Context(_) => "context",
            Self::Usage(_) => "usage",
            Self::Leaving => "leaving",
        }
    }

    fn written(&self) -> Value {
        let object = Writing::kind(self.kind());
        match self {
            Self::Refused(refusal) => object.with("code", refusal.code().as_str()),
            Self::Turn(turn) => object.with("turn", turn.written()),
            Self::Room(room) => object.with("room", room.written()),
            Self::Unasked(missing) => object.with("missing", missing.as_str()),
            Self::Cancelling | Self::Leaving => object,
            Self::Cleared(cleared) => object.with("cleared", cleared.written()),
            Self::Resumed(resumed) => object.with("resumed", resumed.written()),
            Self::Model(model) => object.with("model", model.written()),
            Self::Effort(effort) => object.with("effort", effort.written()),
            Self::Speed(speed) => object.with("speed", speed.written()),
            Self::Mode(mode) => object.with("mode", mode.as_str()),
            Self::Login(login) => object.with("login", login.written()),
            Self::Logout(logout) => object.with("logout", logout.written()),
            Self::Cache(cache) => object.with("cache", cache.written()),
            Self::Cleaned(cleaned) => object.with("cleaned", cleaned.written()),
            Self::Sandbox(sandbox) => object.with("sandbox", sandbox.written()),
            Self::Theme(theme) => object.with("theme", theme.written()),
            Self::Setting(setting) => object.with("setting", setting.written()),
            Self::Help(commands) => object.with(
                "commands",
                commands.iter().map(Name::as_str).collect::<Vec<_>>(),
            ),
            Self::Notes(notes) => object.with("notes", notes.written()),
            Self::Context(context) => object.with("context", context.written()),
            Self::Usage(usage) => object.with("usage", usage.written()),
        }
        .finish()
    }

    fn read(value: Value) -> Result<Self, Refusal> {
        let mut fields = Fields::of(value)?;
        let kind = fields.kind()?;
        let outcome = match kind.as_str() {
            "refused" => Self::Refused(code(&fields.string("code")?)?.into()),
            "turn" => Self::Turn(TurnOutcome::read(fields.take("turn")?)?),
            "room" => Self::Room(RoomOutcome::read(fields.take("room")?)?),
            "unasked" => Self::Unasked(Missing::named(&fields.string("missing")?)?),
            "cancelling" => Self::Cancelling,
            "cleared" => Self::Cleared(ClearOutcome::read(fields.take("cleared")?)?),
            "resumed" => Self::Resumed(ResumeOutcome::read(fields.take("resumed")?)?),
            "model" => Self::Model(ModelOutcome::read(fields.take("model")?)?),
            "effort" => Self::Effort(EffortOutcome::read(fields.take("effort")?)?),
            "speed" => Self::Speed(SpeedOutcome::read(fields.take("speed")?)?),
            "mode" => Self::Mode(fields.string("mode")?.parse()?),
            "login" => Self::Login(LoginOutcome::read(fields.take("login")?)?),
            "logout" => Self::Logout(LogoutOutcome::read(fields.take("logout")?)?),
            "cache" => Self::Cache(CacheOutcome::read(fields.take("cache")?)?),
            "cleaned" => Self::Cleaned(CleanOutcome::read(fields.take("cleaned")?)?),
            "sandbox" => Self::Sandbox(SandboxOutcome::read(fields.take("sandbox")?)?),
            "theme" => Self::Theme(ThemeOutcome::read(fields.take("theme")?)?),
            "setting" => Self::Setting(SettingOutcome::read(fields.take("setting")?)?),
            "help" => Self::Help(
                fields
                    .list("commands")?
                    .iter()
                    .map(|value| Name::new(value.as_str().unwrap_or_default()))
                    .collect::<Result<_, _>>()?,
            ),
            "notes" => Self::Notes(NotesOutcome::read(fields.take("notes")?)?),
            "context" => Self::Context(Context::read(fields.take("context")?)?),
            "usage" => Self::Usage(Usage::read(fields.take("usage")?)?),
            "leaving" => Self::Leaving,
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(outcome)
    }
}

mod codec;
