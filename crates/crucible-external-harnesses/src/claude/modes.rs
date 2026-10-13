//! The reviewed table from the mode a conversation runs in to the permission
//! mode Claude Code is started in.
//!
//! Every mode has a row, and a row either names the one permission mode that
//! keeps the mode's promise or is unsupported: a mode with no faithful
//! permission mode is refused, never approximated by a wider one. No row
//! starts the agent in a mode that skips its permission prompts, so every
//! action the agent's own rules leave to a prompt comes back as a question.

use crucible_execution::Failure;

/// The mode a conversation runs an agent in, as the conversation's own modes
/// are written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Mode {
    /// Ask before anything is changed or run.
    #[default]
    Ask,
    /// Change files without asking; ask before anything that starts a
    /// process.
    AllowEdits,
    /// Do not ask.
    FullAccess,
}

impl Mode {
    /// Every mode, so the table is checked against each.
    pub const ALL: [Self; 3] = [Self::Ask, Self::AllowEdits, Self::FullAccess];
}

/// A permission mode Claude Code starts in, as its command line names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Prompts for anything its rules do not allow.
    Default,
    /// Edits files without prompting.
    AcceptEdits,
    /// Plans and changes nothing.
    Plan,
    /// Lets a classifier decide what would have prompted.
    Auto,
    /// Denies whatever would have prompted.
    DontAsk,
    /// Skips every prompt.
    BypassPermissions,
}

impl Permission {
    /// Every permission mode the reviewed releases have.
    pub const ALL: [Self; 6] = [
        Self::Default,
        Self::AcceptEdits,
        Self::Plan,
        Self::Auto,
        Self::DontAsk,
        Self::BypassPermissions,
    ];

    /// Its name on the command line.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::Auto => "auto",
            Self::DontAsk => "dontAsk",
            Self::BypassPermissions => "bypassPermissions",
        }
    }
}

/// What one mode comes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    /// The agent is started in this permission mode.
    Runs(Permission),
    /// No permission mode keeps this mode's promise, for this reason.
    Unsupported(&'static str),
}

/// The reviewed table.
#[derive(Debug, Clone, Copy)]
pub struct Modes;

impl Modes {
    /// The row of `mode`.
    #[must_use]
    pub const fn row(mode: Mode) -> Row {
        match mode {
            Mode::Ask => Row::Runs(Permission::Default),
            Mode::AllowEdits => Row::Runs(Permission::AcceptEdits),
            Mode::FullAccess => Row::Unsupported(
                "Claude Code does not run in full access here: its only mode that does not ask \
                 would skip every prompt the agent asks, with nothing left to stop it. Use \
                 ask or allow edits instead.",
            ),
        }
    }

    /// The permission mode `mode` starts the agent in.
    ///
    /// # Errors
    ///
    /// A [`Failure`] naming why, where the row is unsupported.
    pub fn permission(mode: Mode) -> Result<Permission, Failure> {
        match Self::row(mode) {
            Row::Runs(permission) => Ok(permission),
            Row::Unsupported(why) => Err(Failure::new(why)),
        }
    }
}
