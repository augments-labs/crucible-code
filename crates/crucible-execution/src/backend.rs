//! The operations every backend answers, and the check that it keeps to its
//! claims.
//!
//! An instance is built for one route by whatever owns its driver, so opening
//! a session names no route: the instance is the route. A turn reports what it
//! does through the [`Sink`] it is handed and answers with how it ended; a
//! decision names the pending action it settles; an interrupt asks the running
//! turn to stop; a close waits until nothing the instance started is still
//! running.
//!
//! Every operation answers [`Error::Unsupported`] for a capability the
//! instance does not claim as supported, before it looks at anything else. A
//! backend that answers `Ok` instead has said something its description denies,
//! and [`conformance`] finds it.

use std::fmt;

use crucible_runtime::BoxFuture;
use crucible_types::TurnId;

use crate::capability::{Capabilities, Capability};
use crate::event::{Event, Text};
use crate::pending::{Decision, Lasting, PendingRef, Pendings, Stale};

/// The most bytes a name in a [`Description`] or a [`SessionRef`] keeps.
pub const NAME_BYTES: usize = 256;

/// What a backend is and what it claims.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Description {
    /// The driver it was made by.
    driver: Text,
    /// This instance's name.
    instance: Text,
    /// The version of the program, as the program reported it.
    version: Option<Text>,
    /// What it claims.
    capabilities: Capabilities,
}

impl Description {
    /// Instance `instance` of `driver`, claiming `capabilities`.
    #[must_use]
    pub fn new(driver: &str, instance: &str, capabilities: Capabilities) -> Self {
        Self {
            driver: Text::clipped(driver, NAME_BYTES),
            instance: Text::clipped(instance, NAME_BYTES),
            version: None,
            capabilities,
        }
    }

    /// This description, with the program reporting `version`.
    #[must_use]
    pub fn reporting(mut self, version: &str) -> Self {
        self.version = Some(Text::clipped(version, NAME_BYTES));
        self
    }

    /// The driver it was made by.
    #[must_use]
    pub const fn driver(&self) -> &Text {
        &self.driver
    }

    /// This instance's name.
    #[must_use]
    pub const fn instance(&self) -> &Text {
        &self.instance
    }

    /// The version the program reported, if it did.
    #[must_use]
    pub const fn version(&self) -> Option<&Text> {
        self.version.as_ref()
    }

    /// What it claims.
    #[must_use]
    pub const fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
}

/// A session, as the backend that opened it names it.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct SessionRef(Box<str>);

impl SessionRef {
    /// The session named `name`, where it is not empty and at most
    /// [`NAME_BYTES`].
    #[must_use]
    pub fn new(name: &str) -> Option<Self> {
        (!name.is_empty() && name.len() <= NAME_BYTES).then(|| Self(name.into()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SessionRef {
    fn fmt(&self, form: &mut fmt::Formatter<'_>) -> fmt::Result {
        form.debug_tuple("SessionRef").field(&self.0).finish()
    }
}

/// One turn to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TurnRequest {
    /// Which turn.
    turn: TurnId,
    /// What the user said.
    prompt: Box<str>,
}

impl TurnRequest {
    /// Turn `turn`, saying `prompt`.
    #[must_use]
    pub fn new(turn: TurnId, prompt: &str) -> Self {
        Self {
            turn,
            prompt: prompt.into(),
        }
    }

    /// Which turn.
    #[must_use]
    pub const fn turn(&self) -> TurnId {
        self.turn
    }

    /// What the user said.
    #[must_use]
    pub fn prompt(&self) -> &str {
        &self.prompt
    }
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Outcome {
    /// It answered and yielded to the user.
    Answered,
    /// It was stopped before it finished.
    Cancelled,
    /// It stopped short of an answer for a reason of its own: out of room, cut
    /// off by a filter, or a stop nobody named.
    CutShort,
}

/// A turn or an operation failed, and what that says of the agent's session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{reason}")]
pub struct Failure {
    /// What went wrong, in a sentence.
    reason: Text,
    /// What it says of the session the agent ran it in.
    fate: SessionFate,
}

/// What a failure says of the agent's own session, the one a later launch
/// would resume: only the backend reading the agent's wire can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionFate {
    /// Nothing about the failure says the session is gone, so a later turn
    /// may resume it.
    Kept,
    /// The agent said it has no session by the name it was asked for, so a
    /// later turn resuming it would fail the same way.
    Gone,
}

impl Failure {
    /// It failed, and `reason` says why.
    #[must_use]
    pub fn new(reason: &str) -> Self {
        Self {
            reason: Text::new(reason),
            fate: SessionFate::Kept,
        }
    }

    /// It failed because the agent has no session by the name it was asked
    /// for, and `reason` says so.
    #[must_use]
    pub fn gone(reason: &str) -> Self {
        Self {
            reason: Text::new(reason),
            fate: SessionFate::Gone,
        }
    }

    /// The same failure, said as `reason`.
    #[must_use]
    pub fn reworded(&self, reason: &str) -> Self {
        Self {
            reason: Text::new(reason),
            fate: self.fate,
        }
    }

    /// What went wrong.
    #[must_use]
    pub const fn reason(&self) -> &Text {
        &self.reason
    }

    /// What it says of the agent's session.
    #[must_use]
    pub const fn fate(&self) -> SessionFate {
        self.fate
    }
}

/// The instance does not claim the capability an operation needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
#[error("this backend does not support {0}")]
pub struct Unsupported(pub Capability);

/// Why an operation was not done.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
pub enum Error {
    /// The instance does not claim what it needs.
    #[error(transparent)]
    Unsupported(#[from] Unsupported),
    /// It was tried and failed.
    #[error(transparent)]
    Failed(#[from] Failure),
    /// The decision does not settle anything open.
    #[error(transparent)]
    Stale(#[from] Stale),
}

/// Where a running turn's events go.
pub trait Sink: Send + Sync {
    /// Reports `event`.
    fn post(&self, event: Event);
}

/// An agent behind crucible's interface.
pub trait Backend: Send + Sync {
    /// What it is and what it claims.
    fn describe(&self) -> Description;

    /// Opens a new session, or the one `session` names.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] for a named session where [`Capability::Resume`]
    /// is not claimed; [`Error::Failed`] where it could not be opened.
    fn open(&self, session: Option<SessionRef>) -> BoxFuture<'_, Result<SessionRef, Error>>;

    /// Runs `turn`, posting what it does to `events`, and answers how it
    /// ended.
    ///
    /// # Errors
    ///
    /// [`Error::Failed`] where the turn could not run or failed.
    fn prompt<'a>(
        &'a self,
        turn: TurnRequest,
        events: &'a dyn Sink,
    ) -> BoxFuture<'a, Result<Outcome, Error>>;

    /// Settles the pending action `pending` names with `decision`.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] for a decision of a kind not claimed;
    /// [`Error::Stale`] for one that settles nothing open.
    fn decide(&self, pending: &PendingRef, decision: Decision) -> Result<(), Error>;

    /// Asks the running turn to stop.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] where [`Capability::Cancel`] is not claimed.
    fn interrupt(&self) -> Result<(), Error>;

    /// Waits until nothing the instance started is still running.
    ///
    /// # Errors
    ///
    /// [`Error::Failed`] where something could not be stopped.
    fn close(&self) -> BoxFuture<'_, Result<(), Error>>;
}

/// A backend said yes to something its description does not claim.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{capability} is not claimed, and the backend answered {answered} rather than refusing it")]
pub struct Nonconformance {
    /// The capability it does not claim.
    capability: Capability,
    /// What it answered instead of [`Unsupported`] naming it.
    answered: &'static str,
}

impl Nonconformance {
    /// The capability it does not claim.
    #[must_use]
    pub const fn capability(&self) -> Capability {
        self.capability
    }
}

/// Checks that `backend` refuses every gated operation whose capability its
/// description does not claim as supported, with [`Unsupported`] naming that
/// capability.
///
/// Only the refusals are exercised: an operation it does claim is never run,
/// so checking a conformant backend changes nothing.
///
/// # Errors
///
/// The first operation it answered other than by refusing.
pub async fn conformance(backend: &dyn Backend) -> Result<(), Nonconformance> {
    let claims = *backend.describe().capabilities();
    let session = SessionRef(Box::from("conformance"));
    for gated in Gated::ALL {
        let capability = gated.needs();
        if claims.supports(capability) {
            continue;
        }
        let answered = match gated.decision() {
            Some(decision) => {
                let pending = Pendings::foreign(session.clone(), TurnId::FIRST);
                backend.decide(&pending, decision)
            }
            None if matches!(gated, Gated::Resume) => {
                backend.open(Some(session.clone())).await.map(drop)
            }
            None => backend.interrupt(),
        };
        let answered = match answered {
            Err(Error::Unsupported(Unsupported(named))) if named == capability => continue,
            Ok(()) => "yes",
            Err(Error::Unsupported(_)) => "a refusal naming another capability",
            Err(Error::Failed(_)) => "a failure",
            Err(Error::Stale(_)) => "a stale decision",
        };
        return Err(Nonconformance {
            capability,
            answered,
        });
    }
    Ok(())
}

/// One operation gated on one capability, as [`conformance`] exercises it.
#[derive(Debug, Clone, Copy)]
enum Gated {
    /// [`Backend::interrupt`].
    Interrupt,
    /// [`Backend::open`] naming a session.
    Resume,
    /// [`Backend::decide`] allowing once.
    AllowOnce,
    /// [`Backend::decide`] allowing for the session.
    AllowSession,
    /// [`Backend::decide`] answering a question.
    Answer,
}

impl Gated {
    /// Every gated operation.
    const ALL: [Self; 5] = [
        Self::Interrupt,
        Self::Resume,
        Self::AllowOnce,
        Self::AllowSession,
        Self::Answer,
    ];

    /// What it needs claimed.
    const fn needs(self) -> Capability {
        match self {
            Self::Interrupt => Capability::Cancel,
            Self::Resume => Capability::Resume,
            Self::AllowOnce => Capability::PermissionOnce,
            Self::AllowSession => Capability::PermissionSession,
            Self::Answer => Capability::Questions,
        }
    }

    /// The decision it sends, where it sends one.
    fn decision(self) -> Option<Decision> {
        match self {
            Self::AllowOnce => Some(Decision::Allow(Lasting::Once)),
            Self::AllowSession => Some(Decision::Allow(Lasting::Session)),
            Self::Answer => Some(Decision::Answer(Text::new("yes"))),
            Self::Interrupt | Self::Resume => None,
        }
    }
}
