//! Owner-only directories and files for sensitive local state.
//!
//! Unix expresses the boundary with modes; Windows uses a protected access
//! control list containing only the current account. Keeping both mechanisms
//! here gives every piece of sensitive local state the same creation and
//! durable-replacement boundary without teaching this crate what the file is.
//!
//! The readers, [`open_read`], [`open_read_append`] and [`open_read_ordinary`],
//! and the lock opener, [`lock`], each refuse a final symbolic link or reparse
//! point, wait on no pipe where Unix can say so, and prove an ordinary file on
//! the handle they return rather than on a name looked at first. Private state
//! is also refused when another hard name reaches it, because a second name is
//! a second way to change it: that is [`open_read`] and [`open_read_append`].
//! [`open_read_ordinary`] is the one open without that last proof, for a file
//! that is read and not trusted, where a second name is what a backup made with
//! hard links leaves on every file, and refusing it would hide the file rather
//! than protect it.
//!
//! [`replace_in`] is the one write that holds its directory: the directory is
//! opened once, as itself, and the file is drafted and renamed into what that
//! open holds, so neither a link at either name nor a name changed while it
//! runs can send the write to another directory.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::hash::BuildHasher as _;
use std::io;
use std::path::Path;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
use unix as platform;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
use windows as platform;

/// A private filesystem operation the operating system refused.
#[derive(Debug, thiserror::Error)]
#[error(transparent)]
pub struct PrivacyError(#[from] io::Error);

impl PrivacyError {
    /// The portable category callers use for exclusive creation.
    #[must_use]
    pub fn kind(&self) -> io::ErrorKind {
        self.0.kind()
    }

    /// Returns the operating-system error to a boundary that adds path context.
    #[must_use]
    pub fn into_io(self) -> io::Error {
        self.0
    }
}

/// Creates or tightens an owner-only directory.
///
/// # Errors
///
/// [`PrivacyError`] when the directory cannot be created or protected.
pub fn directory(path: &Path) -> Result<(), PrivacyError> {
    platform::directory(path).map_err(Into::into)
}

/// Opens an owner-only file for append, creating it when absent.
///
/// # Errors
///
/// [`PrivacyError`] when the file cannot be opened or protected.
pub fn append(path: &Path) -> Result<File, PrivacyError> {
    platform::append(path).map_err(Into::into)
}

/// Opens one existing owner-only ordinary file for bounded inspection.
///
/// Symbolic links, reparse points, non-files and files with another hard name
/// are refused. The validation is made on the returned handle rather than on a
/// pathname inspected before it was opened. On Unix the open itself is
/// non-blocking, so reaching that refusal does not wait on a writer who is not
/// coming; other platforms reach the same refusal, having waited.
///
/// # Errors
///
/// [`PrivacyError`] when the file cannot be opened or is not one ordinary file
/// under one name.
pub fn open_read(path: &Path) -> Result<File, PrivacyError> {
    platform::open_read(path).map_err(Into::into)
}

/// Opens one existing ordinary file for bounded inspection, however many hard
/// names reach it.
///
/// What [`open_read`] proves, less the single name: a symbolic link, a
/// reparse point or a non-file is refused, on the returned handle, and on Unix
/// the open does not wait on a writer who is not coming. Another hard name is
/// accepted, so a caller holds nothing it opened through here as private
/// state it alone can change. Nor does the handle hold the name: on Windows it
/// allows deletion, so a [`replace`] over the file while it is read goes ahead
/// and this reader keeps the file it opened, as it would on Unix.
///
/// # Errors
///
/// [`PrivacyError`] when the file cannot be opened or is not one ordinary file.
pub fn open_read_ordinary(path: &Path) -> Result<File, PrivacyError> {
    platform::open_read_ordinary(path).map_err(Into::into)
}

/// Opens one existing owner-only ordinary file for reading, shortening and
/// appending through the same handle.
///
/// Symbolic links, reparse points, non-files and files with another hard name
/// are refused before the handle is returned.
///
/// # Errors
///
/// [`PrivacyError`] when the file cannot be opened or is not one ordinary file
/// under one name.
pub fn open_read_append(path: &Path) -> Result<File, PrivacyError> {
    platform::open_read_append(path).map_err(Into::into)
}

/// Rechecks that an opened private file still has exactly one filesystem name.
///
/// # Errors
///
/// [`PrivacyError`] when metadata cannot be read or another hard name exists.
pub fn single_name(file: &File) -> Result<(), PrivacyError> {
    platform::single_name(file).map_err(Into::into)
}

/// Tightens an already-opened private file without looking its name up again.
///
/// # Errors
///
/// [`PrivacyError`] when the opened file cannot be protected.
pub fn tighten_open(file: &File) -> Result<bool, PrivacyError> {
    platform::tighten_open(file).map_err(Into::into)
}

/// Tries to lock an opened file's identity without locking its readable bytes.
///
/// Every opened name for the same filesystem object meets the same lock. The
/// Unix advisory primitive already leaves reads alone; Windows uses one byte at
/// 4 EiB because its range locks are mandatory. Callers must keep content below
/// that sentinel.
///
/// # Errors
///
/// [`PrivacyError`] when the operating system could not attempt the lock.
pub fn try_lock_identity(file: &File) -> Result<bool, PrivacyError> {
    platform::try_lock_identity(file).map_err(Into::into)
}

/// Releases a lock taken by [`try_lock_identity`].
///
/// # Errors
///
/// [`PrivacyError`] when the operating system could not release the lock.
pub fn unlock_identity(file: &File) -> Result<(), PrivacyError> {
    platform::unlock_identity(file).map_err(Into::into)
}

/// Exclusively creates an owner-only file for append.
///
/// # Errors
///
/// [`PrivacyError`] when the name exists or the file cannot be protected.
pub fn create_append(path: &Path) -> Result<File, PrivacyError> {
    platform::create_append(path).map_err(Into::into)
}

/// Exclusively creates an owner-only file for writing.
///
/// # Errors
///
/// [`PrivacyError`] when the name exists or the file cannot be protected.
pub fn create_write(path: &Path) -> Result<File, PrivacyError> {
    platform::create_write(path).map_err(Into::into)
}

/// Opens an owner-only lock file for reading and writing, creating it absent.
///
/// A final symbolic link or reparse point is refused rather than followed, so
/// nothing is created, locked, written or protected where one leads, and a
/// non-file is refused on the returned handle before it is protected. On Unix
/// the open does not wait on a pipe's reader; Windows has no pipe to wait on
/// in a directory. Another hard name is accepted, as [`open_read_ordinary`]
/// accepts one.
///
/// # Errors
///
/// [`PrivacyError`] when the file cannot be opened or protected, or is not one
/// ordinary file.
pub fn lock(path: &Path) -> Result<File, PrivacyError> {
    platform::lock(path).map_err(Into::into)
}

/// Tightens an existing file and reports whether its Unix mode changed.
///
/// Windows writes the protected list without a second FFI call to compare it,
/// so a successful operation reports `false` there. On Unix the name is opened
/// without waiting for a peer, so a name that is not one ordinary file is
/// refused without blocking on a writer who is not coming; Windows refuses a
/// reparse point by name and makes no such open.
///
/// # Errors
///
/// [`PrivacyError`] when the existing file cannot be inspected or protected.
pub fn tighten(path: &Path) -> Result<bool, PrivacyError> {
    platform::tighten(path).map_err(Into::into)
}

/// Makes a file replacement durable in the directory that names it, where the
/// platform exposes that operation.
///
/// The file itself must be synced before it is renamed. This second sync is
/// what persists the rename rather than only the bytes the renamed file holds.
///
/// # Errors
///
/// [`PrivacyError`] when the parent cannot be opened or synced.
pub fn sync_parent(path: &Path) -> Result<(), PrivacyError> {
    platform::sync_parent(path).map_err(Into::into)
}

/// Atomically replaces `destination` with the prepared file at `source` and
/// makes that name change durable where the platform exposes the operation.
///
/// Both paths must name files in the same directory. The source is consumed on
/// success and left for the caller to clean up on failure.
///
/// A handle already open on the destination keeps the file it opened while the
/// name moves on to the new one. On Windows that needs a file system that
/// supports POSIX rename semantics and a handle that allowed deletion, as a
/// `std` open and [`open_read_ordinary`] do; this crate's other opens do not,
/// and replacing a file one of them holds fails.
///
/// # Errors
///
/// [`PrivacyError`] when the replacement or its durability step fails.
pub fn replace(source: &Path, destination: &Path) -> Result<(), PrivacyError> {
    platform::replace(source, destination).map_err(Into::into)
}

/// Replaces the file `name` in the owner-only `directory` with one that holds
/// `contents` and nothing else, creating or tightening the directory as
/// [`directory`] does.
///
/// The directory is opened once, as itself, and held: a link or anything but
/// a directory at its name is refused, and every later step is taken in the
/// directory that open holds, so a name changed meanwhile cannot send the
/// write to another directory. A link at `name` is refused before anything is
/// written, and one made there afterwards is replaced rather than followed.
/// The contents are written to an owner-only draft beside `name`, under a name
/// nothing outside this process can predict, synced and renamed over `name`;
/// the draft is removed on any failure once it exists. On Unix the rename is
/// then made durable in the directory, a step Windows does not expose.
///
/// # Errors
///
/// [`PrivacyError`] where `name` is not one name in a directory, either name
/// is a link, the directory cannot be made or protected, or the file cannot
/// be replaced.
pub fn replace_in(directory: &Path, name: &OsStr, contents: &[u8]) -> Result<(), PrivacyError> {
    if Path::new(name).file_name() != Some(name) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "a private file is named by one name in its directory",
        )
        .into());
    }
    platform::replace_in(directory, name, &draft(), contents).map_err(Into::into)
}

/// The name of a draft [`replace_in`] writes, which nothing outside this
/// process can predict: a hash under keys `std` draws from the operating
/// system once per thread and changes for every new state, so neither a name
/// planted first nor one guessed from another draft is the one taken.
fn draft() -> OsString {
    let unpredictable = std::hash::RandomState::new().hash_one(std::process::id());
    format!(".crucible-{unpredictable:016x}.writing").into()
}

#[cfg(test)]
mod tests;
