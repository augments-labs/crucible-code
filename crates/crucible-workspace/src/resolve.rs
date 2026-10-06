//! Turning a path a caller wrote into one the workspace has proven it reaches.
//!
//! A path that must already exist and a path that is about to be created
//! cannot be resolved the same way: the first can be canonicalised whole, and
//! the second has a last component that is not there yet. [`Workspace::existing`]
//! and [`Workspace::creatable`] are the two that mint a proof from either
//! shape. Beside them, [`Workspace::outside`] mints one too, which only a read
//! the user was asked about may hold. [`Workspace::intended`] is the one that
//! mints nothing — it answers about a name, which is what the permission
//! boundary needs.
//!
//! The three that mint a proof answer about an instant. What a canonical path
//! settles is where a name led when it was asked, and a second writer can move
//! it afterwards — so what comes back is a resolved path with no symbolic link
//! anywhere in it, and [`open`](super::WorkspacePath::open) is what proves that
//! still true by walking it. The division is the point: containment is decided
//! here, once, about text somebody sent; whether the tree still agrees is
//! decided there, at the moment of the call.

use std::ffi::OsStr;
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
    ///
    /// The path is walked one component at a time, as the filesystem walks it
    /// when the write opens it. Each name that exists is canonicalised, so a
    /// symbolic link is followed to where it leads, and a `..` then steps out
    /// of that place rather than out of the text: `sub/../config.json` is
    /// `config.json`, and `link/../config.json` is beside wherever `link`
    /// points. Once a name is missing, the ordinary names below it are
    /// appended as written. A `..` after a missing name resolves nothing, since
    /// what it would lead to is not there to ask, and neither does a `..` after
    /// a file.
    ///
    /// Windows is the exception to that walk. Win32 makes the text a full path
    /// before it follows any link: it applies `..` to the text, so `link\..`
    /// is the folder that holds `link` wherever `link` points; it starts
    /// `C:..\x` from the current directory of `C:`; and it trims the dots and
    /// spaces that end the last name. The write's parent is made full that way
    /// here first, as [`Workspace::creatable`] has it made when it
    /// canonicalises it, and the walk starts from what comes back. A relative
    /// path arrives already full, because joining it to the verbatim root
    /// applies its `..` the same way. A verbatim path with a `..` in it
    /// resolves nothing: Win32 passes it on as written, and no file can be
    /// made under a folder called `..`.
    ///
    /// A name that would read as a drive or a root on its own resolves nothing
    /// either. Joined onto the path walked so far, it would replace it.
    ///
    /// What comes back is a plain [`PathBuf`], never a [`WorkspacePath`]. The
    /// permission engine that calls this lives in another crate and needs the
    /// name to describe the call it is settling; handing it a proof instead
    /// would let a question about a path become the authority to open one.
    #[must_use]
    pub fn intended(&self, requested: &str) -> Option<PathBuf> {
        let joined = self.join(requested);
        #[cfg(windows)]
        let joined = win32(&joined)?;
        let mut resolved = PathBuf::new();
        let mut missing: Vec<&OsStr> = Vec::new();

        for part in joined.components() {
            match part {
                Component::Prefix(_) | Component::RootDir => resolved.push(part),
                Component::CurDir => {}
                Component::ParentDir if !missing.is_empty() || !resolved.is_dir() => {
                    return None;
                }
                // Every name in `resolved` was canonicalised, so it holds no
                // link and its parent is where `..` leads.
                Component::ParentDir => {
                    resolved.pop();
                }
                Component::Normal(name)
                    if !matches!(
                        Path::new(name).components().next(),
                        Some(Component::Normal(_))
                    ) =>
                {
                    return None;
                }
                Component::Normal(name) if missing.is_empty() => {
                    match resolved.join(name).canonicalize() {
                        Ok(next) => resolved = next,
                        Err(_) => missing.push(name),
                    }
                }
                Component::Normal(name) => missing.push(name),
            }
        }

        self.roots.containing(&resolved)?;

        for name in missing {
            resolved.push(name);
        }

        self.roots.containing(&resolved).map(|_| resolved)
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

    /// Resolves an existing path that lies outside every directory the
    /// workspace reaches — for a read the user is asked about.
    ///
    /// The proof is settled against the file's own parent directory — or the
    /// directory itself, where the path names one — with the same refusal of a
    /// symbolic link planted below it that a contained path gets. The tool's
    /// `run` resolves afresh through here and holds the answer against the
    /// target its verdict named, so a link retargeted between the question and
    /// the open is refused rather than followed. A path that turns out to be
    /// contained after all is handed back proved against the root that
    /// contains it, exactly as [`Workspace::existing`] would have.
    ///
    /// # Errors
    ///
    /// [`PathError::Missing`] if it does not exist, [`PathError::NoParent`] if
    /// it resolves to a file with no directory over it.
    pub fn outside(&self, requested: &str) -> Result<WorkspacePath, PathError> {
        let resolved =
            self.join(requested)
                .canonicalize()
                .map_err(|source| PathError::Missing {
                    requested: requested.into(),
                    source,
                })?;

        if let Some(root) = self.roots.containing(&resolved) {
            return Ok(WorkspacePath::proven(root, resolved));
        }

        let from: std::sync::Arc<Path> = if resolved.is_dir() {
            resolved.as_path().into()
        } else {
            resolved
                .parent()
                .ok_or_else(|| PathError::NoParent {
                    requested: requested.into(),
                })?
                .into()
        };

        Ok(WorkspacePath::proven(from, resolved))
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

/// The path Win32 opens for this one: its parent made full as
/// `GetFullPathNameW` makes it, which is what canonicalising that parent
/// starts from, with the last name kept as written. A name the walk joins
/// below a canonical path is joined verbatim, so nothing trims it there. A
/// verbatim path is passed on as written, so one holding a `..` is not a path
/// anything could be created at.
#[cfg(windows)]
fn win32(path: &Path) -> Option<PathBuf> {
    let verbatim = matches!(
        path.components().next(),
        Some(Component::Prefix(prefix)) if prefix.kind().is_verbatim()
    );
    if verbatim {
        return (!path.components().any(|part| part == Component::ParentDir))
            .then(|| path.to_path_buf());
    }

    let full = std::path::absolute(path.parent()?).ok()?;
    // Where that parent is there, it is the one `creatable` opens the file
    // under, and the last name joins it verbatim, as it does there. Where it is
    // not, the walk finds how much of it is.
    let full = full.canonicalize().unwrap_or(full);
    Some(full.join(path.file_name()?))
}
