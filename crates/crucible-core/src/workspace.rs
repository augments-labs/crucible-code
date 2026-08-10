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
    /// Returns [`PathError::Missing`] if the directory does not exist or
    /// cannot be resolved.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, PathError> {
        Ok(Self {
            roots: Roots::open(root.as_ref())?,
        })
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
