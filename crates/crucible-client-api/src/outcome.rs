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
use crate::error::{ErrorCode, Refusal};
use crate::request::Correlation;
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

/// Why a model stopped, as this protocol spells it.
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
    /// The provider gave a reason nobody here has heard of.
    Unknown,
}

impl Stop {
    /// Every reason.
    pub const EVERY: [Self; 8] = [
        Self::Yielded,
        Self::WantsTools,
        Self::OutOfTokens,
        Self::WindowExceeded,
        Self::Filtered,
        Self::Paused,
        Self::Cancelled,
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
    /// The turn could not be taken.
    Failed(Problem),
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
    /// It was cancelled, and the conversation is as it was.
    Stopped,
    /// The recap could not be had, and the conversation is as it was.
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
    /// Its log could not be read; the session in hand is untouched.
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

/// How adopting a stored credential ended.
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
    Kept,
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
    /// The private store could not be read.
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
    /// The private store could not be read or updated.
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

/// What came of one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// It was turned away before anything changed.
    Refused(Refusal),
    /// A turn ended.
    Turn(TurnOutcome),
    /// Making room ended.
    Room(RoomOutcome),
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
    /// The commands that ship, by the word each crosses as.
    Help(Vec<Name>),
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
    pub const KINDS: [&'static str; 17] = [
        "refused",
        "turn",
        "room",
        "cancelling",
        "cleared",
        "resumed",
        "model",
        "effort",
        "mode",
        "login",
        "logout",
        "cache",
        "cleaned",
        "sandbox",
        "theme",
        "help",
        "leaving",
    ];

    /// The word this kind crosses as.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Refused(_) => "refused",
            Self::Turn(_) => "turn",
            Self::Room(_) => "room",
            Self::Cancelling => "cancelling",
            Self::Cleared(_) => "cleared",
            Self::Resumed(_) => "resumed",
            Self::Model(_) => "model",
            Self::Effort(_) => "effort",
            Self::Mode(_) => "mode",
            Self::Login(_) => "login",
            Self::Logout(_) => "logout",
            Self::Cache(_) => "cache",
            Self::Cleaned(_) => "cleaned",
            Self::Sandbox(_) => "sandbox",
            Self::Theme(_) => "theme",
            Self::Help(_) => "help",
            Self::Leaving => "leaving",
        }
    }

    fn written(&self) -> Value {
        let object = Writing::kind(self.kind());
        match self {
            Self::Refused(refusal) => object.with("code", refusal.code().as_str()),
            Self::Turn(turn) => object.with("turn", turn.written()),
            Self::Room(room) => object.with("room", room.written()),
            Self::Cancelling | Self::Leaving => object,
            Self::Cleared(cleared) => object.with("cleared", cleared.written()),
            Self::Resumed(resumed) => object.with("resumed", resumed.written()),
            Self::Model(model) => object.with("model", model.written()),
            Self::Effort(effort) => object.with("effort", effort.written()),
            Self::Mode(mode) => object.with("mode", mode.as_str()),
            Self::Login(login) => object.with("login", login.written()),
            Self::Logout(logout) => object.with("logout", logout.written()),
            Self::Cache(cache) => object.with("cache", cache.written()),
            Self::Cleaned(cleaned) => object.with("cleaned", cleaned.written()),
            Self::Sandbox(sandbox) => object.with("sandbox", sandbox.written()),
            Self::Theme(theme) => object.with("theme", theme.written()),
            Self::Help(commands) => object.with(
                "commands",
                commands.iter().map(Name::as_str).collect::<Vec<_>>(),
            ),
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
            "cancelling" => Self::Cancelling,
            "cleared" => Self::Cleared(ClearOutcome::read(fields.take("cleared")?)?),
            "resumed" => Self::Resumed(ResumeOutcome::read(fields.take("resumed")?)?),
            "model" => Self::Model(ModelOutcome::read(fields.take("model")?)?),
            "effort" => Self::Effort(EffortOutcome::read(fields.take("effort")?)?),
            "mode" => Self::Mode(fields.string("mode")?.parse()?),
            "login" => Self::Login(LoginOutcome::read(fields.take("login")?)?),
            "logout" => Self::Logout(LogoutOutcome::read(fields.take("logout")?)?),
            "cache" => Self::Cache(CacheOutcome::read(fields.take("cache")?)?),
            "cleaned" => Self::Cleaned(CleanOutcome::read(fields.take("cleaned")?)?),
            "sandbox" => Self::Sandbox(SandboxOutcome::read(fields.take("sandbox")?)?),
            "theme" => Self::Theme(ThemeOutcome::read(fields.take("theme")?)?),
            "help" => Self::Help(
                fields
                    .list("commands")?
                    .iter()
                    .map(|value| Name::new(value.as_str().unwrap_or_default()))
                    .collect::<Result<_, _>>()?,
            ),
            "leaving" => Self::Leaving,
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(outcome)
    }
}

mod codec;
