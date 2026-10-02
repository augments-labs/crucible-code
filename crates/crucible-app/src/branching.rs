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
//!
//! The same files say which other checkouts the repository has, for the resume
//! picker's widening to a repository's worktrees: the common git directory
//! keeps `worktrees/<name>/gitdir` for each linked checkout, naming that
//! checkout's `.git` file. Read rather than asked of `git worktree list` for
//! the same reasons, and with the same quiet answer — a directory that is not a
//! repository, or one with no other checkout, has none to add.

use std::fs;
use std::path::{Path, PathBuf};

/// The branch checked out at `root`, or `None` where there is no branch to
/// name — no repository, a detached head, or a spelling of `.git` this does
/// not read.
pub fn current(root: &Path) -> Option<String> {
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

/// How many linked checkouts one repository is read for. A listing filter
/// rather than an inventory: a repository past this is left with the first
/// ones its directory yields.
const WORKTREES: usize = 256;

/// This repository's checkouts other than `root`: the main one and every
/// linked one, each spelled the way [`crucible_workspace::Workspace::open`]
/// spells a root, so a session recorded in one compares equal to it. A record
/// whose checkout has since been deleted is left out.
pub fn worktrees(root: &Path) -> Vec<PathBuf> {
    let Some(common) = common(root) else {
        return Vec::new();
    };

    let mut checkouts = Vec::new();

    // The main checkout is the directory the common `.git` sits in. A bare
    // repository's common directory is not called that, and has none.
    if common.file_name() == Some(".git".as_ref())
        && let Some(main) = common.parent()
    {
        checkouts.push(main.to_path_buf());
    }

    if let Ok(entries) = fs::read_dir(common.join("worktrees")) {
        for entry in entries.flatten().take(WORKTREES) {
            let admin = entry.path();
            let Ok(pointed) = fs::read_to_string(admin.join("gitdir")) else {
                continue;
            };
            // Absolute as git writes it by default; relative to this record's
            // directory when the repository asked for relative paths.
            if let Some(checkout) = admin.join(pointed.trim()).parent() {
                checkouts.push(checkout.to_path_buf());
            }
        }
    }

    let here = rooted(root);
    let mut roots: Vec<PathBuf> = checkouts
        .iter()
        .filter_map(|checkout| rooted(checkout))
        .filter(|checkout| Some(checkout) != here.as_ref())
        .collect();
    roots.sort();
    roots.dedup();
    roots
}

/// The git directory every checkout of `root`'s repository shares.
fn common(root: &Path) -> Option<PathBuf> {
    let git = root.join(".git");
    if git.is_dir() {
        return fs::canonicalize(git).ok();
    }

    // A linked checkout's `.git` file names its own git directory, and that
    // directory's `commondir` names the shared one, relative to itself. A
    // `.git` file with no `commondir` beside its target — a submodule — has
    // its own directory as the common one.
    let pointed = fs::read_to_string(&git).ok()?;
    let own = root.join(pointed.strip_prefix("gitdir:")?.trim());
    let common = match fs::read_to_string(own.join("commondir")) {
        Ok(common) => own.join(common.trim()),
        Err(_) => own,
    };
    fs::canonicalize(common).ok()
}

/// `checkout` as a workspace root spells it, or `None` when it is gone.
fn rooted(checkout: &Path) -> Option<PathBuf> {
    crucible_workspace::Workspace::open(checkout)
        .ok()
        .map(|workspace| workspace.root().to_path_buf())
}

#[cfg(test)]
mod tests;
