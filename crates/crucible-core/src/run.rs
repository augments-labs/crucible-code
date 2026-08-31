//! Where one execution sits among the executions around it.
//!
//! An execution is an agent driven from a prompt to an ending. Today there is
//! exactly one of them per turn, and this module looks like an over-answer to
//! that. It is written now because the shape it records is the one thing a
//! later child execution cannot be given retroactively: an event already drawn
//! and a result already returned would both have been recorded without saying
//! whose they were. The session log is the one that still is — it takes a
//! [`crate::Message`] and nothing about who produced it — which is the reason
//! to fix the shape now rather than after a second execution exists to need it.

use crate::ids::RunId;

/// Why persisted execution ancestry could not represent a real tree position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AncestryError {
    /// A root had a parent, or was not its own root.
    #[error("invalid root execution ancestry")]
    InvalidRoot,
    /// A descendant had no parent.
    #[error("invalid descendant execution ancestry")]
    MissingParent,
}

/// One execution's place in the tree of executions.
///
/// Carried by everything an execution produces, so that two runs cannot be read
/// as one. `Copy` and pointer-free, because an event carries it and events are
/// posted per delta.
///
/// A child narrows: it takes a fresh [`RunId`], keeps its parent's root, and
/// counts one deeper. Nothing here lets a child claim a shallower depth or a
/// different root than the parent it was derived from, which is the structural
/// half of the rule that a descendant may narrow what it was given and never
/// widen it.
///
/// What it does not enforce is how many of these a turn holds. One per turn is
/// what the callers do, not something this type can check: [`Ancestry::new`]
/// is public and takes nothing, so a caller minting a second one mid-turn gets
/// a second root rather than an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ancestry {
    run: RunId,
    parent: Option<RunId>,
    root: RunId,
    depth: u16,
}

impl Ancestry {
    /// A top-level execution: nothing started it, so it is its own root.
    #[must_use]
    pub fn new() -> Self {
        let run = RunId::new();
        Self {
            run,
            parent: None,
            root: run,
            depth: 0,
        }
    }

    /// An execution started by this one.
    ///
    /// Derived rather than constructed, so a child cannot be handed a root or a
    /// depth of its own choosing.
    #[must_use]
    pub fn child(&self) -> Self {
        Self {
            run: RunId::new(),
            parent: Some(self.run),
            root: self.root,
            depth: self.depth.saturating_add(1),
        }
    }

    /// Restores ancestry from a protected versioned checkpoint or journal.
    ///
    /// # Errors
    ///
    /// A depth-zero run must be its own root with no parent; a descendant must
    /// name a parent. Callers still revalidate authority independently.
    pub fn restore(
        run: RunId,
        parent: Option<RunId>,
        root: RunId,
        depth: u16,
    ) -> Result<Self, AncestryError> {
        if depth == 0 {
            if parent.is_some() || run != root {
                return Err(AncestryError::InvalidRoot);
            }
        } else if parent.is_none() {
            return Err(AncestryError::MissingParent);
        }
        Ok(Self {
            run,
            parent,
            root,
            depth,
        })
    }

    /// This execution.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.run
    }

    /// The execution that started it, where one did.
    #[must_use]
    pub const fn parent(&self) -> Option<RunId> {
        self.parent
    }

    /// The top-level execution this one descends from.
    #[must_use]
    pub const fn root(&self) -> RunId {
        self.root
    }

    /// How many executions deep this one is, counting a top-level run as zero.
    ///
    /// Saturating, so a depth nobody would configure cannot wrap around into a
    /// number that reads as shallow.
    #[must_use]
    pub const fn depth(&self) -> u16 {
        self.depth
    }
}

/// Every call is a different root, for the reason [`RunId`]'s is.
impl Default for Ancestry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_top_level_run_is_its_own_root_and_has_no_parent() {
        let top = Ancestry::new();

        assert_eq!(top.parent(), None);
        assert_eq!(top.root(), top.run());
        assert_eq!(top.depth(), 0);
    }

    #[test]
    fn two_top_level_runs_are_different_runs() {
        assert_ne!(Ancestry::new().run(), Ancestry::new().run());
    }

    #[test]
    fn a_child_keeps_the_root_and_counts_one_deeper() {
        let top = Ancestry::new();
        let child = top.child();

        assert_ne!(child.run(), top.run(), "a child is its own execution");
        assert_eq!(child.parent(), Some(top.run()));
        assert_eq!(child.root(), top.root());
        assert_eq!(child.depth(), 1);
    }

    #[test]
    fn a_grandchild_still_names_the_top_level_run_as_its_root() {
        let top = Ancestry::new();
        let child = top.child();
        let grandchild = child.child();

        assert_eq!(grandchild.parent(), Some(child.run()));
        assert_eq!(grandchild.root(), top.run());
        assert_eq!(grandchild.depth(), 2);
    }
}
