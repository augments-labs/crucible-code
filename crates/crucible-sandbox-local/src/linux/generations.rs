//! Which publications have touched a writable root, for commands that ran across
//! one.
//!
//! A command sees its root through an overlay, so another command's publication
//! into that root shows in what this command is scanned as holding. Where the
//! root ends up as this command found it — a publication that rolled back, or
//! one whose writes were written over again — comparing the root with the
//! command's baseline cannot see that anything happened, and the difference the
//! scan carries is published as this command's own work.
//!
//! A generation is the witness the comparison lacks. It moves when a publication
//! is about to write into a root and never moves back, so a command that took
//! its baseline before and publishes after can tell. It is kept beside the lock
//! that orders publications, and only ever read or moved while that lock is
//! held.
//!
//! What it holds is a hash of each root's destination, never the pathname: this
//! user's state directory is private, but a file of workspace paths is a
//! description of the machine that nothing here needs.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

/// The file the generations live in, beside the lock that orders publications.
const FILE: &str = "publications";

/// How many roots are remembered.
///
/// A root that falls out of the file reads as one whose generation cannot be
/// known, which refuses a publication rather than allowing one: eviction can
/// cost a command its writes, never let the wrong ones through.
const MOST: usize = 1024;

/// What a root is remembered under.
pub(super) fn key(destination: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt as _;

    let digest: [u8; 32] = Sha256::digest(destination.as_os_str().as_bytes()).into();
    let mut key = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a string cannot fail, and a key that lost a byte would
        // read as another root's.
        let _ = write!(key, "{byte:02x}");
    }
    key
}

fn path(state: &Path) -> PathBuf {
    state.join(FILE)
}

/// Every generation this user's state directory remembers.
fn read(state: &Path) -> io::Result<BTreeMap<String, u64>> {
    let text = match fs::read_to_string(path(state)) {
        Ok(text) => text,
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(problem) => return Err(problem),
    };
    let mut generations = BTreeMap::new();
    for line in text.lines() {
        let Some((remembered, counter)) = line.split_once(' ') else {
            continue;
        };
        let Ok(counter) = counter.parse() else {
            continue;
        };
        generations.insert(remembered.to_owned(), counter);
    }
    Ok(generations)
}

/// What `keys` stand at, `None` for one this file does not remember.
///
/// The caller holds the publication lock, so nothing can move between this and
/// the publication it is read for.
pub(super) fn current(state: &Path, keys: &[String]) -> io::Result<Vec<Option<u64>>> {
    let generations = read(state)?;
    Ok(keys
        .iter()
        .map(|key| generations.get(key).copied())
        .collect())
}

/// Moves the generation of every root in `keys`, before a publication writes.
///
/// Called with the publication lock held and before the first write, so a
/// command whose baseline was taken before this point can tell that a
/// publication happened however the root ends up.
pub(super) fn advance(state: &Path, keys: &[String]) -> io::Result<()> {
    let mut generations = read(state)?;
    for key in keys {
        let counter = generations.entry(key.clone()).or_insert(0);
        *counter = counter.wrapping_add(1);
    }
    while generations.len() > MOST {
        let Some(oldest) = generations
            .iter()
            .min_by_key(|(_, counter)| **counter)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        generations.remove(&oldest);
    }
    let mut text = String::new();
    for (key, counter) in &generations {
        text.push_str(key);
        text.push(' ');
        text.push_str(&counter.to_string());
        text.push('\n');
    }
    let building = state.join(format!("{FILE}.building"));
    {
        let mut file = File::create(&building)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&building, path(state))?;
    File::open(state)?.sync_all()
}
