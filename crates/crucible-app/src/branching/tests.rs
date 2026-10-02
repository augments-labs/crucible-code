//! What `.git/HEAD` spellings come back as a branch, and which come back as
//! nothing; and which checkouts a repository's own files say it has.

use std::fs;
use std::path::{Path, PathBuf};

use super::{current, worktrees};

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

/// A repository laid out as `git worktree add` leaves it: a main checkout at
/// `main` whose `.git` directory keeps `worktrees/<name>/gitdir` for each
/// linked checkout, and each linked checkout's `.git` file pointing back.
struct Repository {
    scratch: Scratch,
}

impl Repository {
    fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        fs::create_dir_all(scratch.root().join("main").join(".git")).expect("the main checkout");
        Self { scratch }
    }

    fn main(&self) -> PathBuf {
        canonical(&self.scratch.root().join("main"))
    }

    /// A linked checkout at `name`, recorded under the common directory the
    /// way git spells it when `relative` is false, and with git's relative
    /// spelling when it is true.
    fn linked(&self, name: &str, relative: bool) -> PathBuf {
        let checkout = self.scratch.root().join(name);
        let admin = self
            .scratch
            .root()
            .join("main")
            .join(".git")
            .join("worktrees")
            .join(name);
        fs::create_dir_all(&checkout).expect("the linked checkout");
        fs::create_dir_all(&admin).expect("the worktree's own git directory");

        let (back, forth) = if relative {
            (
                format!("../../../../{name}/.git"),
                format!("../main/.git/worktrees/{name}"),
            )
        } else {
            (
                checkout.join(".git").display().to_string(),
                admin.display().to_string(),
            )
        };
        fs::write(admin.join("gitdir"), format!("{back}\n")).expect("the gitdir file");
        fs::write(admin.join("commondir"), "../..\n").expect("the commondir file");
        fs::write(checkout.join(".git"), format!("gitdir: {forth}\n")).expect("the .git file");

        canonical(&checkout)
    }
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).expect("the checkout exists")
}

#[test]
fn the_main_checkout_finds_each_linked_one() {
    let repository = Repository::new("from-main");
    let first = repository.linked("first", false);
    let second = repository.linked("second", true);

    let mut found = worktrees(&repository.main());
    found.sort();
    let mut expected = vec![first, second];
    expected.sort();

    assert_eq!(found, expected);
}

#[test]
fn a_linked_checkout_finds_the_main_one_and_its_siblings_but_not_itself() {
    let repository = Repository::new("from-linked");
    let first = repository.linked("first", false);
    let second = repository.linked("second", true);

    let mut found = worktrees(&first);
    found.sort();
    let mut expected = vec![repository.main(), second.clone()];
    expected.sort();
    assert_eq!(found, expected);

    // Spelled relatively, the pointers lead to the same places.
    let mut found = worktrees(&second);
    found.sort();
    let mut expected = vec![repository.main(), first];
    expected.sort();
    assert_eq!(found, expected);
}

#[test]
fn a_checkout_whose_directory_is_gone_is_left_out() {
    // `git worktree prune` has not run yet: the record outlives the checkout.
    let repository = Repository::new("pruned");
    let gone = repository.linked("gone", false);
    fs::remove_dir_all(&gone).expect("the checkout removed");

    assert!(worktrees(&repository.main()).is_empty());
}

#[test]
fn a_repository_without_other_checkouts_or_no_repository_adds_nothing() {
    let repository = Repository::new("alone");
    assert!(worktrees(&repository.main()).is_empty());

    let scratch = Scratch::new("no-repository");
    assert!(worktrees(scratch.root()).is_empty());
}
