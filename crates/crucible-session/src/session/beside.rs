//! One whole version of a small file, put in place or not at all.
//!
//! The append-only logs need none of this: a crash costs the last line and the
//! rest of the file is what it always was. The fixed files beside them are the
//! opposite shape — an index, a prompt history — where every line is rewritten
//! each time one changes, and a crash part-way through a rewrite is a file that
//! is neither the old version nor the new one.
//!
//! So the new version is written somewhere else first, under a name this
//! process created exclusively, synced, and then renamed over the real one. A
//! rename is the one filesystem operation that either happened or did not, so
//! a reader arriving at any moment reads a complete version. The sibling is
//! removed unless its rename lands, which is what stops a directory filling
//! with the leavings of writes that failed.

use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// How many exclusive temporary names a replacement tries.
///
/// The name carries this process's identifier and a counter no two calls
/// share, so a collision means the directory holds the leavings of a process
/// that is gone and whose identifier has come round again. Bounded rather than
/// retried for ever, because a directory that answers "taken" to everything is
/// a fault to report and not one to spin on.
const NAMES: usize = 32;

/// An exclusively-created sibling removed unless its rename lands.
#[derive(Debug)]
pub(super) struct Beside {
    path: PathBuf,
    file: Option<File>,
    landed: bool,
}

impl Beside {
    /// Somewhere new to write, in `directory`, named after `stem`.
    ///
    /// The stem is the caller's, so a directory holding two of these files
    /// says which write left a sibling behind rather than making the reader
    /// guess between them.
    pub(super) fn new(directory: &Path, stem: &str) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);

        for _ in 0..NAMES {
            let sequence = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(".{stem}.{}.{sequence}.tmp", std::process::id()));
            match crucible_privacy::create_write(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        landed: false,
                    });
                }
                Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {}
                Err(problem) => return Err(problem.into_io()),
            }
        }

        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not find a free name for the replacement",
        ))
    }

    /// The handle to write the new version through.
    pub(super) fn file(&mut self) -> io::Result<&mut File> {
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("the temporary is already closed"))
    }

    /// Puts what was written where `path` is, whole.
    pub(super) fn over(mut self, path: &Path) -> io::Result<()> {
        drop(self.file.take());
        crucible_privacy::replace(&self.path, path)
            .map_err(crucible_privacy::PrivacyError::into_io)?;
        self.landed = true;
        Ok(())
    }
}

impl Drop for Beside {
    fn drop(&mut self) {
        if !self.landed {
            drop(self.file.take());
            let _ = fs::remove_file(&self.path);
        }
    }
}
