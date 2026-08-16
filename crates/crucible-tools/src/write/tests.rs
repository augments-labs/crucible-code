//! What `write` puts down, and what it refuses to put down.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use crucible_core::Cancel;

use super::{Ledger, Sensitivity, Tool, ToolArgs, ToolOutput, Write};
use crate::sample::{Sample, allowed, symlink};

/// A call about a file nobody has read, which is what most of these are: a path
/// that is not there yet, or one the refusal is the subject of.
fn write(sample: &Sample, args: &str) -> ToolOutput {
    writing(sample, args, &Ledger::new())
}

/// A call against a record somebody else has already told about a file.
fn writing(sample: &Sample, args: &str, seen: &Ledger) -> ToolOutput {
    let tool = Write::new(sample.workspace(), seen.clone());
    tool.run(allowed(&tool, args)).unwrap()
}

/// Says a file was read, the way `read` does when it shows one.
fn looked_at(sample: &Sample, at: &str) -> Ledger {
    let seen = Ledger::new();
    seen.record(
        sample
            .workspace()
            .existing(at)
            .expect("a file inside the sample workspace")
            .as_path(),
    );
    seen
}

fn read(sample: &Sample, at: &str) -> String {
    fs::read_to_string(sample.root().join(at)).expect("the file the tool wrote")
}

#[test]
fn a_new_file_is_created_with_exactly_what_was_sent() {
    let sample = Sample::new("write-new");

    let output = write(&sample, r#"{"path":"one.txt","content":"alpha\nbeta\n"}"#);

    assert_eq!(read(&sample, "one.txt"), "alpha\nbeta\n");
    assert_eq!(output.text(), "created one.txt, 2 lines");
}

#[test]
fn replacing_a_file_nobody_looked_at_is_refused_rather_than_done() {
    // The one call in this crate that can destroy work without anyone seeing
    // it first. Every other way of changing a file either matches text that is
    // already there or creates something new; this one replaces whatever is at
    // the name, and a model that guessed the name has guessed away the
    // contents. The refusal is a result rather than an error, so the turn
    // continues and the model can go and read it.
    let sample = Sample::new("write-unread");
    sample.write("one.txt", "work nobody looked at\n");

    let output = write(&sample, r#"{"path":"one.txt","content":"new\n"}"#);

    assert!(output.is_failed(), "{}", output.text());
    assert_eq!(read(&sample, "one.txt"), "work nobody looked at\n");
    assert_eq!(
        output.text(),
        "one.txt has not been read, so replacing it would discard what is in it: read it first"
    );
}

#[test]
fn a_file_the_read_tool_showed_may_be_replaced() {
    // The pair, through the real tools rather than a record filled in by hand:
    // one learns and the other asks, and what makes them agree is the value
    // they were both handed. A test that only calls `write` would pass with the
    // two halves speaking about different paths.
    let sample = Sample::new("write-after-read");
    sample.write("one.txt", "old\n");
    let seen = crate::Ledger::new();

    let reader = crate::Read::new(sample.workspace(), Cancel::new(), seen.clone());
    let shown = reader
        .run(allowed(&reader, r#"{"path":"one.txt"}"#))
        .unwrap();
    assert!(!shown.is_failed(), "{}", shown.text());

    let output = writing(&sample, r#"{"path":"one.txt","content":"new\n"}"#, &seen);

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(read(&sample, "one.txt"), "new\n");
}

#[test]
fn a_file_this_tool_put_down_itself_may_be_replaced_without_reading_it_back() {
    // Otherwise correcting what the same turn just wrote costs a read of text
    // the model already has, and the agent has to be told to do it.
    let sample = Sample::new("write-twice");
    let seen = crate::Ledger::new();

    writing(&sample, r#"{"path":"one.txt","content":"first\n"}"#, &seen);
    let output = writing(&sample, r#"{"path":"one.txt","content":"second\n"}"#, &seen);

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(read(&sample, "one.txt"), "second\n");
}

#[test]
fn an_existing_file_is_replaced_rather_than_appended_to() {
    let sample = Sample::new("write-replace");
    sample.write("one.txt", "old\n");

    let output = writing(
        &sample,
        r#"{"path":"one.txt","content":"new\n"}"#,
        &looked_at(&sample, "one.txt"),
    );

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(read(&sample, "one.txt"), "new\n");
    assert_eq!(output.text(), "replaced one.txt, 1 lines");
}

#[cfg(unix)]
#[test]
fn replacing_a_file_preserves_its_existing_mode() {
    let sample = Sample::new("write-mode");
    let path = sample.root().join("one.txt");
    sample.write("one.txt", "old\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

    let output = writing(
        &sample,
        r#"{"path":"one.txt","content":"new\n"}"#,
        &looked_at(&sample, "one.txt"),
    );

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn an_empty_file_is_a_thing_that_can_be_asked_for() {
    let sample = Sample::new("write-empty");

    let output = write(&sample, r#"{"path":"one.txt","content":""}"#);

    assert_eq!(read(&sample, "one.txt"), "");
    assert!(!output.is_failed(), "{}", output.text());
}

#[cfg(unix)]
#[test]
fn the_directories_above_a_new_file_are_made() {
    let sample = Sample::new("write-deep");

    write(
        &sample,
        r#"{"path":"src/cli/parse.rs","content":"fn main() {}\n"}"#,
    );

    assert_eq!(read(&sample, "src/cli/parse.rs"), "fn main() {}\n");
}

#[cfg(windows)]
#[test]
fn missing_directories_are_refused_when_safe_relative_creation_is_unavailable() {
    let sample = Sample::new("write-deep-refused");

    let output = write(
        &sample,
        r#"{"path":"src/cli/parse.rs","content":"fn main() {}\n"}"#,
    );

    assert!(output.is_failed(), "{}", output.text());
    assert!(output.text().contains("unavailable on this platform"));
    assert!(!sample.root().join("src").exists());
}

#[test]
fn an_absolute_path_inside_the_workspace_is_written_like_a_relative_one() {
    // The directories are made by walking the components of the parent, and
    // the walk used to start from an empty path — so the first component of an
    // absolute path was the filesystem root, containment was decided about `/`,
    // and every absolute path was refused. Every other test here sends a
    // relative one, which is why nothing caught it.
    let sample = Sample::new("write-absolute");
    fs::create_dir(sample.root().join("sub")).unwrap();
    let path = format!("{}/sub/one.txt", sample.named());

    let output = write(
        &sample,
        &format!(r#"{{"path":"{path}","content":"alpha\n"}}"#),
    );

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(read(&sample, "sub/one.txt"), "alpha\n");
}

#[test]
fn a_file_is_written_into_a_directory_the_workspace_reaches() {
    // `extraDirectories` names a place the tools may work, and it can only be
    // named absolutely — so the defect above meant nothing could be written
    // into one at all.
    let sample = Sample::new("write-reaching");
    let beside = sample.beside("notes");

    let tool = Write::new(sample.reaching(&beside), Ledger::new());
    let output = tool
        .run(allowed(
            &tool,
            &format!(r#"{{"path":"{beside}/todo.md","content":"buy milk\n"}}"#),
        ))
        .unwrap();

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(
        fs::read_to_string(format!("{beside}/todo.md")).expect("the file the tool wrote"),
        "buy milk\n"
    );
}

#[test]
fn a_path_outside_the_workspace_is_refused_without_writing_it() {
    let sample = Sample::new("write-escape");
    let outside = format!("{}/../outside/secret.txt", sample.named());

    let output = write(
        &sample,
        &format!(r#"{{"path":"{outside}","content":"stolen"}}"#),
    );

    assert!(output.is_failed(), "{}", output.text());
    assert!(!std::path::Path::new(&outside).exists());
}

#[test]
fn no_directory_is_made_outside_the_workspace_on_the_way() {
    // Refusing the file at the end is not enough. Making the directories in one
    // call would put `stray` down outside the tree and only then discover that
    // nothing may be written into it, leaving it behind.
    let sample = Sample::new("write-stray");

    let output = write(&sample, r#"{"path":"../stray/one.txt","content":"x"}"#);

    assert!(output.is_failed(), "{}", output.text());
    assert!(
        !sample.root().join("../stray").exists(),
        "a directory was made outside the workspace"
    );
}

#[test]
fn climbing_out_through_a_directory_it_makes_itself_is_refused() {
    let sample = Sample::new("write-climb");

    let output = write(
        &sample,
        r#"{"path":"made/../../outside/secret.txt","content":"stolen"}"#,
    );

    assert!(output.is_failed(), "{}", output.text());
    assert!(!sample.root().join("../outside/secret.txt").exists());
}

#[test]
fn writing_through_a_symlink_that_leaves_the_workspace_is_refused() {
    // A cloned repository can ship a symlink, and git preserves it. The name
    // the model asks for is inside the tree and the question the user answers
    // says nothing about where it goes, so the containment check is the only
    // thing standing between the two.
    let sample = Sample::new("write-symlink");
    let outside = sample.outside("secret.txt", "original\n");
    symlink(&outside, sample.root().join("notes.txt"));

    let output = write(&sample, r#"{"path":"notes.txt","content":"stolen\n"}"#);

    assert!(output.is_failed(), "{}", output.text());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original\n");
}

#[test]
fn creating_through_a_dangling_symlink_that_leaves_the_workspace_is_refused() {
    // The variant that creates rather than replaces: the target does not exist
    // yet, so the write would put a brand new file outside the tree — a shell
    // profile, a config file, anything that runs later.
    let sample = Sample::new("write-dangling");
    let outside = sample.outside("absent.txt", "");
    fs::remove_file(&outside).unwrap();
    symlink(&outside, sample.root().join("fresh.txt"));

    let output = write(&sample, r#"{"path":"fresh.txt","content":"stolen\n"}"#);

    assert!(output.is_failed(), "{}", output.text());
    assert!(!std::path::Path::new(&outside).exists());
}

#[test]
fn a_link_planted_while_the_question_was_on_screen_is_still_refused() {
    // The path is resolved once to say what the call is about and again in
    // `run`, and the gap between the two is however long somebody took to
    // answer. A link planted in it would make the file that was agreed to a
    // different file from the one written, so the resolution that counts is
    // the second one — and this is what says the first is never trusted for it.
    let sample = Sample::new("write-planted");
    let outside = sample.outside("secret.txt", "original\n");
    let tool = Write::new(sample.workspace(), Ledger::new());
    let args = r#"{"path":"notes.txt","content":"stolen\n"}"#;

    assert_eq!(
        tool.sensitivity(&ToolArgs::new(args)).to_string(),
        "change notes.txt"
    );
    symlink(&outside, sample.root().join("notes.txt"));

    let output = tool.run(allowed(&tool, args)).unwrap();

    assert!(output.is_failed(), "{}", output.text());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "original\n");
}

#[test]
fn a_directory_is_not_a_file_to_write_over() {
    let sample = Sample::new("write-dir");
    sample.write("sub/one.txt", "a\n");

    let output = write(&sample, r#"{"path":"sub","content":"x"}"#);

    assert!(output.is_failed());
    assert_eq!(output.text(), "sub is a directory");
}

#[test]
fn a_call_with_no_content_says_what_is_missing() {
    let sample = Sample::new("write-nocontent");

    let tool = Write::new(sample.workspace(), Ledger::new());
    let problem = tool
        .run(allowed(&tool, r#"{"path":"one.txt"}"#))
        .unwrap_err();

    assert_eq!(problem.to_string(), "write: content is required");
}

#[test]
fn writing_names_the_file_it_would_change() {
    // A rule can be about `one.txt`, so the sensitivity has to say which file
    // this is — and say it before the call runs, from the arguments alone.
    let sample = Sample::new("write-sensitivity");
    let tool = Write::new(sample.workspace(), Ledger::new());

    let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"one.txt"}"#));

    assert!(matches!(sensitivity, Sensitivity::MutatesFile { .. }));
    assert_eq!(sensitivity.to_string(), "change one.txt");
}

#[test]
fn missing_directories_do_not_hide_the_permission_configuration() {
    // Permission is decided before `write` makes these directories. Resolving
    // only a creatable leaf used to lose this target while `.crucible` was
    // absent, which let allow-edits mode reach the one file no mode may write.
    let sample = Sample::new("write-config-sensitivity");
    let tool = Write::new(sample.workspace(), Ledger::new());

    for path in [
        ".crucible/config.json",
        ".crucible/config.local.json",
        "nested/.crucible/config.json",
    ] {
        let sensitivity = tool.sensitivity(&ToolArgs::new(format!(
            r#"{{"path":"{path}","content":"{{}}"}}"#
        )));

        assert_eq!(sensitivity.to_string(), format!("change {path}"));
    }
}

#[test]
fn a_path_that_does_not_resolve_still_says_a_file_is_about_to_change() {
    // No rule matches an unresolved target, so this is asked about rather than
    // waved through by a rule written about somewhere else.
    let sample = Sample::new("write-unresolved");
    let tool = Write::new(sample.workspace(), Ledger::new());

    let sensitivity = tool.sensitivity(&ToolArgs::new("{}"));

    assert!(matches!(sensitivity, Sensitivity::MutatesFile { .. }));
}
