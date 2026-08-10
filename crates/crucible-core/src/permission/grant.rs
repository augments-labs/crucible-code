//! The proof that a call was permitted, and the call it was permitted for.

use std::path::Path;

use crate::tool::{ToolArgs, ToolCall};
use crate::workspace::{Workspace, WorkspacePath};

use super::rule::Denials;
use super::{Target, Verdict};

/// Proof that a verdict was reached and it was to allow.
///
/// The field is private to this module — not to the crate. Widening it to
/// `pub(crate)` would let any core module mint one, which ends the guarantee
/// that a verdict was reached at all.
#[derive(Debug)]
pub struct Grant(());

impl Grant {
    /// The only way one comes into existence.
    pub(super) fn issue(verdict: Verdict) -> Option<Self> {
        match verdict {
            Verdict::Allow => Some(Self(())),
            Verdict::Deny => None,
        }
    }
}

/// A call, together with the proof that this call was permitted.
///
/// A grant and the arguments it was reached about used to travel as two
/// values, and nothing structurally stopped a grant minted for one call being
/// handed to a tool alongside another's arguments. Only the runner's care
/// prevented it, and "the caller is careful" is the guarantee this whole
/// mechanism exists to replace. Carrying both in one value with private fields
/// makes the arguments a tool runs on *the* arguments a verdict was reached
/// about, by construction.
#[derive(Debug)]
pub struct Approved {
    call: ToolCall,
    /// Never read. Holding it is the point: an `Approved` cannot be built
    /// without one, and one cannot be built without an allow.
    _grant: Grant,

    /// What this call may still not read, however far it reaches on its own.
    denied: Denials,
}

impl Approved {
    /// Minted by the engine, once a verdict has been reached about this call.
    pub(super) fn new(call: ToolCall, grant: Grant, denied: Denials) -> Self {
        Self {
            call,
            _grant: grant,
            denied,
        }
    }

    /// Whether a rule refuses a file this call reached by itself.
    ///
    /// A verdict is reached about one thing. For a search that thing is the
    /// directory it walks, so every file below it is one nobody was asked
    /// about — and a rule written about such a file has no other moment to be
    /// honoured in than the moment the walk arrives at it.
    ///
    /// `from` is where the walk began, which the workspace proved before it
    /// started. A path that is not under it is refused rather than allowed:
    /// this call cannot say where it came from, and the answer that costs
    /// nothing is the one that keeps a file out of an answer.
    #[must_use]
    pub fn denies(&self, workspace: &Workspace, from: &WorkspacePath, path: &Path) -> bool {
        if self.denied.is_empty() {
            return false;
        }

        // How the path is written down comes from the very patterns that are
        // about to read it, so a spelling this does not build is one none of
        // them would have looked at. The two calls stay in one expression for
        // that reason: `denied` decides the spelling and then decides what it
        // means, and separating them is what would let a target be matched
        // against a different set of denials than the one it was spelled for.
        Target::walked(workspace, from, path, self.denied.wanted())
            .is_none_or(|reached| self.denied.names(&reached))
    }

    /// The arguments a verdict was reached about.
    #[must_use]
    pub fn args(&self) -> &ToolArgs {
        &self.call.args
    }
}
