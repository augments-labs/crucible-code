//! Which files a tool can see.
//!
//! One place decides it, so `grep` and `glob` can never disagree about what is
//! in the workspace — an agent told a file does not exist by one tool and shown
//! it by the other has no way to tell which answer to believe.

use std::path::Path;

use crucible_core::Workspace;
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

/// How a file the walk reached is named back to the model.
///
/// Relative to the root where it is under it, and left absolute where it is
/// not: a directory the workspace was widened to reach has no spelling
/// relative to the root, and a hit dropped for want of one is a file the model
/// is told does not exist. Here beside the walk for the same reason the walk is
/// here — two tools that read the same tree may not name the same file two
/// ways, because the name is what the model hands back as the next call's
/// `path` and what somebody writes a deny rule about.
pub(crate) fn named(workspace: &Workspace, reached: &Path) -> String {
    crucible_core::written(reached.strip_prefix(workspace.root()).unwrap_or(reached))
}
