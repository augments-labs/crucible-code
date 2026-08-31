//! The proof that a call was permitted, and the call it was permitted for.

use std::path::Path;

use crate::tool::{ToolArgs, ToolCall};
use crate::toolset::ToolGeneration;
use crate::workspace::{Workspace, WorkspacePath};

use super::rule::Denials;
use super::{Sensitivity, Target, Verdict};

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
///
/// The tool is the third thing a call is made of, and it comes out of here for
/// the same reason. A verdict is reached about `write` changing a file; the
/// value that says so names `write`, so the tool it reaches is not something a
/// caller looks up again beside it.
#[derive(Debug)]
pub struct Approved {
    call: ToolCall,

    /// The immutable tool generation that admitted this call. Direct uses of
    /// the permission engine outside the invocation pipeline leave this empty
    /// and therefore cannot resolve through a [`crate::ToolSnapshot`].
    generation: Option<ToolGeneration>,

    /// Never read. Holding it is the point: an `Approved` cannot be built
    /// without one, and one cannot be built without an allow.
    _grant: Grant,

    /// What the verdict was reached about the call *doing* — which is part of
    /// what was decided, so it travels with the proof. A tool that resolves
    /// its path again in `run` reads this to learn what kind of call the
    /// question described, because the filesystem may answer differently by
    /// then and the verdict does not stretch to the new answer.
    sensitivity: Sensitivity,

    /// What this call may still not read, however far it reaches on its own.
    denied: Denials,
}

impl Approved {
    /// Minted by the engine, once a verdict has been reached about this call.
    pub(super) fn new(
        call: ToolCall,
        sensitivity: Sensitivity,
        generation: Option<ToolGeneration>,
        grant: Grant,
        denied: Denials,
    ) -> Self {
        Self {
            call,
            generation,
            _grant: grant,
            sensitivity,
            denied,
        }
    }

    pub(crate) fn generation(&self) -> Option<&ToolGeneration> {
        self.generation.as_ref()
    }

    /// What the verdict was reached about the call doing.
    #[must_use]
    pub fn sensitivity(&self) -> &Sensitivity {
        &self.sensitivity
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

    /// The tool a verdict was reached about.
    ///
    /// What the caller dispatches on, so that the tool a proof arrives at is
    /// the tool the proof is about. A name held separately is a name that can
    /// be the wrong one — self-limiting while every tool refuses arguments it
    /// does not recognise, and only while that stays true of every tool there
    /// is.
    #[must_use]
    pub fn tool(&self) -> &str {
        &self.call.name
    }

    /// The arguments a verdict was reached about.
    #[must_use]
    pub fn args(&self) -> &ToolArgs {
        &self.call.args
    }
}

#[cfg(test)]
mod tests {
    use crate::ids::ToolId;
    use crate::permission::Rules;
    use crate::tool::ToolArgs;

    use super::*;

    fn call() -> ToolCall {
        ToolCall {
            id: ToolId::new("a"),
            name: "write".into(),
            args: ToolArgs::new(r#"{"path":"src/a.rs"}"#),
        }
    }

    #[test]
    fn a_denial_mints_no_proof() {
        // The whole of what the private field buys: there is one constructor
        // and it answers `None` to the verdict that is not an allow.
        assert!(Grant::issue(Verdict::Deny).is_none());
    }

    #[test]
    fn an_approval_names_the_tool_and_the_arguments_of_the_one_call() {
        // Both halves out of one value. A caller that reads the tool from here
        // cannot dispatch a proof about `write` to something else, whatever it
        // happens to be holding beside it.
        let grant = Grant::issue(Verdict::Allow).expect("an allow mints one");
        let sensitivity = Sensitivity::MutatesFile {
            target: Target::at("/w/src/a.rs", Some("src/a.rs")),
        };
        let approved = Approved::new(
            call(),
            sensitivity,
            None,
            grant,
            Rules::new().denials("write"),
        );

        assert_eq!(approved.tool(), "write");
        assert_eq!(approved.args().as_str(), r#"{"path":"src/a.rs"}"#);
    }
}
