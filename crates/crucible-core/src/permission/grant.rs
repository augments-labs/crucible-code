//! The proof that a call was permitted, and the call it was permitted for.

use crate::tool::{ToolArgs, ToolCall};

use super::Verdict;

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
}

impl Approved {
    /// Minted by the engine, once a verdict has been reached about this call.
    pub(super) fn new(call: ToolCall, grant: Grant) -> Self {
        Self {
            call,
            _grant: grant,
        }
    }

    /// The arguments a verdict was reached about.
    #[must_use]
    pub fn args(&self) -> &ToolArgs {
        &self.call.args
    }

    /// The call a verdict was reached about.
    #[must_use]
    pub fn call(&self) -> &ToolCall {
        &self.call
    }
}
