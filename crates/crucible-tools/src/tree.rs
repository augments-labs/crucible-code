//! Which files a tool can see.
//!
//! One place decides it, so `grep` and `glob` can never disagree about what is
//! in the workspace — an agent told a file does not exist by one tool and shown
//! it by the other has no way to tell which answer to believe.

use std::path::Path;

use ignore::WalkBuilder;

/// A walk rooted at `from`, skipping what the workspace says to skip.
pub(crate) fn walk(from: &Path) -> WalkBuilder {
    let mut walk = WalkBuilder::new(from);

    // Hidden files, `.gitignore`, `.ignore`, and the global git excludes.
    walk.standard_filters(true);

    // Read `.gitignore` even where there is no `.git` yet, which is what the
    // `rg` default declines to do. A file listed there is one the user has
    // already called noise; a project that has not been committed yet is still
    // a project, and reporting its build output back to the model wastes the
    // turn on something nobody can act on.
    walk.require_git(false);

    walk
}
