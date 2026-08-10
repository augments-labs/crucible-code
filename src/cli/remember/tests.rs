//! Whether a rule that was written down is one crucible reads back.

use std::ffi::OsString;

use crucible_core::{Command, Reach, Sensitivity, Settled, ToolArgs, ToolCall, ToolId, narrowest};

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
            reach: Reach::Anything,
        },
    }
}

/// Answers `always` to one command, into this sample's own tree.
fn remembering(sample: &Sample, command: &str) -> Result<(), RememberError> {
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

    remembering(&sample, "cargo test").expect("a tree crucible may write in");

    assert!(matches!(
        settles(&sample, "cargo test"),
        Settled::Approved(_)
    ));
    assert!(matches!(settles(&sample, "echo hello"), Settled::Refused));
}

#[test]
fn a_second_answer_joins_the_first_rather_than_replacing_it() {
    let sample = Sample::new("remember-second");

    remembering(&sample, "cargo test").expect("a tree crucible may write in");
    remembering(&sample, "git status").expect("a tree crucible may write in");

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

    let problem = remembering(&sample, "cargo test").expect_err("a file crucible cannot read");

    assert!(matches!(problem, RememberError::Unusable(_)), "{problem:?}");
    assert_eq!(
        fs::read_to_string(&file).expect("the file is still there"),
        "{ oh dear"
    );
}

#[test]
fn nothing_is_left_beside_the_file_it_wrote() {
    // Written beside and renamed over, so the directory holds one file rather
    // than one file and the wreckage of writing it.
    let sample = Sample::new("remember-tidy");

    remembering(&sample, "cargo test").expect("a tree crucible may write in");

    let directory = crucible_config::local(&sample.root());
    let beside = fs::read_dir(directory.parent().expect("a directory")).expect("a temporary tree");
    let names: Vec<_> = beside
        .map(|entry| entry.expect("a readable entry").file_name())
        .collect();

    assert_eq!(
        names,
        vec![OsString::from("config.local.json")],
        "{names:?}"
    );
}
