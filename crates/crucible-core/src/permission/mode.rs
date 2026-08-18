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
    /// Change files without asking, through the tools that change them; ask
    /// before anything that starts a process.
    AllowEdits,
    /// Do not ask.
    FullAccess,
}

impl Mode {
    /// The next mode of the ring, which stepping through them lands on.
    ///
    /// A ring rather than a list with two ends. The step is one key, and a key
    /// that stops working at the end of a list is one somebody presses twice
    /// before working out that nothing is broken.
    ///
    /// The order is the permissiveness the modes are written in above, so the
    /// step is always the same direction and a reader who has pressed it once
    /// knows where the next press goes.
    #[must_use]
    pub const fn next(self) -> Self {
        match self {
            Self::Ask => Self::AllowEdits,
            Self::AllowEdits => Self::FullAccess,
            Self::FullAccess => Self::Ask,
        }
    }

    /// The mode as the row under the prompt box says it.
    ///
    /// Not what [`fmt::Display`] writes, and the difference is who the string
    /// is for: `allowEdits` is what you would *type* into a configuration file,
    /// and this is what you *read* off the screen while it is in force. Both
    /// live here so that a mode added later cannot be given one and not the
    /// other — a table of these in the renderer would have compiled fine and
    /// printed nothing.
    #[must_use]
    pub const fn sentence(self) -> &'static str {
        match self {
            Self::Ask => "ask mode on",
            Self::AllowEdits => "allow edits on",
            Self::FullAccess => "full access mode on",
        }
    }

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

            // A program is asked about however closely its line was read.
            // Reading one can say where its words pointed at the moment they
            // were read, and that is not the moment they are used: a shell
            // opens every one of them again by name when the command runs, so
            // a symbolic link put at one of those names in between sends the
            // write somewhere else and nobody was asked. The file tools have
            // no such gap — they hold on to the directory they proved and
            // never look a path up twice — and a shell cannot be made to work
            // that way, so a proof about a command line is a statement about
            // the past rather than a licence to skip the question.
            Sensitivity::SpawnsProcess { .. } => match self {
                Self::Ask | Self::AllowEdits => Disposition::Ask,
                Self::FullAccess => Disposition::Allow,
            },

            // The only call whose effect is not on this machine, and the only
            // direction anything can leave in. It reads and changes nothing
            // here, so the arm it most resembles is the read above — and that
            // arm allows in every mode, which would make the one call that
            // cannot be recalled the one nobody is ever asked about.
            //
            // `allowEdits` asks, which is the half worth stating. That mode
            // relaxes exactly one thing, writing files in a workspace already
            // entrusted to crucible, and sending a query to somebody else's
            // service is not that.
            Sensitivity::ReachesNetwork { .. } => match self {
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
