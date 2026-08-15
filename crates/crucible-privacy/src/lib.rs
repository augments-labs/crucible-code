//! Owner-only directories and files for sensitive local state.
//!
//! Unix expresses the boundary with modes; Windows uses a protected access
//! control list containing only the current account. Keeping both mechanisms
//! here gives every piece of sensitive local state the same creation and
//! durable-replacement boundary without teaching this crate what the file is.

use std::fs::File;
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
/// pathname inspected before it was opened.
///
/// # Errors
///
/// [`PrivacyError`] when the file cannot be opened or is not one ordinary file
/// under one name.
pub fn open_read(path: &Path) -> Result<File, PrivacyError> {
    platform::open_read(path).map_err(Into::into)
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
/// # Errors
///
/// [`PrivacyError`] when the file cannot be opened or protected.
pub fn lock(path: &Path) -> Result<File, PrivacyError> {
    platform::lock(path).map_err(Into::into)
}

/// Tightens an existing file and reports whether its Unix mode changed.
///
/// Windows writes the protected list without a second FFI call to compare it,
/// so a successful operation reports `false` there.
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
/// # Errors
///
/// [`PrivacyError`] when the replacement or its durability step fails.
pub fn replace(source: &Path, destination: &Path) -> Result<(), PrivacyError> {
    platform::replace(source, destination).map_err(Into::into)
}

#[cfg(test)]
mod tests;
