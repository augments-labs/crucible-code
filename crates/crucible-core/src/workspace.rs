//! The workspace and the paths inside it.
//!
//! This is the only place path semantics are decided. A tool never joins,
//! compares or canonicalises a path itself; it asks the workspace for a
//! [`WorkspacePath`] and the existence of that value is the proof that the
//! path is one the agent was pointed at.
//!
//! Containment is checked against canonical paths, so `..` and symbolic links
//! are resolved before the comparison rather than pattern-matched out of the
//! text. A check that inspects the string a caller supplied can be walked
//! around; a check on the path the operating system actually resolved cannot.
//!
//! A workspace reaches the directory crucible was started in and any further
//! directories configuration named. Which of them a path lands in makes no
//! difference to anything downstream: reach is settled here, once, and what
//! happens to a path that is inside is a permission question rather than a
//! path one.

use std::path::Path;

mod error;
mod path;
mod resolve;
mod roots;
mod spelling;
#[cfg(test)]
mod tests;

pub use error::PathError;
pub use path::WorkspacePath;
pub use spelling::written;

use roots::Roots;

/// A working directory, resolved once.
///
/// Held by the wiring in `main` and handed to the tools, so every tool in a
/// session agrees on what "inside" means.
#[derive(Debug, Clone)]
pub struct Workspace {
    roots: Roots,
}

impl Workspace {
    /// Opens a directory as the workspace root.
    ///
    /// # Errors
    ///
    /// [`PathError::Missing`] if the directory does not exist or cannot be
    /// resolved, and [`PathError::NotText`] if it resolves to a name that is
    /// not valid UTF-8.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PathError> {
        let roots = Roots::open(root.as_ref())?;

        // The root is a path that also has to be text. A session log records it
        // and compares the recorded spelling back to decide which log belongs
        // to this directory, and a name with bytes that are not UTF-8 comes
        // back through that trip as replacement characters — a root that never
        // equals itself, and a `--continue` that reports no earlier session
        // while the log sits in the directory. Refused here, once, so nothing
        // downstream has to carry the question.
        if roots.root().to_str().is_none() {
            return Err(PathError::NotText {
                resolved: written(roots.root()).into(),
            });
        }

        Ok(Self { roots })
    }

    /// Widens the workspace to reach directories outside the root.
    ///
    /// Called once, by the wiring, with what configuration said. It is not a
    /// capability the running agent can grant itself: a widened workspace is
    /// built before the first turn and never after one.
    ///
    /// # Errors
    ///
    /// [`PathError::Relative`] if an entry is not an absolute path, and
    /// [`PathError::Missing`] if it names a directory that is not there.
    pub fn reaching(
        mut self,
        directories: impl IntoIterator<Item = impl AsRef<str>>,
    ) -> Result<Self, PathError> {
        for directory in directories {
            self.roots.reach(directory.as_ref())?;
        }
        Ok(self)
    }

    /// The directory crucible was started in.
    ///
    /// Still the root once other directories are reachable: it is `bash`'s
    /// working directory and the anchor a relative path is joined to, and
    /// neither of those is a set.
    #[must_use]
    pub fn root(&self) -> &Path {
        self.roots.root()
    }
}
