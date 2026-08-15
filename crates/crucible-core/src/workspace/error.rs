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

    /// A workspace root names something other than a directory.
    #[error("{requested} is not a directory")]
    NotDirectory {
        /// The root or extra directory as the caller wrote it.
        requested: Box<str>,
    },

    /// A resolved or walked name no longer names a regular file.
    #[error("{at} is not a regular file")]
    NotFile {
        /// The absolute path the caller reached.
        at: Box<str>,
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

    /// A whole-file transformation was prepared from an earlier file that is
    /// no longer the one at the destination name.
    #[error("{at} changed while its replacement was prepared, so it was not replaced")]
    Changed {
        /// The resolved destination whose identity changed.
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

    /// A directory could not be created at a contained path.
    #[error("{at} could not be created as a directory: {source}")]
    Uncreated {
        /// The resolved directory name.
        at: Box<str>,
        /// What the operating system reported.
        source: std::io::Error,
    },

    /// A whole-file replacement could not be committed.
    #[error("{at} could not be replaced: {source}")]
    Unreplaced {
        /// The resolved path, so the failed destination is unambiguous.
        at: Box<str>,
        /// What the operating system reported.
        source: std::io::Error,
    },

    /// A whole-file replacement landed, but its post-commit flush failed.
    #[error(
        "{at} was replaced, but its commit could not be flushed: durability is not guaranteed: {source}"
    )]
    Unsynced {
        /// The resolved destination whose new contents are already visible.
        at: Box<str>,
        /// What the operating system reported during the post-commit flush.
        source: std::io::Error,
    },

    /// A new file landed, but its private preparation name remains beside it.
    #[error("{at} was created, but its temporary name could not be removed: {source}")]
    Uncleaned {
        /// The resolved destination whose contents are already visible.
        at: Box<str>,
        /// What the operating system reported while removing the extra name.
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
