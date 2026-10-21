//! The reviewed table from the mode a conversation runs in to the permission
//! mode Claude Code is started in.
//!
//! Every mode has a row naming the one permission mode that keeps its promise,
//! and none of the agent's other modes is ever started: asking keeps every
//! prompt the agent's rules leave, allowing edits skips only the prompts for
//! edits, and full access skips every prompt, as the conversation's own full
//! access asks about nothing.

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

    /// The permission mode named `name` on the command line.
    #[must_use]
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|permission| permission.name() == name)
    }

    /// Whether the agent, started in `self`, may run in `running` without
    /// asking less than it was started to: a mode that asks at least as much,
    /// such as `default` after `acceptEdits`, or one that changes nothing or
    /// denies what would have prompted.
    #[must_use]
    pub const fn admits(self, running: Self) -> bool {
        running.lets_through() <= self.lets_through()
    }

    /// How much this mode lets through without asking, least first: changing
    /// nothing and denying what would have prompted let nothing through, and
    /// each mode after lets through everything the one before it does and
    /// more.
    const fn lets_through(self) -> u8 {
        match self {
            Self::Plan | Self::DontAsk => 0,
            Self::Default => 1,
            Self::AcceptEdits => 2,
            Self::Auto => 3,
            Self::BypassPermissions => 4,
        }
    }

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

/// The reviewed table.
#[derive(Debug, Clone, Copy)]
pub struct Modes;

impl Modes {
    /// The permission mode `mode` starts the agent in.
    #[must_use]
    pub const fn permission(mode: Mode) -> Permission {
        match mode {
            Mode::Ask => Permission::Default,
            Mode::AllowEdits => Permission::AcceptEdits,
            Mode::FullAccess => Permission::BypassPermissions,
        }
    }
}
