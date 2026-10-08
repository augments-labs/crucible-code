//! A test build's state directory, named for the checkout it was compiled
//! from, and the reclaiming of one whose checkout is gone.
//!
//! The name is the shipped one followed by a token of the checkout's path, so
//! the path cannot be read back from it. Beside the directory stands its claim,
//! `{state}.checkout`, which holds that path. Every process of the checkout
//! takes a shared lock on the claim the first time it asks for its state and
//! keeps it until it ends, which the kernel releases however it ends.
//!
//! A test build reclaims, once per process and before claiming its own,
//! every directory beside it whose claim it can lock exclusively without
//! waiting, whose path hashes to that directory's name and names nothing that
//! stands: a removed worktree, or a test fixture whose process was killed
//! before it could clean up. A directory with no claim, such as one a build
//! made before claims were written, or the shipped directory, is never
//! touched, since its name alone proves nothing about who uses it. A claim
//! left empty for more than a day, by a process that ended before writing it,
//! is removed alone, since it names no checkout whose state could go with it.

use std::fs::{self, File};
use std::io::{self, Read as _};
use std::os::unix::fs::{FileExt as _, MetadataExt as _};
use std::path::{Path, PathBuf};

use rustix::fs::{FlockOperation, Mode, OFlags};
use sha2::{Digest as _, Sha256};

/// What follows a state directory's name in the name of its claim.
pub(super) const CLAIM_SUFFIX: &str = ".checkout";

/// What follows a state directory's name in the name of the lock a test takes
/// while it holds that directory changed, or reads it.
pub(super) const STATE_LOCK_SUFFIX: &str = ".test-state.lock";

/// What follows a state directory's name in the name of the lock a test takes
/// while it writes through that directory.
pub(super) const WRITERS_LOCK_SUFFIX: &str = ".test-writers.lock";

/// Every lock a test build keeps beside a state directory, removed with it.
pub(super) const TEST_LOCK_SUFFIXES: [&str; 2] = [STATE_LOCK_SUFFIX, WRITERS_LOCK_SUFFIX];

/// How many hexadecimal digits a checkout's token has.
const TOKEN_DIGITS: usize = 16;

/// The longest checkout path a claim is read for; longer is no claim.
const MAX_CLAIM_BYTES: u64 = 4096;

/// How old an empty claim must be before it is taken for one whose claimer
/// ended before writing it.
const EMPTY_CLAIM_AGE: std::time::Duration = std::time::Duration::from_hours(24);

/// How often a claim is opened again after a reclaim removed the one opened.
const CLAIM_ATTEMPTS: u32 = 8;

/// `shipped` followed by a fixed-width token of `checkout`: the first eight
/// bytes of its SHA-256, in hexadecimal, so a path of any length or spelling
/// makes a name of one length and one alphabet.
pub(super) fn checkout_state_name(shipped: &str, checkout: &str) -> String {
    use std::fmt::Write as _;

    let mut name = format!("{shipped}-");
    for byte in Sha256::digest(checkout.as_bytes())
        .iter()
        .take(TOKEN_DIGITS / 2)
    {
        // Writing to a string cannot fail, and a name that lost a byte would
        // be another checkout's.
        let _ = write!(name, "{byte:02x}");
    }
    name
}

/// A held claim on one checkout's state directory: while it is held, no
/// reclaim removes that directory.
pub(super) struct Claim {
    state: PathBuf,
    /// The open claim, whose shared lock is what keeps reclaims away.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "outside tests the claim is only ever held")
    )]
    marker: File,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Claim")
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

impl Claim {
    /// The state directory claimed.
    #[cfg(test)]
    pub(super) fn state(&self) -> &Path {
        &self.state
    }

    /// Removes the claim, once its state directory has been removed by
    /// whoever claimed it.
    #[cfg(test)]
    pub(super) fn remove(self) -> io::Result<()> {
        let path = claim_path(&self.state);
        if still_named(&path, &self.marker)? {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
}

/// The claim of the state directory at `state`.
fn claim_path(state: &Path) -> PathBuf {
    let mut path = state.as_os_str().to_owned();
    path.push(CLAIM_SUFFIX);
    PathBuf::from(path)
}

/// Claims the state directory of `checkout` under `base`, recording its path
/// for a later reclaim, and holds the claim until it is dropped.
pub(super) fn claim(base: &Path, shipped: &str, checkout: &str) -> io::Result<Claim> {
    let state = base.join(checkout_state_name(shipped, checkout));
    let path = claim_path(&state);
    for _ in 0..CLAIM_ATTEMPTS {
        let marker = open_claim(&path, true)?;
        rustix::fs::flock(&marker, FlockOperation::LockShared)?;
        // A reclaim that held the claim while this waited has removed it.
        if !still_named(&path, &marker)? {
            continue;
        }
        let recorded = read_claim(&marker)?.unwrap_or_default();
        if recorded != checkout.as_bytes() {
            // Another process of this checkout may be writing the same path.
            if !checkout.as_bytes().starts_with(&recorded) {
                return Err(io::Error::other(
                    "a sandbox state claim names another checkout",
                ));
            }
            marker.write_all_at(checkout.as_bytes(), 0)?;
            marker.set_len(u64::try_from(checkout.len()).unwrap_or(u64::MAX))?;
        }
        return Ok(Claim { state, marker });
    }
    Err(io::Error::other(
        "a sandbox state claim was removed each time it was opened",
    ))
}

/// Claims this checkout's state directory under `base` the first time this
/// process asks, after reclaiming every abandoned one beside it.
///
/// A claim that cannot be made leaves the directory as it was before claims
/// existed: used, and never reclaimed.
pub(super) fn claim_once(base: &Path, shipped: &str, checkout: &str) {
    static CLAIMED: std::sync::OnceLock<Option<Claim>> = std::sync::OnceLock::new();
    CLAIMED.get_or_init(|| {
        reclaim_abandoned(base, shipped);
        claim(base, shipped, checkout).ok()
    });
}

/// Removes every abandoned checkout's state under `base`, with its locks and
/// its claim, and says how many it removed.
pub(super) fn reclaim_abandoned(base: &Path, shipped: &str) -> usize {
    let Ok(entries) = fs::read_dir(base) else {
        return 0;
    };
    let prefix = format!("{shipped}-");
    let mut reclaimed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(state) = name
            .to_str()
            .and_then(|name| name.strip_suffix(CLAIM_SUFFIX))
        else {
            continue;
        };
        let is_checkout_state = state.strip_prefix(&prefix).is_some_and(|token| {
            token.len() == TOKEN_DIGITS
                && token
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        });
        if is_checkout_state && reclaim(base, state, shipped).unwrap_or(false) {
            reclaimed += 1;
        }
    }
    reclaimed
}

/// Removes the state named `state` under `base` if its claim proves it
/// abandoned, and says whether it did.
fn reclaim(base: &Path, state: &str, shipped: &str) -> io::Result<bool> {
    let directory = base.join(state);
    let path = claim_path(&directory);
    let marker = open_claim(&path, false)?;
    match rustix::fs::flock(&marker, FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => {}
        Err(rustix::io::Errno::WOULDBLOCK) => return Ok(false),
        Err(problem) => return Err(problem.into()),
    }
    if !still_named(&path, &marker)? {
        return Ok(false);
    }
    let Some(checkout) = read_claim(&marker)?.and_then(|bytes| String::from_utf8(bytes).ok())
    else {
        return Ok(false);
    };
    if checkout.is_empty() {
        // A process that ended between creating its claim and writing it left
        // one that names no checkout. Once it is old enough that no claimer
        // can still be about to write it, only the claim goes: what stands
        // beside it may be a live checkout's, whose next claim writes a fresh
        // one.
        if abandoned_empty(&marker)? {
            fs::remove_file(&path)?;
        }
        return Ok(false);
    }
    if checkout_state_name(shipped, &checkout) != state {
        return Ok(false);
    }
    match fs::symlink_metadata(&checkout) {
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => {}
        _ => return Ok(false),
    }
    match fs::symlink_metadata(&directory) {
        Ok(found) if found.is_dir() && found.uid() == rustix::process::getuid().as_raw() => {
            fs::remove_dir_all(&directory)?;
        }
        Ok(_) => return Ok(false),
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => {}
        Err(problem) => return Err(problem),
    }
    for suffix in TEST_LOCK_SUFFIXES {
        let mut lock = directory.as_os_str().to_owned();
        lock.push(suffix);
        match fs::remove_file(&lock) {
            Ok(()) => {}
            // A lock this user may not remove is left; the claim still goes,
            // so the state is not examined again on every run.
            Err(problem)
                if matches!(
                    problem.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::PermissionDenied
                ) => {}
            Err(problem) => return Err(problem),
        }
    }
    // Last, and while still held: a claimer waiting on it finds it unnamed and
    // opens a fresh one.
    fs::remove_file(&path)?;
    Ok(true)
}

/// Whether an empty claim was last written longer ago than
/// [`EMPTY_CLAIM_AGE`].
fn abandoned_empty(claim: &File) -> io::Result<bool> {
    let written = claim.metadata()?.modified()?;
    Ok(std::time::SystemTime::now()
        .duration_since(written)
        .is_ok_and(|age| age > EMPTY_CLAIM_AGE))
}

/// The claim at `path`, refused unless it is this user's plain file.
fn open_claim(path: &Path, create: bool) -> io::Result<File> {
    let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC;
    let flags = if create {
        flags | OFlags::CREATE
    } else {
        flags
    };
    let claim = File::from(rustix::fs::open(path, flags, Mode::RUSR | Mode::WUSR)?);
    let metadata = claim.metadata()?;
    if !metadata.is_file() || metadata.uid() != rustix::process::getuid().as_raw() {
        return Err(io::Error::other(
            "a sandbox state claim is not this user's file",
        ));
    }
    Ok(claim)
}

/// Whether `path` still names the open `claim`.
fn still_named(path: &Path, claim: &File) -> io::Result<bool> {
    let opened = claim.metadata()?;
    match fs::symlink_metadata(path) {
        Ok(named) => Ok(named.dev() == opened.dev() && named.ino() == opened.ino()),
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(problem) => Err(problem),
    }
}

/// The checkout path `claim` records, or `None` when it is longer than any
/// path a claim is written for.
fn read_claim(claim: &File) -> io::Result<Option<Vec<u8>>> {
    let mut recorded = Vec::new();
    claim.take(MAX_CLAIM_BYTES + 1).read_to_end(&mut recorded)?;
    if u64::try_from(recorded.len()).unwrap_or(u64::MAX) > MAX_CLAIM_BYTES {
        return Ok(None);
    }
    Ok(Some(recorded))
}
