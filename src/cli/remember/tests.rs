//! Whether a configuration splice is replaced whole without losing its file.

use std::ffi::OsString;

use crucible_core::{Command, Sensitivity, Settled, ToolArgs, ToolCall, ToolId, narrowest};

use crate::cli::sample::Sample;

use super::*;

fn call() -> ToolCall {
    ToolCall {
        id: ToolId::new("a"),
        name: "bash".into(),
        args: ToolArgs::new("{}"),
    }
}

fn running(command: &str) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            parts: Box::from([Box::from(command)]),
        },
    }
}

/// Writes one rule through the same replacement boundary as user choices.
fn writing_rule(sample: &Sample, command: &str) -> Result<(), RememberError> {
    let rule = narrowest(&call(), &running(command)).expect("one command can be written down");

    allowing(&crucible_config::local(&sample.root()), &rule)
}

/// What the engine does with a command once the files have been read again.
fn settles(sample: &Sample, command: &str) -> Settled {
    sample.settles(&call(), &running(command))
}

#[test]
fn a_project_with_no_crucible_directory_at_all_gains_one_holding_the_rule() {
    let sample = Sample::new("remember-fresh");

    writing_rule(&sample, "cargo test").expect("a tree crucible may write in");

    assert!(matches!(
        settles(&sample, "cargo test"),
        Settled::Approved(_)
    ));
    assert!(matches!(settles(&sample, "echo hello"), Settled::Refused));
}

#[test]
fn a_second_answer_joins_the_first_rather_than_replacing_it() {
    let sample = Sample::new("remember-second");

    writing_rule(&sample, "cargo test").expect("a tree crucible may write in");
    writing_rule(&sample, "git status").expect("a tree crucible may write in");

    assert!(matches!(
        settles(&sample, "cargo test"),
        Settled::Approved(_)
    ));
    assert!(matches!(
        settles(&sample, "git status"),
        Settled::Approved(_)
    ));
}

#[test]
fn a_file_crucible_cannot_read_is_reported_rather_than_replaced() {
    // The file is somebody's, and a rule is not worth losing what they wrote.
    let sample = Sample::new("remember-broken");
    let file = crucible_config::local(&sample.root());
    fs::create_dir_all(file.parent().expect("a directory")).expect("a temporary tree");
    fs::write(&file, "{ oh dear").expect("a temporary tree");

    let problem = writing_rule(&sample, "cargo test").expect_err("a file crucible cannot read");

    assert!(matches!(problem, RememberError::Unusable(_)), "{problem:?}");
    assert_eq!(
        fs::read_to_string(&file).expect("the file is still there"),
        "{ oh dear"
    );
}

#[cfg(unix)]
#[test]
fn a_file_the_user_narrowed_is_still_narrow_after_a_rule_joins_it() {
    use std::os::unix::fs::PermissionsExt as _;

    // A rename puts a new file where the old one was rather than new bytes
    // inside it, so the mode comes from whatever made the new one. This file
    // says what may run without being asked about, and widening who can write
    // to it is not part of replacing one setting.
    let sample = Sample::new("remember-narrow");
    let file = crucible_config::local(&sample.root());

    writing_rule(&sample, "cargo test").expect("a tree crucible may write in");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("a tree with modes");

    writing_rule(&sample, "git status").expect("a tree crucible may write in");

    let mode = fs::metadata(&file)
        .expect("the file is still there")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

#[test]
fn nothing_is_left_beside_the_file_it_wrote() {
    // Written beside and renamed over, so the directory holds one file rather
    // than one file and the wreckage of writing it.
    let sample = Sample::new("remember-tidy");

    writing_rule(&sample, "cargo test").expect("a tree crucible may write in");

    let file = crucible_config::local(&sample.root());
    let names = holds(file.parent().expect("a directory"));

    assert_eq!(
        names,
        vec![OsString::from("config.local.json")],
        "{names:?}"
    );
}

/// Everything in `directory`, by name.
fn holds(directory: &Path) -> Vec<OsString> {
    fs::read_dir(directory)
        .expect("a temporary tree")
        .map(|entry| entry.expect("a readable entry").file_name())
        .collect()
}

#[cfg(unix)]
#[test]
fn nothing_is_left_beside_a_file_that_could_not_be_replaced() {
    // A rule that was not written must not cost the user a stray file
    // holding the whole permission document, under a name nothing will look at
    // again.
    //
    // The rename is made to fail by putting a directory where the file goes:
    // no file renames over one. The directory holding both stays writable, so
    // clearing up is possible and its absence would be a decision rather than
    // a second failure.
    //
    // Written through `put` rather than through `allowing`, because reading a
    // directory as a document fails first and the write never happens. Unix
    // only, for the same reason the mode test is: what a rename refuses and
    // what an undo is allowed to do are both the platform's to say.
    let sample = Sample::new("remember-stranded");
    let file = crucible_config::local(&sample.root());
    fs::create_dir_all(file.join("in the way")).expect("a temporary tree");

    let problem = put(&file, "{}").expect_err("no file renames over a directory");

    let left = holds(file.parent().expect("a directory"));
    assert_eq!(
        left,
        vec![OsString::from("config.local.json")],
        "{left:?} after {problem}"
    );
}
