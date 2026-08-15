//! Turning a path a caller wrote into one the workspace has proven it reaches.
//!
//! Two entry points, because a path that must already exist and a path that is
//! about to be created cannot be resolved the same way: the first can be
//! canonicalised whole, and the second has a last component that is not there
//! yet.
//!
//! Both answer about an instant. What a canonical path settles is where a name
//! led when it was asked, and a second writer can move it afterwards — so what
//! comes back is a resolved path with no symbolic link anywhere in it, and
//! [`open`](super::WorkspacePath::open) is what proves that still true by
//! walking it. The division is the point: containment is decided here, once,
//! about text somebody sent; whether the tree still agrees is decided there,
//! at the moment of the call.

use std::path::{Component, Path, PathBuf};

use super::{PathError, Workspace, WorkspacePath};

impl Workspace {
    /// Resolves the name a write intends to create, through the nearest parent
    /// that already exists.
    ///
    /// Unlike [`Workspace::creatable`], this is only for describing a call at
    /// the permission boundary. A write can make several missing directories
    /// before it makes its file, so requiring the immediate parent here would
    /// turn the target into an unresolved one and hide its name from policy.
    /// The ancestor that does exist is still canonicalised and contained; only
    /// the ordinary names below it are appended. A `..` among the missing
    /// components therefore resolves nothing rather than being guessed at.
    pub(crate) fn intended(&self, requested: &str) -> Option<PathBuf> {
        let mut ancestor = self.join(requested);
        let mut missing = Vec::new();

        loop {
            if let Ok(mut resolved) = ancestor.canonicalize() {
                self.roots.containing(&resolved)?;

                for name in missing.iter().rev() {
                    resolved.push(name);
                }

                return self.roots.containing(&resolved).map(|_| resolved);
            }

            let Component::Normal(name) = ancestor.components().next_back()? else {
                // A root or prefix cannot be appended below an existing
                // ancestor, and a parent component would make the lexical
                // target say something different after the permission
                // question was answered.
                return None;
            };
            missing.push(name.to_owned());
            ancestor = ancestor.parent()?.to_owned();
        }
    }

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
    ///
    /// What comes back carries the directory it was found under as well as the
    /// path, because that directory is where opening it later starts from —
    /// see [`open`](super::WorkspacePath::open).
    fn contain(&self, requested: &str, resolved: PathBuf) -> Result<WorkspacePath, PathError> {
        match self.roots.containing(&resolved) {
            Some(root) => Ok(WorkspacePath::proven(root, resolved)),
            None => Err(PathError::Escapes {
                requested: requested.into(),
            }),
        }
    }
}
