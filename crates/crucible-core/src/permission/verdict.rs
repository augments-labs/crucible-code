//! What a decision is, and how one is sought.
//!
//! A decision and how long it lasts are two axes, not one. Welding them
//! together — an `AllowOnce` beside an `AllowSession` — reads as one enum until
//! a third duration arrives and has to be spelled out against every decision.

use crate::tool::ToolCall;

use super::Sensitivity;

/// A permission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// It may run.
    Allow,
    /// It may not.
    Deny,
}

/// How long a decision lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Remember {
    /// This call only. The next one like it is asked about again.
    Never,
    /// Every call like this one until crucible exits. Nothing is written to
    /// disk, so it does not survive the process that made it.
    Session,
}

/// How a call is put to whoever is answering.
///
/// Implemented above this crate, because asking is a matter of whatever is on
/// screen and core knows nothing about that.
pub trait Ask {
    /// Puts one call to the user and waits for the answer.
    ///
    /// There is no way to say "I could not ask". An implementation with nobody
    /// to ask answers [`Verdict::Deny`], which is what makes a piped
    /// invocation refuse rather than hang on a question no one will see.
    fn ask(&mut self, call: &ToolCall, sensitivity: &Sensitivity) -> (Verdict, Remember);
}
