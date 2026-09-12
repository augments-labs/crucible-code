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
//! What it holds is a hash of where each root is, never the pathname: this
//! user's state directory is private, but a file of workspace paths is a
//! description of the machine that nothing here needs.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::path::{Path, PathBuf};

use rustix::fs::{Mode, OFlags};
use sha2::{Digest as _, Sha256};

/// The file the generations live in, beside the lock that orders publications.
const FILE: &str = "publications";

/// How many roots are remembered.
///
/// What eviction costs is one command's writes: a root this remembered and then
/// forgot reads as moved, which refuses that command's publication. What it
/// cannot do is admit one wrongly for a root that was never remembered at all,
/// because absent on both sides is how a root nothing has published into looks,
/// and that is the same answer before and after. The bound is high enough that a
/// workspace reaches it only with a thousand distinct writable roots.
const MOST: usize = 1024;

/// How much of the file is read before it is called unreasonable.
const MOST_BYTES: u64 = (MOST as u64 + 1) * 96;

/// What one root stands at, and when it was last moved.
#[derive(Clone, Copy)]
struct Standing {
    counter: u64,
    /// Which call last moved it, so eviction can drop what was touched longest
    /// ago rather than what has been touched least often — the root moved once,
    /// a moment ago, is the one a command is most likely to be waiting on.
    touched: u64,
}

/// What a root is remembered under.
pub(super) fn key(host: &Path) -> String {
    use std::fmt::Write as _;
    use std::os::unix::ffi::OsStrExt as _;

    let digest: [u8; 32] = Sha256::digest(host.as_os_str().as_bytes()).into();
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

fn unreadable(problem: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("sandbox root generations {problem}"),
    )
}

/// Every generation this user's state directory remembers.
///
/// Opened without following a link, because the name lives in a directory this
/// user's own processes write, and read to a bound. A line it cannot read is an
/// error rather than an absence: absence is how a root nothing has published
/// into looks, and reading damage that way is the one mistake that admits a
/// command it should refuse.
fn read(state: &Path) -> io::Result<BTreeMap<String, Standing>> {
    let opened = match rustix::fs::open(
        path(state),
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    ) {
        Ok(opened) => opened,
        Err(rustix::io::Errno::NOENT) => return Ok(BTreeMap::new()),
        Err(problem) => return Err(problem.into()),
    };
    let mut text = String::new();
    File::from(opened)
        .take(MOST_BYTES)
        .read_to_string(&mut text)?;
    let mut generations = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.split(' ');
        let (Some(remembered), Some(counter), Some(touched), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            return Err(unreadable("hold a line that is not one"));
        };
        let (Ok(counter), Ok(touched)) = (counter.parse(), touched.parse()) else {
            return Err(unreadable("hold a count that is not a number"));
        };
        generations.insert(remembered.to_owned(), Standing { counter, touched });
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
        .map(|key| generations.get(key).map(|standing| standing.counter))
        .collect())
}

/// Moves the generation of every root in `keys`, before a publication writes.
///
/// Called with the publication lock held and before the first write, so a
/// command whose baseline was taken before this point can tell that a
/// publication happened however the root ends up.
pub(super) fn advance(state: &Path, keys: &[String]) -> io::Result<()> {
    let mut generations = read(state)?;
    let now = generations
        .values()
        .map(|standing| standing.touched)
        .max()
        .unwrap_or(0)
        .wrapping_add(1);
    for key in keys {
        let standing = generations.entry(key.clone()).or_insert(Standing {
            counter: 0,
            touched: now,
        });
        standing.counter = standing.counter.wrapping_add(1);
        standing.touched = now;
    }
    while generations.len() > MOST {
        let Some(stalest) = generations
            .iter()
            .min_by_key(|(_, standing)| standing.touched)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        generations.remove(&stalest);
    }
    let mut text = String::new();
    for (key, standing) in &generations {
        text.push_str(key);
        text.push(' ');
        text.push_str(&standing.counter.to_string());
        text.push(' ');
        text.push_str(&standing.touched.to_string());
        text.push('\n');
    }
    // A fresh file of this user's own: whatever is at the working name, link or
    // leftover, is removed first and the create refuses to reuse anything, so
    // nothing outside this directory is ever written through.
    let building = state.join(format!("{FILE}.building"));
    let _ = fs::remove_file(&building);
    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&building)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&building, path(state))?;
    File::open(state)?.sync_all()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state directory of this test's own, owner-only as the real one is.
    fn state(name: &str) -> PathBuf {
        let at = std::env::temp_dir().join(format!(
            "crucible-generations-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&at);
        fs::create_dir_all(&at).expect("a state directory");
        at
    }

    #[test]
    fn the_file_is_this_user_s_own_and_follows_nothing() {
        use std::os::unix::fs::PermissionsExt as _;

        let at = state("discipline");
        let victim = at.join("victim");
        fs::write(&victim, "not this\n").expect("a file to aim at");
        std::os::unix::fs::symlink(&victim, at.join(format!("{FILE}.building")))
            .expect("a symlink where the working file goes");

        let _ = advance(&at, &[key(Path::new("/somewhere"))]);

        assert_eq!(
            fs::read_to_string(&victim).expect("the file aimed at"),
            "not this\n",
            "the working file followed a symlink out of its own directory"
        );
        let mode = fs::metadata(path(&at))
            .expect("the generations")
            .permissions()
            .mode();
        assert_eq!(
            mode & 0o777,
            0o600,
            "the generations are readable by others"
        );
    }

    #[test]
    fn the_oldest_touch_is_what_eviction_drops() {
        let at = state("eviction");
        let first = key(Path::new("/first"));
        // Touched three times, and longest ago.
        for _ in 0..3 {
            advance(&at, std::slice::from_ref(&first)).expect("the first root");
        }
        for number in 0..MOST {
            advance(&at, &[key(&PathBuf::from(format!("/root-{number}")))]).expect("another root");
        }

        let standing = current(&at, std::slice::from_ref(&first)).expect("the generations");

        assert!(
            standing.first().copied().flatten().is_none(),
            "eviction kept the root touched longest ago because its count was highest"
        );
    }
}
