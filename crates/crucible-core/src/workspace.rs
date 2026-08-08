//! The workspace and the paths inside it.
//!
//! This is the only place path semantics are decided. A tool never joins,
//! compares or canonicalises a path itself; it asks the workspace for a
//! [`WorkspacePath`] and the existence of that value is the proof that the
//! path is inside the tree the agent was pointed at.
//!
//! Containment is checked against canonical paths, so `..` and symbolic links
//! are resolved before the comparison rather than pattern-matched out of the
//! text. A check that inspects the string a caller supplied can be walked
//! around; a check on the path the operating system actually resolved cannot.

use std::fmt;
use std::path::{Path, PathBuf};

/// Why a path could not be resolved inside the workspace.
#[derive(Debug, thiserror::Error)]
pub enum PathError {
    /// The path resolved outside the workspace root.
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

    /// The path has no parent directory, so nothing could be created there.
    #[error("{requested} has no parent directory")]
    NoParent {
        /// The path as the caller wrote it.
        requested: Box<str>,
    },
}

/// A working directory, resolved once.
///
/// Held by the wiring in `main` and handed to the tools, so every tool in a
/// session agrees on what "inside" means.
#[derive(Debug, Clone)]
pub struct Workspace {
    /// Canonical, so containment is a comparison of resolved paths.
    root: PathBuf,
}

impl Workspace {
    /// Opens a directory as the workspace root.
    ///
    /// # Errors
    ///
    /// Returns [`PathError::Missing`] if the directory does not exist or
    /// cannot be resolved.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PathError> {
        let root = root.as_ref();
        let canonical = root.canonicalize().map_err(|source| PathError::Missing {
            requested: root.display().to_string().into(),
            source,
        })?;
        Ok(Self { root: canonical })
    }

    /// The canonical root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
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
    /// created through a directory that was never checked.
    ///
    /// # Errors
    ///
    /// [`PathError::NoParent`] if the path names no directory,
    /// [`PathError::Missing`] if that directory does not exist, and
    /// [`PathError::Escapes`] if the result lands outside the workspace.
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

        self.contain(requested, parent.join(name))
    }

    /// A relative path is relative to the root; an absolute one is taken as
    /// written and will be rejected by containment unless it is already
    /// inside.
    fn join(&self, requested: &str) -> PathBuf {
        let requested = Path::new(requested);
        if requested.is_absolute() {
            requested.to_path_buf()
        } else {
            self.root.join(requested)
        }
    }

    /// The containment check itself, on resolved paths.
    fn contain(&self, requested: &str, resolved: PathBuf) -> Result<WorkspacePath, PathError> {
        if resolved.starts_with(&self.root) {
            Ok(WorkspacePath(resolved))
        } else {
            Err(PathError::Escapes {
                requested: requested.into(),
            })
        }
    }
}

/// A path proven to be inside the workspace.
///
/// There is no public constructor: the only way to hold one is to have asked
/// a [`Workspace`] for it, so a function taking this type cannot be handed a
/// path from anywhere else.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WorkspacePath(PathBuf);

impl WorkspacePath {
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    /// A workspace with a file in it, plus a sibling directory outside it that
    /// the escape attempts aim at.
    struct Fixture {
        outside: PathBuf,
        workspace: Workspace,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let base =
                std::env::temp_dir().join(format!("crucible-ws-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&base);
            let root = base.join("inside");
            let outside = base.join("outside");
            fs::create_dir_all(root.join("sub")).unwrap();
            fs::create_dir_all(&outside).unwrap();
            fs::write(root.join("kept.txt"), "in").unwrap();
            fs::write(outside.join("secret.txt"), "out").unwrap();
            Self {
                outside: outside.canonicalize().unwrap(),
                workspace: Workspace::open(&root).unwrap(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(base) = self.workspace.root().parent() {
                let _ = fs::remove_dir_all(base);
            }
        }
    }

    #[test]
    fn a_path_inside_the_workspace_resolves() {
        let f = Fixture::new("inside");
        let path = f.workspace.existing("kept.txt").unwrap();
        assert!(path.as_path().starts_with(f.workspace.root()));
        assert!(path.as_path().ends_with("kept.txt"));
    }

    #[test]
    fn dot_dot_that_climbs_out_is_rejected() {
        let f = Fixture::new("dotdot");
        let err = f.workspace.existing("../outside/secret.txt").unwrap_err();
        assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
    }

    #[test]
    fn an_absolute_path_outside_is_rejected() {
        let f = Fixture::new("absolute");
        let target = f.outside.join("secret.txt");
        let err = f
            .workspace
            .existing(&target.display().to_string())
            .unwrap_err();
        assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
    }

    #[test]
    fn a_symlink_pointing_out_is_rejected() {
        let f = Fixture::new("symlink");
        let link = f.workspace.root().join("escape.txt");
        std::os::unix::fs::symlink(f.outside.join("secret.txt"), &link).unwrap();

        // The text "escape.txt" says nothing about where it goes. Only the
        // resolved path does, which is why containment runs after
        // canonicalisation and not on what the caller typed.
        let err = f.workspace.existing("escape.txt").unwrap_err();
        assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
    }

    #[test]
    fn dot_dot_that_stays_inside_is_allowed() {
        let f = Fixture::new("staysin");
        let path = f.workspace.existing("sub/../kept.txt").unwrap();
        assert!(path.as_path().ends_with("kept.txt"));
    }

    #[test]
    fn a_new_file_inside_is_creatable() {
        let f = Fixture::new("create");
        let path = f.workspace.creatable("sub/new.txt").unwrap();
        assert!(path.as_path().starts_with(f.workspace.root()));
        assert!(!path.as_path().exists());
    }

    #[test]
    fn a_new_file_outside_is_not_creatable() {
        let f = Fixture::new("createout");
        let err = f
            .workspace
            .creatable("sub/../../outside/new.txt")
            .unwrap_err();
        assert!(matches!(err, PathError::Escapes { .. }), "got {err:?}");
    }

    #[test]
    fn creating_under_a_directory_that_does_not_exist_is_rejected() {
        let f = Fixture::new("nodir");
        let err = f.workspace.creatable("absent/new.txt").unwrap_err();
        assert!(matches!(err, PathError::Missing { .. }), "got {err:?}");
    }

    #[test]
    fn a_missing_file_is_missing_rather_than_escaping() {
        let f = Fixture::new("missing");
        let err = f.workspace.existing("absent.txt").unwrap_err();
        assert!(matches!(err, PathError::Missing { .. }), "got {err:?}");
    }
}
