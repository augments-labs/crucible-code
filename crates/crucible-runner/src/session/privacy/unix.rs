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

/// Opens the mark beside a log, making it if it is not there, out of reach.
///
/// Held at what a log is held at. It is named for a session and sits in the
/// same directory, so what it gives away by existing is what the log beside it
/// gives away, and there is no reason for the two to be at different modes.
///
/// Opened to read and write rather than to append, which is the one way it
/// differs from [`log`]. Nothing is ever written through the handle — the
/// access is what taking a lock on it needs. `flock` asks for none, but the
/// call Windows takes a lock with refuses a handle opened only to append, and a
/// mark that cannot be locked is a claim that is never taken.
pub(in crate::session) fn mark(path: &Path) -> Result<File, io::Error> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
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
