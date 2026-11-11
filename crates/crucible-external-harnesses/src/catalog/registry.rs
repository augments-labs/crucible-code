//! The official ACP Registry, read into the offers crucible is willing to
//! install and run.
//!
//! The registry is unsigned: it is fetched over `https` from one address, and
//! a hash pins what crucible installs; an entry's text may also propose a
//! program on the user's `PATH` and give its arguments and environment, which
//! run only after the user picks that path and only under the probe's
//! admission. That proposal is read by [`OwnEntry`](super::OwnEntry), which
//! asks for no hash. An entry is offered only
//! for the platform this build runs on and only where that platform's target
//! carries a SHA-256 and an archive the hardened unpacker reads. One whose
//! target here has no hash, or an archive of another format, is never offered
//! or installed: it is listed with why, so that it can be shown only as the
//! user's own program where its command is found on `PATH`. An entry with no
//! target here is absent and says why. There is no review, revocation, serial
//! or key, so nothing here consults [`Trust`](super::Trust) or [`KEYS`](super::KEYS).
//!
//! The text is JSON: a `version` whose major is [`SCHEMA`], and `agents`, each
//! an `id`, a `name`, a `version`, a `license` and a `distribution` whose
//! `binary` maps a platform to an `archive`, a `cmd` and a SHA-256, with the
//! `args` and `env` the agent is started with. A key this reader does not know
//! is passed over. The index is bounded before it is parsed and holds at most
//! [`AGENTS`] agents.
//!
//! An offer becomes a [`Entry`] the same way a catalog entry does: the adapter
//! writes the fields the registry gives, the install-time survey and the stored
//! probe record, and reads them back through [`Entry::unsigned`], so the
//! loader-variable, relative-`cmd`, host and path rules apply. Neither [`Entry`]
//! nor [`Target`](super::Target) is built directly here.

use std::time::{SystemTime, UNIX_EPOCH};

use crucible_types::AgentKey;
use serde_json::{Map, Value, json};

use super::{CatalogError, Entry, Format, LISTED_FILES, Offer};
use crate::release::{ARCHIVE_BYTES, OwnedPin, RELEASE_BYTES, UNPACKED_BYTES};
use crate::store::{Differs, Mismatch, Probe, ProbedKind, ProbedMethod, Receipt, Source};

/// Where the official registry is published.
pub const URL: &str = "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json";

/// The hosts a fetch of the registry, or of one of its archives, may be
/// redirected through.
pub const HOSTS: &[&str] = &[
    "github.com",
    "objects.githubusercontent.com",
    "release-assets.githubusercontent.com",
    "github-releases.githubusercontent.com",
];

/// The registry schema's major version, which this reader knows.
pub const SCHEMA: u64 = 1;

/// The most agents one index may hold.
pub const AGENTS: usize = 512;

/// The registry index this build read, and when it read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryIndex {
    agents: Vec<RegistryEntry>,
    read: SystemTime,
}

impl RegistryIndex {
    /// The index `bytes` are, read from the registry at `read`.
    ///
    /// # Errors
    ///
    /// [`RegistryError`] naming why the bytes are not a registry index this
    /// reader knows.
    pub fn parse(bytes: &[u8], read: SystemTime) -> Result<Self, RegistryError> {
        if bytes.len() > super::INDEX_BYTES {
            return Err(RegistryError::Size(bytes.len()));
        }
        let value: Value = serde_json::from_slice(bytes)?;
        let object = value.as_object().ok_or(RegistryError::Field {
            field: "the registry".into(),
            why: "is not an object",
        })?;
        let version = text(object, "version", "")?;
        let major = version
            .split('.')
            .next()
            .and_then(|part| part.parse::<u64>().ok());
        if major != Some(SCHEMA) {
            return Err(RegistryError::Field {
                field: "version".into(),
                why: "is a registry schema this crucible does not know",
            });
        }
        let listed =
            object
                .get("agents")
                .and_then(Value::as_array)
                .ok_or(RegistryError::Field {
                    field: "agents".into(),
                    why: "is not a list",
                })?;
        if listed.len() > AGENTS {
            return Err(RegistryError::Agents(listed.len()));
        }
        let agents = listed
            .iter()
            .enumerate()
            .map(|(index, value)| RegistryEntry::read(value, &format!("agents[{index}]")))
            .collect::<Result<_, _>>()?;
        Ok(Self { agents, read })
    }

    /// When the index was read from the registry.
    #[must_use]
    pub const fn read(&self) -> SystemTime {
        self.read
    }

    /// The day it was read, as `YYYY-MM-DD`.
    #[must_use]
    pub fn read_date(&self) -> String {
        date(self.read)
    }

    /// Every agent it lists, in the order the registry gives them.
    #[must_use]
    pub fn agents(&self) -> &[RegistryEntry] {
        &self.agents
    }

    /// The agent the registry names `id`.
    #[must_use]
    pub fn entry(&self, id: &AgentKey) -> Option<&RegistryEntry> {
        self.agents.iter().find(|entry| entry.id == *id)
    }

    /// Every entry it offers for the platform this build runs on, in registry
    /// order.
    #[must_use]
    pub fn offers(&self) -> Vec<RegistryOffer<'_>> {
        self.agents
            .iter()
            .filter_map(|entry| entry.offered(&platform()).ok())
            .collect()
    }

    /// Every entry with a target for the platform this build runs on, in
    /// registry order, each with its offer or why it is not offered; an entry
    /// with no target here is absent.
    #[must_use]
    pub fn listed(&self) -> Vec<(&RegistryEntry, Result<RegistryOffer<'_>, Unoffered>)> {
        let platform = platform();
        self.agents
            .iter()
            .filter(|entry| entry.target(&platform).is_some())
            .map(|entry| (entry, entry.offered(&platform)))
            .collect()
    }

    /// The offer for the agent `id` on the platform this build runs on, or why
    /// it is not offered.
    ///
    /// # Errors
    ///
    /// [`Unoffered`] naming the platform, the hash or the archive.
    pub fn offer(&self, id: &AgentKey) -> Result<RegistryOffer<'_>, Unoffered> {
        self.entry(id)
            .ok_or_else(|| Unoffered::Unknown { id: id.clone() })?
            .offered(&platform())
    }
}

/// The platform this build runs on, as the registry spells it.
#[must_use]
pub fn platform() -> String {
    super::trust::here()
}

/// Every way `receipt` differs from what this build recorded of the release it
/// describes: a release from the ACP Registry carries the registry source with
/// no catalog serial, and a probe record taken for `target`.
///
/// A receipt whose source and review address disagree, or whose registry
/// serial is not zero, is named `source`; a missing or other-platform probe
/// record is named `probe`. A receipt from the signed catalog whose review is
/// its pull request is named nothing.
#[must_use]
pub fn receipt_mismatches(receipt: &Receipt, target: &str) -> Vec<Mismatch> {
    let registry = receipt.source == Source::Registry;
    let reviewed = &*receipt.listing.review == URL;
    let mut found = Vec::new();
    if registry != reviewed || (registry && receipt.listing.serial != 0) {
        found.push(Mismatch {
            path: "source".into(),
            what: Differs::Record,
        });
    }
    if receipt.unprobed(target).is_some() {
        found.push(Mismatch {
            path: "probe".into(),
            what: Differs::Record,
        });
    }
    found
}

/// One agent the registry lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryEntry {
    /// Its id, which is the key crucible knows it by.
    pub id: AgentKey,
    /// The name its vendor gives it.
    pub name: Box<str>,
    /// The version it publishes.
    pub version: Box<str>,
    /// The licence it is published under.
    pub license: Box<str>,
    /// Where the terms are published, where the registry says.
    pub terms: Option<Box<str>>,
    /// Its binary targets, by platform.
    targets: Vec<RegistryTarget>,
}

impl RegistryEntry {
    fn read(value: &Value, at: &str) -> Result<Self, RegistryError> {
        let object = value.as_object().ok_or_else(|| RegistryError::Field {
            field: at.into(),
            why: "is not an object",
        })?;
        let id = text(object, "id", at)?;
        let id = AgentKey::new(&id).map_err(|_| RegistryError::Field {
            field: field_at(at, "id"),
            why: "is not an agent key",
        })?;
        let name = text(object, "name", at)?;
        let version = text(object, "version", at)?;
        if version.len() > super::VERSION_BYTES
            || matches!(&*version, "." | "..")
            || version
                .chars()
                .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
        {
            return Err(RegistryError::Field {
                field: field_at(at, "version"),
                why: "is not a version that can name one directory",
            });
        }
        let license = text(object, "license", at)?;
        let terms = match optional_https(object, "license_url", at)? {
            Some(url) => Some(url),
            None => match optional_https(object, "website", at)? {
                Some(url) => Some(url),
                None => optional_https(object, "repository", at)?,
            },
        };
        let distribution = match object.get("distribution").filter(|value| !value.is_null()) {
            None => None,
            Some(Value::Object(object)) => Some(object),
            Some(_) => {
                return Err(RegistryError::Field {
                    field: field_at(at, "distribution"),
                    why: "is not an object",
                });
            }
        };
        let binary = match distribution.and_then(|distribution| distribution.get("binary")) {
            None => None,
            Some(Value::Object(binary)) => Some(binary),
            Some(_) => {
                return Err(RegistryError::Field {
                    field: field_at(at, "distribution.binary"),
                    why: "is not an object",
                });
            }
        };
        let mut targets = Vec::new();
        if let Some(binary) = binary {
            for (rail, value) in binary {
                let at = field_at(at, &format!("distribution.binary.{rail}"));
                targets.push(RegistryTarget::read(rail, value, &at)?);
            }
        }
        Ok(Self {
            id,
            name,
            version,
            license,
            terms,
            targets,
        })
    }

    /// The binary target it publishes for `platform`, whether or not it is one
    /// crucible would install.
    #[must_use]
    pub fn target(&self, platform: &str) -> Option<&RegistryTarget> {
        self.targets
            .iter()
            .find(|target| *target.platform == *platform)
    }

    /// The target it offers for `platform`, or why it offers none.
    ///
    /// # Errors
    ///
    /// [`Unoffered`] naming the platform, the missing hash or the archive this
    /// crucible cannot unpack.
    pub fn offered(&self, platform: &str) -> Result<RegistryOffer<'_>, Unoffered> {
        let target = self
            .targets
            .iter()
            .find(|target| *target.platform == *platform)
            .ok_or_else(|| Unoffered::Platform {
                id: self.id.clone(),
                platform: platform.into(),
            })?;
        if !sha256(&target.sha256) {
            return Err(Unoffered::Unhashed {
                id: self.id.clone(),
                platform: platform.into(),
            });
        }
        if target.format.is_none() {
            return Err(Unoffered::Archive {
                id: self.id.clone(),
                platform: platform.into(),
                archive: target.archive.clone(),
            });
        }
        Ok(RegistryOffer {
            entry: self,
            target,
            platform: platform.into(),
        })
    }
}

/// One environment variable and its value, as a target sets them.
pub type Env = Vec<(Box<str>, Box<str>)>;

/// One binary target the registry publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistryTarget {
    /// The platform, as `<os>-<arch>`.
    pub platform: Box<str>,
    /// Where the archive is published.
    pub archive: Box<str>,
    /// Its SHA-256, or empty where the registry gives none.
    pub sha256: Box<str>,
    /// The program, as the registry spells it.
    pub cmd: Box<str>,
    /// The agent's own arguments.
    pub args: Vec<Box<str>>,
    /// The agent's environment.
    pub env: Env,
    /// The archive's format, where this crucible unpacks one of that kind.
    format: Option<Format>,
}

impl RegistryTarget {
    fn read(platform: &str, value: &Value, at: &str) -> Result<Self, RegistryError> {
        let object = value.as_object().ok_or_else(|| RegistryError::Field {
            field: at.into(),
            why: "is not an object",
        })?;
        let archive = text(object, "archive", at)?;
        if !archive.starts_with("https://") {
            return Err(RegistryError::Field {
                field: field_at(at, "archive"),
                why: "is not an https address",
            });
        }
        if host_of(&archive).is_none() {
            return Err(RegistryError::Field {
                field: field_at(at, "archive"),
                why: "is not an address naming a host",
            });
        }
        let cmd = text(object, "cmd", at)?;
        let sha256 = optional_text(object, "sha256", at)?.unwrap_or_default();
        let args = texts(object, "args", at)?;
        let format = RegistryTarget::format_of(&archive);
        Ok(Self {
            platform: platform.into(),
            archive,
            sha256,
            cmd,
            args,
            env: variables(object, "env", at)?,
            format,
        })
    }

    /// The hosts a download of this target may reach: the archive's own host,
    /// which is the first hop, and the redirect hosts. Empty where the archive
    /// is not an address naming a host, which the reader refuses.
    #[must_use]
    pub fn hosts(&self) -> Vec<Box<str>> {
        let mut hosts: Vec<Box<str>> = host_of(&self.archive)
            .map(|host| vec![host.into()])
            .unwrap_or_default();
        for host in HOSTS {
            if !hosts.iter().any(|have| &**have == *host) {
                hosts.push((*host).into());
            }
        }
        hosts
    }

    /// The archive's format, where this crucible unpacks one of that kind.
    #[must_use]
    pub const fn format(&self) -> Option<Format> {
        self.format
    }

    /// The program as the catalog spells it: `./` taken off the front and `\`
    /// read as `/`, as a Windows path is written.
    #[must_use]
    pub fn program(&self) -> String {
        self.cmd
            .strip_prefix("./")
            .unwrap_or(&self.cmd)
            .replace('\\', "/")
    }

    /// The format the archive at `archive` is, where this crucible unpacks one
    /// of that kind.
    #[must_use]
    pub fn format_of(archive: &str) -> Option<Format> {
        let name = archive
            .split(['?', '#'])
            .next()
            .unwrap_or(archive)
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let extension = name.rsplit_once('.').map_or("", |(_, extension)| extension);
        if extension == "zip" {
            Some(Format::Zip)
        } else if extension == "tgz" || (extension == "gz" && name.ends_with("tar.gz")) {
            Some(Format::TarGz)
        } else if extension == "tbz2" || (extension == "bz2" && name.ends_with("tar.bz2")) {
            Some(Format::TarBz2)
        } else if extension == "exe" || !name.contains('.') {
            Some(Format::Bare)
        } else {
            None
        }
    }
}

/// An agent and the target this build is offered, with the platform it was
/// offered for.
#[derive(Debug, Clone)]
pub struct RegistryOffer<'a> {
    entry: &'a RegistryEntry,
    target: &'a RegistryTarget,
    platform: Box<str>,
}

impl<'a> RegistryOffer<'a> {
    /// The agent.
    #[must_use]
    pub const fn entry(&self) -> &'a RegistryEntry {
        self.entry
    }

    /// The target offered.
    #[must_use]
    pub const fn target(&self) -> &'a RegistryTarget {
        self.target
    }

    /// The platform it is offered for.
    #[must_use]
    pub fn platform(&self) -> &str {
        &self.platform
    }

    /// The version it publishes.
    #[must_use]
    pub fn version(&self) -> &'a str {
        &self.entry.version
    }

    /// The offer an install starts from: the registry's program, arguments and
    /// environment, and its archive pinned by SHA-256 and reserved at the most
    /// one may be, with no surveyed file list and no probe record yet.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] naming the first field the catalog reader refuses.
    pub fn offer(&self) -> Result<Offer, CatalogError> {
        let entry = self.provisional()?;
        let target = entry.here().cloned().ok_or_else(|| CatalogError::Field {
            field: "targets".into(),
            why: "names no target for this platform",
        })?;
        Ok(Offer {
            pin: OwnedPin {
                key: entry.key.clone(),
                version: entry.version.clone(),
                lock_revision: None,
                admits: entry.admits.clone(),
                target,
            },
            entry,
            serial: 0,
        })
    }

    /// The entry an install starts from, before anything is downloaded or
    /// probed.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] naming the first field the catalog reader refuses.
    pub fn provisional(&self) -> Result<Entry, CatalogError> {
        self.read(&Artifact::reserved(self.target), None, None)
    }

    /// The entry an installed release is rebuilt as, from the survey the
    /// receipt recorded and the probe record the install stored. The archive
    /// and hash are the receipt's, not the registry's, so a same-version
    /// registry change does not refuse bytes already installed.
    ///
    /// # Errors
    ///
    /// [`CatalogError`] naming the first field the catalog reader refuses.
    pub fn installed(&self, receipt: &Receipt, probe: &Probe) -> Result<Entry, CatalogError> {
        if receipt.files.len() > LISTED_FILES {
            return Err(CatalogError::Field {
                field: "files".into(),
                why: "lists more files than a catalog row carries",
            });
        }
        self.read(
            &Artifact::surveyed(self.target, receipt),
            Some(receipt),
            Some(probe),
        )
    }

    fn read(
        &self,
        artifact: &Artifact,
        receipt: Option<&Receipt>,
        probe: Option<&Probe>,
    ) -> Result<Entry, CatalogError> {
        let (version, admits, methods) = match (receipt, probe) {
            (Some(receipt), Some(probe)) => (
                receipt.version.to_string(),
                json!({
                    "name": &*probe.admits.name,
                    "version": &*probe.admits.version,
                }),
                methods(probe, self.target),
            ),
            _ => (
                self.entry.version.to_string(),
                json!({
                    "name": &*self.entry.name,
                    "version": &*self.entry.version,
                }),
                Vec::new(),
            ),
        };
        let document = json!({
            "key": self.entry.id.as_str(),
            "registry_id": self.entry.id.as_str(),
            "version": version,
            "kind": "binary",
            "license": &*self.entry.license,
            "terms": self.entry.terms.as_deref(),
            "review": URL,
            "admits": admits,
            "auth_methods": methods,
            "targets": [{
                "platform": &*self.platform,
                "cmd": self.target.program(),
                "args": self.target.args.iter().map(|arg| &**arg).collect::<Vec<_>>(),
                "env": self.target.env.iter().map(|(name, value)| (&**name, &**value)).collect::<std::collections::BTreeMap<_, _>>(),
                "hosts": self.target.hosts().iter().map(|host| &**host).collect::<Vec<_>>(),
                "artifacts": [artifact.json()],
            }],
        });
        let bytes = serde_json::to_vec(&document)?;
        Entry::unsigned(&bytes)
    }
}

/// One artifact of a registry entry, as the entry says it.
struct Artifact {
    url: Box<str>,
    sha256: Box<str>,
    size: u64,
    format: Format,
    unpacked: u64,
    decompressed: u64,
    files: Vec<Value>,
}

impl Artifact {
    /// The archive as an install starts with it: held to its SHA-256 and the
    /// most one may be, and surveyed into no list yet.
    fn reserved(target: &RegistryTarget) -> Self {
        Self {
            url: target.archive.clone(),
            sha256: target.sha256.clone(),
            size: ARCHIVE_BYTES,
            format: target.format.unwrap_or(Format::Bare),
            unpacked: UNPACKED_BYTES,
            decompressed: RELEASE_BYTES,
            files: Vec::new(),
        }
    }

    /// The archive as the install recorded it: the address, size and hash the
    /// receipt holds, and the files and ceilings its survey found.
    fn surveyed(target: &RegistryTarget, receipt: &Receipt) -> Self {
        let fetched = receipt.artifacts.first();
        let url = fetched.map_or_else(|| target.archive.clone(), |one| one.url.clone());
        let format = RegistryTarget::format_of(&url)
            .or(target.format)
            .unwrap_or(Format::Bare);
        Self {
            sha256: fetched.map_or_else(|| target.sha256.clone(), |one| one.sha256.clone()),
            size: fetched.map_or(ARCHIVE_BYTES, |one| one.size),
            url,
            format,
            unpacked: receipt.unpacked,
            decompressed: receipt.decompressed,
            files: receipt.files.iter().map(super::File::json).collect(),
        }
    }

    fn json(&self) -> Value {
        json!({
            "url": &*self.url,
            "sha256": &*self.sha256,
            "size": self.size,
            "format": match self.format {
                Format::Zip => "zip",
                Format::TarGz => "tar.gz",
                Format::TarBz2 => "tar.bz2",
                Format::Bare => "bare",
                Format::Wheel => "wheel",
            },
            "unpacked": self.unpacked,
            "decompressed": self.decompressed,
            "files": self.files,
        })
    }
}

/// The sign-in methods the probe recorded, as the entry spells them. A method
/// whose variable the registry does not name, or that this crucible does not
/// carry out, is left out rather than guessed.
fn methods(probe: &Probe, target: &RegistryTarget) -> Vec<Value> {
    probe
        .auth_methods
        .iter()
        .filter_map(|method| method_json(method, target))
        .collect()
}

fn method_json(method: &ProbedMethod, target: &RegistryTarget) -> Option<Value> {
    let (id, name) = (&*method.id, &*method.name);
    match &method.kind {
        ProbedKind::Agent => Some(json!({"id": id, "name": name, "kind": "agent"})),
        ProbedKind::EnvVar => {
            // The probe records that a key is typed, not the variable it is
            // typed into; only a target that sets exactly one variable names
            // one this can be.
            let [(variable, _)] = target.env.as_slice() else {
                return None;
            };
            Some(json!({"id": id, "name": name, "kind": "env_var", "var": &**variable}))
        }
        ProbedKind::Terminal {
            command,
            args,
            env,
            tty,
        } => Some(json!({
            "id": id,
            "name": name,
            "kind": "terminal",
            "command": &**command,
            "args": args.iter().map(|arg| &**arg).collect::<Vec<_>>(),
            "env": env.iter().map(|variable| (&*variable.name, &*variable.value)).collect::<std::collections::BTreeMap<_, _>>(),
            "route": if *tty == Some(true) { "terminal" } else { "piped" },
        })),
        ProbedKind::Other => None,
    }
}

/// Why an entry has no target this build can install.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Unoffered {
    /// The registry lists no agent by that id.
    #[error("the ACP Registry lists no agent {id}")]
    Unknown {
        /// The id asked for.
        id: AgentKey,
    },
    /// The registry publishes no binary target for this platform.
    #[error("the ACP Registry gives {id} no binary for {platform}")]
    Platform {
        /// The agent.
        id: AgentKey,
        /// This build's platform.
        platform: Box<str>,
    },
    /// The target for this platform carries no SHA-256.
    #[error(
        "the ACP Registry gives {id} no SHA-256 for {platform}, so its program cannot be pinned \
         and it is not offered"
    )]
    Unhashed {
        /// The agent.
        id: AgentKey,
        /// This build's platform.
        platform: Box<str>,
    },
    /// The target's archive is not one the hardened unpacker reads.
    #[error(
        "the archive the ACP Registry gives {id} for {platform} is not one crucible unpacks: {archive}"
    )]
    Archive {
        /// The agent.
        id: AgentKey,
        /// This build's platform.
        platform: Box<str>,
        /// The archive's address.
        archive: Box<str>,
    },
}

/// Why the text is not a registry index this reader knows.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    /// It is larger than [`super::INDEX_BYTES`].
    #[error("the ACP Registry index is {0} bytes, more than the one MiB it may be")]
    Size(usize),
    /// It is not JSON.
    #[error("the ACP Registry index is not JSON")]
    Json(#[from] serde_json::Error),
    /// It lists more agents than [`AGENTS`].
    #[error("the ACP Registry index lists {0} agents, more than the {AGENTS} one may")]
    Agents(usize),
    /// A field is missing or not what it must be.
    #[error("{field} {why}")]
    Field {
        /// Which.
        field: Box<str>,
        /// What is wrong with it.
        why: &'static str,
    },
}

/// The field at `at`, named `name`.
fn field_at(at: &str, name: &str) -> Box<str> {
    if at.is_empty() {
        name.into()
    } else {
        format!("{at}.{name}").into()
    }
}

fn text(object: &Map<String, Value>, name: &str, at: &str) -> Result<Box<str>, RegistryError> {
    optional_text(object, name, at)?.ok_or_else(|| RegistryError::Field {
        field: field_at(at, name),
        why: "is missing",
    })
}

fn optional_text(
    object: &Map<String, Value>,
    name: &str,
    at: &str,
) -> Result<Option<Box<str>>, RegistryError> {
    match object.get(name).filter(|value| !value.is_null()) {
        None => Ok(None),
        Some(Value::String(value)) if !value.is_empty() => Ok(Some(value.as_str().into())),
        Some(Value::String(_)) => Err(RegistryError::Field {
            field: field_at(at, name),
            why: "is empty",
        }),
        Some(_) => Err(RegistryError::Field {
            field: field_at(at, name),
            why: "is not text",
        }),
    }
}

fn optional_https(
    object: &Map<String, Value>,
    name: &str,
    at: &str,
) -> Result<Option<Box<str>>, RegistryError> {
    match optional_text(object, name, at)? {
        Some(url) if url.starts_with("https://") && url.len() > "https://".len() => Ok(Some(url)),
        Some(_) => Err(RegistryError::Field {
            field: field_at(at, name),
            why: "is not an https address",
        }),
        None => Ok(None),
    }
}

fn texts(
    object: &Map<String, Value>,
    name: &str,
    at: &str,
) -> Result<Vec<Box<str>>, RegistryError> {
    let values = match object.get(name).filter(|value| !value.is_null()) {
        None => return Ok(Vec::new()),
        Some(Value::Array(values)) => values,
        Some(_) => {
            return Err(RegistryError::Field {
                field: field_at(at, name),
                why: "is not a list",
            });
        }
    };
    values
        .iter()
        .enumerate()
        .map(|(index, value)| match value.as_str() {
            Some(value) => Ok(value.into()),
            None => Err(RegistryError::Field {
                field: field_at(at, &format!("{name}[{index}]")),
                why: "is not text",
            }),
        })
        .collect()
}

fn variables(object: &Map<String, Value>, name: &str, at: &str) -> Result<Env, RegistryError> {
    let set = match object.get(name).filter(|value| !value.is_null()) {
        None => return Ok(Vec::new()),
        Some(Value::Object(set)) => set,
        Some(_) => {
            return Err(RegistryError::Field {
                field: field_at(at, name),
                why: "is not an object",
            });
        }
    };
    set.iter()
        .map(|(variable, value)| match value {
            Value::String(value) => Ok((variable.as_str().into(), value.as_str().into())),
            _ => Err(RegistryError::Field {
                field: field_at(at, &format!("{name}.{variable}")),
                why: "is not text",
            }),
        })
        .collect()
}

/// Whether `digest` is a SHA-256 in lower-case hexadecimal.
fn sha256(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The host the `https` address `url` names, in lower case and without a
/// trailing dot: no user, port, address or path, or nothing where it is not
/// one.
fn host_of(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() || host.contains(['@', ':', '[', ']']) {
        return None;
    }
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    (!host.is_empty()).then_some(host)
}

/// The day `at` is, as `YYYY-MM-DD`, with the days before 1970 read as the
/// epoch.
fn date(at: SystemTime) -> String {
    let seconds = at
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month, day) = civil(i64::try_from(seconds / 86_400).unwrap_or(i64::MAX));
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian year, month and day `days` after 1970-01-01, as
/// Howard Hinnant's `civil_from_days` computes them.
fn civil(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let of_era = shifted - era * 146_097;
    let year_of_era = (of_era - of_era / 1_460 + of_era / 36_524 - of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_prime + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    })
    .unwrap_or(1);
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::civil;

    #[test]
    fn the_civil_date_of_a_day_is_the_calendar_one() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(20_460), (2026, 1, 7));
        assert_eq!(civil(20_733), (2026, 10, 7));
    }
}
