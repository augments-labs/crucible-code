//! Turning a path a caller wrote into one the workspace has proven it reaches.
//!
//! Two entry points, because a path that must already exist and a path that is
//! about to be created cannot be resolved the same way: the first can be
//! canonicalised whole, and the second has a last component that is not there
//! yet.

use std::path::{Path, PathBuf};

use super::{PathError, Workspace, WorkspacePath};

impl Workspace {
    /// Resolves a path that must already exist — what `read`, `grep` and
    /// `edit` need.
    ///
    /// # Errors
    ///
    /// [`PathError::Missing`] if it does not exist, [`PathError::Escapes`] if
    /// it resolves outside the workspace.
    pub fn existing(&self, requested: &str) -> Result<WorkspacePath, PathError> {
        let resolved =
            self.join(requested)
                .canonicalize()
                .map_err(|source| PathError::Missing {
                    requested: requested.into(),
                    source,
                })?;
        self.contain(requested, resolved)
    }

    /// Resolves a path that may not exist yet — what `write` needs.
    ///
    /// The parent directory must exist and must itself be inside the
    /// workspace, which is what stops `subdir/../../outside.txt` from being
    /// created through a directory that was never checked. A symbolic link at
    /// the final component is resolved too, so a link inside the tree cannot
    /// be used to write through to a file outside it.
    ///
    /// # Errors
    ///
    /// [`PathError::NoParent`] if the path names no directory,
    /// [`PathError::Missing`] if that directory does not exist,
    /// [`PathError::Dangling`] if the last component is a symbolic link whose
    /// target cannot be resolved, and [`PathError::Escapes`] if the result
    /// lands outside the workspace.
    pub fn creatable(&self, requested: &str) -> Result<WorkspacePath, PathError> {
        let joined = self.join(requested);

        let (parent, name) =
            joined
                .parent()
                .zip(joined.file_name())
                .ok_or_else(|| PathError::NoParent {
                    requested: requested.into(),
                })?;

        let parent = parent.canonicalize().map_err(|source| PathError::Missing {
            requested: requested.into(),
            source,
        })?;

        let leaf = parent.join(name);

        // The parent is resolved but the last component is not, and a symbolic
        // link there points wherever it likes: `notes.txt -> ~/.ssh/authorized_keys`
        // is lexically inside the workspace and writes outside it. So a leaf
        // that is a link is resolved as well, and one whose target cannot be
        // resolved is refused rather than guessed at — writing through a
        // dangling link creates the file at the far end.
        //
        // Refused under its own name, though. A link that leads nowhere may
        // well point back inside the tree, and calling that an escape tells the
        // model to stop trying to reach a path it is perfectly entitled to.
        if leaf.is_symlink() {
            let resolved = leaf.canonicalize().map_err(|source| PathError::Dangling {
                requested: requested.into(),
                source,
            })?;
            return self.contain(requested, resolved);
        }

        self.contain(requested, leaf)
    }

    /// A relative path is relative to the root; an absolute one is taken as
    /// written and will be rejected by containment unless it is already inside
    /// a directory the workspace reaches.
    fn join(&self, requested: &str) -> PathBuf {
        let requested = Path::new(requested);
        if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.roots.root().join(requested)
        }
    }

    /// The containment check itself, on resolved paths.
    fn contain(&self, requested: &str, resolved: PathBuf) -> Result<WorkspacePath, PathError> {
        if self.roots.contains(&resolved) {
            Ok(WorkspacePath::proven(resolved))
        } else {
            Err(PathError::Escapes {
                requested: requested.into(),
            })
        }
    }
}
