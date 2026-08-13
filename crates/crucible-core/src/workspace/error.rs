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

    /// The name led to a different file by the time it was opened than the one
    /// containment was settled about.
    #[error("{at} was replaced after it was checked, so it was not opened")]
    Swapped {
        /// The resolved path rather than the caller's spelling, which is the
        /// one thing that did not change. Naming the file is what says what
        /// happened; naming the request would describe a path that is still
        /// perfectly good to ask for again.
        at: Box<str>,
    },

    /// The operating system refused to open a path that resolved inside the
    /// workspace.
    #[error("{at} could not be opened: {source}")]
    Unopened {
        /// The resolved path, for the reason above.
        at: Box<str>,
        /// What the operating system reported.
        source: std::io::Error,
    },

    /// The root resolves to a name that is not valid UTF-8, so it cannot be
    /// written down and read back as the same directory.
    #[error("{resolved} is not a directory name this can write down: it is not valid UTF-8")]
    NotText {
        /// The resolved path, spelled as closely as text allows — which is the
        /// whole complaint, so it is the only thing there is to show.
        resolved: Box<str>,
    },
}
