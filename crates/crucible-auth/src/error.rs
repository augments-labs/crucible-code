//! What can go wrong writing a credential down.
//!
//! Only writing: a store nobody can read must not stop a launch that needs no
//! store, so reading answers with an empty one and a sentence out of
//! [`crate::StoredCredentials::trouble`] rather than with an error.

use std::path::{Path, PathBuf};

/// Why a credential could not be written down.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// The directory, the lock or the file itself would not open.
    ///
    /// Names the path, because the remedy is nearly always about that path —
    /// a read-only home, a full disk, a directory somebody else owns.
    #[error("{path}: {source}")]
    Unwritable {
        /// What crucible was trying to open.
        path: PathBuf,
        /// What the operating system said.
        source: std::io::Error,
    },

    /// Another crucible is writing to the store and did not finish in time.
    ///
    /// Waiting is what the lock is for; giving up is what stops two logins
    /// racing from silently dropping one of them. The remedy is to try again,
    /// which is why this says so rather than naming a lock file nobody set up.
    #[error("another crucible is writing to {path} — try again")]
    Busy {
        /// The store, not the lock file beside it.
        path: PathBuf,
    },

    /// The store on disk could not be parsed, so writing would destroy it.
    ///
    /// Reading tolerates this and reports nobody logged in. Writing may not:
    /// a read-modify-write over a file this program cannot read is not a
    /// modification, it is a replacement, and the thing being replaced is the
    /// only copy of somebody's other credentials.
    #[error(
        "{path} cannot be read, so it will not be written over — move it aside to log in again"
    )]
    Unreadable {
        /// The store.
        path: PathBuf,
    },

    /// The store exceeded the boundary this process will hold while parsing.
    #[error("{path} is larger than the {maximum}-byte auth store limit")]
    TooLarge {
        /// The store.
        path: PathBuf,
        /// The greatest accepted byte length.
        maximum: usize,
    },
}

impl AuthError {
    /// An I/O failure, tagged with what it was reaching for.
    pub(crate) fn at(path: &Path) -> impl FnOnce(std::io::Error) -> Self {
        let path = path.to_path_buf();
        |source| Self::Unwritable { path, source }
    }
}
