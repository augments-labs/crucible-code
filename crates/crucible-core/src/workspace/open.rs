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
//! So the open happens here, beside the proof, in the shapes that carry
//! something of it into the call itself:
//!
//! - **Creating** goes through `O_CREAT | O_EXCL` — [`File::create_new`] —
//!   which the operating system refuses to satisfy through a symbolic link at
//!   the last component, dangling or not. There is no second lookup for a swap
//!   to win: that is a closure rather than a narrowing, on every platform.
//! - **Opening what is already there** asks about the name without following a
//!   link at it, refuses a name that has become one, and then asks the file it
//!   opened whether it is the file that answer was about. The refusal covers
//!   the whole gap since containment ran; the comparison covers the two
//!   adjacent calls the refusal is made of, which a swap could otherwise land
//!   between.
//!
//! What none of that reaches is the directories *above* the last component.
//! Every call here resolves those the same way and at the same moment, so a
//! directory replaced with a link to somewhere else is followed by all of them
//! and agreed on by all of them. Only resolving each step relative to a
//! directory already held open would see it, and safe Rust cannot ask for that.
//! A second writer can therefore still move a file's ancestor out of the tree;
//! what it has to win to do so is a race between two adjacent system calls,
//! which is the narrowest a call that names a path can be made and is not
//! nothing.

use std::fs::{File, Metadata, OpenOptions};
use std::io::{Error, ErrorKind};

use super::{PathError, WorkspacePath, written};

impl WorkspacePath {
    /// Opens the proven file for reading.
    ///
    /// # Errors
    ///
    /// [`PathError::Swapped`] if the name no longer leads to the file
    /// containment was settled about, and [`PathError::Unopened`] if the
    /// operating system refused.
    pub fn open(&self) -> Result<File, PathError> {
        self.opened(OpenOptions::new().read(true))
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
        self.opened(OpenOptions::new().read(true).write(true))
    }

    /// Creates the proven file, which nothing may be occupying.
    ///
    /// # Errors
    ///
    /// [`PathError::Swapped`] if something is already at the name — which, to a
    /// caller that has just seen nothing there, is something that arrived
    /// since — and [`PathError::Unopened`] if the operating system refused.
    pub fn create(&self) -> Result<File, PathError> {
        File::create_new(self).map_err(|source| {
            if source.kind() == ErrorKind::AlreadyExists {
                self.swapped()
            } else {
                self.unopened(source)
            }
        })
    }

    /// The open both shapes above share, with the identity check around it.
    fn opened(&self, options: &OpenOptions) -> Result<File, PathError> {
        // `symlink_metadata` rather than `metadata`: this asks about the name,
        // and the question is whether the name has become a link since
        // resolving proved it was not one. Following it here would answer about
        // the far end and lose the only thing being asked.
        let named = self
            .as_path()
            .symlink_metadata()
            .map_err(|source| self.unopened(source))?;

        if named.is_symlink() {
            return Err(self.swapped());
        }

        let opened = options.open(self).map_err(|source| self.unopened(source))?;
        let got = opened.metadata().map_err(|source| self.unopened(source))?;

        if same(&named, &got) {
            Ok(opened)
        } else {
            Err(self.swapped())
        }
    }

    /// The name led somewhere else by the time it was opened.
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

/// Whether two descriptions are of the same file.
///
/// A device and an inode number are what the operating system itself tells two
/// files apart by, and on Unix safe Rust reads both. What this compares is the
/// name a moment ago against the file just opened, so what it catches is a swap
/// landing between those two calls; it says nothing about anything that
/// happened before the first of them, which is the refusal above's job.
///
/// One way to pass this while also being outside the workspace is a second hard
/// name for a file that is. A hard link is not a link that resolves to
/// something, it *is* the file, so canonicalising sees nothing to follow and
/// neither does this. That belongs to the filesystem rather than to the race,
/// and no check that works on names reaches it at all.
#[cfg(unix)]
fn same(named: &Metadata, opened: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    named.dev() == opened.dev() && named.ino() == opened.ino()
}

/// Windows keeps the same pair behind an interface safe Rust reaches only for
/// an already-open handle, so there this cannot be asked and the symbolic-link
/// refusal above is what stands. Saying so here rather than at the call site
/// keeps the shape of the check identical on both platforms.
#[cfg(not(unix))]
fn same(_named: &Metadata, _opened: &Metadata) -> bool {
    true
}
