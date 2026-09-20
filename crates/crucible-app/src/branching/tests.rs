//! What `.git/HEAD` spellings come back as a branch, and which come back as
//! nothing.

use std::fs;
use std::path::{Path, PathBuf};

use super::current;

/// A directory of its own to lay a checkout out in, deleted with it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("crucible-branching-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("a temporary directory");

        Self(base)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    /// A `.git/HEAD` holding `content`, laid out the way an ordinary checkout
    /// leaves it.
    fn headed(&self, content: &str) {
        fs::create_dir_all(self.0.join(".git")).expect("a .git directory");
        fs::write(self.0.join(".git").join("HEAD"), content).expect("a HEAD file");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_checkout_on_a_branch_names_it() {
    let scratch = Scratch::new("named");
    scratch.headed("ref: refs/heads/feature/caret-drift\n");

    assert_eq!(
        current(scratch.root()).as_deref(),
        Some("feature/caret-drift")
    );
}

#[test]
fn a_detached_head_is_no_branch() {
    // A bare commit hash is where the checkout is, not a name anybody gave it.
    let scratch = Scratch::new("detached");
    scratch.headed("a94a8fe5ccb19ba61c4c0873d391e987982fbbd3\n");

    assert_eq!(current(scratch.root()), None);
}

#[test]
fn a_directory_without_a_repository_is_no_branch() {
    let scratch = Scratch::new("bare");

    assert_eq!(current(scratch.root()), None);
}

#[test]
fn a_linked_worktree_names_its_own_branch() {
    // In a worktree `.git` is a file pointing at the real directory, and the
    // HEAD that answers for this checkout lives there — not in the main
    // checkout's, which is on a different branch by construction.
    let scratch = Scratch::new("worktree");
    let elsewhere = scratch.root().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("the pointed-at git directory");
    fs::write(elsewhere.join("HEAD"), "ref: refs/heads/fix/wrapping\n").expect("a HEAD file");

    let checkout = scratch.root().join("checkout");
    fs::create_dir_all(&checkout).expect("the worktree checkout");
    fs::write(
        checkout.join(".git"),
        format!("gitdir: {}\n", elsewhere.display()),
    )
    .expect("the .git pointer file");

    assert_eq!(current(&checkout).as_deref(), Some("fix/wrapping"));
}
