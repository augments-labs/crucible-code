//! The directories the workspace reaches, and the containment check over them.
//!
//! The predicate lives beside the data it tests because the two change
//! together: every directory added here is a directory `contains` must accept,
//! and there is no version of this file where one is right and the other is
//! not.

use std::path::{Path, PathBuf};

use super::PathError;

/// The set of directories a workspace reaches, each resolved once.
#[derive(Debug, Clone)]
pub(super) struct Roots {
    /// The directory crucible was started in, canonical. Relative paths are
    /// joined to this one and `bash` runs here, so it stays distinguishable
    /// from the rest however many others are added.
    root: PathBuf,

    /// Further directories, named in configuration rather than discovered.
    /// Reaching them is a decision somebody wrote down; nothing here grows on
    /// its own.
    extra: Vec<PathBuf>,
}

impl Roots {
    /// Resolves the root, which must exist.
    pub(super) fn open(root: &Path) -> Result<Self, PathError> {
        Ok(Self {
            root: canonical(root, &root.display().to_string())?,
            extra: Vec::new(),
        })
    }

    /// Adds a directory outside the root that the workspace should also reach.
    ///
    /// It must be absolute: an entry read from a configuration file is not
    /// relative to anything the file itself knows, and resolving it against
    /// whichever directory crucible happened to start in would give one
    /// setting a different meaning per invocation.
    pub(super) fn reach(&mut self, directory: &str) -> Result<(), PathError> {
        let given = Path::new(directory);
        if !given.is_absolute() {
            return Err(PathError::Relative {
                requested: directory.into(),
            });
        }
        self.extra.push(canonical(given, directory)?);
        Ok(())
    }

    /// The directory crucible was started in.
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// Whether a resolved path lies under any directory the workspace reaches.
    ///
    /// The argument is already canonical — that is the whole reason this
    /// comparison can be trusted. A check against the text a caller supplied
    /// can be walked around with `..` or a symbolic link; a check against the
    /// path the operating system resolved cannot.
    pub(super) fn contains(&self, resolved: &Path) -> bool {
        resolved.starts_with(&self.root)
            || self
                .extra
                .iter()
                .any(|reached| resolved.starts_with(reached))
    }
}

/// Resolves a directory, reporting it under the name the caller used rather
/// than the one this function built.
fn canonical(directory: &Path, requested: &str) -> Result<PathBuf, PathError> {
    directory
        .canonicalize()
        .map_err(|source| PathError::Missing {
            requested: requested.into(),
            source,
        })
}
