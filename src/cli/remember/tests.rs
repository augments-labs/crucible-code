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

    allowing(&sample.user_file(), &rule)
}

/// What the engine does with a command once the files have been read again.
fn settles(sample: &Sample, command: &str) -> Settled {
    sample.settles(&call(), &running(command))
}

#[test]
fn a_user_with_no_config_directory_gains_one_holding_the_rule() {
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
    let file = sample.user_file();
    fs::create_dir_all(file.parent().expect("a directory")).expect("a temporary tree");
    fs::write(&file, "{ oh dear").expect("a temporary tree");

    let problem = writing_rule(&sample, "cargo test").expect_err("a file crucible cannot read");

    assert!(matches!(problem, RememberError::Unusable(_)), "{problem:?}");
    assert_eq!(
        fs::read_to_string(&file).expect("the file is still there"),
        "{ oh dear"
    );
}

#[test]
fn a_file_over_the_configuration_bound_is_not_read_or_replaced() {
    let sample = Sample::new("remember-too-large");
    let file = sample.user_file();
    fs::create_dir_all(file.parent().expect("a directory")).expect("a temporary tree");
    let written = " ".repeat(crucible_config::MAX_DOCUMENT_BYTES + 1);
    fs::write(&file, &written).expect("a temporary tree");

    let problem =
        choosing(&file, "anthropic", "claude-opus-5").expect_err("a document beyond the boundary");

    assert!(
        matches!(
            problem,
            RememberError::Unusable(ConfigError::TooLarge {
                maximum: crucible_config::MAX_DOCUMENT_BYTES,
                ..
            })
        ),
        "{problem:?}"
    );
    assert_eq!(fs::read_to_string(file).unwrap(), written);
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
    let file = sample.user_file();

    writing_rule(&sample, "cargo test").expect("a tree crucible may write in");
    fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).expect("a tree with modes");

    writing_rule(&sample, "git status").expect("a tree crucible may write in");

    let mode = fs::metadata(&file)
        .expect("the file is still there")
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600, "{mode:o}");
}

#[cfg(unix)]
#[test]
fn fresh_user_configuration_state_is_owner_only() {
    use std::os::unix::fs::PermissionsExt as _;

    let sample = Sample::new("remember-private");
    let file = sample.user_file();
    let lock = lock_name(&file);
    writing_rule(&sample, "cargo test").expect("a private user file");

    for (path, wanted) in [
        (file.parent().expect("the user directory"), 0o700),
        (file.as_path(), 0o600),
        (lock.as_path(), 0o600),
    ] {
        let mode = fs::metadata(path)
            .expect("the private state exists")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, wanted, "{} was {mode:o}", path.display());
    }
}

#[test]
fn nothing_is_left_beside_the_file_it_wrote() {
    // Written beside and renamed over, so the only sibling is the durable lock
    // that serializes later changes rather than wreckage holding a document.
    let sample = Sample::new("remember-tidy");

    writing_rule(&sample, "cargo test").expect("a tree crucible may write in");

    let file = sample.user_file();
    let names = holds(file.parent().expect("a directory"));

    assert_eq!(
        names,
        vec![
            OsString::from("config.json"),
            OsString::from("config.json.lock")
        ],
        "{names:?}"
    );
}

/// Everything in `directory`, by name.
fn holds(directory: &Path) -> Vec<OsString> {
    let mut names: Vec<_> = fs::read_dir(directory)
        .expect("a temporary tree")
        .map(|entry| entry.expect("a readable entry").file_name())
        .collect();
    names.sort();
    names
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
    let file = sample.user_file();
    fs::create_dir_all(file.join("in the way")).expect("a temporary tree");

    let problem = put(&file, "{}").expect_err("no file renames over a directory");

    let left = holds(file.parent().expect("a directory"));
    assert_eq!(
        left,
        vec![OsString::from("config.json")],
        "{left:?} after {problem}"
    );
}

#[test]
fn simultaneous_answers_keep_every_provider() {
    let sample = Sample::new("remember-concurrent");
    let file = sample.user_file();
    let ready = std::sync::Arc::new(std::sync::Barrier::new(17));

    std::thread::scope(|scope| {
        let mut writes = Vec::new();
        for number in 0..16 {
            let ready = std::sync::Arc::clone(&ready);
            let file = file.clone();
            writes.push(scope.spawn(move || {
                ready.wait();
                choosing(
                    &file,
                    &format!("provider-{number}"),
                    &format!("model-{number}"),
                )
            }));
        }
        ready.wait();
        for write in writes {
            write.join().expect("a writer did not panic").unwrap();
        }
    });

    let written = fs::read_to_string(file).expect("every answer was written");
    for number in 0..16 {
        assert!(
            written.contains(&format!("model-{number}")),
            "model-{number} was lost from {written}"
        );
    }
}

#[cfg(unix)]
#[test]
fn a_planted_temporary_symlink_is_not_followed() {
    use std::os::unix::fs::symlink;

    let sample = Sample::new("remember-temp-symlink");
    let directory = sample.root().join("private");
    fs::create_dir_all(&directory).unwrap();
    let victim = sample.root().join("victim");
    fs::write(&victim, "keep me").unwrap();
    let planted = directory.join(".writing.planted");
    symlink(&victim, &planted).unwrap();

    let problem = Beside::at(planted).expect_err("exclusive creation must refuse a symlink");

    assert_eq!(problem.kind(), io::ErrorKind::AlreadyExists);
    assert_eq!(fs::read_to_string(victim).unwrap(), "keep me");
}
