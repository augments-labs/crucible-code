//! The release layout, checked against its receipt and the running
//! executable.
//!
//! What may be trusted is decided entry by entry, before an entry is read.
//! Each directory from the prefix down, the receipt and each executable must
//! be owned by root or by the user running crucible, and writable by nobody
//! else, which is the rule the broker lookup holds its image to. That is what
//! makes reading them in turn sound: nobody who is not already trusted can
//! change an entry between its check and its use, and a tree anybody else made
//! fails on its first entry, whatever led to it. The files are opened without
//! following a link and examined through the handle that is read.
//!
//! The directories above the prefix are not examined. Whoever may change one
//! of them may already replace the `crucible` link in the directory the
//! installer was given, which is what the user runs, and a prefix they put in
//! its place is refused on its first entry unless root or the user made it.
//! What holding them to the rule would still catch is a swap to another tree
//! root or the user made, which it leaves to the receipt's hashes.
//!
//! The layout's links are read rather than followed. `current` must say
//! exactly `releases/<version>`, so it cannot reach out of the prefix, and a
//! release directory, a receipt or an executable that is itself a link is
//! refused. The unit holds exactly the files its receipt accounts for.

use std::fmt;
use std::fs::{self, File, Metadata};
use std::io::{self, Read as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use super::receipt::{self, Digest, Receipt, ReceiptError, Target, Version};

/// The prefix's own name, inside the directory the installer was given.
pub(crate) const PREFIX: &str = ".crucible-install";

/// The link naming the active release.
const CURRENT: &str = "current";

/// The directory each release unit is kept under.
const RELEASES: &str = "releases";

/// The receipt's name inside its unit.
const RECEIPT: &str = "receipt";

/// The executable's name inside its unit.
const CRUCIBLE: &str = "crucible";

/// The broker's name inside its unit.
const BROKER: &str = "crucible-sandbox-broker";

/// The most an executable may hold. Hashing stops one byte past it and the
/// file is refused; a release binary is a few tens of MiB, so this only bounds
/// the time a hash can take.
const EXECUTABLE_CEILING: u64 = 256 * 1024 * 1024;

/// How much of an executable is hashed at a time.
const HASH_BUFFER: usize = 64 * 1024;

/// A managed install's active release unit, with its receipt, each checked
/// against the other and against the running executable.
#[derive(Debug, Clone)]
pub struct ReceiptLayout {
    /// The canonical prefix.
    prefix: PathBuf,
    /// The receipt of the active unit.
    receipt: Receipt,
}

/// Why a layout was not taken as a managed install.
///
/// Each variant names the entry by its place in the layout rather than by its
/// path, which is somebody's home directory.
#[derive(Debug, thiserror::Error)]
pub enum LayoutError {
    /// The executable is not `releases/<version>/crucible` under a prefix: a
    /// flat install, a package manager's copy or a build tree.
    #[error("crucible is not running from an installer-managed release")]
    Unmanaged,
    /// An entry could not be examined or read.
    #[error("could not read the install's {entry}")]
    Io {
        /// The entry.
        entry: LayoutEntry,
        /// What the operating system said.
        #[source]
        source: io::Error,
    },
    /// The prefix is reached through a link, or its path is not canonical.
    #[error("the install's prefix is not its own canonical path")]
    NotCanonical,
    /// An entry is owned by another user than root or the one running
    /// crucible.
    #[error("the install's {entry} belongs to another user")]
    Foreign {
        /// The entry.
        entry: LayoutEntry,
    },
    /// An entry may be written by its group or by anybody.
    #[error("the install's {entry} may be written by other users")]
    Writable {
        /// The entry.
        entry: LayoutEntry,
    },
    /// An entry is a link where a directory or a file belongs.
    #[error("the install's {entry} is a link")]
    Link {
        /// The entry.
        entry: LayoutEntry,
    },
    /// An entry is not the kind of entry its place holds.
    #[error("the install's {entry} is not a {kind}")]
    Kind {
        /// The entry.
        entry: LayoutEntry,
        /// What it should be.
        kind: EntryKind,
    },
    /// A file has another name than its place in the layout.
    #[error("the install's {entry} has another name as well")]
    HardLink {
        /// The entry.
        entry: LayoutEntry,
    },
    /// The executable is a release that is no longer the active one.
    #[error("crucible is running from a release that is no longer the active one")]
    Inactive,
    /// `current` names something other than one release unit.
    #[error("the install's active release is not one of its releases")]
    Current,
    /// The unit holds a file its receipt does not account for, or lacks one
    /// it does.
    #[error("the active release holds other files than its receipt names")]
    Contents,
    /// The receipt does not follow the format.
    #[error("the install's receipt is not valid")]
    Receipt(#[source] ReceiptError),
    /// The receipt says something the layout or the running build contradicts.
    #[error("the install's receipt names another {claim}")]
    Mismatch {
        /// What disagrees.
        claim: ReceiptClaim,
    },
    /// A file is larger than a release ever is.
    #[error("the install's {entry} is too large to be a release")]
    TooLarge {
        /// The entry.
        entry: LayoutEntry,
    },
    /// A file's SHA-256 is not the one its receipt records.
    #[error("the install's {entry} is not the file its receipt records")]
    Digest {
        /// The entry.
        entry: LayoutEntry,
    },
}

/// A place in the layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutEntry {
    /// The prefix, `<dir>/.crucible-install`.
    Prefix,
    /// `releases`, which holds the release units.
    Releases,
    /// `current`, the link naming the active release.
    Current,
    /// The active release unit, `releases/<version>`.
    Unit,
    /// The active unit's receipt.
    Receipt,
    /// The active unit's `crucible`.
    Executable,
    /// The active unit's `crucible-sandbox-broker`.
    Broker,
}

/// The kind of entry a place in the layout holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// A directory.
    Directory,
    /// An ordinary file.
    File,
    /// A symbolic link.
    Link,
}

/// What a receipt says that the layout or the running build can contradict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReceiptClaim {
    /// The prefix the receipt is kept under.
    Prefix,
    /// The release the unit holds.
    Release,
    /// The platform the release was built for.
    Platform,
}

impl fmt::Display for LayoutEntry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Prefix => "prefix",
            Self::Releases => "releases directory",
            Self::Current => "active-release link",
            Self::Unit => "active release",
            Self::Receipt => "receipt",
            Self::Executable => "executable",
            Self::Broker => "broker",
        })
    }
}

impl fmt::Display for EntryKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Directory => "directory",
            Self::File => "file",
            Self::Link => "link",
        })
    }
}

impl fmt::Display for ReceiptClaim {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Prefix => "prefix",
            Self::Release => "release",
            Self::Platform => "platform",
        })
    }
}

/// Whose files an install may be made of, besides root's: the user running
/// crucible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Owner(u32);

impl Owner {
    /// The user this process runs as.
    fn running() -> Self {
        Self(rustix::process::getuid().as_raw())
    }
}

impl ReceiptLayout {
    /// Takes the layout the running executable belongs to.
    ///
    /// `executable` is where the process was started from,
    /// [`std::env::current_exe`]; it may be the link in the directory the
    /// installer was given. It must resolve to `crucible` in the unit
    /// `current` names, so a process left running from a release that is no
    /// longer active is refused as well: what it would act on is not what it
    /// is.
    ///
    /// # Errors
    ///
    /// [`LayoutError`] naming the first entry that is not as a managed install
    /// leaves it.
    pub fn of_executable(executable: &Path) -> Result<Self, LayoutError> {
        Self::taken(executable, Owner::running())
    }

    /// Takes the layout `executable` belongs to, whose entries `owner` or root
    /// must own.
    fn taken(executable: &Path, owner: Owner) -> Result<Self, LayoutError> {
        let executable = fs::canonicalize(executable).map_err(io(LayoutEntry::Executable))?;
        let prefix = managed(&executable).ok_or(LayoutError::Unmanaged)?;
        let layout = Self::read(prefix, owner)?;
        if executable != layout.unit().join(CRUCIBLE) {
            return Err(LayoutError::Inactive);
        }
        Ok(layout)
    }

    /// The canonical prefix.
    pub fn prefix(&self) -> &Path {
        &self.prefix
    }

    /// The receipt of the active unit.
    pub fn receipt(&self) -> &Receipt {
        &self.receipt
    }

    /// The active unit's directory, which holds its executable and broker.
    pub fn unit(&self) -> PathBuf {
        self.prefix
            .join(RELEASES)
            .join(self.receipt.version().as_str())
    }

    /// Reads the layout under `prefix`, whose entries `owner` or root must
    /// own.
    fn read(prefix: &Path, owner: Owner) -> Result<Self, LayoutError> {
        if fs::canonicalize(prefix).map_err(io(LayoutEntry::Prefix))? != prefix {
            return Err(LayoutError::NotCanonical);
        }
        directory(prefix, LayoutEntry::Prefix, owner)?;
        let version = current(&prefix.join(CURRENT))?;
        let releases = prefix.join(RELEASES);
        directory(&releases, LayoutEntry::Releases, owner)?;
        let unit = releases.join(version.as_str());
        directory(&unit, LayoutEntry::Unit, owner)?;

        let mut bytes = Vec::new();
        file(&unit.join(RECEIPT), LayoutEntry::Receipt, owner)?
            .take(receipt::MAX_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io(LayoutEntry::Receipt))?;
        let receipt = Receipt::parse(&bytes).map_err(LayoutError::Receipt)?;
        contents(&unit, receipt.broker().is_some())?;
        if receipt.prefix().as_os_str() != prefix.as_os_str() {
            return Err(LayoutError::Mismatch {
                claim: ReceiptClaim::Prefix,
            });
        }
        if *receipt.version() != version {
            return Err(LayoutError::Mismatch {
                claim: ReceiptClaim::Release,
            });
        }
        if Target::running() != Some(receipt.target()) {
            return Err(LayoutError::Mismatch {
                claim: ReceiptClaim::Platform,
            });
        }

        for (entry, name, recorded) in [
            (LayoutEntry::Executable, CRUCIBLE, Some(receipt.crucible())),
            (LayoutEntry::Broker, BROKER, receipt.broker()),
        ] {
            let Some(recorded) = recorded else { continue };
            let opened = file(&unit.join(name), entry, owner)?;
            if digest(opened, entry, EXECUTABLE_CEILING)? != *recorded {
                return Err(LayoutError::Digest { entry });
            }
        }

        Ok(Self {
            prefix: prefix.to_path_buf(),
            receipt,
        })
    }
}

/// The prefix a canonical executable path at `<prefix>/releases/<version>/crucible`
/// names, or `None` for any other path.
fn managed(executable: &Path) -> Option<&Path> {
    let unit = executable.parent()?;
    let releases = unit.parent()?;
    let prefix = releases.parent()?;
    let shaped = executable.file_name()? == CRUCIBLE
        && releases.file_name()? == RELEASES
        && prefix.file_name()? == PREFIX;
    shaped.then_some(prefix)
}

/// The release the active-release link at `at` names, which it must spell
/// exactly `releases/<version>`.
fn current(at: &Path) -> Result<Version, LayoutError> {
    let entry = LayoutEntry::Current;
    let metadata = fs::symlink_metadata(at).map_err(io(entry))?;
    if !metadata.file_type().is_symlink() {
        return Err(LayoutError::Kind {
            entry,
            kind: EntryKind::Link,
        });
    }
    let target = fs::read_link(at).map_err(io(entry))?;
    target
        .as_os_str()
        .as_bytes()
        .strip_prefix(RELEASES.as_bytes())
        .and_then(|rest| rest.strip_prefix(b"/"))
        .and_then(Version::parse)
        .ok_or(LayoutError::Current)
}

/// Holds the directory at `at` to the layout: a directory, not a link, that
/// only `owner` or root may change.
fn directory(at: &Path, entry: LayoutEntry, owner: Owner) -> Result<(), LayoutError> {
    let metadata = fs::symlink_metadata(at).map_err(io(entry))?;
    if metadata.file_type().is_symlink() {
        return Err(LayoutError::Link { entry });
    }
    if !metadata.is_dir() {
        return Err(LayoutError::Kind {
            entry,
            kind: EntryKind::Directory,
        });
    }
    held(&metadata, entry, owner)
}

/// Opens the file at `at` once it is held to the layout: an ordinary file,
/// not a link, under one name, that only `owner` or root may change. The
/// checks are made again on the handle returned, so the file read is the one
/// that was examined.
fn file(at: &Path, entry: LayoutEntry, owner: Owner) -> Result<File, LayoutError> {
    let named = fs::symlink_metadata(at).map_err(io(entry))?;
    if named.file_type().is_symlink() {
        return Err(LayoutError::Link { entry });
    }
    ordinary(&named, entry, owner)?;
    let opened = crucible_privacy::open_read(at).map_err(|error| LayoutError::Io {
        entry,
        source: error.into_io(),
    })?;
    let metadata = opened.metadata().map_err(io(entry))?;
    if (metadata.dev(), metadata.ino()) != (named.dev(), named.ino()) {
        return Err(LayoutError::Io {
            entry,
            source: io::Error::other("the entry was replaced while it was examined"),
        });
    }
    ordinary(&metadata, entry, owner)?;
    Ok(opened)
}

/// Holds a file's metadata to the layout's rule for files.
fn ordinary(metadata: &Metadata, entry: LayoutEntry, owner: Owner) -> Result<(), LayoutError> {
    if !metadata.is_file() {
        return Err(LayoutError::Kind {
            entry,
            kind: EntryKind::File,
        });
    }
    if metadata.nlink() != 1 {
        return Err(LayoutError::HardLink { entry });
    }
    held(metadata, entry, owner)
}

/// Holds an entry to the trust rule.
fn held(metadata: &Metadata, entry: LayoutEntry, owner: Owner) -> Result<(), LayoutError> {
    if !owned(owner, metadata.uid()) {
        return Err(LayoutError::Foreign { entry });
    }
    if !trusted(owner, metadata.uid(), metadata.mode()) {
        return Err(LayoutError::Writable { entry });
    }
    Ok(())
}

/// Whether the unit at `unit` holds exactly its receipt, its executable and,
/// when `broker`, its broker. Only one entry more than that is ever read.
fn contents(unit: &Path, broker: bool) -> Result<(), LayoutError> {
    let entry = LayoutEntry::Unit;
    let mut expected = vec![RECEIPT, CRUCIBLE];
    if broker {
        expected.push(BROKER);
    }
    let mut found = Vec::new();
    for listed in fs::read_dir(unit)
        .map_err(io(entry))?
        .take(expected.len() + 1)
    {
        found.push(listed.map_err(io(entry))?.file_name());
    }
    found.sort();
    expected.sort_unstable();
    if found
        .iter()
        .map(|name| name.as_os_str().as_bytes())
        .ne(expected.iter().map(|name| name.as_bytes()))
    {
        return Err(LayoutError::Contents);
    }
    Ok(())
}

/// The SHA-256 of `file`, read to its end, which must come within `ceiling`
/// bytes.
fn digest(file: File, entry: LayoutEntry, ceiling: u64) -> Result<Digest, LayoutError> {
    let mut hasher = Sha256::new();
    let mut limited = file.take(ceiling + 1);
    let mut buffer = vec![0; HASH_BUFFER];
    loop {
        let read = match limited.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                return Err(LayoutError::Io {
                    entry,
                    source: error,
                });
            }
        };
        hasher.update(buffer.get(..read).unwrap_or_default());
    }
    if limited.limit() == 0 {
        return Err(LayoutError::TooLarge { entry });
    }
    Ok(Digest::new(hasher.finalize().into()))
}

/// Builds the error for an entry the operating system would not examine.
fn io(entry: LayoutEntry) -> impl FnOnce(io::Error) -> LayoutError {
    move |source| LayoutError::Io { entry, source }
}

/// Whether `uid` may own part of `owner`'s install.
fn owned(owner: Owner, uid: u32) -> bool {
    uid == 0 || uid == owner.0
}

/// Whether an entry `uid` owns with `mode` may be part of `owner`'s install.
fn trusted(owner: Owner, uid: u32, mode: u32) -> bool {
    owned(owner, uid) && mode & 0o022 == 0
}

#[cfg(test)]
mod tests;
