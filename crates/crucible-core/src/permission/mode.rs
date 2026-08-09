//! The disposition for calls no rule spoke about.
//!
//! A mode is one value for the session and it changes exactly one thing: what
//! happens in the arm no rule matched. It is never a way round the engine.
//! Every call takes the same route to running whatever the mode is, so the
//! number of ways a tool can run stays one — which is the property that stops
//! this from multiplying into a state per mode per tool.

use std::fmt;

use super::Sensitivity;
use super::rule::Disposition;

/// What happens to a call no rule spoke about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Ask before anything is changed or run.
    #[default]
    Ask,
    /// Change files without asking; still ask before running a program.
    AllowEdits,
    /// Do not ask.
    FullAccess,
}

impl Mode {
    /// The disposition for a call the rules said nothing about.
    ///
    /// A read is allowed in every mode. It is not that reading is harmless —
    /// `deny` rules exist because it is not — but that a question nobody would
    /// meaningfully answer is a question that trains people to press yes.
    /// Standing policy is where a read is stopped, and standing policy is a
    /// rule rather than a mode.
    pub(super) fn default_arm(self, sensitivity: &Sensitivity) -> Disposition {
        // One row per kind of call, one arm per mode inside it. Nothing is
        // closed with a wildcard: a mode added here, or a sensitivity added
        // there, has to be decided about rather than quietly inheriting
        // whichever arm happened to be written last.
        match sensitivity {
            Sensitivity::ReadOnly { .. } => match self {
                Self::Ask | Self::AllowEdits | Self::FullAccess => Disposition::Allow,
            },

            Sensitivity::MutatesFile { .. } => match self {
                Self::Ask => Disposition::Ask,
                Self::AllowEdits | Self::FullAccess => Disposition::Allow,
            },

            Sensitivity::SpawnsProcess { .. } => match self {
                Self::Ask | Self::AllowEdits => Disposition::Ask,
                Self::FullAccess => Disposition::Allow,
            },
        }
    }
}

impl fmt::Display for Mode {
    /// Spelled the way configuration spells it, so what is on screen is what
    /// you would type to change it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ask => "ask",
            Self::AllowEdits => "allowEdits",
            Self::FullAccess => "fullAccess",
        })
    }
}
