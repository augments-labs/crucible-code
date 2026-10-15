//! The installed agents this build can run, one row each.
//!
//! A [`Row`] is everything the rest of the application needs to know about an
//! agent besides what its configuration says: which driver it is, how its
//! wire is spoken in each mode a conversation runs in, or why it is not run
//! in one, which key names its profile directory, the content-use
//! route it is held under, the section of `docs/providers/agents.md` that
//! documents it, the hosts it must reach, the command a person signs it in
//! with where it has one, and whether this release ships it.
//! The chooser, the launch and every test read the [`Registry`] they were
//! built over, and the shipped one is [`REGISTRY`]; a row that does not ship
//! is listed nowhere and refused at launch with the sentence that names its
//! section.
//!
//! The rows agree with the configuration's driver list and with the
//! content-use routes by a test, because neither can be derived from the
//! other: a driver is a word the user writes, and its route is asked about in
//! the vendor's own terms.

use crucible_config::Driver;
use crucible_execution::{Failure, Text};
use crucible_external_harnesses::Dialect;
use crucible_external_harnesses::antigravity::Antigravity;
use crucible_external_harnesses::claude::Claude;
use crucible_tools::Mode;

/// The most rows a registry holds, which is more agents than any release has
/// offered at once.
pub const ROWS: usize = 16;

/// One installed agent this build knows how to run.
#[derive(Clone, Copy)]
pub struct Row {
    /// Which agent, as the configuration names it.
    pub driver: Driver,
    /// Its wire, fresh for each run, as it is spoken in the conversation's
    /// mode, or why the agent does not run in that mode.
    pub dialect: fn(Mode) -> Result<Box<dyn Dialect>, Failure>,
    /// The key of its `externalAgents` entry that names its profile directory.
    pub profile_key: &'static str,
    /// The content-use route a launch of it is held under.
    pub route: &'static str,
    /// Its section of `docs/providers/agents.md`, as the anchor after `#`.
    pub docs_section: &'static str,
    /// The hosts it must reach, which its sandbox lets it.
    pub reaches: &'static [&'static str],
    /// The command a person runs in a terminal to sign the profile directory
    /// given in, or the agent's usual one; `None` for an agent that is signed
    /// in from its own window and has no such command.
    pub sign_in: fn(Option<&std::path::Path>) -> Option<Text>,
    /// Whether this release lets it run.
    pub shipping: bool,
}

impl std::fmt::Debug for Row {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Row")
            .field("driver", &self.driver)
            .field("route", &self.route)
            .field("shipping", &self.shipping)
            .finish_non_exhaustive()
    }
}

impl Row {
    /// Where its documentation is, as a path a user can open.
    #[must_use]
    pub fn documented(&self) -> String {
        format!("docs/providers/agents.md#{}", self.docs_section)
    }
}

/// The agents one build can run.
#[derive(Debug, Clone, Copy)]
pub struct Registry(&'static [Row]);

impl Registry {
    /// `rows`, of which at most [`ROWS`] are read.
    #[must_use]
    pub const fn new(rows: &'static [Row]) -> Self {
        Self(rows)
    }

    /// Every row, whether or not it ships.
    pub fn all(&self) -> impl Iterator<Item = &'static Row> {
        self.0.iter().take(ROWS)
    }

    /// The rows this release lets run, which are the ones a chooser lists.
    pub fn shipping(&self) -> impl Iterator<Item = &'static Row> {
        self.all().filter(|row| row.shipping)
    }

    /// The row for `driver`, shipping or not.
    #[must_use]
    pub fn row(&self, driver: Driver) -> Option<&'static Row> {
        self.all().find(|row| row.driver == driver)
    }
}

/// The registry this build ships.
pub static REGISTRY: Registry = Registry::new(&[CLAUDE, ANTIGRAVITY]);

/// Claude Code's row, as this release ships it.
pub(crate) const CLAUDE: Row = Row {
    driver: Driver::Claude,
    // Started in the permission mode the reviewed table gives the
    // conversation's mode, and refused in one it gives none.
    dialect: |mode| {
        Claude::in_mode(claude_mode(mode)).map(|claude| Box::new(claude) as Box<dyn Dialect>)
    },
    profile_key: "configDir",
    route: "agent:claude",
    docs_section: "claude-code",
    // The service it asks, and the sign-in pages and token exchange of
    // a subscription account.
    reaches: &[
        "api.anthropic.com",
        "claude.ai",
        "claude.com",
        "platform.claude.com",
    ],
    sign_in: |profile| Some(crucible_external_harnesses::claude::sign_in(profile)),
    shipping: true,
};

/// Antigravity's row, as this release ships it.
pub(crate) const ANTIGRAVITY: Row = Row {
    driver: Driver::Antigravity,
    // The agent asks about every action itself, whatever the
    // conversation's mode says.
    dialect: |_| Ok(Box::new(Antigravity::default())),
    profile_key: "profileDir",
    route: "agent:antigravity",
    docs_section: "antigravity",
    // Its sign-in page, the token exchange behind it, and the service it
    // asks, in its released and its daily build.
    reaches: &[
        "accounts.google.com",
        "oauth2.googleapis.com",
        "cloudcode-pa.googleapis.com",
        "daily-cloudcode-pa.googleapis.com",
    ],
    sign_in: |_| None,
    shipping: true,
};

/// The mode Claude Code's table has for the conversation's `mode`.
const fn claude_mode(mode: Mode) -> crucible_external_harnesses::claude::Mode {
    use crucible_external_harnesses::claude::Mode as Agents;
    match mode {
        Mode::Ask => Agents::Ask,
        Mode::AllowEdits => Agents::AllowEdits,
        Mode::FullAccess => Agents::FullAccess,
    }
}

#[cfg(test)]
mod tests;
