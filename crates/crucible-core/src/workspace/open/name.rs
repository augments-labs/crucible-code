//! Reaching a proven path by name.
//!
//! What this holds is the last component: a name that has become a symbolic
//! link since it was resolved is refused, and a new file is created through the
//! flag the operating system refuses to satisfy through a link at all. What it
//! does not hold is anything above that component — the whole path is handed
//! over and resolved afresh, so a directory replaced between the check and the
//! call is followed. `super` says why that line falls here.

use std::fs::{File, OpenOptions};
use std::io::ErrorKind;

use super::{Access, PathError, WorkspacePath};

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
        Access::Read => options.read(true),
        Access::Change => options.read(true).write(true),
    };

    options.open(path).map_err(|source| path.unopened(source))
}

/// Creates the named file, which nothing may be occupying.
pub(super) fn created(path: &WorkspacePath) -> Result<File, PathError> {
    File::create_new(path).map_err(|source| {
        if source.kind() == ErrorKind::AlreadyExists {
            path.swapped()
        } else {
            path.unopened(source)
        }
    })
}
