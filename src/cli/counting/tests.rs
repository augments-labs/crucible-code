//! What a remote comes back as a repository, and what comes back as nothing.

use std::fs;
use std::path::{Path, PathBuf};

use super::{forge, read};

/// A directory of its own to lay a checkout out in, deleted with it.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let base =
            std::env::temp_dir().join(format!("crucible-counting-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).expect("a temporary directory");

        Self(base)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    /// A `.git/config` holding `content`, laid out the way an ordinary
    /// checkout leaves it.
    fn configured(&self, content: &str) {
        fs::create_dir_all(self.0.join(".git")).expect("a .git directory");
        fs::write(self.0.join(".git").join("config"), content).expect("a config file");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Where `number` points, for a checkout cloned from `url`.
fn against(url: &str, number: &str) -> Option<String> {
    Some(read(url)?.address(None, number))
}

#[test]
fn a_clone_over_https_is_read() {
    assert_eq!(
        against("https://github.com/augments-labs/crucible-code.git", "487").as_deref(),
        Some("https://github.com/augments-labs/crucible-code/issues/487")
    );
}

#[test]
fn a_clone_over_ssh_is_read_as_the_same_repository() {
    // Two spellings of one remote, and the page is the same page: what the
    // clone authenticated as is not part of where anything is.
    for url in [
        "git@github.com:augments-labs/crucible-code.git",
        "ssh://git@github.com/augments-labs/crucible-code.git",
        "git://github.com/augments-labs/crucible-code",
        "https://github.com/augments-labs/crucible-code/",
    ] {
        assert_eq!(
            against(url, "487").as_deref(),
            Some("https://github.com/augments-labs/crucible-code/issues/487"),
            "{url:?}"
        );
    }
}

#[test]
fn a_forge_that_nests_its_pages_is_spelled_the_way_it_spells_them() {
    assert_eq!(
        against("git@gitlab.com:a-group/a-project.git", "12").as_deref(),
        Some("https://gitlab.com/a-group/a-project/-/issues/12")
    );
}

#[test]
fn a_project_under_a_group_keeps_the_whole_path() {
    assert_eq!(
        against("https://gitlab.com/a-group/a-nested/a-project.git", "12").as_deref(),
        Some("https://gitlab.com/a-group/a-nested/a-project/-/issues/12")
    );
}

#[test]
fn a_remote_this_does_not_read_is_no_repository() {
    for url in [
        "",
        "/srv/repositories/bare.git",
        "https://github.com",
        "file:///srv/repositories/bare.git",
        "not a url at all",
    ] {
        assert_eq!(read(url), None, "{url:?}");
    }
}

#[test]
fn the_url_under_origin_is_the_one_that_is_read() {
    // A checkout with more than one remote, and the fork is not the answer.
    let scratch = Scratch::new("origin");
    scratch.configured(concat!(
        "[core]\n\trepositoryformatversion = 0\n",
        "[remote \"upstream\"]\n\turl = https://github.com/somebody/else.git\n",
        "[remote \"origin\"]\n\turl = https://github.com/augments-labs/crucible-code.git\n",
        "\tfetch = +refs/heads/*:refs/remotes/origin/*\n",
    ));

    assert_eq!(
        forge(scratch.root()).map(|forge| forge.address(None, "487")),
        Some("https://github.com/augments-labs/crucible-code/issues/487".to_owned())
    );
}

#[test]
fn a_checkout_with_no_remote_is_no_repository() {
    let scratch = Scratch::new("bare");
    scratch.configured("[core]\n\trepositoryformatversion = 0\n\tbare = false\n");

    assert_eq!(forge(scratch.root()), None);
}

#[test]
fn a_directory_that_is_not_a_checkout_is_no_repository() {
    let scratch = Scratch::new("plain");

    assert_eq!(forge(scratch.root()), None);
}

#[test]
fn a_linked_worktree_is_counted_against_the_repository_it_belongs_to() {
    // `.git` is a file there, and the configuration is the repository's rather
    // than the worktree's.
    let scratch = Scratch::new("worktree");
    let repository = scratch.root().join("repository").join(".git");
    let worktree = repository.join("worktrees").join("a-branch");
    fs::create_dir_all(&worktree).expect("a worktree directory");
    fs::write(
        repository.join("config"),
        "[remote \"origin\"]\n\turl = git@github.com:augments-labs/crucible-code.git\n",
    )
    .expect("a config file");
    fs::write(worktree.join("commondir"), "../..\n").expect("a commondir file");

    let checkout = scratch.root().join("checkout");
    fs::create_dir_all(&checkout).expect("a checkout directory");
    fs::write(
        checkout.join(".git"),
        format!("gitdir: {}\n", worktree.display()),
    )
    .expect("a .git file");

    assert_eq!(
        forge(&checkout).map(|forge| forge.address(None, "487")),
        Some("https://github.com/augments-labs/crucible-code/issues/487".to_owned())
    );
}
