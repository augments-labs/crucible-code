//! The installed agents this build can run, one row each.
//!
//! A [`Row`] is everything the rest of the application needs to know about an
//! agent built in besides what its configuration says: which driver it is, how
//! its wire is spoken in each mode a conversation runs in, or why it is not run
//! in one, which key names its profile directory, the content-use
//! route it is held under, the section of `docs/providers/agents.md` that
//! documents it, the hosts every run of it must reach (a start adds the one a
//! configured Google Cloud location names), the command a person signs it in
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
//! that profile grants, and always ships, since it is offered only for an
//! entry read from the catalog or the ACP Registry. It holds the warning its
//! entry's statement of content use gives its route; where a registry entry
//! gives none it holds [`crate::content_use::generic`]'s warning instead, and a
//! signed entry that gives none is listed and never launched.
//! [`Registry::row`] answers a built-in driver with its row and a catalog
//! instance with the row listed for its key, each as an [`AgentRow`], which
//! also says by which methods its agent signs in: its entry's for a catalog
//! agent, and for one built in what its driver's own module offers.
//!
//! The registry this build ships lists no catalog row: a catalog instance's
//! row is made per question by [`believed_row`], or for a release the ACP
//! Registry lists by [`believed_registry_row`], the one place each chooses it,
//! from the verified offer the catalog believed now pins to the version the
//! instance's program lies in, or from the cached registry index, the receipt's
//! survey and the probe record it stored. Nothing in the configuration alone
//! makes one, and neither does anything the agent replies. The reasons there is
//! none are [`Unbelieved`], each naming what to do. A row so made is owned by
//! the question it answers, so none is kept past it.
//!
//! The rows agree with the configuration's driver list and with the
//! content-use routes by a test, because neither can be derived from the
//! other: a driver is a word the user writes, and its route is asked about in
//! the vendor's own terms.

use std::path::{Path, PathBuf};

use crucible_config::{Driver, NATIVE_RUNNER};
use crucible_execution::{Failure, Text};
use crucible_external_harnesses::Dialect;
use crucible_external_harnesses::acp::{Acp, Allowing};
use crucible_external_harnesses::antigravity::{self, Antigravity};
use crucible_external_harnesses::catalog::registry::{RegistryIndex, Unoffered};
use crucible_external_harnesses::catalog::{
    AuthMethod, CatalogError, CatalogProfile, Entry, Offer, Target, Trust, Unprofiled, Unusable,
};
use crucible_external_harnesses::claude::Claude;
use crucible_external_harnesses::release::OwnedPin;
use crucible_external_harnesses::store::{Probe, Source as ReleaseSource, Store, StoreError};
use crucible_sandbox::SandboxDomainPattern;
use crucible_tools::Mode;
use crucible_types::AgentKey;

use crate::catalog_fetch::RegistryUnread;
use crate::content_use::{Unstated, Warned, agent_route, generic, stated};

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
    /// runs on, which a connect offers to put in crucible's home: the
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
    /// Where the entry came from.
    source: ReleaseSource,
    /// The day the ACP Registry was read, for a row built from it.
    read: Option<Box<str>>,
    /// The warning a launch of it is held under, built from its entry's
    /// statement of content use, from the registry's generic warning where a
    /// registry entry gives none, or why no warning can be given.
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
            source: ReleaseSource::Catalog,
            read: None,
            warned: stated(entry).map(Box::new),
        })
    }

    /// The row of the agent `entry` the ACP Registry lists, installed from its
    /// `target`, read from the registry on `read`: its entry carries no
    /// statement of content use, so the row holds the registry's generic
    /// warning. An entry that does carry a statement keeps it, as a catalog
    /// entry's is kept.
    ///
    /// # Errors
    ///
    /// [`Unprofiled`] where the entry gives no profile to run it under.
    pub fn registry(entry: &Entry, target: &Target, read: Box<str>) -> Result<Self, Unprofiled> {
        let warned = match stated(entry) {
            Ok(warned) => Ok(Box::new(warned)),
            Err(Unstated::Missing) => Ok(Box::new(generic(entry.key.as_str(), &read))),
            Err(why) => Err(why),
        };
        Ok(Self {
            key: entry.key.clone(),
            route: agent_route(entry.key.as_str()),
            profile: CatalogProfile::from_entry(entry, target)?,
            source: ReleaseSource::Registry,
            read: Some(read),
            warned,
        })
    }

    /// Where its entry came from.
    #[must_use]
    pub const fn source(&self) -> ReleaseSource {
        self.source
    }

    /// The day the ACP Registry was read, for a row built from it.
    #[must_use]
    pub fn read(&self) -> Option<&str> {
        self.read.as_deref()
    }

    /// The warning a launch of it is held under, which the consent of the run
    /// learns its route from so that the route is asked about; or why its entry
    /// gives none, in which case it is never launched.
    ///
    /// # Errors
    ///
    /// [`Unstated`] where its entry gives no statement a launch can be asked
    /// under.
    pub fn warned(&self) -> Result<&Warned, &Unstated> {
        self.warned.as_ref().map(|warned| &**warned)
    }

    /// The agent, as its instance names it.
    #[must_use]
    pub const fn key(&self) -> &AgentKey {
        &self.key
    }

    /// The content-use route a launch of it is held under.
    #[must_use]
    pub fn route(&self) -> &str {
        &self.route
    }

    /// How it is run, as its entry reviewed it.
    #[must_use]
    pub const fn profile(&self) -> &CatalogProfile {
        &self.profile
    }

    /// Its wire, fresh for a run, as it is spoken in the conversation's
    /// `mode`: its entry's profile, answering yes by itself to what the mode
    /// allows.
    #[must_use]
    pub fn dialect(&self, mode: Mode) -> Box<dyn Dialect> {
        Box::new(Acp::cataloged(self.profile.clone(), allowing(mode)))
    }
}

/// Why an `externalAgents` entry has no row to run as, each saying what to do.
#[derive(Debug, thiserror::Error)]
pub enum Unbelieved {
    /// The registry has no row for its driver, and it names no catalog agent.
    #[error("this release has no launch for its driver")]
    NoLaunch,
    /// Its key spells a word a built-in driver or the native runner is written
    /// with, which is not the same agent whatever the catalog signed.
    #[error(
        "the name {key} is kept for what is built into crucible, so no catalog agent runs \
         under it; run `crucible agents remove {key}` and choose another agent"
    )]
    Reserved {
        /// The key, as the instance names it.
        key: AgentKey,
    },
    /// The trust state that says what the catalog believes cannot be used.
    #[error(
        "the catalog trust state at {} cannot be used ({why}), so nothing says what {key} is; \
         repair or remove that file",
        .file.display()
    )]
    Untrusted {
        /// The key, as the instance names it.
        key: AgentKey,
        /// The file the trust state is kept in.
        file: PathBuf,
        /// Why it cannot be used.
        why: Unusable,
    },
    /// Its program is not where crucible installed a release of the agent.
    #[error(
        "{key} is not run from a release crucible installed; run `crucible agents install {key}`"
    )]
    NotInstalled {
        /// The key, as the instance names it.
        key: AgentKey,
    },
    /// The version its program lies in has no readable record of its install.
    #[error(
        "{key} {version} has no record of being installed; run `crucible agents remove {key}` \
         and then `crucible agents install {key}`"
    )]
    Unreceipted {
        /// The key, as the instance names it.
        key: AgentKey,
        /// The version, as the store names it.
        version: Box<str>,
    },
    /// Its program is not the one its release says is started.
    #[error(
        "{key} {version} is not started by the program configured for it; run \
         `crucible agents install {key}`"
    )]
    Elsewhere {
        /// The key, as the instance names it.
        key: AgentKey,
        /// The version, as the store names it.
        version: Box<str>,
    },
    /// The catalog revoked the version its program lies in.
    #[error(
        "{key} {version} was revoked by the agent catalog{}, so it is not started; run \
         `crucible agents update {key}`",
        .reason.as_deref().map_or_else(String::new, |reason| format!(" ({reason})"))
    )]
    Revoked {
        /// The key, as the instance names it.
        key: AgentKey,
        /// The version revoked.
        version: Box<str>,
        /// Why, quoted, where the catalog says.
        reason: Option<Box<str>>,
    },
    /// The catalog believed offers the agent, but not that version.
    #[error(
        "the agent catalog no longer offers {key} {version}, so it is not started; run \
         `crucible agents update {key}`"
    )]
    Unoffered {
        /// The key, as the instance names it.
        key: AgentKey,
        /// The version no longer offered.
        version: Box<str>,
    },
    /// No catalog believed offers the agent.
    #[error(
        "no agent catalog this run can believe offers {key}, so it is not started; run \
         `crucible agents search` to fetch the catalog"
    )]
    NoCatalog {
        /// The key, as the instance names it.
        key: AgentKey,
    },
    /// Its entry gives no profile to run it under.
    #[error("{key}'s catalog entry cannot be run ({why}); run `crucible agents update {key}`")]
    Unprofiled {
        /// The key, as the instance names it.
        key: AgentKey,
        /// Why not.
        why: Unprofiled,
    },
    /// The ACP Registry lists no entry this build can install under that key.
    #[error("the ACP Registry does not offer {key}: {why}")]
    Unregistered {
        /// The key, as the instance names it.
        key: AgentKey,
        /// Why it is not offered.
        why: Unoffered,
    },
    /// The cached ACP Registry index could not be read.
    #[error("no ACP Registry index can be read, so {key} is not started: {why}")]
    Registry {
        /// The key, as the instance names it.
        key: AgentKey,
        /// Why it could not be read.
        why: RegistryUnread,
    },
    /// The install recorded nothing of what the agent answered when it was
    /// probed.
    #[error("{key} {version} cannot be started: {why}")]
    Unprobed {
        /// The key, as the instance names it.
        key: AgentKey,
        /// The version, quoted.
        version: Box<str>,
        /// Why there is no probe record.
        why: &'static str,
    },
    /// The entry the adapter built from the registry, the survey and the probe
    /// record is not one the catalog reader accepts.
    #[error("{key}'s ACP Registry entry cannot be read: {why}")]
    Unreadable {
        /// The key, as the instance names it.
        key: AgentKey,
        /// Why not.
        why: CatalogError,
    },
    /// The install store could not be read.
    #[error(transparent)]
    Store(#[from] StoreError),
}

/// Whether a catalog key spells a word a built-in driver or the native runner
/// is written with, which no catalog agent runs as.
#[must_use]
pub fn kept_word(key: &str) -> bool {
    key == NATIVE_RUNNER || Driver::ALL.iter().any(|driver| driver.name() == key)
}

/// The verified offer an instance of the catalog agent `key`, started as
/// `executable`, runs under: the one the catalog believed through `trust` pins
/// to the version in `store` the program lies in, and the program that
/// release's receipt and `cmd` say is started there. Reads the store and the
/// trust state it is handed; nothing is fetched.
///
/// # Errors
///
/// [`Unbelieved`], saying what to do.
pub fn believed(
    trust: &Trust,
    store: &Store,
    key: &AgentKey,
    executable: &Path,
) -> Result<Offer, Unbelieved> {
    let agent = || key.clone();
    if kept_word(key.as_str()) {
        return Err(Unbelieved::Reserved { key: agent() });
    }
    if let Some(why) = trust.unusable() {
        return Err(Unbelieved::Untrusted {
            key: agent(),
            file: trust.file(),
            why,
        });
    }
    let version = executable
        .is_absolute()
        .then(|| store.version_of(key, executable))
        .flatten()
        .ok_or_else(|| Unbelieved::NotInstalled { key: agent() })?;
    let (named, revision) = (version.version(), version.lock_revision());
    let shown = || -> Box<str> { crate::launching::quoted(named).into() };
    if trust.revoked(key, named, revision) {
        return Err(Unbelieved::Revoked {
            key: agent(),
            version: shown(),
            reason: trust
                .reason(key, named, revision)
                .map(|reason| crate::launching::quoted(reason).into()),
        });
    }
    let offers = trust.offers(Some(key));
    if offers.is_empty() {
        return Err(Unbelieved::NoCatalog { key: agent() });
    }
    let offer = offers
        .into_iter()
        .find(|offer| *offer.pin.version == *named && offer.pin.lock_revision == revision)
        .ok_or_else(|| Unbelieved::Unoffered {
            key: agent(),
            version: shown(),
        })?;
    if !matches!(store.receipt(key, &version), Ok(Some(_))) {
        return Err(Unbelieved::Unreceipted {
            key: agent(),
            version: shown(),
        });
    }
    // Both followed: a link the release holds, or an instance pointed at it,
    // counts as where it leads.
    let root = store.directory(key, &version);
    let started = std::fs::canonicalize(root.join(&*offer.pin.target.cmd)).ok();
    let inside = std::fs::canonicalize(&root).ok();
    let found = std::fs::canonicalize(executable).ok();
    let the_release_s = started.is_some()
        && started == found
        && found
            .as_deref()
            .zip(inside.as_deref())
            .is_some_and(|(found, inside)| found.starts_with(inside));
    if !the_release_s {
        return Err(Unbelieved::Elsewhere {
            key: agent(),
            version: shown(),
        });
    }
    Ok(offer)
}

/// The row of [`believed`]'s offer, the one a catalog instance runs as.
///
/// # Errors
///
/// As [`believed`], and [`Unbelieved::Unprofiled`] where its entry gives no
/// profile to run it under.
pub fn believed_row(
    trust: &Trust,
    store: &Store,
    key: &AgentKey,
    executable: &Path,
) -> Result<CatalogRow, Unbelieved> {
    let offer = believed(trust, store, key, executable)?;
    CatalogRow::new(&offer.entry, &offer.pin.target).map_err(|why| Unbelieved::Unprofiled {
        key: key.clone(),
        why,
    })
}

/// The verified offer an instance of the ACP Registry agent `key`, started as
/// `executable`, runs under: the registry's entry for this platform, the survey
/// the install recorded in `store`'s receipt, and the `probe` record that
/// install stored. It reads no trust state, no key and no signature, and
/// fetches nothing: the registry is unsigned, and the receipt's own hash and
/// probe record are what say the installed bytes are the ones offered.
///
/// # Errors
///
/// [`Unbelieved`], saying what to do.
pub fn believed_registry(
    index: &RegistryIndex,
    store: &Store,
    key: &AgentKey,
    executable: &Path,
    probe: &Probe,
) -> Result<Offer, Unbelieved> {
    if kept_word(key.as_str()) {
        return Err(Unbelieved::Reserved { key: key.clone() });
    }
    let offered = index.offer(key).map_err(|why| Unbelieved::Unregistered {
        key: key.clone(),
        why,
    })?;
    let version = executable
        .is_absolute()
        .then(|| store.version_of(key, executable))
        .flatten()
        .ok_or_else(|| Unbelieved::NotInstalled { key: key.clone() })?;
    let shown = || -> Box<str> { crate::launching::quoted(version.version()).into() };
    let receipt = store
        .receipt(key, &version)?
        .ok_or_else(|| Unbelieved::Unreceipted {
            key: key.clone(),
            version: shown(),
        })?;
    if let Some(why) = receipt.unprobed(&crucible_external_harnesses::catalog::registry::platform())
    {
        return Err(Unbelieved::Unprobed {
            key: key.clone(),
            version: shown(),
            why,
        });
    }
    if *probe.target != crucible_external_harnesses::catalog::registry::platform() {
        return Err(Unbelieved::Unprobed {
            key: key.clone(),
            version: shown(),
            why: crucible_external_harnesses::store::UNPROBED,
        });
    }
    let entry = offered
        .installed(&receipt, probe)
        .map_err(|why| Unbelieved::Unreadable {
            key: key.clone(),
            why,
        })?;
    let target = entry
        .here()
        .cloned()
        .ok_or_else(|| Unbelieved::Unreadable {
            key: key.clone(),
            why: CatalogError::Field {
                field: "targets".into(),
                why: "names no target for this platform",
            },
        })?;
    // Both followed: a link the release holds, or an instance pointed at it,
    // counts as where it leads.
    let root = store.directory(key, &version);
    let started = std::fs::canonicalize(root.join(&*target.cmd)).ok();
    let inside = std::fs::canonicalize(&root).ok();
    let found = std::fs::canonicalize(executable).ok();
    let the_release_s = started.is_some()
        && started == found
        && found
            .as_deref()
            .zip(inside.as_deref())
            .is_some_and(|(found, inside)| found.starts_with(inside));
    if !the_release_s {
        return Err(Unbelieved::Elsewhere {
            key: key.clone(),
            version: shown(),
        });
    }
    Ok(Offer {
        pin: OwnedPin {
            key: key.clone(),
            version: receipt.version.clone(),
            lock_revision: None,
            admits: entry.admits.clone(),
            target,
        },
        entry,
        serial: 0,
    })
}

/// The row of [`believed_registry`]'s offer, the one a registry instance runs
/// as, carrying the registry's generic content-use warning and the day the
/// index was read.
///
/// # Errors
///
/// As [`believed_registry`], and [`Unbelieved::Unprofiled`] where its entry
/// gives no profile to run it under.
pub fn believed_registry_row(
    index: &RegistryIndex,
    store: &Store,
    key: &AgentKey,
    executable: &Path,
    probe: &Probe,
) -> Result<CatalogRow, Unbelieved> {
    let offer = believed_registry(index, store, key, executable, probe)?;
    CatalogRow::registry(&offer.entry, &offer.pin.target, index.read_date().into()).map_err(|why| {
        Unbelieved::Unprofiled {
            key: key.clone(),
            why,
        }
    })
}

/// An agent's row: one built in, or one made from the catalog entry of the
/// agent an instance names.
// A row is made for one question and dropped with it, never collected; boxing
// the catalog row would put a dereference in every match to save no allocation.
#[allow(clippy::large_enum_variant)]
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
                unusable(row, trust)?;
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

    /// The hosts a sign-in its agent runs as a command may reach: a catalog
    /// agent's entry's, and none for an agent the catalog does not list, which
    /// has no such method.
    #[must_use]
    pub fn sign_in_hosts(&self) -> Vec<&str> {
        match self {
            Self::Built(_) => Vec::new(),
            Self::Catalog(row) => row
                .profile
                .sign_in_hosts()
                .iter()
                .map(SandboxDomainPattern::as_str)
                .collect(),
        }
    }

    /// The methods its agent signs in by, in the order offered: a catalog
    /// agent's entry's, and Antigravity's own four; none for Claude Code,
    /// which signs in at its own command.
    #[must_use]
    pub fn methods(&self) -> Vec<AuthMethod> {
        match self {
            Self::Built(row) => match row.driver {
                Driver::Antigravity => antigravity::methods(),
                Driver::Claude | Driver::Catalog => Vec::new(),
            },
            Self::Catalog(row) => row.profile.methods().to_vec(),
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

    /// Every name its program is looked for under on `PATH`, in the order
    /// tried, as its driver's own module lists them: Antigravity's, and for
    /// every other row the one [`command`](Self::command) gives.
    #[must_use]
    pub fn commands(&self) -> Vec<String> {
        match self {
            Self::Built(row) => match row.driver {
                Driver::Antigravity => antigravity::COMMANDS
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                Driver::Claude | Driver::Catalog => vec![self.command()],
            },
            Self::Catalog(_) => vec![self.command()],
        }
    }

    /// What the user is told of their own program of its agent, one outside
    /// crucible's install store, with the sandbox on as `sandboxed` says:
    /// Antigravity's note on whose sign-in it uses, and nothing for every
    /// other row, whose own program signs in as its release does.
    #[must_use]
    pub fn own_note(&self, sandboxed: bool) -> Option<&'static str> {
        match self {
            Self::Built(row) => match row.driver {
                Driver::Antigravity => Some(antigravity::own_note(sandboxed)),
                Driver::Claude | Driver::Catalog => None,
            },
            Self::Catalog(_) => None,
        }
    }

    /// Its wire for the user's own `program`, one that lies outside
    /// crucible's install store, fresh for a run in `mode`, as
    /// [`dialect`](Self::dialect) makes it for a release: for Antigravity,
    /// the driver's own program, admitted by what it answers, which reads the
    /// trust state only where its file is named as a release's program is and
    /// is started with a release's arguments; every other row's wire is the
    /// same either way.
    ///
    /// # Errors
    ///
    /// As [`dialect`](Self::dialect).
    pub fn own_dialect(&self, mode: Mode, trust: Option<&Trust>, program: &Path) -> Wire {
        match self {
            Self::Built(row) => match row.driver {
                Driver::Antigravity => {
                    let offered = if antigravity::named_as_release(program) {
                        unusable(row, trust)?;
                        antigravity_offered(trust, Some(program))?
                    } else {
                        Vec::new()
                    };
                    Ok(Box::new(Antigravity::own(
                        program,
                        allowing(mode),
                        &offered,
                    )))
                }
                Driver::Claude | Driver::Catalog => self.dialect(mode, trust, Some(program)),
            },
            Self::Catalog(_) => self.dialect(mode, trust, Some(program)),
        }
    }
}

/// Refuses `row`'s releases while `trust`, given, cannot be used, where they
/// are the catalog's.
fn unusable(row: &Row, trust: Option<&Trust>) -> Result<(), Failure> {
    let unusable = trust
        .filter(|_| row.catalogued)
        .and_then(|trust| Some((trust.file(), trust.unusable()?)));
    match unusable {
        Some((file, why)) => Err(Failure::new(&format!(
            "The catalog trust state at {} cannot be used ({why}), so no release of {} is \
             admitted. Repair or remove that file.",
            file.display(),
            row.driver.name()
        ))),
        None => Ok(()),
    }
}

/// The releases of Antigravity `trust` offers that the program at `program`
/// can be, as [`antigravity::started`] says; none given no trust state.
///
/// # Errors
///
/// Where the program is kept in a version's directory that the catalog does
/// not offer.
fn antigravity_offered(
    trust: Option<&Trust>,
    program: Option<&Path>,
) -> Result<Vec<OwnedPin>, Failure> {
    let offered = trust.map_or_else(Vec::new, antigravity::offered);
    if let Some(version) = antigravity::unoffered(&offered, program) {
        let listed: Vec<&str> = offered.iter().map(|pin| &*pin.version).collect();
        return Err(Failure::new(&format!(
            "Version {version} of the agent, where this program is kept, is a release the \
             catalog does not offer; it offers {}.",
            listed.join(", ")
        )));
    }
    Ok(antigravity::started(offered, program))
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

/// The registry this build ships, which lists no catalog row: those are made
/// per question by [`believed_row`], for the agents door and a conversation
/// alike.
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
        let offered = antigravity_offered(trust, program)?;
        Ok(Box::new(Antigravity::allowing(allowing(mode), &offered)))
    },
    profile_key: "profileDir",
    route: "agent:antigravity",
    docs_section: "antigravity",
    // Its sign-in page, the token exchange behind it, and the service it
    // asks, in its released and its daily build; the Gemini API a key signs
    // in to; Agent Platform; the business service's global host; and the
    // account details a Google sign-in reads. A business sign-in's regional
    // host is its entry's location's, which the start adds.
    reaches: &[
        "accounts.google.com",
        "oauth2.googleapis.com",
        "cloudcode-pa.googleapis.com",
        "daily-cloudcode-pa.googleapis.com",
        "generativelanguage.googleapis.com",
        "aiplatform.googleapis.com",
        "businessaicode.googleapis.com",
        "www.googleapis.com",
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
