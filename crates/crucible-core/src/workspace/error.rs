//! Why a path could not be resolved inside the workspace.
//!
//! Each variant is a different sentence to a model deciding what to try next,
//! which is why "cannot be resolved" is never collapsed into "is outside".

/// Why a path could not be resolved inside the workspace.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// The path resolved outside every directory the workspace reaches.
    #[error("{requested} resolves outside the workspace")]
    Escapes {
        /// The path as the caller wrote it.
        requested: Box<str>,
    },

    /// The path, or the directory that would contain it, does not exist.
    #[error("{requested} does not exist")]
    Missing {
        /// The path as the caller wrote it.
        requested: Box<str>,
        /// What the operating system reported.
        source: std::io::Error,
    },

    /// The path is a symbolic link whose target could not be resolved, so
    /// there is nothing to decide containment about.
    #[error("{requested} is a link that leads nowhere this can resolve: {source}")]
    Dangling {
        /// The path as the caller wrote it.
        requested: Box<str>,
        /// What the operating system reported about the target.
        source: std::io::Error,
    },

    /// The path has no parent directory, so nothing could be created there.
    #[error("{requested} has no parent directory")]
    NoParent {
        /// The path as the caller wrote it.
        requested: Box<str>,
    },

    /// A directory the workspace was asked to reach was given as a relative
    /// path, which names a different place depending on where crucible was
    /// started.
    #[error("{requested} must be an absolute path")]
    Relative {
        /// The path as the caller wrote it.
        requested: Box<str>,
    },
}
