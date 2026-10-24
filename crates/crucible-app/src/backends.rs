//! The installed agents this build can run, one row each.
//!
//! A [`Row`] is everything the rest of the application needs to know about an
//! agent built in besides what its configuration says: which driver it is, how
//! its wire is spoken in each mode a conversation runs in, or why it is not run
//! in one, which key names its profile directory, the content-use
//! route it is held under, the section of `docs/providers/agents.md` that
//! documents it, the hosts it must reach, the command a person signs it in
//! with where it has one, the release of it crucible installs, and whether
//! this release ships it.
//! The chooser, the launch and every test read the [`Registry`] they were
//! built over, and the shipped one is [`REGISTRY`]; a row that does not ship
//! is listed nowhere and refused at launch with the sentence that names its
//! section.
//!
//! The release a row installs, and the releases its agent is admitted as, are
//! the catalog's to say: the trust state offers them, the newest installable
//! one is the row's [`pinned`](Row::pinned), and a row that takes its releases
//! from the catalog says so, so that a trust state that cannot be used is told
//! to the user and not read as nothing to install.
//!
//! An agent the catalog reviewed has no row built in: its row is a
//! [`CatalogRow`], owned and made from the entry and target it was installed
//! from, and held under the agent route of its key. It is spoken to over the
//! Agent Client Protocol as its entry's profile says, reaches only the hosts
//! that profile grants, and always ships, since a registry lists one only for
//! an entry the catalog reviewed. It also holds the warning its entry's
//! statement of content use gives its route, or why the entry gives none, in
//! which case the agent is listed and never launched. [`Registry::row`] answers a built-in driver
//! with its row and a catalog instance with the row listed for its key, each
//! as an [`AgentRow`]. The registry this build ships lists no catalog row, so
//! a catalog instance has none to run until one is listed.
//!
//! The rows agree with the configuration's driver list and with the
//! content-use routes by a test, because neither can be derived from the
//! other: a driver is a word the user writes, and its route is asked about in
//! the vendor's own terms.

use std::path::Path;

use crucible_config::Driver;
use crucible_execution::{Failure, Text};
use crucible_external_harnesses::Dialect;
use crucible_external_harnesses::acp::{Acp, Allowing};
use crucible_external_harnesses::antigravity::{self, Antigravity};
use crucible_external_harnesses::catalog::{CatalogProfile, Entry, Target, Trust, Unprofiled};
use crucible_external_harnesses::claude::Claude;
use crucible_external_harnesses::release::OwnedPin;
use crucible_sandbox::SandboxDomainPattern;
use crucible_tools::Mode;
use crucible_types::AgentKey;

use crate::content_use::{Unstated, Warned, agent_route, stated};

/// A row's wire as it is made for a run, or why the agent does not run in the
/// mode asked for.
pub type Wire = Result<Box<dyn Dialect>, Failure>;

/// The most rows a registry holds, which is more agents than any release has
/// offered at once.
pub const ROWS: usize = 16;

/// One installed agent this build knows how to run.
#[derive(Clone, Copy)]
pub struct Row {
    /// Which agent, as the configuration names it.
    pub driver: Driver,
    /// Its wire, fresh for each run, as it is spoken in the conversation's
    /// mode, or why the agent does not run in that mode. Every shipped row
    /// runs in every mode. An agent whose releases the catalog offers is
    /// admitted only as one `trust` offers; given no trust state it is
    /// admitted as none, which is all that asking whether it runs in a mode,
    /// or what it is called, needs.
    pub dialect: fn(Mode, Option<&Trust>, Option<&Path>) -> Wire,
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
    /// The release of it crucible downloads for the user on the platform it
    /// runs on, which `crucible agents install` puts in crucible's home: the
    /// newest that `trust` offers and that is one archive of two files; `None`
    /// for an agent the user installs themselves, and while nothing is
    /// offered.
    pub pinned: fn(&Trust) -> Option<OwnedPin>,
    /// Whether its releases are the catalog's, so that it is offered nothing,
    /// and says why, while the trust state cannot be used.
    pub catalogued: bool,
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

/// The row of an agent the catalog reviewed, made from its entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogRow {
    /// The agent, as its instance names it.
    key: AgentKey,
    /// The content-use route a launch of it is held under.
    route: String,
    /// How it is run, as its entry reviewed it.
    profile: CatalogProfile,
    /// The warning a launch of it is held under, built from its entry's
    /// statement of content use, or why the entry gives none.
    warned: Result<Box<Warned>, Unstated>,
}

impl CatalogRow {
    /// The row of the agent `entry` reviewed, installed from its `target`.
    ///
    /// # Errors
    ///
    /// [`Unprofiled`] where the entry gives no profile to run it under.
    pub fn new(entry: &Entry, target: &Target) -> Result<Self, Unprofiled> {
        Ok(Self {
            key: entry.key.clone(),
            route: agent_route(entry.key.as_str()),
            profile: CatalogProfile::from_entry(entry, target)?,
            warned: stated(entry).map(Box::new),
        })
    }

    /// The warning a launch of it is held under, which the build gives its
    /// [`Routes`](crate::content_use::Routes) so the route is asked about; or
    /// why its entry gives none, in which case it is never launched.
    ///
    /// # Errors
    ///
    /// [`Unstated`] where its entry gives no statement a launch can be asked
    /// under.
    pub fn warned(&self) -> Result<&Warned, &Unstated> {
        self.warned.as_ref().map(|warned| &**warned)
    }

    /// The content-use route a launch of it is held under.
    #[must_use]
    pub fn route(&self) -> &str {
        &self.route
    }

    /// Its wire, fresh for a run, as it is spoken in the conversation's
    /// `mode`: its entry's profile, answering yes by itself to what the mode
    /// allows.
    #[must_use]
    pub fn dialect(&self, mode: Mode) -> Box<dyn Dialect> {
        Box::new(Acp::cataloged(self.profile.clone(), allowing(mode)))
    }
}

/// An agent's row: one built in, or one made from the catalog entry of the
/// agent an instance names.
#[derive(Debug, Clone)]
pub enum AgentRow {
    /// A row this build ships for its driver.
    Built(&'static Row),
    /// The row of an agent the catalog reviewed.
    Catalog(CatalogRow),
}

impl AgentRow {
    /// The content-use route a launch of it is held under.
    #[must_use]
    pub fn route(&self) -> &str {
        match self {
            Self::Built(row) => row.route,
            Self::Catalog(row) => row.route(),
        }
    }

    /// Its wire, fresh for a run, as it is spoken in the conversation's
    /// `mode`, admitting the agent as the releases `trust` offers; as none
    /// given no trust state.
    ///
    /// # Errors
    ///
    /// Why the agent does not run in `mode`.
    pub fn dialect(&self, mode: Mode, trust: Option<&Trust>, program: Option<&Path>) -> Wire {
        match self {
            Self::Built(row) => {
                let unusable = trust
                    .filter(|_| row.catalogued)
                    .and_then(|trust| Some((trust.file(), trust.unusable()?)));
                if let Some((file, why)) = unusable {
                    return Err(Failure::new(&format!(
                        "The catalog trust state at {} cannot be used ({why}), so no release of \
                         {} is admitted. Repair or remove that file.",
                        file.display(),
                        row.driver.name()
                    )));
                }
                (row.dialect)(mode, trust, program)
            }
            Self::Catalog(row) => Ok(row.dialect(mode)),
        }
    }

    /// The hosts it must reach, which its sandbox lets it.
    #[must_use]
    pub fn reaches(&self) -> Vec<&str> {
        match self {
            Self::Built(row) => row.reaches.to_vec(),
            Self::Catalog(row) => row
                .profile
                .reach()
                .iter()
                .map(SandboxDomainPattern::as_str)
                .collect(),
        }
    }

    /// What it is called where nothing warns about its route: its driver's
    /// word, or a catalog agent's key.
    #[must_use]
    pub fn named(&self) -> &str {
        match self {
            Self::Built(row) => row.driver.name(),
            Self::Catalog(row) => row.key.as_str(),
        }
    }

    /// The command it is run as, as its wire names it: the same in every
    /// mode, and asking is a mode every row runs in. A catalog agent is
    /// looked up as nothing, since it runs from the path its instance names,
    /// and is called by its key.
    #[must_use]
    pub fn command(&self) -> String {
        self.dialect(Mode::default(), None, None).map_or_else(
            |_| self.named().to_owned(),
            |dialect| dialect.program().to_owned(),
        )
    }
}

/// The agents one build can run.
#[derive(Debug, Clone, Copy)]
pub struct Registry {
    /// The rows built in.
    rows: &'static [Row],
    /// The rows of the agents the catalog reviewed that it lists.
    catalog: &'static [CatalogRow],
}

impl Registry {
    /// `rows`, of which at most [`ROWS`] are read, and no catalog row.
    #[must_use]
    pub const fn new(rows: &'static [Row]) -> Self {
        Self { rows, catalog: &[] }
    }

    /// The same registry, listing `catalog` as the rows of the agents the
    /// catalog reviewed.
    #[must_use]
    pub const fn cataloging(self, catalog: &'static [CatalogRow]) -> Self {
        Self { catalog, ..self }
    }

    /// Every row built in, whether or not it ships.
    pub fn all(&self) -> impl Iterator<Item = &'static Row> {
        self.rows.iter().take(ROWS)
    }

    /// The rows built in that this release lets run, which are the ones a
    /// chooser lists.
    pub fn shipping(&self) -> impl Iterator<Item = &'static Row> {
        self.all().filter(|row| row.shipping)
    }

    /// The row built in for `driver`, shipping or not.
    #[must_use]
    pub fn built(&self, driver: Driver) -> Option<&'static Row> {
        self.all().find(|row| row.driver == driver)
    }

    /// The row of an instance of `driver` naming the catalog agent `agent`:
    /// a built-in driver's own, shipping or not, and for a catalog instance
    /// the row listed for its key; none for a catalog instance naming no key
    /// or one not listed.
    #[must_use]
    pub fn row(&self, driver: Driver, agent: Option<&AgentKey>) -> Option<AgentRow> {
        match (driver, agent) {
            (Driver::Catalog, Some(agent)) => self
                .catalog
                .iter()
                .find(|row| row.key == *agent)
                .cloned()
                .map(AgentRow::Catalog),
            (Driver::Catalog, None) => None,
            (Driver::Claude | Driver::Antigravity, _) => self.built(driver).map(AgentRow::Built),
        }
    }
}

/// The registry this build ships.
pub static REGISTRY: Registry = Registry::new(&[CLAUDE, ANTIGRAVITY]);

/// Claude Code's row, as this release ships it.
pub(crate) const CLAUDE: Row = Row {
    driver: Driver::Claude,
    // Started in the permission mode the reviewed table gives the
    // conversation's mode.
    dialect: |mode, _, _| Ok(Box::new(Claude::in_mode(claude_mode(mode)))),
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
    // It installs and updates itself.
    pinned: |_| None,
    catalogued: false,
    shipping: true,
};

/// Antigravity's row, as this release ships it.
pub(crate) const ANTIGRAVITY: Row = Row {
    driver: Driver::Antigravity,
    // The agent asks about every action itself, and what the
    // conversation's mode allows is answered yes without asking.
    dialect: |mode, trust, program| {
        let offered = trust.map_or_else(Vec::new, antigravity::offered);
        if let Some(version) = antigravity::unoffered(&offered, program) {
            let listed: Vec<&str> = offered.iter().map(|pin| &*pin.version).collect();
            return Err(Failure::new(&format!(
                "Version {version} of the agent, where this program is kept, is a release the \
                 catalog does not offer; it offers {}.",
                listed.join(", ")
            )));
        }
        let offered = antigravity::started(offered, program);
        Ok(Box::new(Antigravity::allowing(allowing(mode), &offered)))
    },
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
    // The newest release the catalog offers that the installer can unpack.
    pinned: |trust| {
        antigravity::offered(trust)
            .into_iter()
            .find(|pin| pin.pin().is_some())
    },
    catalogued: true,
    shipping: true,
};

/// What an agent spoken to over the Agent Client Protocol, Antigravity or one
/// the catalog reviewed, answers yes to by itself in the conversation's `mode`.
const fn allowing(mode: Mode) -> Allowing {
    match mode {
        Mode::Ask => Allowing::Nothing,
        Mode::AllowEdits => Allowing::Edits,
        Mode::FullAccess => Allowing::Everything,
    }
}

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
pub(crate) mod tests;
