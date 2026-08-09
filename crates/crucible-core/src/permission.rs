//! Deciding whether a call may run, and proving that it may.
//!
//! Everything a tool can do arrives here first. There is one route to running
//! a tool and it goes through [`Permission::decide`]; nothing else can mint the
//! token a tool needs, so a call that skipped this cannot be made to run by
//! forgetting a check.
//!
//! Two inputs settle a call. **Rules** are standing statements read from
//! configuration — three kinds, `deny` beating `ask` beating `allow` whatever
//! the patterns look like. A **mode** supplies the answer for the arm no rule
//! matched, and does nothing else: it never branches around this module, so
//! adding a mode adds one default and not a second way for a tool to run.
//!
//! A refusal has two shapes on purpose. A rule is standing policy and stops one
//! call; the model meets the wall, is told, and gets on with something else. A
//! human saying no is about this moment, and ends the turn — otherwise a model
//! can reshape the same question until one of the shapes gets a yes.

use std::collections::HashSet;

use crate::tool::ToolCall;

mod grant;
mod mode;
mod rule;
mod sensitivity;
#[cfg(test)]
mod tests;
mod verdict;

pub use grant::{Approved, Grant};
pub use mode::Mode;
pub use rule::{Disposition, RuleError, Rules};
pub use sensitivity::{Command, Sensitivity, Target};
pub use verdict::{Ask, Remember, Verdict};

/// What the engine settled on for one call.
#[derive(Debug)]
pub enum Settled {
    /// It may run, and here is the proof.
    Approved(Approved),

    /// A rule forbids it. The turn carries on and the model is told: standing
    /// policy costs nothing to hit twice, and ending the turn on one would let
    /// a single stray call throw away a piece of work.
    Forbidden,

    /// The user said no. The turn ends.
    Refused,
}

/// The permission engine for one session.
#[derive(Debug)]
pub struct Permission {
    mode: Mode,
    rules: Rules,

    /// What the user allowed for the rest of the session, by scope. Held in
    /// memory and never written down, so it dies with the process that earned
    /// it.
    remembered: HashSet<Box<str>>,
}

impl Permission {
    /// A session with no rules, asking about everything it would change or run.
    #[must_use]
    pub fn new() -> Self {
        Self::with(Mode::default(), Rules::new())
    }

    /// A session with the mode and rules configuration asked for.
    #[must_use]
    pub fn with(mode: Mode, rules: Rules) -> Self {
        Self {
            mode,
            rules,
            remembered: HashSet::new(),
        }
    }

    /// The mode in force, which the prompt line shows at all times.
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Decides one call, asking the user if that is what it comes to.
    pub fn decide(
        &mut self,
        call: &ToolCall,
        sensitivity: &Sensitivity,
        ask: &mut dyn Ask,
    ) -> Settled {
        match self.disposition(call, sensitivity) {
            Disposition::Allow => Self::approve(call, Verdict::Allow),
            Disposition::Deny => Settled::Forbidden,
            Disposition::Ask => self.put(call, sensitivity, ask),
        }
    }

    /// What is to happen to this call before anybody is asked.
    fn disposition(&self, call: &ToolCall, sensitivity: &Sensitivity) -> Disposition {
        let stated = self
            .rules
            .stated(call, sensitivity)
            .unwrap_or_else(|| self.mode.default_arm(sensitivity));

        // A read is never put to the user, so an `ask` about one has to become
        // something. It becomes a refusal: whoever wrote `ask read(secrets/**)`
        // asked not to have it go through unwatched, and refusing is the only
        // answer left that respects that. No mode produces this arm — every one
        // of them allows a read — so it is only ever somebody's own rule.
        match (sensitivity, stated) {
            (Sensitivity::ReadOnly { .. }, Disposition::Ask) => Disposition::Deny,
            (_, settled) => settled,
        }
    }

    /// Asks, unless this scope was already allowed for the session.
    fn put(&mut self, call: &ToolCall, sensitivity: &Sensitivity, ask: &mut dyn Ask) -> Settled {
        let scope = Self::scope(call, sensitivity);
        if self.remembered.contains(&scope) {
            return Self::approve(call, Verdict::Allow);
        }

        let (verdict, remember) = ask.ask(call, sensitivity);
        if verdict == Verdict::Allow && remember == Remember::Session {
            self.remembered.insert(scope);
        }

        // Nothing is remembered about a no. The turn ends on one, so there is
        // no next call in it to remember for, and the next turn is a fresh
        // instruction that deserves its own question.
        Self::approve(call, verdict)
    }

    /// Mints the proof, or reports that the user said no.
    fn approve(call: &ToolCall, verdict: Verdict) -> Settled {
        match Grant::issue(verdict) {
            Some(grant) => Settled::Approved(Approved::new(call.clone(), grant)),
            None => Settled::Refused,
        }
    }

    /// What a session-long allow covers.
    ///
    /// For a tool that changes files it is the tool: that is what the question
    /// named, and a session allow may not quietly cover more than was asked
    /// about. For one that runs programs it is the tool and the command, since
    /// agreeing to `cargo test` is not agreeing to `curl`.
    fn scope(call: &ToolCall, sensitivity: &Sensitivity) -> Box<str> {
        match sensitivity {
            Sensitivity::ReadOnly { .. } | Sensitivity::MutatesFile { .. } => call.name.clone(),
            Sensitivity::SpawnsProcess { command } => format!("{}:{command}", call.name).into(),
        }
    }
}

impl Default for Permission {
    fn default() -> Self {
        Self::new()
    }
}
