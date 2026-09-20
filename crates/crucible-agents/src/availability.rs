//! Which of the tools a run can reach this agent is allowed to use.

use crucible_tools::{ToolSnapshot, ToolsetError};

/// The tools a definition declares.
///
/// A run resolves and materializes one immutable roster; this says how much of
/// that roster the agent asking is offered. It can only take names away: a
/// declaration naming a tool the run does not have gives the agent nothing,
/// because what exists is settled by what the wiring installed and not by what
/// a definition asks for. That direction is the whole point — two agents can
/// differ in what they may reach without either of them being able to reach
/// further than the run itself can.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Availability {
    /// Whatever the run has. The default, and what every agent had before
    /// there was anything else to say.
    #[default]
    Everything,
    /// Only these names, where the run has them.
    Named(Box<[Box<str>]>),
}

impl Availability {
    /// Only the tools named, where the run has them.
    #[must_use]
    pub fn naming<'a>(names: impl IntoIterator<Item = &'a str>) -> Self {
        Self::Named(names.into_iter().map(Into::into).collect())
    }

    /// Whether this declaration offers the tool called `name`.
    #[must_use]
    pub fn offers(&self, name: &str) -> bool {
        match self {
            Self::Everything => true,
            Self::Named(named) => named.iter().any(|one| &**one == name),
        }
    }

    /// The roster this agent is offered, out of the one the run materialized.
    ///
    /// An open declaration hands the generation straight back, unchanged and
    /// undivided: an agent that declares nothing is asked exactly what it was
    /// asked before there was anything to declare, down to the generation the
    /// calls it makes are admitted through.
    ///
    /// A narrowed one is its own generation, minted here, and it is the one the
    /// whole pass then uses — described, advertised, admitted and resolved — so
    /// the three views a call passes through cannot disagree about which set of
    /// tools it belonged to.
    ///
    /// # Errors
    ///
    /// [`ToolsetError`] where the narrowed roster cannot be materialized. The
    /// bounds it is held to are the same ones the roster it came from passed,
    /// so a subset that fails them is a roster that changed underneath rather
    /// than a declaration asking for too much.
    pub fn narrowing(&self, roster: &ToolSnapshot) -> Result<ToolSnapshot, ToolsetError> {
        match self {
            Self::Everything => Ok(roster.clone()),
            Self::Named(_) => ToolSnapshot::new(
                roster
                    .entries()
                    .iter()
                    .filter(|entry| self.offers(entry.descriptor().name()))
                    .cloned(),
            ),
        }
    }
}
