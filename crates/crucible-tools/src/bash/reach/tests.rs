//! Which command lines were proved to stay inside, and the far larger number
//! that were not.
//!
//! One test per program, because a flag table is a claim about that program's
//! grammar and a claim is worth exactly the case that would have caught it
//! being wrong. Each asserts the shape the table recognises and a flag it does
//! not, since the failure that costs something is a flag being waved through
//! after taking a word that was never a path.

use std::fs;

use crucible_core::Reach;

use super::of;
use crate::sample::{Sample, symlink};

/// A tree with something in it to move about.
fn sample(name: &str) -> Sample {
    let sample = Sample::new(name);
    sample.write("src/a.rs", "fn a() {}");
    sample.write("build/out", "");
    sample
}

/// How far a line reaches, read the way the tool reads it.
fn reach(sample: &Sample, line: &str) -> Reach {
    let parts: Vec<Box<str>> = line.split(';').map(|part| part.trim().into()).collect();
    of(&parts, &sample.workspace())
}

/// Whether a line is one nothing was proved about.
fn asks(sample: &Sample, line: &str) -> bool {
    reach(sample, line) == Reach::Anything
}

#[test]
fn mkdir_reaches_no_further_than_the_directory_it_names() {
    let sample = sample("reach-mkdir");

    assert_eq!(reach(&sample, "mkdir src/net"), Reach::Workspace);
    assert_eq!(reach(&sample, "mkdir -p src/net"), Reach::Workspace);
    assert_eq!(reach(&sample, "mkdir --parents src/net"), Reach::Workspace);

    // Only as far as a directory that is there. Where `src/net` does not exist
    // yet, nothing under it has a parent to resolve, and a path this could not
    // resolve is a path it proved nothing about.
    assert!(asks(&sample, "mkdir -p src/net/http"));

    // `-m` takes the mode as the next word, so a checker that did not know
    // that would resolve `755` as a path and find it comfortably inside.
    assert!(asks(&sample, "mkdir -m 755 src/net"));
    assert!(asks(&sample, "mkdir -Z src/net"));
}

#[test]
fn rmdir_reaches_no_further_than_the_directory_it_names() {
    let sample = sample("reach-rmdir");

    assert_eq!(reach(&sample, "rmdir build"), Reach::Workspace);
    assert_eq!(reach(&sample, "rmdir -p build"), Reach::Workspace);

    assert!(asks(&sample, "rmdir --ignore-fail-on-non-empty build"));
}

#[test]
fn touch_reaches_no_further_than_the_files_it_names() {
    let sample = sample("reach-touch");

    assert_eq!(reach(&sample, "touch src/b.rs"), Reach::Workspace);
    assert_eq!(reach(&sample, "touch -c src/a.rs"), Reach::Workspace);

    // `-r` takes a reference file, `-d` a date. Both put a word in the line
    // that is not the file being changed.
    assert!(asks(&sample, "touch -r src/a.rs src/b.rs"));
    assert!(asks(&sample, "touch -d yesterday src/a.rs"));
}

#[test]
fn rm_reaches_no_further_than_the_paths_it_names() {
    let sample = sample("reach-rm");

    assert_eq!(reach(&sample, "rm src/a.rs"), Reach::Workspace);
    assert_eq!(reach(&sample, "rm -f src/a.rs"), Reach::Workspace);

    // The flag whose entire purpose is to reach the one place nothing here
    // may reach.
    assert!(asks(&sample, "rm -rf --no-preserve-root build"));
}

#[test]
fn a_recursive_delete_is_asked_about_even_where_it_cannot_leave_the_tree() {
    // The proof holds and is not the question. `rm -rf .` from the root cannot
    // reach outside the workspace and still takes everything uncommitted with
    // it, where every other line here changes what it names and costs what was
    // typed. A difference in kind, so it is asked about.
    let sample = sample("reach-recursive");

    assert!(asks(&sample, "rm -rf ."));
    assert_eq!(reach(&sample, "rm build/out"), Reach::Workspace);
}

#[test]
fn a_recursive_delete_is_the_same_answer_in_every_spelling() {
    // A list holding two of the three spellings is a rule about spelling. The
    // bundle is the one a later edit breaks first: `-rf` is a single word that
    // no match against `-r` would ever see.
    let sample = sample("reach-recursive-spelled");

    for line in [
        "rm -r build",
        "rm -R build",
        "rm --recursive build",
        "rm -rf build",
        "rm -fr build",
        "rm -Rf build",
        "rm -f -r build",
    ] {
        assert!(
            asks(&sample, line),
            "{line} descends and was not asked about"
        );
    }

    // `-d` is `rmdir` under another name: an empty directory and nothing under
    // it, so it keeps the treatment `rmdir` keeps.
    assert_eq!(reach(&sample, "rm -d build"), Reach::Workspace);
}

#[test]
fn a_word_that_merely_contains_a_flag_is_still_a_path() {
    // The test that would catch a scan over the line's text rather than over
    // the words the parse already called flags.
    let sample = sample("reach-flaggy");
    sample.write("build-r/out", "");
    sample.write("src/-rf.txt", "");

    assert_eq!(reach(&sample, "rm build-r/out"), Reach::Workspace);
    assert_eq!(reach(&sample, "rm src/-rf.txt"), Reach::Workspace);

    // A word that genuinely starts with a dash is read as a flag, whatever it
    // was meant to be — and `.txt` is not a flag this knows, so the line is
    // asked about. Conservative in the direction that costs a question.
    assert!(asks(&sample, "rm -rf.txt"));
}

#[test]
fn a_path_spelled_like_a_flag_after_a_terminator_is_asked_about() {
    // `--` ends the options, and it is in no table — so the line stops being
    // read there and nothing is proved about any of it. Which is the answer a
    // path spelled `-r` wants anyway: whichever thing it is, it was not read.
    let sample = sample("reach-terminator");

    assert!(asks(&sample, "rm -- -r"));
    assert!(asks(&sample, "rm -f -- src/a.rs"));
}

#[test]
fn cp_reaches_no_further_than_either_end() {
    let sample = sample("reach-cp");

    assert_eq!(reach(&sample, "cp src/a.rs src/b.rs"), Reach::Workspace);
    assert_eq!(
        reach(&sample, "cp -f src/a.rs build/a.rs"),
        Reach::Workspace
    );

    // `-t` takes the destination directory, which would leave the source
    // looking like the only path in the line.
    assert!(asks(&sample, "cp -t build src/a.rs"));
    assert!(asks(&sample, "cp --parents src/a.rs build"));
}

#[test]
fn mv_reaches_no_further_than_either_end() {
    let sample = sample("reach-mv");

    assert_eq!(reach(&sample, "mv src/a.rs src/b.rs"), Reach::Workspace);
    assert_eq!(
        reach(&sample, "mv -f src/a.rs build/a.rs"),
        Reach::Workspace
    );

    assert!(asks(&sample, "mv -t build src/a.rs"));
    assert!(asks(&sample, "mv --suffix .bak src/a.rs build"));
}

#[test]
fn a_copy_into_a_directory_is_proved_nothing_about() {
    // The file written is `outbox/a.rs`, and no word in the line spells it —
    // so nothing resolved it, and a symbolic link a cloned repository left
    // there is followed straight out of the tree.
    let sample = sample("reach-copy-into");
    let outside = sample.outside("stolen", "");
    fs::create_dir(sample.root().join("outbox")).expect("a writable temporary directory");
    symlink(&outside, sample.root().join("outbox/a.rs"));

    assert!(asks(&sample, "cp src/a.rs outbox"));
    assert!(asks(&sample, "mv src/a.rs outbox"));

    // The working directory is the same shape with a shorter name.
    assert!(asks(&sample, "cp build/out ."));

    // And more than one source says the last word is a directory whatever is
    // on disk when this reads it.
    assert!(asks(&sample, "cp src/a.rs build/out outbox"));
    assert!(asks(&sample, "mv src/a.rs build/out outbox"));
}

#[test]
fn a_recursive_copy_is_proved_nothing_about() {
    // `-r` turns each word into a tree, and the files under one are paths no
    // word named and nothing resolved. Whether every one of them lands inside
    // is then a question about what the copy does with the links it meets,
    // which is not something the line says.
    let sample = sample("reach-copy-recursive");

    for line in [
        "cp -r src build/src",
        "cp -R src build/src",
        "cp --recursive src build/src",
        "cp -rf src build/src",
        "cp -a src build/src",
        "cp --archive src build/src",
    ] {
        assert!(
            asks(&sample, line),
            "{line} descends and was not asked about"
        );
    }

    // What is left is the copy this can read: two paths, both resolved, and
    // the second one is the file that changes.
    assert_eq!(reach(&sample, "cp src/a.rs src/b.rs"), Reach::Workspace);
}

#[test]
fn no_line_that_makes_a_second_name_for_a_file_is_proved() {
    // The engine refuses a write to its own configuration by the path, and a
    // hard link is a second path to the same inode that resolving does not
    // undo. Nothing here may make one, which is what holds that closed — and
    // `cp` is one letter in a table away from being able to.
    let sample = sample("reach-hard-link");

    for line in [
        "ln src/a.rs src/b.rs",
        "cp -l src/a.rs src/b.rs",
        "cp --link src/a.rs src/b.rs",
        "cp -al src build/src",
    ] {
        assert!(asks(&sample, line), "{line} makes a second name for a file");
    }
}

#[test]
fn a_word_that_reaches_the_permission_configuration_is_proved_nothing_about() {
    // What no file tool may write, no proved command line may write either.
    // The refusal is on the directory as well as the files, because a program
    // handed a directory works the filename out itself.
    //
    // Empty, the directory counts the same: one made this turn is a single
    // copy away from holding a configuration, so what this reads is where the
    // word lands and never what happens to be there while it reads.
    let bare = sample("reach-configuration-bare");
    assert!(asks(&bare, "mkdir .crucible"));
    assert_eq!(
        reach(&bare, "mkdir x.crucible"),
        Reach::Workspace,
        "a directory that merely ends in the name is somebody else's"
    );

    let sample = sample("reach-configuration");
    sample.write(".crucible/config.json", "{}");

    for line in [
        "cp src/a.rs .crucible/config.json",
        "rm .crucible/config.local.json",
        "mv .crucible/config.json build/out",
        "touch .crucible/config.local.json",
        "cp src/a.rs .crucible",
        "rmdir .crucible",
    ] {
        assert!(asks(&sample, line), "{line} reaches the configuration");
    }
}

#[test]
fn a_program_nothing_here_reads_is_proved_nothing_about() {
    // The set is small on purpose. Every program outside it — including the
    // harmless-looking ones — reaches whatever the user can as far as this is
    // concerned.
    let sample = sample("reach-unknown");

    for line in [
        "ls src",
        "cat src/a.rs",
        "sed -i s/a/b/ src/a.rs",
        "git add .",
    ] {
        assert!(asks(&sample, line), "{line} was not read by anything here");
    }

    // Including the same program under a name that resolves somewhere else.
    assert!(asks(&sample, "/bin/rm src/a.rs"));
    assert!(asks(&sample, "./rm src/a.rs"));
}

#[test]
fn a_path_that_lands_outside_is_proved_nothing_about() {
    let sample = sample("reach-outside");
    let outside = sample.outside("secret", "");

    assert!(asks(&sample, &format!("rm {outside}")));
    assert!(asks(&sample, "rm ../outside/secret"));
    assert!(asks(&sample, "cp src/a.rs ../outside/copy"));

    // The far end of a symbolic link is where the change lands.
    let link = sample.root().join("link");
    symlink(&outside, &link);
    assert!(asks(&sample, "rm link"));
}

#[test]
fn a_word_the_shell_would_change_is_not_the_path_it_looks_like() {
    // Every one of these is resolved by the shell after this ran, so the word
    // checked here is not the word the program receives.
    let sample = sample("reach-expanded");

    for line in [
        "rm src/*.rs",
        "rm src/a.??",
        "rm src/[ab].rs",
        "rm ~/secret",
        r#"rm "src/a.rs""#,
        "rm src/a\\ b.rs",
    ] {
        assert!(asks(&sample, line), "{line} is not the path it looks like");
    }
}

#[test]
fn a_command_with_no_path_at_all_is_proved_nothing_about() {
    let sample = sample("reach-pathless");

    assert!(asks(&sample, "rm -rf"));
    assert!(asks(&sample, "mkdir --version"));
}

#[test]
fn a_proof_covers_one_command_or_it_covers_nothing() {
    // The second command is where the interesting one hides, and a line that
    // needs two proofs is a line nobody asked this to reason about.
    let sample = sample("reach-chain");

    assert!(asks(&sample, "mkdir src/net; rm src/a.rs"));
}
