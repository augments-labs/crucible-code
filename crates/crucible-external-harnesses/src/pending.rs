//! The questions a supervised agent has asked and nobody has answered yet.
//!
//! An agent names what it asks in its own words, an identifier on its wire,
//! and the contract names it a [`PendingRef`]. [`Asked`] keeps the two side by
//! side and nothing else: whether a decision may settle an action is the
//! execution table's to say, and every settle goes through it, so a decision
//! for a run of the program that has since restarted, for a turn that has
//! ended or for another session is refused there before its wire identifier is
//! looked at. A wire identifier is a number the program chose and may reuse in
//! its next run; the reference is not.

use crucible_execution::{Decision, Pending, PendingRef, Pendings, SessionRef, Stale, Unraised};
use crucible_types::TurnId;

/// The most bytes of an agent's own name for a question kept.
pub const NATIVE_BYTES: usize = 256;

/// The questions one supervised agent has open, under both their names.
#[derive(Debug)]
pub struct Asked {
    /// The contract's table, which every settle goes through.
    pendings: Pendings,
    /// Each open question's reference and its name on the agent's wire.
    native: Vec<(PendingRef, Box<str>)>,
}

impl Asked {
    /// An empty table, or nothing once this process has numbered every
    /// instance it can.
    #[must_use]
    pub fn new() -> Option<Self> {
        Some(Self {
            pendings: Pendings::new()?,
            native: Vec::new(),
        })
    }

    /// `turn` of `session` has begun.
    pub fn begin(&mut self, session: SessionRef, turn: TurnId) {
        self.native.clear();
        self.pendings.begin(session, turn);
    }

    /// The running turn has ended, and whatever it asked can no longer be
    /// answered.
    pub fn end(&mut self) {
        self.native.clear();
        self.pendings.end();
    }

    /// The program has been started again, and whatever its last run asked can
    /// no longer be answered. False once this process has numbered every run
    /// it can, and the table then refuses every decision.
    pub fn relaunch(&mut self) -> bool {
        self.native.clear();
        self.pendings.relaunch().is_some()
    }

    /// Opens `pending`, which the agent named `native`.
    ///
    /// # Errors
    ///
    /// [`Unraised`] where the contract's table would not open it, and
    /// [`Unraised::Cut`] where `native` is longer than [`NATIVE_BYTES`]: an
    /// answer sent under a name cut short answers something else.
    pub fn raise(&mut self, native: &str, pending: Pending) -> Result<PendingRef, Unraised> {
        if native.len() > NATIVE_BYTES {
            return Err(Unraised::Cut);
        }
        let reference = self.pendings.raise(pending)?;
        self.native.push((reference.clone(), native.into()));
        Ok(reference)
    }

    /// Settles what `reference` names with `decision`, and hands back the
    /// agent's name for it with what it waited on.
    ///
    /// # Errors
    ///
    /// The [`Stale`] reason the decision was refused for.
    pub fn settle(
        &mut self,
        reference: &PendingRef,
        decision: &Decision,
    ) -> Result<(Box<str>, Pending), Stale> {
        let pending = self.pendings.settle(reference, decision)?;
        let at = self
            .native
            .iter()
            .position(|(open, _)| open == reference)
            .ok_or(Stale::Settled)?;
        let (_, native) = self.native.swap_remove(at);
        Ok((native, pending))
    }

    /// Whether a question the agent named `native` is open: a second one under
    /// that name would make an answer to either answer both.
    #[must_use]
    pub fn holds(&self, native: &str) -> bool {
        self.native.iter().any(|(_, open)| &**open == native)
    }

    /// What is open, oldest first.
    pub fn open(&self) -> impl Iterator<Item = &PendingRef> {
        self.native.iter().map(|(reference, _)| reference)
    }
}
