//! Reaching a proven path by name, where a walk through descriptors cannot be
//! had.
//!
//! Windows validates the final path of every opened handle before returning it,
//! and creates a new file under a held, validated parent directory through a
//! handle-relative rename. Other non-Unix platforms retain the by-name fallback
//! and its narrower last-component protection.

use std::fs::{File, OpenOptions};
#[cfg(not(windows))]
use std::io::ErrorKind;

use super::{Access, PathError, WorkspacePath};

#[cfg(windows)]
mod windows;

/// Opens the named file, after asking what the name is.
pub(super) fn opened(path: &WorkspacePath, access: Access) -> Result<File, PathError> {
    // `symlink_metadata` rather than `metadata`: this asks about the name, and
    // the question is whether the name has become a link since resolving proved
    // it was not one. Following it here would answer about the far end and lose
    // the only thing being asked.
    let named = path
        .as_path()
        .symlink_metadata()
        .map_err(|source| path.unopened(source))?;

    if named.is_symlink() {
        return Err(path.swapped());
    }

    let mut options = OpenOptions::new();
    match access {
        Access::Read | Access::ReadFile => options.read(true),
        Access::Change | Access::ChangeFile => options.read(true).write(true),
    };

    let file = options.open(path).map_err(|source| path.unopened(source))?;
    #[cfg(windows)]
    windows::validate(&file, path)?;

    if matches!(access, Access::ReadFile | Access::ChangeFile)
        && !file
            .metadata()
            .map_err(|source| path.unopened(source))?
            .is_file()
    {
        return Err(path.not_file());
    }
    Ok(file)
}

/// Creates the named file, which nothing may be occupying.
pub(super) fn created(path: &WorkspacePath) -> Result<File, PathError> {
    #[cfg(windows)]
    return windows::created(path);

    #[cfg(not(windows))]
    Err(path.unopened(std::io::Error::new(
        ErrorKind::Unsupported,
        "safe file creation is unavailable on this platform",
    )))
}

/// Refuses a by-name directory creation where no descriptor-relative primitive
/// exists. Creating first and validating afterwards would detect an escape only
/// after it had already left a directory outside the workspace.
pub(super) fn created_directory(path: &WorkspacePath) -> Result<(), PathError> {
    Err(path.uncreated(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "safe creation of missing directories is unavailable on this platform",
    )))
}
