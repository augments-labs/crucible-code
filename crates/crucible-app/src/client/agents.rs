//! The agents door: listing, installing, updating, rolling back, removing and
//! verifying the agents the reviewed catalog offers, with no conversation.
//!
//! A front end with a conversation and one with none come through the same
//! door, lent a [`Shelf`]: where the catalog is published and the keys it is
//! signed with, crucible's home, from which the user's own configuration file
//! is the one whose `executable` lines are written, and the launcher of a
//! conversation where there is one. Only a client that claimed
//! [`Capability::Install`] is served, and that is checked before anything
//! else; any other is refused as [`ErrorCode::NeedsCapability`] and nothing
//! is read, fetched or changed. A command the door does not serve is refused
//! as [`ErrorCode::Busy`], as the account door refuses one.
//!
//! The names a command carries are only names. The agent is checked as an
//! agent key before it is used, and a version is found among the releases the
//! trust state offers by equality; neither is read as a path or an argument.
//! Every command first fetches the catalog as a person asking for it
//! ([`Trigger::Opening`]), whatever `updates.check` says, and keeps what is
//! believed in the trust state; a catalog that is not believed, or not
//! reached, leaves what was believed, and a listing says it was not refreshed.
//! What is offered is then read from the trust state, never from the network
//! answer itself.
//!
//! An install, update, rollback or removal changes nothing before it is
//! answered. It is put to the client as [`Pending::Install`], whose facts are
//! the release's version and size, the hosts it would be fetched from, its
//! licence, terms and caution, what it is started with, the instances whose
//! `executable` it moves, and that it is not smoke-tested and why; the
//! identity is the process's one count, and the answer is held against the
//! action put. A sentence that would be cut is not put to a client that does
//! not draw the whole value, and a no, a cancel, or nobody, leaves every file
//! as it was. What is checked before that question is put is everything that
//! can refuse the release without a byte fetched: its kind, its file lists,
//! its download hosts, and its size, summed with checked arithmetic, against
//! the most a release may be and the room the install store has.
//!
//! After a yes, the release the question named is the one installed: each
//! artifact is downloaded within its pin by [`crate::agent_download`],
//! unpacked into one staging directory against its file list, and committed
//! to the store with its receipt; only then is every instance whose
//! `executable` lies in the agent's directory in the store written to the new
//! program, in one update of the user file, and the store pruned to what the
//! instances name and one version before. A cancel, or a download or unpack
//! that fails, removes the staging and leaves the version named before. A
//! rollback names the newest other version the store holds that no
//! revocation names, the same program in it, and fetches nothing; a removal
//! is refused while an instance still runs a version from the store.
//!
//! The door does not start the agent: a switch is not smoke-tested here, and
//! the question says so.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crucible_client_api::bounds::ITEMS;
use crucible_client_api::{
    Capability, Choice, Command, Decision, ErrorCode, InstallProgress, Name, Pending, Refusal,
    Request, Text,
};
use crucible_config::{ConfigError, Driver, ExternalAgent, Home, Settings};
use crucible_external_harnesses::catalog::{Files, Kind, Launch, Offer, Trust};
use crucible_external_harnesses::release::{RELEASE_BYTES, UnpackError, Unpacker};
use crucible_external_harnesses::store::{
    Listing, Mismatch, Provenance, Store, StoreError, Version,
};
use crucible_http::Origin;
use crucible_runtime::Cancel;
use crucible_types::AgentKey;

use super::deciding::{Confirmation, Front, Shown, mint, turned_away};
use crate::agent_download::{self, DownloadError};
use crate::catalog_fetch::{self, FetchError, Source, Trigger};
use crate::launching::Launcher;
use crate::remember::{self, RememberError};

/// The most catalog agents a listing names; one with more says so.
pub const LISTED: usize = 32;

/// What the door is lent by the front end that asks it.
#[derive(Debug)]
pub struct Shelf<'a> {
    /// Where the catalog is published, who signed it and which hosts a
    /// redirect from it may reach.
    pub catalog: Source,
    /// Crucible's home: the programs, the trust state and the user's own
    /// configuration file are under it.
    pub home: &'a Home,
    /// The launcher of the conversation asking, where there is one.
    pub smoke: Option<&'a Launcher<'a>>,
}

/// Where an install says how far it has got, and how it is cancelled.
pub struct Watching<'a> {
    /// Told after every piece of a download that arrives.
    pub progress: &'a mut (dyn FnMut(InstallProgress) + Send),
    /// Raised to give the download up, which then commits nothing.
    pub cancel: &'a Cancel,
}

impl std::fmt::Debug for Watching<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Watching").finish_non_exhaustive()
    }
}

/// One catalog agent, as a listing names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The agent.
    pub agent: AgentKey,
    /// The versions offered for this platform, newest source first.
    pub offered: Vec<Box<str>>,
    /// The licence of the newest offered.
    pub license: Box<str>,
    /// Where its terms are published, where it has any.
    pub terms: Option<Box<str>>,
    /// What the reviewer wants a user to know first, where anything.
    pub caution: Option<Box<str>>,
    /// The versions the install store holds with a receipt.
    pub installed: Vec<Box<str>>,
}

/// Which move a confirmed switch was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// A version installed.
    Installed,
    /// The version the catalog now offers, installed.
    Updated,
    /// The version before, named again.
    RolledBack,
}

/// What came of one command at the agents door.
#[derive(Debug)]
pub enum Outcome {
    /// It was turned away before anything was read or changed.
    Refused(Refusal),
    /// The agents the catalog offers.
    Catalog {
        /// Each agent, at most [`LISTED`].
        agents: Vec<Listed>,
        /// Whether there were more than are named.
        more: bool,
        /// Why the catalog was not refreshed first, where it was not: what is
        /// named is what was believed before.
        unrefreshed: Option<FetchError>,
    },
    /// Nobody said yes, so nothing was fetched or changed.
    Undecided,
    /// A version is in place, and the instances named run it.
    Moved {
        /// The agent.
        agent: AgentKey,
        /// Which move it was.
        how: How,
        /// The version now named.
        version: Box<str>,
        /// The instances whose `executable` now names it.
        instances: Vec<Box<str>>,
        /// Whether anything was downloaded for it.
        downloaded: bool,
        /// Why the store was not pruned afterwards, where it was not.
        unpruned: Option<StoreError>,
    },
    /// An update found the instances already on the version offered.
    Current {
        /// The agent.
        agent: AgentKey,
        /// The version.
        version: Box<str>,
    },
    /// Every version of the agent was removed.
    Removed {
        /// The agent.
        agent: AgentKey,
        /// The versions removed.
        versions: Vec<Box<str>>,
    },
    /// The installed versions, each hashed against its receipt.
    Verified {
        /// The agent.
        agent: AgentKey,
        /// Each version and how it differs from its receipt; nothing where it
        /// is as recorded.
        versions: Vec<(Box<str>, Vec<Mismatch>)>,
    },
}

/// Why a command at the agents door did not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum AgentsError {
    /// The user's configuration could not be read.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The catalog offers no such version of the agent for this platform.
    #[error("the catalog offers no release {version} of {agent} for this platform")]
    Unoffered {
        /// The agent.
        agent: AgentKey,
        /// The version asked for, or the newest where none was.
        version: Box<str>,
    },
    /// The catalog offers no release of the agent for this platform.
    #[error("the catalog offers no release of {0} for this platform")]
    Unlisted(AgentKey),
    /// No instance runs a version of the agent from the install store.
    #[error("no instance runs {0} from crucible's install store")]
    NotInstalled(AgentKey),
    /// The store holds no other version to go back to.
    #[error("crucible's install store holds no earlier version of {0} to go back to")]
    NoPrevious(AgentKey),
    /// Instances still run a version from the store.
    #[error("{agent} is still the program of {}, so it is not removed", .instances.join(", "))]
    InUse {
        /// The agent.
        agent: AgentKey,
        /// The instances.
        instances: Vec<Box<str>>,
    },
    /// The release is of a kind this door does not install yet.
    #[error("{0}, which this build does not install through the agents door")]
    Unsupported(&'static str),
    /// The release is larger than a release may be.
    #[error("the release is larger than the {RELEASE_BYTES} bytes a release may be")]
    TooLarge,
    /// The install store refused.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An artifact did not arrive as its pin.
    #[error(transparent)]
    Download(#[from] DownloadError),
    /// An artifact is not what its file list says.
    #[error(transparent)]
    Unpacked(#[from] UnpackError),
    /// The user's configuration could not be written.
    #[error(transparent)]
    Remember(#[from] RememberError),
    /// Signing an agent in is not served here.
    #[error("signing an agent in is not available through the agents door in this build")]
    SignIn,
}

/// Carries out `request` where it is one of the agent catalog's commands,
/// lent `shelf`, asking `front` before anything changes, and telling
/// `watching` how far a download has got.
///
/// # Errors
///
/// [`AgentsError`] naming what stopped a command a client was entitled to.
pub async fn run(
    request: &Request,
    shelf: &Shelf<'_>,
    front: &mut dyn Front,
    watching: Watching<'_>,
) -> Result<Outcome, AgentsError> {
    let command = request.command();
    match command.needs() {
        Some(needed) if request.capabilities().has(needed) => {}
        Some(_) => return Ok(Outcome::Refused(ErrorCode::NeedsCapability.into())),
        None => return Ok(Outcome::Refused(ErrorCode::Busy.into())),
    }
    let agent = match command {
        Command::AgentCatalog => None,
        Command::InstallAgent { agent, .. }
        | Command::UpdateAgent { agent }
        | Command::RollbackAgent { agent }
        | Command::RemoveAgent { agent }
        | Command::VerifyAgent { agent } => match AgentKey::new(agent.as_str()) {
            Ok(key) => Some(key),
            Err(_) => return Ok(Outcome::Refused(ErrorCode::InvalidArgument.into())),
        },
        Command::SignInAgent { .. } => return Err(AgentsError::SignIn),
        _ => return Ok(Outcome::Refused(ErrorCode::Busy.into())),
    };
    let state = catalog_fetch::state_in(shelf.home.path());
    let unrefreshed = catalog_fetch::refresh(&state, &shelf.catalog, Trigger::Opening)
        .await
        .err();
    let door = Door {
        request,
        shelf,
        trust: Trust::load(&state, SystemTime::now()),
        store: Store::new(shelf.home.path().join("programs")),
        user: crucible_config::user(shelf.home),
        instances: Settings::read_home(shelf.home)?.external_agents(),
    };
    let Some(agent) = agent else {
        return door.listing(unrefreshed);
    };
    match command {
        Command::InstallAgent { version, .. } => {
            let offer = door
                .trust
                .offers(Some(&agent))
                .into_iter()
                .find(|offer| *offer.pin.version == *version.as_str())
                .ok_or_else(|| AgentsError::Unoffered {
                    agent: agent.clone(),
                    version: version.as_str().into(),
                })?;
            door.switch(&offer, How::Installed, front, watching).await
        }
        Command::UpdateAgent { .. } => {
            let offer = door
                .trust
                .offers(Some(&agent))
                .into_iter()
                .next()
                .ok_or_else(|| AgentsError::Unlisted(agent.clone()))?;
            let moving = door.moving(&agent);
            if moving.is_empty() {
                return Err(AgentsError::NotInstalled(agent));
            }
            if moving.iter().all(|(_, version)| {
                version.version() == &*offer.pin.version
                    && version.lock_revision() == offer.pin.lock_revision
            }) {
                return Ok(Outcome::Current {
                    agent,
                    version: offer.pin.version,
                });
            }
            door.switch(&offer, How::Updated, front, watching).await
        }
        Command::RollbackAgent { .. } => door.rollback(&agent, front).await,
        Command::RemoveAgent { .. } => door.remove(&agent, front).await,
        Command::VerifyAgent { .. } => door.verify(agent),
        _ => Ok(Outcome::Refused(ErrorCode::Busy.into())),
    }
}

/// What one command is carried out with.
struct Door<'a> {
    request: &'a Request,
    shelf: &'a Shelf<'a>,
    trust: Trust,
    store: Store,
    user: PathBuf,
    instances: Vec<ExternalAgent>,
}

impl Door<'_> {
    /// The agents offered, but those whose key is a word a built-in driver is
    /// spelled with: the entry that backs a built-in is that driver's, and is
    /// no catalog row.
    fn listing(&self, unrefreshed: Option<FetchError>) -> Result<Outcome, AgentsError> {
        let mut agents: Vec<Listed> = Vec::new();
        let mut more = false;
        for offer in self.trust.offers(None) {
            let key = &offer.entry.key;
            if Driver::ALL
                .iter()
                .any(|driver| driver.name() == key.as_str())
            {
                continue;
            }
            if let Some(listed) = agents.iter_mut().find(|listed| listed.agent == *key) {
                listed.offered.push(offer.pin.version);
                continue;
            }
            if agents.len() == LISTED {
                more = true;
                continue;
            }
            let installed = self
                .store
                .installed(key)?
                .into_iter()
                .map(|receipt| receipt.version)
                .collect();
            agents.push(Listed {
                agent: key.clone(),
                offered: vec![offer.pin.version],
                license: offer.entry.license,
                terms: offer.entry.terms,
                caution: offer.entry.caution,
                installed,
            });
        }
        Ok(Outcome::Catalog {
            agents,
            more,
            unrefreshed,
        })
    }

    /// Every instance whose `executable` lies in the store's directory of
    /// `agent`, with the version it lies in.
    fn moving(&self, agent: &AgentKey) -> Vec<(&ExternalAgent, Version)> {
        self.instances
            .iter()
            .filter_map(|instance| {
                let version = self
                    .store
                    .version_of(agent, Path::new(instance.executable()))?;
                Some((instance, version))
            })
            .collect()
    }

    /// Why the switch is not smoke-tested.
    fn unsmoked(&self) -> String {
        match self.shelf.smoke {
            None => "Not smoke-tested: no conversation lent a launcher to start it with.".into(),
            Some(_) => "Not smoke-tested: the new version is not started before it is used.".into(),
        }
    }

    /// Installs `offer` of `agent` after a yes, and moves every instance the
    /// store runs it for onto it.
    async fn switch(
        &self,
        offer: &Offer,
        how: How,
        front: &mut dyn Front,
        watching: Watching<'_>,
    ) -> Result<Outcome, AgentsError> {
        let agent = &offer.entry.key;
        let target = &offer.pin.target;
        if offer.entry.kind != Kind::Binary || target.launch != Launch::Binary {
            return Err(AgentsError::Unsupported(
                "the release is started by a pinned interpreter or Node",
            ));
        }
        if target
            .artifacts
            .iter()
            .any(|artifact| matches!(artifact.files, Files::Asset(_)))
        {
            return Err(AgentsError::Unsupported(
                "the release's file list is kept as a catalog asset",
            ));
        }
        agent_download::named(&target.hosts)?;
        let total = sized(offer)?;
        self.store.fits(target)?;
        let version = Version::new(&offer.pin.version, offer.pin.lock_revision)?;
        let kept = self.store.receipt(agent, &version)?.is_some();
        let moving = self.moving(agent);
        let program = self.store.directory(agent, &version).join(&*target.cmd);

        let confirmation = self.installing(offer, how, kept, &moving);
        if confirmed(self.request, front, &confirmation).await != Some(0) {
            return Ok(Outcome::Undecided);
        }

        if !kept {
            let staging = self.store.stage(agent)?;
            let name = Name::new(agent.as_str()).ok();
            let mut files = Vec::new();
            let mut fetched = Vec::new();
            let mut before: u64 = 0;
            for artifact in &target.artifacts {
                let progress = &mut *watching.progress;
                let mut told = |done: u64, _: u64| {
                    let done = before.saturating_add(done).min(total);
                    if let Some(said) = name
                        .clone()
                        .and_then(|name| InstallProgress::new(name, done, total))
                    {
                        progress(said);
                    }
                };
                let downloaded = agent_download::artifact(
                    &self.shelf.catalog,
                    &target.hosts,
                    artifact,
                    &mut told,
                    watching.cancel,
                )
                .await?;
                before = before.saturating_add(downloaded.fetched.size);
                let staged = Unpacker::unpack(&downloaded.bytes, artifact, staging.directory())?;
                files.extend_from_slice(staged.files());
                fetched.push(downloaded.fetched);
            }
            let provenance = Provenance {
                listing: Listing {
                    serial: offer.serial,
                    review: offer.entry.review.clone(),
                    terms: offer.entry.terms.clone(),
                },
                fetched: SystemTime::now(),
                artifacts: fetched,
            };
            self.store.commit(staging, &version, &files, provenance)?;
        }
        let instances = self.point(&moving, &program)?;
        Ok(Outcome::Moved {
            agent: agent.clone(),
            how,
            version: offer.pin.version.clone(),
            instances,
            downloaded: !kept,
            unpruned: self.prune(agent, &moving, &program).err(),
        })
    }

    /// The question an install or update puts.
    fn installing(
        &self,
        offer: &Offer,
        how: How,
        kept: bool,
        moving: &[(&ExternalAgent, Version)],
    ) -> Confirmation {
        let target = &offer.pin.target;
        let entry = &offer.entry;
        // Summed before the question is put, which a size past the bound
        // never reaches.
        let total = sized(offer).unwrap_or(0);
        let (asks, choice) = match how {
            How::Updated => (
                format!("Update {} to {}?", entry.key, offer.pin.version),
                "Update",
            ),
            How::Installed | How::RolledBack => (
                format!("Install {} {}?", entry.key, offer.pin.version),
                "Install",
            ),
        };
        let mut facts = vec![if kept {
            format!(
                "{} {} is already installed, so nothing is downloaded.",
                entry.key, offer.pin.version
            )
        } else {
            format!(
                "{} {}, {total} bytes to download.",
                entry.key, offer.pin.version
            )
        }];
        let mut from: Vec<String> = Vec::new();
        let mut mirrors: Vec<String> = Vec::new();
        for artifact in &target.artifacts {
            if artifact.replaced_by.is_some() {
                if let Some(origin) = Origin::of(&self.shelf.catalog.base) {
                    from.push(origin.host().to_owned());
                }
                continue;
            }
            from.extend(target.hosts.iter().map(ToString::to_string));
            if let Some(origin) = artifact.mirror.as_deref().and_then(Origin::of) {
                mirrors.push(origin.host().to_owned());
            }
        }
        dedup(&mut from);
        dedup(&mut mirrors);
        if !kept {
            facts.push(if mirrors.is_empty() {
                format!("Fetched from {}.", from.join(", "))
            } else {
                format!(
                    "Fetched from {}, or from the catalog's copy at {} if that fails.",
                    from.join(", "),
                    mirrors.join(", ")
                )
            });
        }
        facts.push(match &entry.terms {
            Some(terms) => format!("Licence {}; terms at {terms}.", entry.license),
            None => format!("Licence {}.", entry.license),
        });
        if let Some(caution) = &entry.caution {
            facts.push(format!("Caution: {caution}"));
        }
        let mut started = vec![target.cmd.to_string()];
        started.extend(target.args.iter().map(ToString::to_string));
        facts.push(format!("Started as: {}", started.join(" ")));
        facts.push(if target.env.is_empty() {
            "Started with no environment of its own.".to_owned()
        } else {
            let set: Vec<String> = target
                .env
                .iter()
                .map(|variable| format!("{}={}", variable.name, variable.value))
                .collect();
            format!("Started with the environment {}.", set.join(", "))
        });
        facts.push(moves(&entry.key, &offer.pin.version, moving));
        facts.push(self.unsmoked());
        Confirmation {
            asks,
            facts,
            choices: vec![
                (
                    choice.to_owned(),
                    format!(
                        "Put {} in place and move the instances named onto it",
                        offer.pin.version
                    ),
                ),
                ("Cancel".to_owned(), "Leave everything as it is".to_owned()),
            ],
        }
    }

    /// Writes each of `moving` onto `program`, in one update of the user
    /// file, and gives the instances written.
    fn point(
        &self,
        moving: &[(&ExternalAgent, Version)],
        program: &Path,
    ) -> Result<Vec<Box<str>>, AgentsError> {
        if moving.is_empty() {
            return Ok(Vec::new());
        }
        let program = program.to_string_lossy();
        let pairs: Vec<(&str, &str)> = moving
            .iter()
            .map(|(instance, _)| (instance.name(), &*program))
            .collect();
        remember::pointing(&self.user, &pairs)?;
        Ok(moving
            .iter()
            .map(|(instance, _)| instance.name().into())
            .collect())
    }

    /// Prunes the store of `agent` to what the instances name now: those in
    /// `moving` naming `program`, and every other as it was.
    fn prune(
        &self,
        agent: &AgentKey,
        moving: &[(&ExternalAgent, Version)],
        program: &Path,
    ) -> Result<(), StoreError> {
        let mut named: Vec<&Path> = vec![program];
        for instance in &self.instances {
            if !moving
                .iter()
                .any(|(moved, _)| moved.name() == instance.name())
            {
                named.push(Path::new(instance.executable()));
            }
        }
        self.store.prune(agent, &named).map(|_| ())
    }

    /// Names the version before again, after a yes.
    async fn rollback(
        &self,
        agent: &AgentKey,
        front: &mut dyn Front,
    ) -> Result<Outcome, AgentsError> {
        let moving = self.moving(agent);
        let Some((first, current)) = moving.first() else {
            return Err(AgentsError::NotInstalled(agent.clone()));
        };
        let within = Path::new(first.executable())
            .strip_prefix(self.store.directory(agent, current))
            .map(Path::to_path_buf)
            .ok();
        let previous = self
            .store
            .installed(agent)?
            .into_iter()
            .filter(|receipt| {
                !moving.iter().any(|(_, named)| {
                    named.version() == &*receipt.version
                        && named.lock_revision() == receipt.lock_revision
                })
            })
            .filter(|receipt| {
                !self
                    .trust
                    .revoked(agent, &receipt.version, receipt.lock_revision)
            })
            .max_by_key(|receipt| receipt.fetched)
            .ok_or_else(|| AgentsError::NoPrevious(agent.clone()))?;
        let version = Version::new(&previous.version, previous.lock_revision)?;
        let program = within
            .filter(|within| {
                let spelled = within
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                previous.files.iter().any(|file| {
                    *file.path == *spelled
                        && matches!(
                            file.kind,
                            crucible_external_harnesses::catalog::FileKind::File { exec: true, .. }
                        )
                })
            })
            .map(|within| self.store.directory(agent, &version).join(within))
            .ok_or_else(|| AgentsError::NoPrevious(agent.clone()))?;
        let mut facts = vec![format!(
            "{agent} {} is installed, so nothing is downloaded.",
            previous.version
        )];
        facts.push(match &previous.listing.terms {
            Some(terms) => format!("Its terms, accepted when it was installed: {terms}."),
            None => "It was installed with no terms to accept.".to_owned(),
        });
        facts.push(moves(agent, &previous.version, &moving));
        facts.push(self.unsmoked());
        let confirmation = Confirmation {
            asks: format!("Roll {agent} back to {}?", previous.version),
            facts,
            choices: vec![
                (
                    "Roll back".to_owned(),
                    format!("Move the instances named back onto {}", previous.version),
                ),
                ("Cancel".to_owned(), "Leave everything as it is".to_owned()),
            ],
        };
        if confirmed(self.request, front, &confirmation).await != Some(0) {
            return Ok(Outcome::Undecided);
        }
        let instances = self.point(&moving, &program)?;
        Ok(Outcome::Moved {
            agent: agent.clone(),
            how: How::RolledBack,
            version: previous.version,
            instances,
            downloaded: false,
            unpruned: self.prune(agent, &moving, &program).err(),
        })
    }

    /// Removes every version of `agent`, after a yes, where no instance runs
    /// one.
    async fn remove(
        &self,
        agent: &AgentKey,
        front: &mut dyn Front,
    ) -> Result<Outcome, AgentsError> {
        let moving = self.moving(agent);
        if !moving.is_empty() {
            return Err(AgentsError::InUse {
                agent: agent.clone(),
                instances: moving
                    .iter()
                    .map(|(instance, _)| instance.name().into())
                    .collect(),
            });
        }
        let installed: Vec<Box<str>> = self
            .store
            .installed(agent)?
            .into_iter()
            .map(|receipt| receipt.version)
            .collect();
        if installed.is_empty() {
            return Err(AgentsError::NotInstalled(agent.clone()));
        }
        let confirmation = Confirmation {
            asks: format!("Remove {agent} from this machine?"),
            facts: vec![
                format!("Removes {agent} {}.", installed.join(", ")),
                "No configured instance runs it from crucible's install store.".to_owned(),
            ],
            choices: vec![
                ("Remove".to_owned(), "Remove every version".to_owned()),
                ("Cancel".to_owned(), "Leave everything as it is".to_owned()),
            ],
        };
        if confirmed(self.request, front, &confirmation).await != Some(0) {
            return Ok(Outcome::Undecided);
        }
        let executables: Vec<&Path> = self
            .instances
            .iter()
            .map(|instance| Path::new(instance.executable()))
            .collect();
        let removed = self.store.remove(agent, &executables)?;
        Ok(Outcome::Removed {
            agent: agent.clone(),
            versions: removed
                .iter()
                .map(|version| version.version().into())
                .collect(),
        })
    }

    /// Hashes every installed version of `agent` against its receipt.
    fn verify(&self, agent: AgentKey) -> Result<Outcome, AgentsError> {
        let installed = self.store.installed(&agent)?;
        if installed.is_empty() {
            return Err(AgentsError::NotInstalled(agent));
        }
        let mut versions = Vec::new();
        for receipt in installed {
            let version = Version::new(&receipt.version, receipt.lock_revision)?;
            versions.push((receipt.version, self.store.verify(&agent, &version)?));
        }
        Ok(Outcome::Verified { agent, versions })
    }
}

/// What downloading `offer` takes, in bytes, refused where it or what it
/// unpacks to is more than a release may be.
fn sized(offer: &Offer) -> Result<u64, AgentsError> {
    let mut downloaded: u64 = 0;
    let mut unpacked: u64 = 0;
    for artifact in &offer.pin.target.artifacts {
        let size = artifact
            .replaced_by
            .as_ref()
            .map_or(artifact.size, |asset| asset.size);
        downloaded = downloaded
            .checked_add(size)
            .filter(|bytes| *bytes <= RELEASE_BYTES)
            .ok_or(AgentsError::TooLarge)?;
        unpacked = unpacked
            .checked_add(artifact.unpacked)
            .filter(|bytes| *bytes <= RELEASE_BYTES)
            .ok_or(AgentsError::TooLarge)?;
    }
    Ok(downloaded)
}

/// The sentence saying which instances move onto `version` of `agent`.
fn moves(agent: &AgentKey, version: &str, moving: &[(&ExternalAgent, Version)]) -> String {
    if moving.is_empty() {
        return format!(
            "Moves no configured instance: none runs {agent} from crucible's install store."
        );
    }
    let named: Vec<String> = moving
        .iter()
        .map(|(instance, from)| format!("{} from {}", instance.name(), from.version()))
        .collect();
    format!("Moves {} onto {version}.", named.join(", "))
}

/// `names` with each kept once, in the order first given.
fn dedup(names: &mut Vec<String>) {
    let mut seen: Vec<String> = Vec::new();
    names.retain(|name| {
        if seen.contains(name) {
            false
        } else {
            seen.push(name.clone());
            true
        }
    });
}

/// Puts `confirmation` through `front` as an install confirmation, and
/// answers with the place in its choices of the one picked, or `None` where
/// nobody picked one: it declined, answered nothing, or was turned away
/// [`TRIES`](super::TRIES) times.
///
/// Put whole or not at all: a sentence about what would be fetched, cut short,
/// is not what the person agreed to.
async fn confirmed(
    request: &Request,
    front: &mut dyn Front,
    confirmation: &Confirmation,
) -> Option<usize> {
    if !request.capabilities().has(Capability::Install) || confirmation.choices.len() > ITEMS {
        return None;
    }
    let asks = Text::cut(&confirmation.asks);
    let facts: Vec<Text> = confirmation
        .facts
        .iter()
        .map(|fact| Text::cut(fact))
        .collect();
    let choices: Vec<Choice> = confirmation
        .choices
        .iter()
        .enumerate()
        .map(|(at, (name, says))| Choice {
            name: Text::cut(name),
            says: Text::cut(says),
            recommended: at == 0,
        })
        .collect();
    let cut = asks.truncated()
        || facts.iter().any(Text::truncated)
        || choices
            .iter()
            .any(|choice| choice.name.truncated() || choice.says.truncated());
    if facts.len() > ITEMS || (cut && !front.draws_whole()) {
        return None;
    }
    let pending = Pending::Install {
        id: mint(),
        asks,
        facts,
        choices,
    };

    let mut tries = 0;
    loop {
        let decision = front.put(&pending, Shown::Install(confirmation)).await?;
        let refusal: Refusal = if decision.id() == pending.id() {
            match decision {
                Decision::Picked { choice, .. } => {
                    match confirmation
                        .choices
                        .iter()
                        .position(|(name, _)| name == choice.as_str())
                    {
                        Some(at) => return Some(at),
                        None => ErrorCode::InvalidArgument.into(),
                    }
                }
                Decision::Declined { .. } => return None,
                Decision::Ruled { .. } | Decision::Answered { .. } | Decision::Accepted { .. } => {
                    ErrorCode::WrongDecision.into()
                }
            }
        } else {
            ErrorCode::StaleDecision.into()
        };
        if turned_away(front, refusal, &mut tries) {
            return None;
        }
    }
}
