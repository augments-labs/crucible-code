//! The directories the workspace reaches, and the containment check over them.
//!
//! The predicate lives beside the data it tests because the two change
//! together: every directory added here is a directory `contains` must accept,
//! and there is no version of this file where one is right and the other is
//! not.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::PathError;

/// The set of directories a workspace reaches, each resolved once.
///
/// Each is held behind an `Arc` because every path proven against one keeps it:
/// opening that path walks down from the directory it was proved against, so
/// the two travel together and are cloned together.
#[derive(Debug, Clone)]
pub(super) struct Roots {
    /// The directory crucible was started in, canonical. Relative paths are
    /// joined to this one and `bash` runs here, so it stays distinguishable
    /// from the rest however many others are added.
    root: Arc<Path>,

    /// Further directories, named in configuration rather than discovered.
    /// Reaching them is a decision somebody wrote down; nothing here grows on
    /// its own.
    extra: Vec<Arc<Path>>,
}

impl Roots {
    /// Resolves the root, which must exist.
    pub(super) fn open(root: &Path) -> Result<Self, PathError> {
        Ok(Self {
            root: canonical_directory(root, &root.display().to_string())?.into(),
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
        self.extra
            .push(canonical_directory(given, directory)?.into());
        Ok(())
    }

    /// The directory crucible was started in.
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// All explicitly granted roots, with the primary root first.
    pub(super) fn iter(&self) -> impl Iterator<Item = &Path> {
        std::iter::once(self.root.as_ref()).chain(self.extra.iter().map(AsRef::as_ref))
    }

    /// Which directory the workspace reaches a resolved path through, if any.
    ///
    /// The argument is already canonical — that is the whole reason this
    /// comparison can be trusted. A check against the text a caller supplied
    /// can be walked around with `..` or a symbolic link; a check against the
    /// path the operating system resolved cannot.
    ///
    /// The answer is the directory rather than a yes, because opening the path
    /// later walks down to it from exactly here. A walk that started anywhere
    /// else would be proving something about a tree nobody was pointed at.
    pub(super) fn containing(&self, resolved: &Path) -> Option<Arc<Path>> {
        std::iter::once(&self.root)
            .chain(&self.extra)
            .find(|reached| resolved.starts_with(reached))
            .map(Arc::clone)
    }
}

/// Resolves a directory, reporting it under the name the caller used rather
/// than the one this function built.
fn canonical_directory(directory: &Path, requested: &str) -> Result<PathBuf, PathError> {
    let resolved = directory
        .canonicalize()
        .map_err(|source| PathError::Missing {
            requested: requested.into(),
            source,
        })?;
    let metadata = resolved.metadata().map_err(|source| PathError::Missing {
        requested: requested.into(),
        source,
    })?;
    if !metadata.is_dir() {
        return Err(PathError::NotDirectory {
            requested: requested.into(),
        });
    }
    Ok(resolved)
}
