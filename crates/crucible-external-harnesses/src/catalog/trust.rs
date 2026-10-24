//! What a user is offered of an agent, and the memory that decides it.
//!
//! A release is offered only if the compiled [`SNAPSHOT`] or the catalog most
//! lately believed lists it for this platform, that catalog has not passed its
//! `expires` by more than [`GRACE`] at the time the [`Trust`] was loaded, and no
//! revocation ever taken in names it. A revocation names a version, or only one
//! lock revision of it when it sets one. Revocations accumulate: one survives
//! the next catalog that omits it and the expiry of the one that carried it,
//! and at most [`REVOKED_CAP`] are kept, so a catalog that would take the count
//! past that is refused whole. A release is offered once, however many sources
//! list it, from the newest source.
//!
//! The memory is one owner-only file in `agent-catalog` under the state
//! directory, never in a cache, so clearing caches touches none of it. It holds
//! the highest serial believed, the revocations, a digest of the set of keys
//! they were taken in under, and the text of the catalog most lately believed.
//! That catalog was verified when it was taken in and is read back as it was
//! kept: [`Trust::load`] has no keys to check it under, and reads without any
//! check of them, so a launch sees the revocations whichever keys made them. The
//! snapshot's serial is the floor: no catalog below it is believed, and none
//! below the highest serial held. A state that is missing, cannot be read or is
//! in another format is read as deleted: it is rebuilt from the snapshot, with
//! no revocations until a catalog repeats them.
//!
//! The keys are checked where a catalog is taken in. When the set of keys given
//! to [`Trust::take_in`] is not the set the state was taken in under, the state
//! is discarded before the new catalog is verified, the same rebuilt-from-the-
//! snapshot state as a deleted one: its floor, its catalog and its revocations
//! go. That is why a signing key that pushed the serial to the largest number
//! cannot make the catalogs of its replacement refused for ever. Until the
//! first catalog is taken in under the new keys, what the old keys made held is
//! still offered and what they revoked is still withheld, and the expiry of the
//! held catalog bounds the first.
//!
//! [`Trust::load`] creates nothing. [`Trust::take_in`] keeps a catalog only
//! whole: it is verified, its releases are checked for repeats, its revocations
//! are counted, and the file is replaced, and until the file is replaced nothing
//! the value says has changed. Two crucible processes taking a catalog in at
//! once each replace the file whole, so the later one's view stands; neither
//! tears it.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crucible_types::{AGENT_KEY_BYTES, AgentKey};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    Catalog, CatalogError, Entry, GRACE, Held, INDEX_BYTES, Keys, Revoked, SNAPSHOT, Target,
    VERSION_BYTES, header,
};

/// The directory under the state directory the trust state is kept in.
pub(super) const DIRECTORY: &str = "agent-catalog";

/// The file in it.
pub(super) const FILE: &str = "trust.json";

/// The most revocations kept; a catalog that would take the count past it is
/// refused.
pub(super) const REVOKED_CAP: usize = 4096;

/// The most bytes one revocation takes in the file: a key of the longest
/// length, a version of the longest length with every byte escaped, a revision
/// of twenty digits, and the punctuation around them.
const REVOCATION_BYTES: usize = AGENT_KEY_BYTES
    + 2 * VERSION_BYTES
    + 20
    + "{\"key\":\"\",\"version\":\"\",\"lock_revision\":},".len();

/// The largest state file read, in bytes: the catalog at its largest with
/// every byte escaped, the revocations at their largest, and room for the rest
/// of the file.
pub(super) const STATE_BYTES: usize = 2 * INDEX_BYTES + REVOKED_CAP * REVOCATION_BYTES + 1024;

/// The format of the file this module writes and reads.
const FORMAT: u64 = 1;

/// One release the trust state offers, for the platform this build runs on.
///
/// It is owned, so it outlives the catalog it was read from, and it holds what
/// an install needs of one entry and one of its targets: which release it is,
/// and the target with its archives, files and launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedPin {
    /// The agent.
    pub key: AgentKey,
    /// The version.
    pub version: Box<str>,
    /// The revision of the version's package tree, for an agent whose tree is
    /// resolved again under the same version.
    pub lock_revision: Option<u64>,
    /// The agent as published for this platform.
    pub target: Target,
}

/// Why a catalog was not taken in.
#[derive(Debug, thiserror::Error)]
pub enum TrustError {
    /// It was not believed.
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    /// It lists one release more than once.
    #[error("the catalog lists version {version} of {key} more than once")]
    Repeated {
        /// The agent.
        key: AgentKey,
        /// The version.
        version: Box<str>,
    },
    /// Its revocations, with those already taken in, are more than are kept.
    #[error("the catalog would take the revocations kept past the {REVOKED_CAP} crucible keeps")]
    Revocations,
    /// The trust state could not be kept.
    #[error("the catalog trust state could not be kept: {0}")]
    Io(#[from] io::Error),
}

/// One withdrawal, as the trust state keeps it: the reason a catalog gave is
/// for the user who is told, and nothing here shows one.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Withdrawn {
    key: AgentKey,
    version: Box<str>,
    lock_revision: Option<u64>,
}

impl Withdrawn {
    fn of(revoked: &Revoked) -> Self {
        Self {
            key: revoked.key.clone(),
            version: revoked.version.clone(),
            lock_revision: revoked.lock_revision,
        }
    }
}

/// What the trust state holds.
#[derive(Debug)]
struct State {
    /// A digest of the set of keys it was taken in under.
    keys: String,
    /// The highest serial believed, and never below the snapshot's.
    serial: u64,
    /// Every withdrawal taken in.
    withdrawn: BTreeSet<Withdrawn>,
    /// The catalog most lately believed.
    held: Option<Catalog>,
}

impl State {
    /// The state of a machine that has believed nothing, which no set of keys
    /// is the one it was taken in under.
    fn rebuilt() -> Self {
        Self {
            keys: String::new(),
            serial: SNAPSHOT.serial(),
            withdrawn: BTreeSet::new(),
            held: None,
        }
    }

    /// Whether a withdrawal names `entry`.
    fn withdraws(&self, entry: &Entry) -> bool {
        let named = |lock_revision| {
            self.withdrawn.contains(&Withdrawn {
                key: entry.key.clone(),
                version: entry.version.clone(),
                lock_revision,
            })
        };
        // One that sets no lock revision names the version in every one.
        named(None) || named(entry.lock_revision)
    }
}

/// What a machine believes about the agents the catalog reviewed.
#[derive(Debug)]
pub struct Trust {
    /// Where the state is kept.
    directory: PathBuf,
    /// The time it was loaded, which a held catalog's expiry is judged at.
    now: SystemTime,
    state: State,
}

impl Trust {
    /// The trust state kept under `state` as of `now`. Creates nothing, checks
    /// no keys, and a state that is not there to be read is a state of nothing
    /// believed.
    #[must_use]
    pub fn load(state: &Path, now: SystemTime) -> Self {
        let directory = state.join(DIRECTORY);
        let state = read(&directory).unwrap_or_else(State::rebuilt);
        Self {
            directory,
            now,
            state,
        }
    }

    /// Believes the catalog `index` with its detached minisign `signature`,
    /// verified under `keys` as of `now`, and keeps it with the revocations it
    /// brings.
    ///
    /// # Errors
    ///
    /// [`TrustError`] naming the first check the catalog fails, or the way the
    /// state could not be kept. The state is then as it was.
    pub fn take_in(
        &mut self,
        index: &[u8],
        signature: &[u8],
        keys: &Keys,
        now: SystemTime,
    ) -> Result<(), TrustError> {
        let identity = identity(keys);
        // What another set of keys made held is not held to, and is dropped
        // when this catalog is kept: the floor is the snapshot's again, with
        // none of those revocations.
        let same = self.state.keys == identity;
        let floor = if same {
            self.state.serial
        } else {
            SNAPSHOT.serial()
        };
        let catalog = Catalog::verify(index, signature, keys, Held(floor), now)?;
        unrepeated(&catalog)?;
        let mut withdrawn = if same {
            self.state.withdrawn.clone()
        } else {
            BTreeSet::new()
        };
        withdrawn.extend(catalog.revoked().iter().map(Withdrawn::of));
        if withdrawn.len() > REVOKED_CAP {
            return Err(TrustError::Revocations);
        }
        let state = State {
            keys: identity,
            serial: catalog.serial(),
            withdrawn,
            held: Some(catalog),
        };
        keep(&self.directory, &state, index)?;
        self.state = state;
        Ok(())
    }

    /// The releases of the agent `key` that are offered for this platform: the
    /// held catalog's, if it has not expired, then the snapshot's, each release
    /// once and none that a revocation names.
    #[must_use]
    pub fn offered(&self, key: &AgentKey) -> Vec<OwnedPin> {
        let platform = here();
        let held: &[Entry] = self
            .state
            .held
            .as_ref()
            .filter(|catalog| !expired(catalog, self.now))
            .map_or(&[], Catalog::entries);
        let mut pins: Vec<OwnedPin> = Vec::new();
        for entry in held.iter().chain(SNAPSHOT.entries()) {
            if entry.key != *key || self.state.withdraws(entry) {
                continue;
            }
            let Some(target) = entry
                .targets
                .iter()
                .find(|target| *target.platform == *platform)
            else {
                continue;
            };
            if pins
                .iter()
                .any(|pin| pin.version == entry.version && pin.lock_revision == entry.lock_revision)
            {
                continue;
            }
            pins.push(OwnedPin {
                key: entry.key.clone(),
                version: entry.version.clone(),
                lock_revision: entry.lock_revision,
                target: target.clone(),
            });
        }
        pins
    }
}

/// How the catalog spells the platform `os` and `arch`: `<os>-<arch>`, with
/// macOS written `darwin` and the architecture as Rust writes it.
pub(super) fn platform(os: &str, arch: &str) -> String {
    let os = if os == "macos" { "darwin" } else { os };
    format!("{os}-{arch}")
}

/// The platform this build runs on, as the catalog spells it.
pub(super) fn here() -> String {
    platform(std::env::consts::OS, std::env::consts::ARCH)
}

/// Whether `catalog` has passed its `expires` by more than [`GRACE`] at `now`,
/// as [`Catalog::verify`] judges it.
fn expired(catalog: &Catalog, now: SystemTime) -> bool {
    catalog
        .expires()
        .checked_add(GRACE)
        .is_some_and(|last| now > last)
}

/// `catalog`, unless it lists one release more than once.
fn unrepeated(catalog: &Catalog) -> Result<(), TrustError> {
    let mut listed = BTreeSet::new();
    for entry in catalog.entries() {
        if !listed.insert((&entry.key, &entry.version, entry.lock_revision)) {
            return Err(TrustError::Repeated {
                key: entry.key.clone(),
                version: entry.version.clone(),
            });
        }
    }
    Ok(())
}

/// A digest of the set of `keys`, the same for the same keys in any order.
fn identity(keys: &Keys) -> String {
    let mut lines = keys.0.to_vec();
    lines.sort_unstable();
    lines.dedup();
    let mut digest = Sha256::new();
    for line in lines {
        digest.update(line.as_bytes());
        digest.update(b"\n");
    }
    digest
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// The state kept in `directory`, or nothing where it is not there, cannot be
/// read, is larger than it can be or is not in this format.
fn read(directory: &Path) -> Option<State> {
    let file = directory.join(FILE);
    crucible_privacy::tighten(&file).ok()?;
    let opened = crucible_privacy::open_read(&file).ok()?;
    let mut bytes = Vec::new();
    opened
        .take(
            u64::try_from(STATE_BYTES)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > STATE_BYTES {
        return None;
    }
    decode(&bytes)
}

/// The state `bytes` hold, or nothing where they are not a state of this
/// format.
fn decode(bytes: &[u8]) -> Option<State> {
    let document: Value = serde_json::from_slice(bytes).ok()?;
    let document = document.as_object()?;
    if document.get("format")?.as_u64()? != FORMAT {
        return None;
    }
    let revoked = document.get("revoked")?.as_array()?;
    if revoked.len() > REVOKED_CAP {
        return None;
    }
    Some(State {
        keys: document.get("keys")?.as_str()?.to_owned(),
        serial: document.get("serial")?.as_u64()?.max(SNAPSHOT.serial()),
        withdrawn: revoked.iter().map(withdrawal).collect::<Option<_>>()?,
        held: held(document),
    })
}

/// The withdrawal `value` holds.
fn withdrawal(value: &Value) -> Option<Withdrawn> {
    let fields = value.as_object()?;
    let version = fields.get("version")?.as_str()?;
    if version.is_empty() || version.len() > VERSION_BYTES {
        return None;
    }
    Some(Withdrawn {
        key: AgentKey::new(fields.get("key")?.as_str()?).ok()?,
        version: version.into(),
        lock_revision: match fields.get("lock_revision")? {
            Value::Null => None,
            revision => Some(revision.as_u64()?),
        },
    })
}

/// The catalog `document` holds, read as it was kept: it was verified when it
/// was taken in, and no serial or expiry is judged here.
fn held(document: &Map<String, Value>) -> Option<Catalog> {
    let index = document.get("index")?.as_str()?;
    if index.len() > INDEX_BYTES {
        return None;
    }
    let parsed: Value = serde_json::from_str(index).ok()?;
    Catalog::read(&header(&parsed).ok()?).ok()
}

/// Replaces the file in `directory` with `state` and the text of the catalog
/// it holds, durably, or leaves it as it was.
fn keep(directory: &Path, state: &State, index: &[u8]) -> io::Result<()> {
    let revoked: Vec<Value> = state
        .withdrawn
        .iter()
        .map(|withdrawn| {
            json!({
                "key": withdrawn.key.as_str(),
                "version": &*withdrawn.version,
                "lock_revision": withdrawn.lock_revision,
            })
        })
        .collect();
    let mut bytes = json!({
        "format": FORMAT,
        "keys": state.keys,
        "serial": state.serial,
        "revoked": revoked,
        "index": text(index)?,
    })
    .to_string()
    .into_bytes();
    bytes.push(b'\n');
    if bytes.len() > STATE_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "the trust state is larger than crucible keeps",
        ));
    }
    crucible_privacy::directory(directory).map_err(crucible_privacy::PrivacyError::into_io)?;
    replace(directory, &bytes)
}

/// `bytes` as the text a believed catalog is.
fn text(bytes: &[u8]) -> io::Result<&str> {
    std::str::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "a believed catalog is not text"))
}

/// Replaces the state file in `directory` with `bytes`, through a draft of its
/// own beside it that is removed unless it replaced the file.
fn replace(directory: &Path, bytes: &[u8]) -> io::Result<()> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    for _ in 0..16 {
        let next = NEXT.fetch_add(1, Ordering::Relaxed);
        let draft = directory.join(format!(".trust.{}.{next}.writing", std::process::id()));
        let mut file = match crucible_privacy::create_write(&draft) {
            Ok(file) => file,
            Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(problem) => return Err(problem.into_io()),
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        let landed = written.and_then(|()| {
            crucible_privacy::replace(&draft, &directory.join(FILE))
                .map_err(crucible_privacy::PrivacyError::into_io)
        });
        if landed.is_err() {
            // A draft left behind is ignored by every reader.
            let _ = fs::remove_file(&draft);
        }
        return landed;
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no free name for the trust state's draft",
    ))
}
