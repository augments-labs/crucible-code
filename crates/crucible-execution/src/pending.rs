//! What a backend is waiting on the user for, and the one answer each takes.
//!
//! A pending action is named by a [`PendingRef`] carrying everything it was
//! raised under: the instance, the session, the turn, the generation of the
//! program that raised it and its own request number. A decision is held
//! against all five. One composed for another instance, another session, a
//! turn that has ended, or a program that has since been started again is
//! refused with the [`Stale`] reason that says which, as is a second decision
//! for one already settled and a decision of the wrong kind; none reaches the
//! program. A generation exists because a restarted program may number its
//! requests from the start again, and an answer meant for the old one's
//! question must not settle the new one's.
//!
//! A decision is never a permission. It names a pending action and is held
//! against it; what a backend does with an allowed one is its own permission
//! engine's to settle.
//!
//! [`Pendings`] is that table, bounded at [`PENDINGS`] open at once.

use std::sync::atomic::{AtomicU64, Ordering};

use crucible_types::TurnId;

use crate::backend::SessionRef;
use crate::event::Text;

/// The most pending actions one table holds open.
pub const PENDINGS: usize = 64;

/// The next generation or instance any table in this process is given.
static NUMBERS: AtomicU64 = AtomicU64::new(0);

/// The next number nothing in this process has been given, or nothing once
/// they run out. Never wraps: a number handed out twice would refuse nothing.
fn fresh() -> Option<u64> {
    NUMBERS
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |next| {
            next.checked_add(1)
        })
        .ok()
}

/// One backend instance, as its pending actions name it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Instance(u64);

/// One run of the program behind a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Generation(u64);

/// One pending action's number within its generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RequestId(u64);

/// Everything a pending action was raised under.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PendingRef {
    /// The instance that raised it.
    instance: Instance,
    /// The session it was raised in.
    session: SessionRef,
    /// The turn it was raised in.
    turn: TurnId,
    /// The run of the program that raised it.
    generation: Generation,
    /// Its own number.
    request: RequestId,
}

impl PendingRef {
    /// The session it was raised in.
    #[must_use]
    pub const fn session(&self) -> &SessionRef {
        &self.session
    }

    /// The turn it was raised in.
    #[must_use]
    pub const fn turn(&self) -> TurnId {
        self.turn
    }

    /// Its own number.
    #[must_use]
    pub const fn request(&self) -> RequestId {
        self.request
    }
}

/// What a backend is waiting on the user for.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Pending {
    /// Leave to take an action.
    Permission {
        /// What it wants to use, as it named it.
        tool: Text,
        /// What it wants to do with it.
        summary: Text,
        /// The command line or address exactly as the call sends it, where it
        /// sends one: the summary names what the rules match, and this is what
        /// leaves.
        sent: Option<Text>,
    },
    /// An answer to a question.
    Question {
        /// The question.
        prompt: Text,
    },
}

/// How long an allowed action stays allowed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lasting {
    /// For this one action.
    Once,
    /// For actions like it for the rest of the session.
    Session,
}

/// The user's answer to one pending action.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Decision {
    /// Go ahead.
    Allow(Lasting),
    /// Do not.
    Deny,
    /// This is the answer to the question.
    Answer(Text),
}

/// Why a decision was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum Stale {
    /// It names a pending action another instance raised.
    #[error("that decision is for another backend")]
    Instance,
    /// It names a pending action another session raised.
    #[error("that decision is for another session")]
    Session,
    /// The program that raised it has been started again since.
    #[error("that decision is for a program that has since restarted")]
    Generation,
    /// The turn it was raised in has ended.
    #[error("that decision is for a turn that has ended")]
    Turn,
    /// Nothing open has that number: it was settled already, or never raised.
    #[error("nothing is waiting on that decision")]
    Settled,
    /// It is the wrong kind of answer for what was asked.
    #[error("that is the wrong kind of answer for what was asked")]
    Kind,
}

/// Why a pending action was not opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, thiserror::Error)]
pub enum Unraised {
    /// The table already holds [`PENDINGS`] open actions.
    #[error("too many decisions are already waiting")]
    Full,
    /// No turn is running to raise it in.
    #[error("no turn is running")]
    Idle,
    /// This run of the program has numbered every request it can.
    #[error("no request number is left")]
    Exhausted,
}

/// One open action and the reference it was raised under.
#[derive(Debug)]
struct Open {
    /// What it was raised under.
    reference: PendingRef,
    /// What it waits on.
    pending: Pending,
}

/// The pending actions one backend instance has open.
#[derive(Debug)]
pub struct Pendings {
    /// Which instance this is.
    instance: Instance,
    /// The live run of its program.
    generation: Generation,
    /// The session and turn now running, if one is.
    turn: Option<(SessionRef, TurnId)>,
    /// The next request number in this generation.
    next: u64,
    /// What is open, at most [`PENDINGS`].
    open: Vec<Open>,
}

impl Pendings {
    /// An empty table for a new instance, or nothing once this process has
    /// handed out every number it has.
    #[must_use]
    pub fn new() -> Option<Self> {
        Some(Self {
            instance: Instance(fresh()?),
            generation: Generation(fresh()?),
            turn: None,
            next: 0,
            open: Vec::new(),
        })
    }

    /// The live run of its program.
    #[must_use]
    pub const fn generation(&self) -> Generation {
        self.generation
    }

    /// `turn` of `session` has begun; whatever was open is dropped.
    pub fn begin(&mut self, session: SessionRef, turn: TurnId) {
        self.open.clear();
        self.turn = Some((session, turn));
    }

    /// The running turn has ended; whatever was open is dropped, and a
    /// decision for any of it is refused as [`Stale::Turn`].
    pub fn end(&mut self) {
        self.open.clear();
        self.turn = None;
    }

    /// The program has been started again; whatever was open is dropped, and
    /// a decision for any of it is refused as [`Stale::Generation`].
    ///
    /// Nothing once this process has handed out every number it has, and the
    /// table is left refusing every decision.
    pub fn relaunch(&mut self) -> Option<Generation> {
        self.open.clear();
        self.turn = None;
        self.next = 0;
        self.generation = Generation(fresh()?);
        Some(self.generation)
    }

    /// Opens `pending` in the running turn and names it.
    ///
    /// # Errors
    ///
    /// The [`Unraised`] reason it was not opened.
    pub fn raise(&mut self, pending: Pending) -> Result<PendingRef, Unraised> {
        let Some((session, turn)) = self.turn.clone() else {
            return Err(Unraised::Idle);
        };
        if self.open.len() >= PENDINGS {
            return Err(Unraised::Full);
        }
        let request = RequestId(self.next);
        self.next = self.next.checked_add(1).ok_or(Unraised::Exhausted)?;
        let reference = PendingRef {
            instance: self.instance,
            session,
            turn,
            generation: self.generation,
            request,
        };
        self.open.push(Open {
            reference: reference.clone(),
            pending,
        });
        Ok(reference)
    }

    /// Settles the action `reference` names with `decision`, and hands back
    /// what it waited on.
    ///
    /// # Errors
    ///
    /// The [`Stale`] reason it was refused for; the action stays open unless
    /// it was settled.
    pub fn settle(
        &mut self,
        reference: &PendingRef,
        decision: &Decision,
    ) -> Result<Pending, Stale> {
        if reference.instance != self.instance {
            return Err(Stale::Instance);
        }
        if reference.generation != self.generation {
            return Err(Stale::Generation);
        }
        let Some((session, turn)) = &self.turn else {
            return Err(Stale::Turn);
        };
        if reference.session != *session {
            return Err(Stale::Session);
        }
        if reference.turn != *turn {
            return Err(Stale::Turn);
        }
        let at = self
            .open
            .iter()
            .position(|open| open.reference == *reference)
            .ok_or(Stale::Settled)?;
        let fits = self.open.get(at).is_some_and(|open| {
            matches!(
                (&open.pending, decision),
                (
                    Pending::Permission { .. },
                    Decision::Allow(_) | Decision::Deny
                ) | (
                    Pending::Question { .. },
                    Decision::Answer(_) | Decision::Deny
                )
            )
        });
        if !fits {
            return Err(Stale::Kind);
        }
        Ok(self.open.swap_remove(at).pending)
    }

    /// A reference no table has handed out, for a check that must be refused.
    pub(crate) fn foreign(session: SessionRef, turn: TurnId) -> PendingRef {
        PendingRef {
            instance: Instance(u64::MAX),
            session,
            turn,
            generation: Generation(u64::MAX),
            request: RequestId(u64::MAX),
        }
    }
}
