//! What a user is offered of an agent, and the memory that decides it.
//!
//! A release is offered only if the compiled [`SNAPSHOT`] or the catalog most
//! lately believed lists it for this platform, that catalog has not passed its
//! `expires` by more than [`GRACE`] at the time the [`Trust`] was loaded, and no
//! revocation ever taken in names it. A revocation names a version, or only one
//! lock revision of it when it sets one. Revocations accumulate: one survives
//! the next catalog that omits it and the expiry of the one that carried it, and
//! the snapshot's own are among them. At most [`REVOKED_CAP`] are kept, so a
//! catalog that would take the count past that is refused whole. A release is
//! offered once, however many sources list it, from the newest source.
//!
//! The memory is one owner-only file in `agent-catalog` under the state
//! directory, never in a cache, so clearing caches touches none of it. It holds
//! the highest serial believed, the revocations, a digest of the set of keys
//! they were taken in under, and the text of the catalog most lately believed.
//! That catalog was verified when it was taken in and is read back as it was
//! kept: [`Trust::load`] has no keys to check it under, and checks none. Nothing
//! in the file is signed, so a hand edit to it, or a file that another process
//! running as the user planted, is believed as written; the owner-only mode of
//! the file and its directory is what protects it.
//!
//! A state that is not there is a machine that has believed nothing: it is
//! rebuilt from the snapshot, whose serial is the floor, below which no catalog
//! is believed, and whose own revocations are the only ones until a catalog
//! repeats the others. A state that is there and cannot be used is not that. If
//! it cannot be read, is larger than is kept, is in another format or is not a
//! trust state, its revocations cannot be known, so [`Trust::offered`] offers
//! nothing for any agent and [`Trust::take_in`] refuses to write over it, each
//! saying [`why`](Unusable) and leaving a front end to tell the user to remove
//! or repair the file, which [`Trust::file`] names.
//!
//! The keys are checked where a catalog is taken in. When the set of keys given
//! to [`Trust::take_in`] is not the set the state was taken in under, the floor
//! falls to the snapshot's and the held catalog is dropped, and the revocations
//! are carried across unless carrying them would pass [`REVOKED_CAP`], when the
//! snapshot's and the new catalog's stand alone. That is why a signing key that
//! pushed the serial to the largest number cannot make the catalogs of its
//! replacement refused for ever, and why a change of keys does not make a
//! revoked release offerable. Until the first catalog is taken in under the new
//! keys, what the old keys made held is still offered, and the expiry of the
//! held catalog bounds that.
//!
//! [`Trust::load`] creates nothing. [`Trust::take_in`] verifies the catalog and
//! checks it for repeated releases first, so a catalog that does not pass
//! touches nothing. Then, holding an exclusive lock on a file beside the state,
//! waited for a bounded time and refused after it, it reads the file again,
//! merges the catalog into what the file holds, and replaces the file whole. So
//! two processes that loaded before either took a catalog in lose nothing: the
//! floor is the highest serial either saw, and the revocations are both. A
//! draft is written only under that lock, so a draft found under it is a
//! crashed writer's, and is removed. A catalog that cannot be kept, because the
//! lock or the file could not be had, still narrows the process that took it in:
//! its revocations, and under the same keys its serial, are applied to the
//! value before the error returns, since they can only narrow what is offered.
//! The file may or may not hold it then, as a replacement that landed before the
//! directory could be synced leaves it holding the catalog; the next
//! [`Trust::take_in`] reads the file again and sees whichever it is.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime};

use crucible_types::{AGENT_KEY_BYTES, AgentKey};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use super::{
    Catalog, CatalogError, Entry, GRACE, Held, INDEX_BYTES, Keys, Revoked, SNAPSHOT, VERSION_BYTES,
    header,
};
use crate::release::OwnedPin;

/// The directory under the state directory the trust state is kept in.
pub(super) const DIRECTORY: &str = "agent-catalog";

/// The file in it.
pub(super) const FILE: &str = "trust.json";

/// The file beside it that the state is changed under the lock on.
pub(super) const LOCK: &str = ".lock";

/// How long a catalog waits for another process's change to the state to
/// finish: a change is a read, a merge and one small write.
const LOCK_WAIT: Duration = Duration::from_secs(5);

/// How often the lock is asked for while it is waited for.
const LOCK_POLL: Duration = Duration::from_millis(10);

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

/// Why a state that is there cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unusable {
    /// It could not be opened or read: its mode, its owner, another name that
    /// reaches it or something else that is not a file.
    #[error("it cannot be read ({0})")]
    Unreadable(io::ErrorKind),
    /// It is larger than any state this crucible keeps.
    #[error("it is larger than the {STATE_BYTES} bytes crucible keeps")]
    Oversize,
    /// It is in a format other than the one this crucible keeps, as one
    /// written by another version is.
    #[error("it is in format {0}, and this crucible keeps format {FORMAT}")]
    Format(u64),
    /// It is not a trust state: not JSON, not an object, or without the
    /// fields of one.
    #[error("it is not a trust state")]
    Corrupt,
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
    /// The trust state is there and cannot be used, so it is not written over.
    #[error("the catalog trust state cannot be used, and is left as it is: {0}")]
    Unusable(Unusable),
    /// Another crucible process held the trust state for longer than a change
    /// to it takes.
    #[error("another crucible process is changing the catalog trust state")]
    Contended,
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
pub(super) struct State {
    /// A digest of the set of keys it was taken in under.
    keys: String,
    /// The highest serial believed, and never below the snapshot's.
    serial: u64,
    /// Every withdrawal taken in, the snapshot's own among them.
    withdrawn: BTreeSet<Withdrawn>,
    /// The catalog most lately believed.
    held: Option<Catalog>,
}

impl State {
    /// The state of a machine that has believed nothing under `snapshot`,
    /// which no set of keys is the one it was taken in under.
    pub(super) fn rebuilt(snapshot: &Catalog) -> Self {
        Self {
            keys: String::new(),
            serial: snapshot.serial(),
            withdrawn: snapshot.revoked().iter().map(Withdrawn::of).collect(),
            held: None,
        }
    }

    /// Whether a withdrawal names `entry`.
    pub(super) fn withdraws(&self, entry: &Entry) -> bool {
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

/// What a [`Trust`] knows of the file.
#[derive(Debug)]
enum Memory {
    /// What it held, or nothing believed where it was not there.
    Usable(State),
    /// It was there and could not be used.
    Unusable(Unusable),
}

/// What a machine believes about the agents the catalog reviewed.
#[derive(Debug)]
pub struct Trust {
    /// Where the state is kept.
    directory: PathBuf,
    /// The time it was loaded, which a held catalog's expiry is judged at.
    now: SystemTime,
    memory: Memory,
    /// How long a catalog waits for another process's change to the state.
    wait: Duration,
}

/// Why a catalog was not kept, by what the value should then do.
enum Failure {
    /// It is not to be believed, or is believed no further than it was.
    Refused(TrustError),
    /// The file is there and unusable.
    Unusable(Unusable),
    /// It could not be written, and revocations in it still apply here.
    Unkept(TrustError),
}

impl Trust {
    /// The trust state kept under `state` as of `now`. Creates nothing, checks
    /// no keys, and a state that is not there is a state of nothing believed;
    /// one that is there and cannot be used is [`Trust::unusable`].
    #[must_use]
    pub fn load(state: &Path, now: SystemTime) -> Self {
        let directory = state.join(DIRECTORY);
        let memory = match read(&directory, &SNAPSHOT) {
            Ok(Some(state)) => Memory::Usable(state),
            Ok(None) => Memory::Usable(State::rebuilt(&SNAPSHOT)),
            Err(why) => Memory::Unusable(why),
        };
        Self {
            directory,
            now,
            memory,
            wait: LOCK_WAIT,
        }
    }

    /// This value, waiting at most `wait` for another process in
    /// [`Trust::take_in`], so a test need not wait out the real bound.
    #[cfg(all(test, feature = "proof"))]
    pub(super) fn waiting(mut self, wait: Duration) -> Self {
        self.wait = wait;
        self
    }

    /// The file the state is kept in, which is the one to repair or remove
    /// when the state cannot be used. It may not be there.
    #[must_use]
    pub fn file(&self) -> PathBuf {
        self.directory.join(FILE)
    }

    /// Why the state, which is there, cannot be used, or nothing where it can.
    /// Nothing is offered while it cannot.
    #[must_use]
    pub fn unusable(&self) -> Option<Unusable> {
        match &self.memory {
            Memory::Unusable(why) => Some(*why),
            Memory::Usable(_) => None,
        }
    }

    /// Believes the catalog `index` with its detached minisign `signature`,
    /// verified under `keys` as of `now`, and keeps it with the revocations it
    /// brings, merged with what another process has kept since this value was
    /// loaded. Blocks for at most a few seconds while another process changes
    /// the state.
    ///
    /// # Errors
    ///
    /// [`TrustError`] naming the first check the catalog fails, that the state
    /// is there and unusable, or the way it could not be kept. When the catalog
    /// fails a check the value is as it was. When the state is unusable the
    /// value is unusable too, offering nothing. When it could not be kept the
    /// catalog's revocations still apply to the value, as the module
    /// documentation says.
    pub fn take_in(
        &mut self,
        index: &[u8],
        signature: &[u8],
        keys: &Keys,
        now: SystemTime,
    ) -> Result<(), TrustError> {
        // The floor is the file's, and is judged under the lock.
        let catalog = Catalog::verify(index, signature, keys, Held(0), now)?;
        unrepeated(&catalog)?;
        let identity = identity(keys);
        match kept(&self.directory, &catalog, index, &identity, self.wait) {
            Ok(state) => {
                self.memory = Memory::Usable(state);
                Ok(())
            }
            Err(Failure::Refused(error)) => Err(error),
            Err(Failure::Unusable(why)) => {
                self.memory = Memory::Unusable(why);
                Err(TrustError::Unusable(why))
            }
            Err(Failure::Unkept(error)) => {
                self.narrowed(&catalog, &identity);
                Err(error)
            }
        }
    }

    /// Applies what `catalog` can only narrow: its revocations, if they fit
    /// beside those held, and under the same keys its serial.
    fn narrowed(&mut self, catalog: &Catalog, identity: &str) {
        let Memory::Usable(state) = &mut self.memory else {
            return;
        };
        let mut withdrawn = state.withdrawn.clone();
        withdrawn.extend(catalog.revoked().iter().map(Withdrawn::of));
        if withdrawn.len() > REVOKED_CAP {
            return;
        }
        state.withdrawn = withdrawn;
        if state.keys == identity {
            state.serial = state.serial.max(catalog.serial());
        }
    }

    /// The releases of the agent `key` that are offered for this platform: the
    /// held catalog's, if it has not expired, then the snapshot's, each release
    /// once and none that a revocation names. Nothing, for any agent, while the
    /// state is unusable.
    #[must_use]
    pub fn offered(&self, key: &AgentKey) -> Vec<OwnedPin> {
        self.offers(Some(key))
            .into_iter()
            .map(|offer| offer.pin)
            .collect()
    }

    /// The releases offered for this platform, as [`Trust::offered`] gives
    /// them, each with the entry that lists it and the serial of the catalog it
    /// was listed in: of the agent `key`, or of every agent, in the order the
    /// sources list them, where it is `None`.
    #[must_use]
    pub fn offers(&self, key: Option<&AgentKey>) -> Vec<Offer> {
        let Memory::Usable(state) = &self.memory else {
            return Vec::new();
        };
        let platform = here();
        let held = state
            .held
            .as_ref()
            .filter(|catalog| !expired(catalog, self.now));
        let sources = held
            .into_iter()
            .map(|catalog| (catalog.serial(), catalog.entries()))
            .chain(std::iter::once((SNAPSHOT.serial(), SNAPSHOT.entries())));
        let mut offers: Vec<Offer> = Vec::new();
        for (serial, entries) in sources {
            for entry in entries {
                if key.is_some_and(|key| entry.key != *key) || state.withdraws(entry) {
                    continue;
                }
                let Some(pin) = release(entry, &platform) else {
                    continue;
                };
                if offers.iter().any(|held| {
                    held.pin.key == pin.key
                        && held.pin.version == pin.version
                        && held.pin.lock_revision == pin.lock_revision
                }) {
                    continue;
                }
                offers.push(Offer {
                    pin,
                    entry: entry.clone(),
                    serial,
                });
            }
        }
        offers
    }

    /// Whether a revocation taken in names `version` of the agent `key` at
    /// `lock_revision`, as one names an offered release; every release is,
    /// while the state is unusable, since its revocations cannot be known.
    #[must_use]
    pub fn revoked(&self, key: &AgentKey, version: &str, lock_revision: Option<u64>) -> bool {
        let Memory::Usable(state) = &self.memory else {
            return true;
        };
        let named = |lock_revision| {
            state.withdrawn.contains(&Withdrawn {
                key: key.clone(),
                version: version.into(),
                lock_revision,
            })
        };
        named(None) || lock_revision.is_some_and(|revision| named(Some(revision)))
    }
}

/// One release the trust state offers, with what its catalog said of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The release, for the platform this build runs on.
    pub pin: OwnedPin,
    /// The entry that lists it.
    pub entry: Entry,
    /// The serial of the catalog that lists it: the one held, or the
    /// snapshot's.
    pub serial: u64,
}

/// The release `entry` lists, as published for `platform`, where it is.
pub(super) fn release(entry: &Entry, platform: &str) -> Option<OwnedPin> {
    let target = entry
        .targets
        .iter()
        .find(|target| *target.platform == *platform)?;
    Some(OwnedPin {
        key: entry.key.clone(),
        version: entry.version.clone(),
        lock_revision: entry.lock_revision,
        admits: entry.admits.clone(),
        target: target.clone(),
    })
}

/// `catalog`, verified and unrepeated, merged into the state the file holds
/// under its lock, and kept: the state now.
fn kept(
    directory: &Path,
    catalog: &Catalog,
    index: &[u8],
    identity: &str,
    wait: Duration,
) -> Result<State, Failure> {
    let unkept = |problem: crucible_privacy::PrivacyError| {
        Failure::Unkept(TrustError::Io(problem.into_io()))
    };
    crucible_privacy::directory(directory).map_err(unkept)?;
    let _hold = hold(directory, wait)?;
    let base = match read(directory, &SNAPSHOT) {
        Ok(Some(state)) => state,
        Ok(None) => State::rebuilt(&SNAPSHOT),
        Err(why) => return Err(Failure::Unusable(why)),
    };
    let state = merge(base, catalog, identity, &SNAPSHOT).map_err(Failure::Refused)?;
    // Drafts are written only under the lock, so one found under it is a
    // writer's that died.
    remove_drafts(directory);
    write(directory, &state, index).map_err(|problem| Failure::Unkept(TrustError::Io(problem)))?;
    Ok(state)
}

/// `catalog` taken into `base`, a state `snapshot` is the floor of: under the
/// same `identity` of keys the floor is `base`'s serial, and under others the
/// snapshot's, with `base`'s revocations carried across unless they would pass
/// the cap.
///
/// # Errors
///
/// [`TrustError`] when the catalog is below the floor, or the revocations are
/// more than are kept.
pub(super) fn merge(
    base: State,
    catalog: &Catalog,
    identity: &str,
    snapshot: &Catalog,
) -> Result<State, TrustError> {
    let same = base.keys == identity;
    let floor = if same { base.serial } else { snapshot.serial() };
    if catalog.serial() < floor {
        return Err(CatalogError::Serial {
            offered: catalog.serial(),
            held: floor,
        }
        .into());
    }
    let incoming = || catalog.revoked().iter().map(Withdrawn::of);
    let mut withdrawn = base.withdrawn;
    withdrawn.extend(incoming());
    if !same && withdrawn.len() > REVOKED_CAP {
        withdrawn = snapshot
            .revoked()
            .iter()
            .map(Withdrawn::of)
            .chain(incoming())
            .collect();
    }
    if withdrawn.len() > REVOKED_CAP {
        return Err(TrustError::Revocations);
    }
    Ok(State {
        keys: identity.to_owned(),
        serial: catalog.serial(),
        withdrawn,
        held: Some(catalog.clone()),
    })
}

/// The exclusive lock on the state, released when dropped.
#[derive(Debug)]
struct Hold(File);

impl Drop for Hold {
    fn drop(&mut self) {
        // The lock goes with the handle whatever this says.
        let _ = crucible_privacy::unlock_identity(&self.0);
    }
}

/// The lock on the state in `directory`, waited for at most `wait`.
fn hold(directory: &Path, wait: Duration) -> Result<Hold, Failure> {
    let io = |problem: crucible_privacy::PrivacyError| {
        Failure::Unkept(TrustError::Io(problem.into_io()))
    };
    let file = crucible_privacy::lock(&directory.join(LOCK)).map_err(io)?;
    let started = Instant::now();
    loop {
        if crucible_privacy::try_lock_identity(&file).map_err(io)? {
            return Ok(Hold(file));
        }
        if started.elapsed() >= wait {
            return Err(Failure::Unkept(TrustError::Contended));
        }
        std::thread::sleep(LOCK_POLL);
    }
}

/// Removes the drafts a writer that died left in `directory`.
fn remove_drafts(directory: &Path) {
    let Ok(listing) = fs::read_dir(directory) else {
        return;
    };
    for entry in listing.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(".trust.") && name.ends_with(".writing") {
            let _ = fs::remove_file(entry.path());
        }
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

/// The state kept in `directory`, nothing where it is not there, or why it is
/// there and cannot be used.
fn read(directory: &Path, snapshot: &Catalog) -> Result<Option<State>, Unusable> {
    let file = directory.join(FILE);
    let opened =
        match crucible_privacy::tighten(&file).and_then(|_| crucible_privacy::open_read(&file)) {
            Ok(opened) => opened,
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(problem) => return Err(Unusable::Unreadable(problem.kind())),
        };
    let mut bytes = Vec::new();
    opened
        .take(
            u64::try_from(STATE_BYTES)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )
        .read_to_end(&mut bytes)
        .map_err(|problem| Unusable::Unreadable(problem.kind()))?;
    if bytes.len() > STATE_BYTES {
        return Err(Unusable::Oversize);
    }
    decode(&bytes, snapshot).map(Some)
}

/// The state `bytes` hold, with `snapshot`'s revocations among its own, or why
/// they are not a state of this format.
pub(super) fn decode(bytes: &[u8], snapshot: &Catalog) -> Result<State, Unusable> {
    let document: Value = serde_json::from_slice(bytes).map_err(|_| Unusable::Corrupt)?;
    let document = document.as_object().ok_or(Unusable::Corrupt)?;
    match document.get("format").and_then(Value::as_u64) {
        Some(FORMAT) => {}
        Some(other) => return Err(Unusable::Format(other)),
        None => return Err(Unusable::Corrupt),
    }
    fields(document, snapshot).ok_or(Unusable::Corrupt)
}

/// The state the object `document`, which is in this format, holds.
fn fields(document: &Map<String, Value>, snapshot: &Catalog) -> Option<State> {
    let revoked = document.get("revoked")?.as_array()?;
    if revoked.len() > REVOKED_CAP {
        return None;
    }
    let mut withdrawn: BTreeSet<Withdrawn> =
        revoked.iter().map(withdrawal).collect::<Option<_>>()?;
    withdrawn.extend(snapshot.revoked().iter().map(Withdrawn::of));
    Some(State {
        keys: document.get("keys")?.as_str()?.to_owned(),
        serial: document.get("serial")?.as_u64()?.max(snapshot.serial()),
        withdrawn,
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
fn write(directory: &Path, state: &State, index: &[u8]) -> io::Result<()> {
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
