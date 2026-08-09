//! The proof that a path is one the workspace reaches.

use std::fmt;
use std::path::{Path, PathBuf};

/// A path proven to be inside the workspace.
///
/// There is no public constructor: the only way to hold one is to have asked
/// a [`Workspace`](super::Workspace) for it, so a function taking this type
/// cannot be handed a path from anywhere else.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspacePath(PathBuf);

impl WorkspacePath {
    /// Called only after containment has been decided, which is why this is
    /// visible to the workspace and to nothing else.
    pub(super) fn proven(resolved: PathBuf) -> Self {
        Self(resolved)
    }

    /// The resolved, absolute path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for WorkspacePath {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0.display())
    }
}
