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
//! out of the tree and the file below it goes along. It takes write access to
//! the workspace and yields the mover a file they could already read, which is
//! why it is the residue rather than the hole. A second hard name for a file is
//! not a link at all, so nothing distinguishes it from the original — that
//! belongs to the filesystem, and no check on names or on components reaches it.
//!
//! Windows is where this stops. `openat` is a Unix notion and the equivalent
//! there is reached only through FFI this crate does not write, so `name`
//! opens the path as one: the refusal of a link at the last component and the
//! flag that refuses one at a creation both stand, and a directory above the
//! file can still be replaced between the check and the call.

use std::fs::File;
use std::io::Error;

use super::{PathError, WorkspacePath, written};

#[cfg(unix)]
mod descent;
#[cfg(unix)]
use descent::{created, opened};

#[cfg(not(unix))]
mod name;
#[cfg(not(unix))]
use name::{created, opened};

/// What the caller means to do with the file it asked for.
///
/// The two shapes below differ in this and in nothing else, and how it is said
/// differs per platform — a set of flags on one side, an `OpenOptions` on the
/// other. Naming the intent rather than either spelling is what keeps the two
/// implementations answering the same question.
#[derive(Clone, Copy)]
enum Access {
    /// Read what is there.
    Read,
    /// Read what is there and rewrite it where it lies.
    Change,
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
}
