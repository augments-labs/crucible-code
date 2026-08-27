//! Which branch a workspace is on, read the way git leaves it written.
//!
//! A session records the branch it started on so the resume picker can put it
//! on the row — somebody who works in branches remembers `fix/caret-drift`
//! long after the first prompt's words are gone. The answer comes from
//! `.git/HEAD` directly rather than from running `git`, because this runs at
//! session start on every launch: a child process would put an exec on the
//! startup path, and a repository without git installed still has the file.
//!
//! Everything short of a branch name is `None` — a detached head, a directory
//! that is not a repository, a file this build does not recognise. The branch
//! is decoration on a listing, and a listing is not the place to report what
//! is unusual about a checkout.

use std::fs;
use std::path::Path;

/// The branch checked out at `root`, or `None` where there is no branch to
/// name — no repository, a detached head, or a spelling of `.git` this does
/// not read.
pub(crate) fn current(root: &Path) -> Option<String> {
    let git = root.join(".git");

    // In a linked worktree `.git` is a file naming where the real directory
    // is, and that is where this worktree's own HEAD lives.
    let head = if git.is_file() {
        let pointed = fs::read_to_string(&git).ok()?;
        Path::new(pointed.strip_prefix("gitdir:")?.trim()).join("HEAD")
    } else {
        git.join("HEAD")
    };

    let head = fs::read_to_string(head).ok()?;

    // `ref: refs/heads/<branch>` on a branch; a bare commit hash detached.
    Some(head.strip_prefix("ref: refs/heads/")?.trim().to_owned())
        .filter(|branch| !branch.is_empty())
}

#[cfg(test)]
mod tests;
