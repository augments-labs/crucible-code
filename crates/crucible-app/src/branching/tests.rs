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

/// What `read` answers, or `None` when it has not answered within a few
/// seconds: a read that waits on a pipe never returns, and a bound held outside
/// this test would leave the suite waiting instead of reporting.
#[cfg(unix)]
fn promptly<T: Send + 'static>(read: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (said, heard) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = said.send(read());
    });
    heard.recv_timeout(std::time::Duration::from_secs(5)).ok()
}

/// A pipe at `at`, which an ordinary open for reading waits on until a writer
/// comes.
#[cfg(unix)]
fn piped(at: &Path) {
    let _ = fs::remove_file(at);
    let made = std::process::Command::new("mkfifo")
        .arg(at)
        .status()
        .expect("mkfifo is available on Unix");
    assert!(made.success());
}

#[cfg(unix)]
#[test]
fn a_pipe_where_git_keeps_a_file_is_no_repository_and_no_wait() {
    // A checkout is hostile input: a `.git` left as a pipe would hold
    // `/resume` until something wrote to it.
    let scratch = Scratch::new("pipe-dot-git");
    piped(&scratch.root().join(".git"));
    let root = scratch.root().to_path_buf();
    assert_eq!(promptly(move || worktrees(&root)), Some(Vec::new()));
    let root = scratch.root().to_path_buf();
    assert_eq!(promptly(move || current(&root)), Some(None));

    // HEAD, where the branch is read from.
    let scratch = Scratch::new("pipe-head");
    fs::create_dir_all(scratch.root().join(".git")).expect("a .git directory");
    piped(&scratch.root().join(".git").join("HEAD"));
    let root = scratch.root().to_path_buf();
    assert_eq!(promptly(move || current(&root)), Some(None));

    // `commondir`, read from a linked checkout to find the shared directory.
    let repository = Repository::new("pipe-commondir");
    let first = repository.linked("first", false);
    piped(
        &repository
            .scratch
            .root()
            .join("main/.git/worktrees/first/commondir"),
    );
    let at = first.clone();
    let found = promptly(move || worktrees(&at)).expect("an answer without waiting");
    assert!(!found.contains(&repository.main()), "{found:?}");

    // A linked checkout's `gitdir` record, read from the main one.
    let repository = Repository::new("pipe-gitdir");
    let _second = repository.linked("second", false);
    piped(
        &repository
            .scratch
            .root()
            .join("main/.git/worktrees/second/gitdir"),
    );
    let main = repository.main();
    assert_eq!(promptly(move || worktrees(&main)), Some(Vec::new()));
}

#[test]
fn a_git_file_past_a_few_kilobytes_is_not_one_git_wrote() {
    // Any path git writes fits in a few KiB; a record padded past that is
    // read as nothing rather than grown into memory.
    let padding = " ".repeat(64 * 1024);

    let repository = Repository::new("oversize-gitdir");
    let linked = repository.linked("linked", false);
    let record = repository
        .scratch
        .root()
        .join("main/.git/worktrees/linked/gitdir");
    fs::write(
        &record,
        format!("{}{padding}\n", linked.join(".git").display()),
    )
    .expect("an oversize gitdir record");
    assert!(worktrees(&repository.main()).is_empty());

    let scratch = Scratch::new("oversize-dot-git");
    let elsewhere = scratch.root().join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("the pointed-at git directory");
    fs::write(elsewhere.join("HEAD"), "ref: refs/heads/main\n").expect("a HEAD file");
    let checkout = scratch.root().join("checkout");
    fs::create_dir_all(&checkout).expect("the checkout");
    fs::write(
        checkout.join(".git"),
        format!("gitdir: {}{padding}\n", elsewhere.display()),
    )
    .expect("an oversize .git file");
    assert_eq!(current(&checkout), None);
}

#[cfg(unix)]
#[test]
fn a_dot_git_file_that_is_a_link_out_of_the_checkout_is_followed_and_still_bounded() {
    // Tools that keep a checkout's git pointer elsewhere leave `.git` as a
    // symbolic link to a file outside the checkout. Git follows it, so its
    // branch and its sibling checkouts read; the file it leads to is held to
    // the same bound and the same refusal of a pipe as one standing there.
    use std::os::unix::fs::symlink;

    let repository = Repository::new("linked-dot-git");
    let first = repository.linked("first", false);
    let second = repository.linked("second", false);
    fs::write(
        repository
            .scratch
            .root()
            .join("main/.git/worktrees/first/HEAD"),
        "ref: refs/heads/fix/linked\n",
    )
    .expect("the linked checkout's HEAD");

    let kept = repository.scratch.root().join("kept");
    fs::create_dir_all(&kept).expect("a directory outside the checkout");
    fs::rename(first.join(".git"), kept.join("first.git")).expect("the pointer moved out");
    symlink(kept.join("first.git"), first.join(".git")).expect("a .git link");

    assert_eq!(current(&first).as_deref(), Some("fix/linked"));
    let mut found = worktrees(&first);
    found.sort();
    let mut expected = vec![repository.main(), second];
    expected.sort();
    assert_eq!(found, expected);

    // Past the bound, the link leads to no repository.
    let padding = " ".repeat(64 * 1024);
    let pointed = fs::read_to_string(kept.join("first.git")).expect("the pointer");
    fs::write(
        kept.join("first.git"),
        format!("{}{padding}\n", pointed.trim()),
    )
    .expect("an oversize pointer");
    assert_eq!(current(&first), None);
    assert!(worktrees(&first).is_empty());

    // A pipe at the link's end is refused without waiting on it.
    piped(&kept.join("first.git"));
    let at = first.clone();
    assert_eq!(promptly(move || current(&at)), Some(None));
    let at = first.clone();
    assert_eq!(promptly(move || worktrees(&at)), Some(Vec::new()));
}
