//! The proof that a path is one the workspace reaches.

use std::fmt;
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use super::{PathError, open, written};

/// A path proven to be inside the workspace.
///
/// There is no public constructor: the only way to hold one is to have asked
/// a [`Workspace`](super::Workspace) for it, so a function taking this type
/// cannot be handed a path from anywhere else.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspacePath {
    /// The directory containment was settled against — the root, or whichever
    /// reached directory this path turned out to lie under.
    ///
    /// Kept rather than looked up again, because it is where opening this path
    /// starts from: `open` walks down to the file one component at a time, and
    /// the first step has to be a directory somebody pointed crucible at.
    /// Shared rather than copied, so cloning a path stays the cost of one
    /// `PathBuf`.
    root: Arc<Path>,

    /// The resolved, absolute path.
    resolved: PathBuf,
}

/// Files opened from the directories reached by one non-following tree walk.
///
/// A walker normally yields sibling files together. On Unix this keeps the
/// last sibling directory open, so every file after the first is one `openat`
/// instead of another descent from the workspace root. The directory remains
/// the one containment reached even if its name is replaced in the meantime.
#[derive(Debug)]
pub struct WalkFiles {
    pub(super) from: WorkspacePath,
    #[cfg(unix)]
    pub(super) parent: Option<(PathBuf, File)>,
}

impl WorkspacePath {
    /// Called only after containment has been decided, which is why this is
    /// visible to the workspace and to nothing else.
    pub(super) fn proven(root: Arc<Path>, resolved: PathBuf) -> Self {
        Self { root, resolved }
    }

    /// Proves a name yielded below this path by a non-following tree walk.
    ///
    /// A walker has already reached every directory component and reports its
    /// entries without following symbolic links. Only ordinary names below the
    /// proven starting path are accepted here. Opening the result repeats the
    /// containment-sensitive part at the operating-system boundary: a
    /// descriptor walk on Unix and final-handle validation on Windows.
    #[must_use]
    pub fn walked(&self, reached: &Path) -> Option<Self> {
        if has_parent(reached) {
            return None;
        }
        let below = reached.strip_prefix(&self.resolved).ok()?;
        if !below
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return None;
        }

        Some(Self {
            root: Arc::clone(&self.root),
            resolved: reached.to_owned(),
        })
    }

    /// Opens regular files yielded by a non-following walk below this path.
    ///
    /// The returned value keeps at most one directory descriptor. Make one per
    /// walk worker rather than sharing it: workers commonly visit different
    /// directories, and a shared cache would serialize the search.
    #[must_use]
    pub fn walk_files(&self) -> WalkFiles {
        WalkFiles {
            from: self.clone(),
            #[cfg(unix)]
            parent: None,
        }
    }

    /// The resolved, absolute path.
    #[must_use]
    pub fn as_path(&self) -> &Path {
        &self.resolved
    }

    /// The directory a walk down to this path starts from.
    ///
    /// Both of these belong to that walk, which is Unix's — the root is kept on
    /// every platform because it is half of what this value *is*, and asked for
    /// only where there is something to ask it for.
    #[cfg(unix)]
    pub(super) fn root(&self) -> &Path {
        &self.root
    }

    /// The part of this path below that directory, which is what such a walk
    /// takes one step at a time.
    ///
    /// `starts_with` decided containment and `strip_prefix` answers the same
    /// question, so the `None` arm is a case the type system carries and the
    /// filesystem cannot reach. It is written as the root itself rather than as
    /// an error, because that is the answer that opens the fewest things.
    #[cfg(unix)]
    pub(super) fn below_root(&self) -> &Path {
        self.resolved
            .strip_prefix(&self.root)
            .unwrap_or(Path::new(""))
    }
}

impl WalkFiles {
    /// Proves and opens one regular file the walk yielded.
    ///
    /// `None` means the name is not made only of ordinary components below the
    /// walk root. Filesystem changes and refusals remain typed path errors.
    ///
    /// # Errors
    ///
    /// The same path errors as [`WorkspacePath::open_regular`].
    pub fn open_regular(
        &mut self,
        reached: &Path,
    ) -> Result<Option<(WorkspacePath, File)>, PathError> {
        let Some(path) = self.from.walked(reached) else {
            return Ok(None);
        };
        let file = open::walked_regular(self, &path)?;
        Ok(Some((path, file)))
    }
}

/// Whether `path` contains a lexical parent component before prefix matching.
///
/// Windows' `strip_prefix` compares normalized components and can erase the
/// parent spelling before the remainder is inspected. Its path strings are
/// Unicode, so inspect separator-delimited text there; Unix keeps backslashes
/// as ordinary filename bytes and therefore uses `components`.
pub(super) fn has_parent(path: &Path) -> bool {
    #[cfg(windows)]
    return path
        .as_os_str()
        .to_string_lossy()
        .split(['/', '\\'])
        .any(|part| part == "..");

    #[cfg(not(windows))]
    path.components()
        .any(|part| matches!(part, Component::ParentDir))
}

impl AsRef<Path> for WorkspacePath {
    fn as_ref(&self) -> &Path {
        &self.resolved
    }
}

impl fmt::Display for WorkspacePath {
    /// Through [`written`] rather than `Path::display`. This is a resolved path
    /// becoming text somebody reads, which is the one job that door exists for;
    /// showing the spelling resolving happened to produce would put a second
    /// name for the same file into whatever printed it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&written(&self.resolved))
    }
}
