//! A file mode says it.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::Path;

/// What the sessions directory is held at.
///
/// The listing itself is worth keeping private: one entry per session says how
/// often crucible ran and when. And a group-writable directory would let another
/// account drop a log in for `--continue` to find, which is the injection the
/// mode on the log guards against from the other side.
const DIRECTORY: u32 = 0o700;

/// What a session log is held at.
const LOG: u32 = 0o600;

/// Makes the sessions directory, out of every other account's reach.
pub(in crate::session) fn directory(path: &Path) -> Result<(), io::Error> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIRECTORY)
        .create(path)?;

    narrow(path, DIRECTORY)
}

/// Opens a log for appending, making it if it is not there, out of reach.
pub(in crate::session) fn log(path: &Path) -> Result<File, io::Error> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(LOG)
        .open(path)?;

    narrow(path, LOG)?;

    Ok(file)
}

/// Makes a log that was not there a moment ago, out of reach.
///
/// `create_new` rather than `create`, which is what makes the name a session
/// starts under its own: two crucibles that minted one identifier both arrive
/// here, and the filesystem decides between them — one creates the file and the
/// other is told it exists. Opened the way [`log`] opens one, the loser would
/// write its header onto the winner's log and the two would then interleave a
/// session each into one file, with neither of them able to tell.
///
/// The mode is what [`log`] holds one at. A log is a log whichever call made
/// it, and the one difference between these two is what they do about a name
/// that is already there.
pub(in crate::session) fn fresh(path: &Path) -> Result<File, io::Error> {
    let file = OpenOptions::new()
        .create_new(true)
        .append(true)
        .mode(LOG)
        .open(path)?;

    narrow(path, LOG)?;

    Ok(file)
}

/// Puts `path` at exactly `mode`, and only when it is not already there.
///
/// `DirBuilderExt::mode` and `OpenOptionsExt::mode` apply only when the call
/// creates the thing, so a sessions directory or a log left by an earlier build
/// keeps whatever it was made with until something sets it.
///
/// Reading the mode first is what keeps the ordinary case — a directory already
/// at 0700, every run after the first — from calling `chmod` at all. That call
/// fails on a filesystem carrying no Unix modes, and a startup that dies over a
/// permission which is already correct helps nobody. It is compared exactly and
/// not as "at least this tight", because the two ways to be wrong both need
/// fixing and only one of them is about secrecy: too open hands the transcript
/// to every account on the machine, and too tight is a session that cannot
/// start, reported against the log rather than the directory that refused.
/// Where the mode really cannot be set, the error stands.
///
/// Setting it also clears any set-user, set-group or sticky bit, which is why
/// the ones already correct are left alone rather than rewritten.
fn narrow(path: &Path, mode: u32) -> Result<(), io::Error> {
    if std::fs::metadata(path)?.permissions().mode() & 0o777 == mode {
        return Ok(());
    }

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
}
