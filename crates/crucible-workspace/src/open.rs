//! Opening the file a proven path was proven about.
//!
//! Containment is settled about a name, and a name is not a file. Between the
//! moment [`existing`](super::Workspace::existing) or
//! [`creatable`](super::Workspace::creatable) established where a name led and
//! the moment something opens it, anything else on the machine that can write
//! into the workspace can make the same name lead elsewhere — replace the last
//! component with a symbolic link, or swap a directory above it for one. A
//! [`WorkspacePath`] holds a string rather than an open file, so nothing it
//! carries closes that gap; the check is true of the instant it ran and of no
//! other instant.
//!
//! So the open happens here, beside the proof, and on Unix it never hands the
//! whole path over: `descent` walks down from the directory containment was
//! settled against, one component at a time against a descriptor it already
//! holds, and each step is one system call with no inside for a swap to land
//! in. A link somebody *meant* is unaffected — resolving followed it and
//! settled containment about where it led, so a root reached through a link and
//! a project that links to its own files both work as they read.
//!
//! What that leaves is renaming: a directory the walk is holding can be moved
//! out of the tree and the files below it go along. The descriptor continues to
//! name that same directory object, including for a create or replacement. That
//! takes local filesystem authority outside a file-tool call, and no portable
//! filesystem transaction locks a directory into its former ancestry. A second
//! hard name for a file is not a link at all either, so nothing distinguishes it
//! from the original — that belongs to the filesystem, and no check on names or
//! on components reaches it.
//!
//! Windows opens the path as one, then asks the resulting handle for its final
//! path before any caller can read it. An ancestor swap may make the open reach
//! outside, but the handle is fixed by then; a final path other than the one
//! containment proved is refused without disclosing a byte. New files are made
//! privately and claimed by handle-relative rename under a validated parent.

use std::fs::File;
#[cfg(unix)]
use std::fs::Permissions;
use std::io::Error;

use super::{PathError, WalkFiles, WorkspacePath, written};

#[cfg(unix)]
mod descent;
#[cfg(unix)]
use descent::{created, created_directory, opened};

#[cfg(not(unix))]
mod name;
#[cfg(not(unix))]
use name::{created, created_directory, opened};

/// Opens one file through the directory cache held by a tree-walk worker.
pub(super) fn walked_regular(
    files: &mut WalkFiles,
    path: &WorkspacePath,
) -> Result<File, PathError> {
    #[cfg(unix)]
    return descent::walked_regular(files, path);

    #[cfg(not(unix))]
    {
        let _ = files;
        opened(path, Access::ReadFile)
    }
}

/// What the caller means to do with the file it asked for.
///
/// Reading and changing each have an unrestricted and a regular-file-only
/// shape. How those intentions are said differs per platform — a set of flags
/// on one side, an `OpenOptions` on the other. Naming the intent rather than
/// either spelling is what keeps the two implementations answering the same
/// question.
#[derive(Clone, Copy)]
enum Access {
    /// Read what is there.
    Read,
    /// Read only when the opened descriptor is a regular file.
    ReadFile,
    /// Read what is there and rewrite it where it lies.
    Change,
    /// Open for a replacement only when the descriptor is a regular file.
    ChangeFile,
}

impl WorkspacePath {
    /// Opens the proven file for reading.
    ///
    /// # Errors
    ///
    /// [`PathError::Swapped`] if the path no longer leads through the
    /// directories containment was settled about, and [`PathError::Unopened`]
    /// if the operating system refused.
    pub fn open(&self) -> Result<File, PathError> {
        opened(self, Access::Read)
    }

    /// Opens the proven path only when it is still a regular file.
    ///
    /// Used whenever file content will be consumed, including names yielded by
    /// a tree walk. A file can become a pipe or device after resolution or
    /// directory inspection; this opens Unix names non-blocking and validates
    /// the opened descriptor, so that swap neither hangs the caller nor turns
    /// the read into one from another resource.
    ///
    /// # Errors
    ///
    /// As [`Self::open`], plus [`PathError::NotFile`] if the opened handle is
    /// not a regular file.
    pub fn open_regular(&self) -> Result<File, PathError> {
        opened(self, Access::ReadFile)
    }

    /// Opens the proven file for reading and rewriting where it lies.
    ///
    /// One descriptor for both halves, because naming the file to read it and
    /// naming it again to write it is two lookups with a gap between them — and
    /// a caller that decides what to write from what it read must not put the
    /// answer somewhere else. Truncating and seeking act on the descriptor
    /// rather than the name, so they cannot arrive at a different file either.
    ///
    /// # Errors
    ///
    /// As [`Self::open`].
    pub fn open_to_change(&self) -> Result<File, PathError> {
        opened(self, Access::Change)
    }

    /// Opens the proven path for changing only when it is a regular file.
    ///
    /// Unix opens non-blocking before inspecting the descriptor. A pipe placed
    /// at the name therefore cannot hold a file-changing tool waiting for a
    /// peer merely so the tool can discover that the object was not a file.
    ///
    /// # Errors
    ///
    /// As [`Self::open_to_change`], plus [`PathError::NotFile`] if the opened
    /// handle is not a regular file.
    pub fn open_regular_to_change(&self) -> Result<File, PathError> {
        opened(self, Access::ChangeFile)
    }

    /// Creates the proven file, which nothing may be occupying.
    ///
    /// # Errors
    ///
    /// [`PathError::Swapped`] if something is already at the name — which, to a
    /// caller that has just seen nothing there, is something that arrived
    /// since — and [`PathError::Unopened`] if the operating system refused.
    pub fn create(&self) -> Result<File, PathError> {
        created(self)
    }

    /// Creates the proven directory, which nothing may be occupying.
    ///
    /// Unix creates the last component relative to the opened, proven parent.
    /// Platforms without a safe relative-directory primitive refuse the call;
    /// a by-name fallback would be able to leave a directory outside the
    /// workspace after an ancestor swap.
    ///
    /// # Errors
    ///
    /// [`PathError::Swapped`] if something is already at the name, and
    /// [`PathError::Uncreated`] if the operating system refused or this
    /// platform cannot create it without reopening the full path.
    pub fn create_directory(&self) -> Result<(), PathError> {
        created_directory(self)
    }

    /// Replaces this file as one whole operation.
    ///
    /// The new bytes are written to a fresh file beside the destination,
    /// synced, and renamed through the descriptor of the proven parent
    /// directory. A failure before the rename leaves the destination untouched.
    /// `expected` names the descriptor an edit was derived from; a different
    /// file at the destination when it is checked makes the replacement fail.
    /// The identity check and rename are separate operating-system calls, so
    /// this is detection rather than a portable compare-and-swap primitive.
    /// `permissions` preserves an existing file's mode; `None` uses the
    /// account's ordinary creation mode for a new file.
    ///
    /// Unix only because the descriptor-relative operations are Unix's. The
    /// tools use the platform replacement calls directly on Windows.
    ///
    /// # Errors
    ///
    /// [`PathError::Unreplaced`] if writing, syncing, or renaming failed,
    /// [`PathError::Unsynced`] if the rename landed but syncing its directory
    /// failed, and the same path errors as [`Self::open`] if the proven parent
    /// changed.
    #[cfg(unix)]
    pub fn replace_with(
        &self,
        permissions: Option<Permissions>,
        expected: Option<&File>,
        write: impl FnOnce(&mut File) -> Result<(), Error>,
    ) -> Result<(), PathError> {
        descent::replaced(self, permissions, expected, write)
    }

    /// The path led somewhere else by the time it was opened.
    fn swapped(&self) -> PathError {
        PathError::Swapped {
            at: written(self.as_path()).into(),
        }
    }

    /// The operating system refused, and the reason is its own.
    fn unopened(&self, source: Error) -> PathError {
        PathError::Unopened {
            at: written(self.as_path()).into(),
            source,
        }
    }

    /// The operating system refused to create a directory at this path.
    fn uncreated(&self, source: Error) -> PathError {
        PathError::Uncreated {
            at: written(self.as_path()).into(),
            source,
        }
    }

    /// The file an edit was derived from is no longer at its name.
    #[cfg(unix)]
    fn changed(&self) -> PathError {
        PathError::Changed {
            at: written(self.as_path()).into(),
        }
    }

    /// The name changed to a filesystem object a file-consuming tool does not read.
    fn not_file(&self) -> PathError {
        PathError::NotFile {
            at: written(self.as_path()).into(),
        }
    }

    /// A replacement failed without changing the destination.
    #[cfg(unix)]
    fn unreplaced(&self, source: Error) -> PathError {
        PathError::Unreplaced {
            at: written(self.as_path()).into(),
            source,
        }
    }

    /// A committed replacement whose directory entry is not proven durable.
    #[cfg(unix)]
    fn unsynced(&self, source: Error) -> PathError {
        PathError::Unsynced {
            at: written(self.as_path()).into(),
            source,
        }
    }

    /// A created destination whose private hard-link name remains beside it.
    #[cfg(unix)]
    fn uncleaned(&self, source: Error) -> PathError {
        PathError::Uncleaned {
            at: written(self.as_path()).into(),
            source,
        }
    }
}
