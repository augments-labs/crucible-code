//! The install store: the reviewed releases of catalog agents crucible keeps
//! under `programs/<key>/`, each version whole in a directory of its own with
//! the receipt that says where it came from.
//!
//! A version's directory is named `<version>`, or `<version>+<revision>` for
//! an agent whose package tree is resolved again under the same version, so
//! two revisions sit side by side. Every name the store makes a directory
//! for, a key's and a version's, is one Windows can hold, by the rules the
//! unpacker holds a member's name to: printable ASCII, none of the characters
//! Windows refuses, no trailing dot or space, and no device Windows reserves
//! in any spelling. A version also has no `+`, so no two releases share a
//! directory, and does not begin with a dot, so none takes a name the store
//! keeps for itself: `.lock`, `.staging`, and in a version `.in-use`,
//! `.receipt` and `.receipt.writing`.
//!
//! A version is made in a staging directory under `.staging/`, which the
//! unpackers write into, and committed by renaming that directory into place;
//! only then is its receipt written into it, as `.receipt`. A version is
//! installed once its receipt is there. One that a process renamed into place
//! and died before recording is not, and the next commit of that version
//! replaces it. Nothing names a version before its commit returns, since the
//! instance's `executable` is written after it, so a crash leaves either the
//! program named before or the new one whole. A commit is refused unless the
//! staging directory holds exactly the files given for it, of the kinds and
//! sizes given, and it replaces a version already there only when no process
//! holds that version.
//!
//! The receipt is the truth about what is installed, whatever a catalog says
//! later. It records the catalog serial and the review the release was taken
//! from, the terms link accepted, when it was fetched, each artifact's address,
//! the hosts the download came through, its size and SHA-256, and every file
//! with its size, SHA-256 and exec flag, or kind and link target. It is JSON in
//! format [`FORMAT`], read only up to [`RECEIPT_BYTES`], and one in another
//! format, or that names another release or a path outside its version, is
//! refused. Nothing in it is signed: the owner-only mode of the store's
//! directories and of the receipt is what protects it.
//!
//! Every change to an agent's store, a staging directory made, a commit, an
//! adoption or a prune, takes an exclusive lock on `programs/<key>/.lock`,
//! waits for it at most [`LOCK_WAIT`], and is refused after it. A change starts
//! by removing what a dead process left in staging: a staging directory is
//! live while the lock on the `.lock` file beside it is held, which its owner
//! holds until it is committed or given up, and everything else in `.staging/`
//! is a dead process's. A version removed, or replaced by a commit, is first
//! renamed into `.staging/`, so a version is whole or gone.
//!
//! A running agent holds its version through a file of its own in the
//! version's `.in-use/`, named for the process that holds it, which that
//! process keeps locked for the run's life. The hold is made under the store's
//! lock, so no prune can take the version between finding it there and holding
//! it. A prune keeps every version an instance's `executable` names, and the
//! newest of the rest by when it was fetched that has a receipt, and removes
//! every other version no live process holds; a holder whose lock it can take
//! belongs to a process that has ended, and is removed.
//!
//! An install made before the store kept receipts, as 0.48 left Antigravity in
//! `programs/antigravity/1.3.0/`, is adopted by hashing its files against the
//! file list the catalog gives for it, and given a receipt when they match.
//! One that differs in any way is left without one, for the user to check and
//! reinstall. Verifying hashes every file a receipt lists in the same way, and
//! names each difference: a file missing, changed, of another size or kind or
//! exec flag, a link pointing elsewhere, or a file the receipt does not list,
//! which would otherwise run unreviewed beside the files that were reviewed.
//!
//! What the store keeps is bounded by [`PROGRAMS_BYTES`]: an artifact whose
//! download, or what it decompresses or unpacks to, is larger than that, or a
//! release whose artifacts together unpack to more, is refused before it is
//! downloaded, and a release that would take what the store's receipts
//! account for past it is refused then and again before it is committed.
//! Sizes are summed with checked arithmetic. The room is judged under one
//! agent's lock, so two agents committed at once by two processes can pass it
//! by one release.

use std::collections::HashSet;
use std::fs::{self, File as Handle};
use std::io::{self, Read as _, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_privacy::PrivacyError;
use crucible_types::AgentKey;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

use crate::catalog::{File, FileKind, Files, OwnedPin, Target};

/// The most the store holds, in bytes: the bound on an artifact's download,
/// decompressed and unpacked sizes, and on everything its receipts account
/// for together.
pub const PROGRAMS_BYTES: u64 = 16 << 30;

/// The largest receipt read or written, in bytes: the per-file lists of every
/// artifact of a release, each at most a catalog asset, with room to spare.
pub const RECEIPT_BYTES: usize = 16 << 20;

/// The receipt format this store writes and reads.
pub const FORMAT: u64 = 1;

/// How long a change waits for another process's change to the same agent's
/// store to finish: at most a few renames, a receipt, the removal of a version
/// or the hashing of one version's files.
pub const LOCK_WAIT: Duration = Duration::from_secs(30);

/// How often the lock is asked for while it is waited for.
const LOCK_POLL: Duration = Duration::from_millis(10);

/// The file in an agent's directory that its store is changed under the lock
/// on, and the suffix of the file beside each staging directory whose lock
/// says it is live.
const LOCK: &str = ".lock";

/// The directory in an agent's directory where versions are made and removed.
const STAGING: &str = ".staging";

/// The directory in a version where the processes running it hold it.
const IN_USE: &str = ".in-use";

/// A version's receipt.
const RECEIPT: &str = ".receipt";

/// A receipt being written, which replaces the receipt once it is whole.
const DRAFT: &str = ".receipt.writing";

/// The names the store keeps for itself at the top of a version.
const OWN: [&str; 3] = [IN_USE, RECEIPT, DRAFT];

/// The most entries read from one of the store's own directories: the keys,
/// one agent's versions, a version's holders or what staging was left.
const ENTRIES: usize = 4096;

/// The most files a comparison names that a list does not.
const UNLISTED: usize = 64;

/// The most of a name a refusal repeats, in characters.
const NAMED: usize = 64;

/// The longest name a directory the store makes may have, in bytes.
const COMPONENT_BYTES: usize = 255;

/// The longest path a receipt may list, in bytes.
const PATH_BYTES: usize = 4096;

/// How many names are tried for a directory or file of a change's own before
/// it gives up.
const ATTEMPTS: usize = 16;

/// The devices Windows reserves a name for whatever follows it after a dot or
/// a space, besides `com0` to `com9` and `lpt0` to `lpt9`.
const DEVICES: [&str; 6] = ["con", "prn", "aux", "nul", "conin$", "conout$"];

/// The characters Windows refuses in a name, besides the backslash and the
/// colon refused as a separator and a drive.
const WINDOWS_REFUSES: [char; 6] = ['<', '>', '"', '|', '?', '*'];

/// The number the next name a change makes for itself takes in this process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// The releases crucible keeps for catalog agents.
#[derive(Debug)]
pub struct Store {
    /// The `programs` directory in crucible's home.
    root: PathBuf,
    /// How long a change waits for another process's.
    wait: Duration,
    /// The most it holds.
    ceiling: u64,
    /// Where a commit says it has made its rename, and then waits to be
    /// killed, so a test can end the process between the rename and the
    /// receipt.
    #[cfg(all(test, feature = "proof"))]
    pause: Option<PathBuf>,
}

/// One version of an agent as the store names its directory: the version,
/// and the revision of its package tree where it has one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    /// The version, as the catalog spells it.
    text: Box<str>,
    lock_revision: Option<u64>,
    /// The directory's name.
    name: Box<str>,
}

/// A staging directory of one change's own, live while this value is, that a
/// version is unpacked into before it is committed; removed with the value
/// unless it was.
#[derive(Debug)]
pub struct Staging {
    key: AgentKey,
    directory: PathBuf,
    /// The lock that says the directory is live.
    lock: Option<Handle>,
    lock_path: PathBuf,
    /// Whether the directory was renamed into place.
    committed: bool,
}

/// What the catalog said of the release a receipt is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The serial of the catalog it was taken from.
    pub serial: u64,
    /// The review pull request that admitted it.
    pub review: Box<str>,
    /// The terms the user accepted by installing it, where it has any.
    pub terms: Option<Box<str>>,
}

/// Where a release being committed came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// What the catalog said of it.
    pub listing: Listing,
    /// When its artifacts were fetched.
    pub fetched: SystemTime,
    /// Each artifact, as it was fetched.
    pub artifacts: Vec<Fetched>,
}

/// One artifact of a release, as it was fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fetched {
    /// The address it was fetched from.
    pub url: Box<str>,
    /// The hosts the download came through.
    pub hosts: Vec<Box<str>>,
    /// Its size in bytes.
    pub size: u64,
    /// Its SHA-256, in lowercase hexadecimal.
    pub sha256: Box<str>,
}

/// How a version came to be kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Fetched and committed by this store.
    Fetched,
    /// Found installed before the store kept receipts, and matched against
    /// the catalog's list.
    Adopted,
}

/// What the store recorded of one installed version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    /// The agent.
    pub key: AgentKey,
    /// The version.
    pub version: Box<str>,
    /// The revision of the version's package tree, where it has one.
    pub lock_revision: Option<u64>,
    /// What the catalog said of it.
    pub listing: Listing,
    /// When its artifacts were fetched, to the second; for a version adopted,
    /// when it was adopted, since the install before kept no time.
    pub fetched: SystemTime,
    /// How it came to be kept.
    pub origin: Origin,
    /// Each artifact it was installed from: as it was fetched, or for a
    /// version adopted as it was pinned, through hosts not known.
    pub artifacts: Vec<Fetched>,
    /// Every file it holds.
    pub files: Vec<File>,
}

/// A version held by a running agent, until this value is dropped.
#[derive(Debug)]
pub struct InUse {
    file: Option<Handle>,
    path: PathBuf,
}

/// What adopting an install made before the store kept receipts found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Adoption {
    /// It is the release listed, and has a receipt now, or already had one.
    Adopted(Receipt),
    /// It differs from the list, so it has no receipt, and is for the user to
    /// check and reinstall.
    CheckNeeded(Vec<Mismatch>),
}

/// One way a version differs from what it is said to hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mismatch {
    /// The path, relative to the version's directory.
    pub path: Box<str>,
    /// How it differs.
    pub what: Differs,
}

/// How a file differs from what it is said to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Differs {
    /// It is not there, or not reached through directories alone.
    Missing,
    /// It is there as another kind: a file, a directory or a link.
    Kind,
    /// It holds another number of bytes.
    Size,
    /// It can be run where it should not, or not where it should.
    Exec,
    /// It holds other bytes.
    Bytes,
    /// It is a link to somewhere else.
    Target,
    /// It is not listed at all.
    Unlisted,
    /// It could not be read to tell.
    Unreadable,
}

/// What a prune did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pruned {
    /// The versions removed.
    pub removed: Vec<Version>,
    /// The versions that would have been removed, and are kept because a
    /// process runs them.
    pub held: Vec<Version>,
}

/// Why the store did not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// A key or version is not one the store makes a directory for.
    #[error("{name:?} {why}, so the install store makes no directory for it")]
    Name {
        /// The name, cut short.
        name: String,
        /// What is wrong with it.
        why: &'static str,
    },
    /// Another process held the agent's store for longer than a change takes.
    #[error("another crucible process is changing this agent's install store")]
    Contended,
    /// A process runs the version, so it is not replaced.
    #[error("a running agent uses this version, so it is not replaced")]
    InUse,
    /// The version is not there.
    #[error("this version is not installed")]
    NotInstalled,
    /// The version has no receipt, so what it holds cannot be checked.
    #[error("this version has no receipt, so it was not installed whole or not by this store")]
    Unreceipted,
    /// The receipt is in another format, as one written by a later crucible.
    #[error("the version's receipt is in format {0}, and this crucible reads format {FORMAT}")]
    Format(u64),
    /// The receipt is not one: not JSON, without the fields of one, naming
    /// another release or a path outside its version.
    #[error("the version's receipt is not one this crucible can read")]
    Corrupt,
    /// The receipt is larger than [`RECEIPT_BYTES`].
    #[error("the version's receipt is larger than the {RECEIPT_BYTES} bytes crucible reads")]
    Oversize,
    /// The release, or one of its artifacts, is larger than the store holds.
    #[error("the release is larger than the {ceiling} bytes the install store holds")]
    TooLarge {
        /// What the store holds.
        ceiling: u64,
    },
    /// The release would take the store past what it holds.
    #[error(
        "the install store has no room for the release within its {ceiling} bytes; remove an agent first"
    )]
    Full {
        /// What the store holds.
        ceiling: u64,
    },
    /// The release holds a name the store keeps for itself.
    #[error("the release holds {name:?}, a name the install store keeps for itself")]
    Reserved {
        /// The name, cut short.
        name: String,
    },
    /// What was staged is not the list given for it.
    #[error("what was staged differs from its file list at {:?}", .0.path)]
    Unrecorded(Mismatch),
    /// The release's file list is a catalog asset, which is fetched and read
    /// into a list first.
    #[error("the release's file list is a catalog asset that has not been read")]
    Asset,
    /// Something that is not a version is where a version goes.
    #[error("{} holds something other than a version", .path.display())]
    InTheWay {
        /// Where.
        path: PathBuf,
    },
    /// One of the store's directories holds more than it reads.
    #[error("{} holds more entries than the install store reads", .path.display())]
    Crowded {
        /// Which.
        path: PathBuf,
    },
    /// The file system refused.
    #[error("the install store could not be changed at {}", .path.display())]
    Io {
        /// Where.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: io::Error,
    },
}

impl Version {
    /// `version`, at `lock_revision` where its package tree has one, once
    /// both make a directory name the store can hold.
    ///
    /// # Errors
    ///
    /// [`StoreError::Name`] naming why not.
    pub fn new(version: &str, lock_revision: Option<u64>) -> Result<Self, StoreError> {
        holdable(version)?;
        let refused = |why| {
            Err(StoreError::Name {
                name: shown(version),
                why,
            })
        };
        if version.starts_with('.') {
            return refused("begins with a dot, as only a name the store keeps does");
        }
        if version.contains('+') {
            return refused("holds a `+`, which begins a revision in a version's directory");
        }
        let name = match lock_revision {
            Some(revision) => format!("{version}+{revision}"),
            None => version.to_owned(),
        };
        holdable(&name)?;
        Ok(Self {
            text: version.into(),
            lock_revision,
            name: name.into(),
        })
    }

    /// The version.
    #[must_use]
    pub fn version(&self) -> &str {
        &self.text
    }

    /// The revision of its package tree, where it has one.
    #[must_use]
    pub fn lock_revision(&self) -> Option<u64> {
        self.lock_revision
    }

    /// Its directory's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The version a directory named `name` holds, where it is one the store
    /// would make.
    fn parse(name: &str) -> Option<Self> {
        let (version, lock_revision) = match name.split_once('+') {
            Some((version, revision)) => (version, Some(revision.parse().ok()?)),
            None => (name, None),
        };
        Self::new(version, lock_revision)
            .ok()
            .filter(|parsed| *parsed.name == *name)
    }
}

impl Staging {
    /// The directory, which only its owner may reach, to unpack into.
    #[must_use]
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.committed {
            let _ = fs::remove_dir_all(&self.directory);
        }
        if let Some(lock) = self.lock.take() {
            // The lock goes with the handle whatever this says.
            let _ = crucible_privacy::unlock_identity(&lock);
        }
        let _ = fs::remove_file(&self.lock_path);
    }
}

impl Drop for InUse {
    fn drop(&mut self) {
        if let Some(file) = self.file.take() {
            let _ = crucible_privacy::unlock_identity(&file);
        }
        let _ = fs::remove_file(&self.path);
    }
}

impl Receipt {
    /// The bytes its files hold, past which nothing is counted.
    fn unpacked(&self) -> u64 {
        self.files.iter().fold(0, |total, file| match file.kind {
            FileKind::File { size, .. } => total.saturating_add(size),
            FileKind::Dir | FileKind::Link { .. } => total,
        })
    }

    /// It as the file holds it, once it is no larger than [`RECEIPT_BYTES`].
    fn encoded(&self) -> Result<Vec<u8>, StoreError> {
        let artifacts: Vec<Value> = self
            .artifacts
            .iter()
            .map(|artifact| {
                json!({
                    "url": &*artifact.url,
                    "hosts": artifact.hosts.iter().map(|host| &**host).collect::<Vec<_>>(),
                    "size": artifact.size,
                    "sha256": &*artifact.sha256,
                })
            })
            .collect();
        let mut bytes = json!({
            "format": FORMAT,
            "key": self.key.as_str(),
            "version": &*self.version,
            "lock_revision": self.lock_revision,
            "serial": self.listing.serial,
            "review": &*self.listing.review,
            "terms": self.listing.terms.as_deref(),
            "fetched": seconds(self.fetched),
            "origin": match self.origin {
                Origin::Fetched => "fetched",
                Origin::Adopted => "adopted",
            },
            "artifacts": artifacts,
            "files": self.files.iter().map(File::json).collect::<Vec<_>>(),
        })
        .to_string()
        .into_bytes();
        bytes.push(b'\n');
        if bytes.len() > RECEIPT_BYTES {
            return Err(StoreError::Oversize);
        }
        Ok(bytes)
    }
}

/// The lock on one agent's store, released when dropped.
#[derive(Debug)]
struct Locked(Handle);

impl Drop for Locked {
    fn drop(&mut self) {
        // The lock goes with the handle whatever this says.
        let _ = crucible_privacy::unlock_identity(&self.0);
    }
}

/// One change to an agent's store, under its lock.
#[derive(Debug)]
struct Change {
    /// The agent's directory.
    directory: PathBuf,
    /// Its staging directory.
    area: PathBuf,
    _lock: Locked,
}

/// Whether a version's files are hashed when it is compared with a list, or
/// only their kinds, sizes and exec flags are read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Contents {
    Hashed,
    Unread,
}

impl Store {
    /// The store in `root`, crucible's `programs` directory. Touches nothing.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            wait: LOCK_WAIT,
            ceiling: PROGRAMS_BYTES,
            #[cfg(all(test, feature = "proof"))]
            pause: None,
        }
    }

    /// This store, waiting at most `wait` for another process's change, so a
    /// test need not wait out the real bound.
    #[cfg(all(test, feature = "proof"))]
    pub(crate) fn waiting(self, wait: Duration) -> Self {
        Self { wait, ..self }
    }

    /// This store, holding at most `ceiling` bytes, so a test can fill it.
    #[cfg(all(test, feature = "proof"))]
    pub(crate) fn bounded(self, ceiling: u64) -> Self {
        Self { ceiling, ..self }
    }

    /// This store, whose commits write `ready` once they have renamed their
    /// staging into place and then wait to be killed.
    #[cfg(all(test, feature = "proof"))]
    pub(crate) fn pausing(self, ready: PathBuf) -> Self {
        Self {
            pause: Some(ready),
            ..self
        }
    }

    /// Where `version` of the agent `key` is kept.
    #[must_use]
    pub fn directory(&self, key: &AgentKey, version: &Version) -> PathBuf {
        self.root.join(key.as_str()).join(version.name())
    }

    /// The version of the agent `key` that `executable` lies in, where it lies
    /// in one, as its path is written or once both it and the agent's
    /// directory are resolved.
    #[must_use]
    pub fn version_of(&self, key: &AgentKey, executable: &Path) -> Option<Version> {
        let agent = self.root.join(key.as_str());
        within(executable, &agent).or_else(|| {
            within(
                &fs::canonicalize(executable).ok()?,
                &fs::canonicalize(&agent).ok()?,
            )
        })
    }

    /// Whether `target` can be installed: each artifact no larger than the
    /// store, downloaded, decompressed or unpacked, the release no larger
    /// either, and room for it beside what the receipts kept account for.
    /// Writes nothing, so it is asked before anything is downloaded.
    ///
    /// # Errors
    ///
    /// [`StoreError::TooLarge`] or [`StoreError::Full`].
    pub fn fits(&self, target: &Target) -> Result<(), StoreError> {
        let too_large = || StoreError::TooLarge {
            ceiling: self.ceiling,
        };
        let mut wanted: u64 = 0;
        for artifact in &target.artifacts {
            let downloaded = artifact
                .replaced_by
                .as_ref()
                .map_or(artifact.size, |asset| asset.size);
            let sizes = [
                artifact.size,
                downloaded,
                artifact.unpacked,
                artifact.decompressed,
            ];
            if sizes.iter().any(|bytes| *bytes > self.ceiling) {
                return Err(too_large());
            }
            wanted = wanted
                .checked_add(artifact.unpacked)
                .filter(|wanted| *wanted <= self.ceiling)
                .ok_or_else(too_large)?;
        }
        self.room(None, wanted)
    }

    /// A new staging directory for the agent `key`, once what a dead process
    /// left in staging is removed.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the key makes no directory the store can hold, the
    /// store is held past the wait, or the directory could not be made.
    pub fn stage(&self, key: &AgentKey) -> Result<Staging, StoreError> {
        let change = self.change(key)?;
        for _ in 0..ATTEMPTS {
            let name = format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            );
            let directory = change.area.join(&name);
            let lock_path = change.area.join(format!("{name}{LOCK}"));
            if exists(&lock_path) {
                continue;
            }
            // Made, not found: a name somebody else made first is passed by.
            match fs::create_dir(&directory) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => return Err(at(&directory, source)),
            }
            let lock = crucible_privacy::lock(&lock_path).map_err(privacy(&lock_path))?;
            let staging = Staging {
                key: key.clone(),
                directory,
                lock: Some(lock),
                lock_path,
                committed: false,
            };
            let held = staging
                .lock
                .as_ref()
                .map(crucible_privacy::try_lock_identity)
                .transpose()
                .map_err(privacy(&staging.lock_path))?;
            if held != Some(true) {
                continue;
            }
            crucible_privacy::directory(&staging.directory).map_err(privacy(&staging.directory))?;
            return Ok(staging);
        }
        Err(at(
            &self.root.join(key.as_str()).join(STAGING),
            io::ErrorKind::AlreadyExists.into(),
        ))
    }

    /// Commits `staging`, which holds exactly `files`, as `version` of its
    /// agent: renames it into place, replacing a version there that no process
    /// holds, and then writes its receipt from `files` and `provenance`.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the store is held past the wait, the files take a
    /// name the store keeps or are not what staging holds, the release is
    /// larger than the store or its room, the version there is held, or the
    /// file system refused. Nothing is renamed into place on a refusal before
    /// the rename, and staging is removed.
    pub fn commit(
        &self,
        mut staging: Staging,
        version: &Version,
        files: &[File],
        provenance: Provenance,
    ) -> Result<Receipt, StoreError> {
        let key = staging.key.clone();
        let change = self.change(&key)?;
        reserved(files)?;
        if let Some(own) = OWN.iter().find(|own| exists(&staging.directory.join(own))) {
            return Err(StoreError::Reserved {
                name: (*own).to_owned(),
            });
        }
        let wanted = files
            .iter()
            .try_fold(0_u64, |total, file| match file.kind {
                FileKind::File { size, .. } => total.checked_add(size),
                FileKind::Dir | FileKind::Link { .. } => Some(total),
            })
            .filter(|wanted| *wanted <= self.ceiling)
            .ok_or(StoreError::TooLarge {
                ceiling: self.ceiling,
            })?;
        self.room(Some((&key, version)), wanted)?;
        if let Some(first) = compared(&staging.directory, files, Contents::Unread)
            .into_iter()
            .next()
        {
            return Err(StoreError::Unrecorded(first));
        }
        let receipt = Receipt {
            key,
            version: version.text.clone(),
            lock_revision: version.lock_revision,
            listing: provenance.listing,
            fetched: whole(provenance.fetched),
            origin: Origin::Fetched,
            artifacts: provenance.artifacts,
            files: files.to_vec(),
        };
        let bytes = receipt.encoded()?;
        let destination = change.directory.join(version.name());
        let aside = set_aside(&change.area, &destination)?;
        if let Err(source) = fs::rename(&staging.directory, &destination) {
            if let Some(aside) = &aside {
                let _ = fs::rename(aside, &destination);
            }
            return Err(at(&destination, source));
        }
        staging.committed = true;
        crucible_privacy::sync_parent(&destination).map_err(privacy(&destination))?;
        #[cfg(all(test, feature = "proof"))]
        self.paused();
        recorded(&destination, &bytes)?;
        if let Some(aside) = aside {
            let _ = fs::remove_dir_all(aside);
        }
        Ok(receipt)
    }

    /// The receipt of `version` of the agent `key`, or nothing where it has
    /// none, or is not there as a directory of its own: a link where a
    /// version goes is not one.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the receipt is larger than is read, not one, in
    /// another format, or names another release, or could not be read.
    pub fn receipt(
        &self,
        key: &AgentKey,
        version: &Version,
    ) -> Result<Option<Receipt>, StoreError> {
        let directory = self.directory(key, version);
        if !is_directory(&directory) {
            return Ok(None);
        }
        let path = directory.join(RECEIPT);
        let opened = match crucible_privacy::open_read(&path) {
            Ok(opened) => opened,
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(problem) => return Err(privacy(&path)(problem)),
        };
        let mut bytes = Vec::new();
        opened
            .take(
                u64::try_from(RECEIPT_BYTES)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_to_end(&mut bytes)
            .map_err(|source| at(&path, source))?;
        if bytes.len() > RECEIPT_BYTES {
            return Err(StoreError::Oversize);
        }
        let receipt = decoded(&bytes)?;
        let same = receipt.key == *key
            && receipt.version == version.text
            && receipt.lock_revision == version.lock_revision;
        if same {
            Ok(Some(receipt))
        } else {
            Err(StoreError::Corrupt)
        }
    }

    /// Hashes every file `version` of the agent `key` holds against its
    /// receipt, and names each way it differs; nothing where it is as
    /// recorded.
    ///
    /// # Errors
    ///
    /// [`StoreError::NotInstalled`], [`StoreError::Unreceipted`], or why its
    /// receipt could not be read.
    pub fn verify(&self, key: &AgentKey, version: &Version) -> Result<Vec<Mismatch>, StoreError> {
        let directory = self.directory(key, version);
        if !is_directory(&directory) {
            return Err(StoreError::NotInstalled);
        }
        let receipt = self.receipt(key, version)?.ok_or(StoreError::Unreceipted)?;
        Ok(compared(&directory, &receipt.files, Contents::Hashed))
    }

    /// Adopts the version `pin` names, installed before the store kept
    /// receipts, by hashing its files against the list `pin` gives, and
    /// writes its receipt, adopted at `at`, with what `listing` says, when
    /// they match. A version that already has a receipt keeps it.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the version is not there, the store is held past
    /// the wait, the list is a catalog asset or takes a name the store keeps,
    /// or the receipt could not be read or written.
    pub fn adopt(
        &self,
        pin: &OwnedPin,
        listing: Listing,
        at: SystemTime,
    ) -> Result<Adoption, StoreError> {
        let version = Version::new(&pin.version, pin.lock_revision)?;
        let files = listed(&pin.target)?;
        reserved(&files)?;
        let change = self.change(&pin.key)?;
        let directory = change.directory.join(version.name());
        if !is_directory(&directory) {
            return Err(StoreError::NotInstalled);
        }
        if let Some(receipt) = self.receipt(&pin.key, &version)? {
            return Ok(Adoption::Adopted(receipt));
        }
        let mismatches = compared(&directory, &files, Contents::Hashed);
        if !mismatches.is_empty() {
            return Ok(Adoption::CheckNeeded(mismatches));
        }
        // Kept from now on as a version this store made, only its owner may
        // reach it.
        crucible_privacy::directory(&directory).map_err(privacy(&directory))?;
        let receipt = Receipt {
            key: pin.key.clone(),
            version: version.text.clone(),
            lock_revision: version.lock_revision,
            listing,
            fetched: whole(at),
            origin: Origin::Adopted,
            artifacts: pin
                .target
                .artifacts
                .iter()
                .map(|artifact| Fetched {
                    url: artifact.url.clone(),
                    hosts: Vec::new(),
                    size: artifact.size,
                    sha256: artifact.sha256.clone(),
                })
                .collect(),
            files,
        };
        recorded(&directory, &receipt.encoded()?)?;
        Ok(Adoption::Adopted(receipt))
    }

    /// Holds `version` of the agent `key` for a run, so that no prune or
    /// commit takes it, until the value given is dropped.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the version is not there, the store is held past
    /// the wait, or the hold could not be made.
    pub fn hold(&self, key: &AgentKey, version: &Version) -> Result<InUse, StoreError> {
        let agent = self.root.join(key.as_str());
        if !is_directory(&agent) {
            return Err(StoreError::NotInstalled);
        }
        let _lock = self.locked(&agent)?;
        let directory = agent.join(version.name());
        if !is_directory(&directory) {
            return Err(StoreError::NotInstalled);
        }
        let holders = directory.join(IN_USE);
        crucible_privacy::directory(&holders).map_err(privacy(&holders))?;
        for _ in 0..ATTEMPTS {
            let path = holders.join(format!(
                "{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let file = crucible_privacy::lock(&path).map_err(privacy(&path))?;
            if crucible_privacy::try_lock_identity(&file).map_err(privacy(&path))? {
                return Ok(InUse {
                    file: Some(file),
                    path,
                });
            }
        }
        Err(at(&holders, io::ErrorKind::AlreadyExists.into()))
    }

    /// Removes every version of the agent `key` that none of `executables`
    /// lies in, but the newest of the rest that has a receipt, and any a
    /// process holds; and the holders whose process has ended.
    ///
    /// # Errors
    ///
    /// [`StoreError`] when the store is held past the wait, holds more
    /// versions than are read, or a version could not be removed; those
    /// removed before stay removed.
    pub fn prune(&self, key: &AgentKey, executables: &[&Path]) -> Result<Pruned, StoreError> {
        let change = self.change(key)?;
        let named: Vec<Version> = executables
            .iter()
            .filter_map(|executable| self.version_of(key, executable))
            .collect();
        let versions = self.versions(key, &change.directory)?;
        let previous = versions
            .iter()
            .filter(|(version, _)| !named.contains(version))
            .filter_map(|(version, receipt)| Some((version, receipt.as_ref()?.fetched)))
            .max_by(|(one, at_one), (other, at_other)| {
                at_one.cmp(at_other).then_with(|| one.name.cmp(&other.name))
            })
            .map(|(version, _)| version.clone());
        let mut pruned = Pruned::default();
        for (version, _) in versions {
            let directory = change.directory.join(version.name());
            let live = swept(&directory);
            if named.contains(&version) || previous.as_ref() == Some(&version) {
                continue;
            }
            if live {
                pruned.held.push(version);
                continue;
            }
            let aside = change.area.join(format!(
                "removed-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::rename(&directory, &aside).map_err(|source| at(&directory, source))?;
            let _ = fs::remove_dir_all(&aside);
            pruned.removed.push(version);
        }
        Ok(pruned)
    }

    /// Writes where the test that set it waits, and waits to be killed.
    #[cfg(all(test, feature = "proof"))]
    fn paused(&self) {
        if let Some(ready) = &self.pause {
            let _ = fs::write(ready, b"renamed\n");
            loop {
                std::thread::park();
            }
        }
    }

    /// A change to the agent `key`'s store: its lock, once the directory is
    /// one the store can make and what a dead process left in staging is
    /// removed.
    fn change(&self, key: &AgentKey) -> Result<Change, StoreError> {
        holdable(key.as_str())?;
        let directory = self.root.join(key.as_str());
        crucible_privacy::directory(&directory).map_err(privacy(&directory))?;
        let lock = self.locked(&directory)?;
        let area = directory.join(STAGING);
        crucible_privacy::directory(&area).map_err(privacy(&area))?;
        cleaned(&area);
        Ok(Change {
            directory,
            area,
            _lock: lock,
        })
    }

    /// The lock on the store in the agent's `directory`, waited for at most
    /// the store's wait.
    fn locked(&self, directory: &Path) -> Result<Locked, StoreError> {
        let path = directory.join(LOCK);
        let file = crucible_privacy::lock(&path).map_err(privacy(&path))?;
        let started = Instant::now();
        loop {
            if crucible_privacy::try_lock_identity(&file).map_err(privacy(&path))? {
                return Ok(Locked(file));
            }
            if started.elapsed() >= self.wait {
                return Err(StoreError::Contended);
            }
            std::thread::sleep(LOCK_POLL);
        }
    }

    /// The versions of the agent `key` in its `directory`, by name, each with
    /// its receipt where it has one that can be read.
    fn versions(
        &self,
        key: &AgentKey,
        directory: &Path,
    ) -> Result<Vec<(Version, Option<Receipt>)>, StoreError> {
        let mut versions = Vec::new();
        let entries = match fs::read_dir(directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(versions),
            Err(source) => return Err(at(directory, source)),
        };
        for (index, entry) in entries.enumerate() {
            if index >= ENTRIES {
                return Err(StoreError::Crowded {
                    path: directory.to_owned(),
                });
            }
            let entry = entry.map_err(|source| at(directory, source))?;
            let Some(version) = entry.file_name().to_str().and_then(Version::parse) else {
                continue;
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let receipt = self.receipt(key, &version).ok().flatten();
            versions.push((version, receipt));
        }
        versions.sort_by(|(one, _), (other, _)| one.name.cmp(&other.name));
        Ok(versions)
    }

    /// What the receipts in the store account for, but `except`'s.
    fn used(&self, except: Option<(&AgentKey, &Version)>) -> u64 {
        let Ok(agents) = fs::read_dir(&self.root) else {
            return 0;
        };
        let mut total: u64 = 0;
        for entry in agents.take(ENTRIES).flatten() {
            let Some(key) = entry
                .file_name()
                .to_str()
                .and_then(|name| AgentKey::new(name).ok())
            else {
                continue;
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(versions) = self.versions(&key, &entry.path()) else {
                continue;
            };
            for (version, receipt) in versions {
                if except.is_some_and(|(agent, kept)| *agent == key && *kept == version) {
                    continue;
                }
                total = total.saturating_add(receipt.map_or(0, |receipt| receipt.unpacked()));
            }
        }
        total
    }

    /// Whether `wanted` more bytes fit beside what the receipts in the store,
    /// but `except`'s, account for.
    fn room(&self, except: Option<(&AgentKey, &Version)>, wanted: u64) -> Result<(), StoreError> {
        if self
            .used(except)
            .checked_add(wanted)
            .is_some_and(|total| total <= self.ceiling)
        {
            Ok(())
        } else {
            Err(StoreError::Full {
                ceiling: self.ceiling,
            })
        }
    }
}

/// The version of the agent whose directory is `agent` that `path` lies in,
/// read from the path's components alone.
fn within(path: &Path, agent: &Path) -> Option<Version> {
    let mut rest = path.strip_prefix(agent).ok()?.components();
    let Some(Component::Normal(name)) = rest.next() else {
        return None;
    };
    let mut inside = false;
    for component in rest {
        // A `..` would lead somewhere the components do not say.
        if !matches!(component, Component::Normal(_)) {
            return None;
        }
        inside = true;
    }
    if !inside {
        return None;
    }
    Version::parse(name.to_str()?)
}

/// Refuses `name` where it is not one Windows can hold, by the rules the
/// unpacker holds a member's name to.
fn holdable(name: &str) -> Result<(), StoreError> {
    let why = if name.is_empty() {
        "is empty"
    } else if name.len() > COMPONENT_BYTES {
        "is longer than a directory's name may be"
    } else if !name.bytes().all(|byte| matches!(byte, b' '..=b'~')) {
        "is not printable ASCII, and a name that is not can be spelled another way that reads alike"
    } else if name.contains(['/', '\\', ':']) || name.contains(WINDOWS_REFUSES) {
        "holds a character Windows refuses in a name"
    } else if name.ends_with(['.', ' ']) {
        "ends in a dot or a space, which Windows drops"
    } else if reserved_device(name) {
        "names a device Windows reserves"
    } else {
        return Ok(());
    };
    Err(StoreError::Name {
        name: shown(name),
        why,
    })
}

/// Whether Windows takes `name` for a device, in any case and whatever
/// follows it after a dot, with or without spaces before that.
fn reserved_device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(' ')
        .to_ascii_lowercase();
    DEVICES.contains(&stem.as_str())
        || ["com", "lpt"].iter().any(|device| {
            stem.strip_prefix(device)
                .is_some_and(|digit| digit.len() == 1 && digit.bytes().all(|b| b.is_ascii_digit()))
        })
}

/// Refuses `files` where one of them is, or is under, a name the store keeps
/// at the top of a version, in any case.
fn reserved(files: &[File]) -> Result<(), StoreError> {
    for file in files {
        let top = file
            .path
            .split('/')
            .next()
            .unwrap_or(&file.path)
            .to_ascii_lowercase();
        if OWN.contains(&top.as_str()) {
            return Err(StoreError::Reserved {
                name: shown(&file.path),
            });
        }
    }
    Ok(())
}

/// Every file the artifacts of `target` list, where each lists them itself.
fn listed(target: &Target) -> Result<Vec<File>, StoreError> {
    let mut files = Vec::new();
    for artifact in &target.artifacts {
        match &artifact.files {
            Files::Listed(listed) => files.extend(listed.iter().cloned()),
            Files::Asset(_) => return Err(StoreError::Asset),
        }
    }
    Ok(files)
}

/// Moves the version at `destination` into `area` to be removed, unless a
/// process holds it; nothing where nothing is there.
fn set_aside(area: &Path, destination: &Path) -> Result<Option<PathBuf>, StoreError> {
    match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(at(destination, source)),
        Ok(metadata) if !metadata.is_dir() => Err(StoreError::InTheWay {
            path: destination.to_owned(),
        }),
        Ok(_) => {
            if swept(destination) {
                return Err(StoreError::InUse);
            }
            let aside = area.join(format!(
                "replaced-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::rename(destination, &aside).map_err(|source| at(destination, source))?;
            Ok(Some(aside))
        }
    }
}

/// Removes the holders of the version in `directory` whose process has ended,
/// and says whether any is left: one that is held, or that cannot be shown
/// not to be.
fn swept(directory: &Path) -> bool {
    let holders = directory.join(IN_USE);
    let entries = match fs::read_dir(&holders) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return false,
        Err(_) => return true,
    };
    let mut live = false;
    for (index, entry) in entries.enumerate() {
        if index >= ENTRIES {
            return true;
        }
        let Ok(entry) = entry else {
            live = true;
            continue;
        };
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_file()) && !held(&path) {
            let _ = fs::remove_file(&path);
        } else {
            live = true;
        }
    }
    live
}

/// Whether the lock on the file at `path` is held by a live process. One that
/// cannot be opened or asked is taken to be held, so that nothing is removed
/// on a guess.
fn held(path: &Path) -> bool {
    let Ok(file) = crucible_privacy::lock(path) else {
        return true;
    };
    match crucible_privacy::try_lock_identity(&file) {
        Ok(true) => {
            let _ = crucible_privacy::unlock_identity(&file);
            false
        }
        Ok(false) | Err(_) => true,
    }
}

/// Removes what a dead process left in the staging directory `area`: every
/// entry but the live staging directories and their locks.
fn cleaned(area: &Path) {
    let Ok(entries) = fs::read_dir(area) else {
        return;
    };
    let names: Vec<_> = entries
        .take(ENTRIES)
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    let live: HashSet<String> = names
        .iter()
        .filter_map(|name| {
            let stem = name.to_str()?.strip_suffix(LOCK)?;
            held(&area.join(name)).then(|| stem.to_owned())
        })
        .collect();
    for name in &names {
        let text = name.to_string_lossy();
        if live.contains(text.strip_suffix(LOCK).unwrap_or(&text)) {
            continue;
        }
        let path = area.join(name);
        let _ = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&path),
            _ => fs::remove_file(&path),
        };
    }
}

/// Writes `bytes` as the receipt of the version in `directory`, through a
/// draft beside it that replaces it once whole, durably, or leaves it as it
/// was. Only a change under the store's lock writes one, so a draft found is
/// a dead writer's.
fn recorded(directory: &Path, bytes: &[u8]) -> Result<(), StoreError> {
    let draft = directory.join(DRAFT);
    match fs::remove_file(&draft) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(source) => return Err(at(&draft, source)),
    }
    let mut file = crucible_privacy::create_write(&draft).map_err(privacy(&draft))?;
    let written = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    let receipt = directory.join(RECEIPT);
    let landed = written
        .and_then(|()| crucible_privacy::replace(&draft, &receipt).map_err(PrivacyError::into_io));
    if let Err(source) = landed {
        let _ = fs::remove_file(&draft);
        return Err(at(&receipt, source));
    }
    Ok(())
}

/// The receipt `bytes` hold, or why they are not one of this format.
fn decoded(bytes: &[u8]) -> Result<Receipt, StoreError> {
    let document: Value = serde_json::from_slice(bytes).map_err(|_| StoreError::Corrupt)?;
    let fields = document.as_object().ok_or(StoreError::Corrupt)?;
    match fields.get("format").and_then(Value::as_u64) {
        Some(FORMAT) => {}
        Some(other) => return Err(StoreError::Format(other)),
        None => return Err(StoreError::Corrupt),
    }
    read_receipt(fields).ok_or(StoreError::Corrupt)
}

/// The receipt the object `fields`, which is in this format, holds.
fn read_receipt(fields: &Map<String, Value>) -> Option<Receipt> {
    let text = |name: &str| fields.get(name)?.as_str();
    let terms = match fields.get("terms")? {
        Value::Null => None,
        terms => Some(terms.as_str()?.into()),
    };
    Some(Receipt {
        key: AgentKey::new(text("key")?).ok()?,
        version: text("version")?.into(),
        lock_revision: match fields.get("lock_revision")? {
            Value::Null => None,
            revision => Some(revision.as_u64()?),
        },
        listing: Listing {
            serial: fields.get("serial")?.as_u64()?,
            review: text("review")?.into(),
            terms,
        },
        fetched: UNIX_EPOCH.checked_add(Duration::from_secs(fields.get("fetched")?.as_u64()?))?,
        origin: match text("origin")? {
            "fetched" => Origin::Fetched,
            "adopted" => Origin::Adopted,
            _ => return None,
        },
        artifacts: fields
            .get("artifacts")?
            .as_array()?
            .iter()
            .map(read_fetched)
            .collect::<Option<_>>()?,
        files: fields
            .get("files")?
            .as_array()?
            .iter()
            .map(read_file)
            .collect::<Option<_>>()?,
    })
}

/// The artifact `value` records.
fn read_fetched(value: &Value) -> Option<Fetched> {
    let fields = value.as_object()?;
    Some(Fetched {
        url: fields.get("url")?.as_str()?.into(),
        hosts: fields
            .get("hosts")?
            .as_array()?
            .iter()
            .map(|host| host.as_str().map(Into::into))
            .collect::<Option<_>>()?,
        size: fields.get("size")?.as_u64()?,
        sha256: sha256(fields.get("sha256")?.as_str()?)?,
    })
}

/// The file `value` records, as an artifact's `files` spells one, once its
/// path stays inside the version and its link target is relative.
fn read_file(value: &Value) -> Option<File> {
    let fields = value.as_object()?;
    let path = fields.get("path")?.as_str()?;
    let inside = !path.is_empty()
        && path.len() <= PATH_BYTES
        && !path.contains(['\\', ':', '\0'])
        && path.split('/').all(|part| !matches!(part, "" | "." | ".."));
    if !inside {
        return None;
    }
    let kind = match fields.get("kind")?.as_str()? {
        "file" => FileKind::File {
            size: fields.get("size")?.as_u64()?,
            sha256: sha256(fields.get("sha256")?.as_str()?)?,
            exec: fields.get("exec")?.as_bool()?,
        },
        "dir" => FileKind::Dir,
        "link" => {
            let target = fields.get("target")?.as_str()?;
            if target.is_empty()
                || target.len() > PATH_BYTES
                || target.starts_with('/')
                || target.contains(['\\', ':', '\0'])
            {
                return None;
            }
            FileKind::Link {
                target: target.into(),
            }
        }
        _ => return None,
    };
    Some(File {
        path: path.into(),
        kind,
    })
}

/// `text`, once it is a SHA-256 in lowercase hexadecimal.
fn sha256(text: &str) -> Option<Box<str>> {
    (text.len() == 64
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)))
    .then(|| text.into())
}

/// Every way the version in `root` differs from `files`: each file as it is
/// listed, by `contents`, and then what it holds that is not listed, beside
/// the store's own names at its top.
fn compared(root: &Path, files: &[File], contents: Contents) -> Vec<Mismatch> {
    let mut directories = HashSet::from([String::new()]);
    let mut found: Vec<Mismatch> = files
        .iter()
        .filter_map(|file| {
            examined(root, file, contents, &mut directories).map(|what| Mismatch {
                path: file.path.clone(),
                what,
            })
        })
        .collect();
    found.extend(unlisted(root, files));
    found
}

/// How the file `file` lists differs from what is at its path under `root`,
/// reached through directories alone, which `directories` remembers once
/// found.
fn examined(
    root: &Path,
    file: &File,
    contents: Contents,
    directories: &mut HashSet<String>,
) -> Option<Differs> {
    let mut above = String::new();
    let mut parts = file.path.split('/').peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        if !above.is_empty() {
            above.push('/');
        }
        above.push_str(part);
        if !directories.contains(&above) {
            if !is_directory(&located(root, &above)) {
                return Some(Differs::Missing);
            }
            directories.insert(above.clone());
        }
    }
    let path = located(root, &file.path);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Some(Differs::Missing),
        Err(_) => return Some(Differs::Unreadable),
    };
    match &file.kind {
        FileKind::Dir => (!metadata.is_dir()).then_some(Differs::Kind),
        FileKind::Link { target } => {
            if !metadata.file_type().is_symlink() {
                return Some(Differs::Kind);
            }
            match fs::read_link(&path) {
                Ok(found) if linked(&found) == **target => None,
                Ok(_) => Some(Differs::Target),
                Err(_) => Some(Differs::Unreadable),
            }
        }
        FileKind::File { size, sha256, exec } => {
            if !metadata.is_file() {
                return Some(Differs::Kind);
            }
            if metadata.len() != *size {
                return Some(Differs::Size);
            }
            if runnable(&metadata).is_some_and(|runs| runs != *exec) {
                return Some(Differs::Exec);
            }
            if contents == Contents::Unread {
                return None;
            }
            match hashed(&path, *size) {
                Ok(found) if *found == **sha256 => None,
                Ok(_) => Some(Differs::Bytes),
                Err(_) => Some(Differs::Unreadable),
            }
        }
    }
}

/// What the version in `root` holds that `files` does not list, at most
/// [`UNLISTED`] of it, by path: every entry of a directory listed or above a
/// file listed but the store's own names at its top, looked into through
/// directories alone.
fn unlisted(root: &Path, files: &[File]) -> Vec<Mismatch> {
    let mut expected: HashSet<&str> = HashSet::new();
    for file in files {
        let mut path: &str = &file.path;
        expected.insert(path);
        while let Some((above, _)) = path.rsplit_once('/') {
            expected.insert(above);
            path = above;
        }
    }
    let mut found = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(located(root, &directory)) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if directory.is_empty() && OWN.contains(&name.as_str()) {
                continue;
            }
            let path = if directory.is_empty() {
                name
            } else {
                format!("{directory}/{name}")
            };
            if expected.contains(path.as_str()) {
                if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    pending.push(path);
                }
                continue;
            }
            found.push(Mismatch {
                path: path.into(),
                what: Differs::Unlisted,
            });
            if found.len() >= UNLISTED {
                break;
            }
        }
        if found.len() >= UNLISTED {
            break;
        }
    }
    found.sort_by(|one, other| one.path.cmp(&other.path));
    found
}

/// Where the relative `path` is under `root`.
fn located(root: &Path, path: &str) -> PathBuf {
    path.split('/')
        .filter(|part| !part.is_empty())
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// A link's target as a list spells it, with `/` between its components.
fn linked(target: &Path) -> String {
    let text = target.to_string_lossy();
    if cfg!(windows) {
        text.replace('\\', "/")
    } else {
        text.into_owned()
    }
}

/// Whether a file with `metadata` can be run, where the platform says; on
/// Windows every file can, so there is nothing to compare.
#[cfg(unix)]
#[allow(clippy::unnecessary_wraps)]
fn runnable(metadata: &fs::Metadata) -> Option<bool> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn runnable(_metadata: &fs::Metadata) -> Option<bool> {
    None
}

/// The SHA-256 of what the ordinary file at `path` holds, read no further
/// than one byte past `size`.
fn hashed(path: &Path, size: u64) -> io::Result<String> {
    let file = crucible_privacy::open_read_ordinary(path).map_err(PrivacyError::into_io)?;
    let mut reader = file.take(size.saturating_add(1));
    let mut digest = Sha256::new();
    let mut chunk = vec![0_u8; 1 << 16];
    loop {
        let read = match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        digest.update(chunk.get(..read).unwrap_or_default());
    }
    Ok(hex(&digest.finalize()))
}

/// `bytes` as lowercase hexadecimal.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

/// Whether anything is at `path`, a link not followed.
fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Whether `path` is a directory, and not a link to one.
fn is_directory(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

/// The whole seconds since the Unix epoch at `time`, none before it.
fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

/// `time` to the second, as a receipt keeps it.
fn whole(time: SystemTime) -> SystemTime {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds(time)))
        .unwrap_or(time)
}

/// At most [`NAMED`] characters of `name`, for a refusal to repeat.
fn shown(name: &str) -> String {
    name.chars().take(NAMED).collect()
}

/// The refusal for what the file system said at `path`.
fn at(path: &Path, source: io::Error) -> StoreError {
    StoreError::Io {
        path: path.to_owned(),
        source,
    }
}

/// The refusal for what `crucible-privacy` said at `path`.
fn privacy(path: &Path) -> impl Fn(PrivacyError) -> StoreError + '_ {
    move |problem| at(path, problem.into_io())
}

#[cfg(all(test, feature = "proof"))]
mod tests;
