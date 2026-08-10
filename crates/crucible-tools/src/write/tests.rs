//! What `write` puts down, and what it refuses to put down.

use std::fs;

use super::{Sensitivity, Tool, ToolArgs, ToolOutput, Write};
use crate::sample::{Sample, allowed, symlink};

fn write(sample: &Sample, args: &str) -> ToolOutput {
    let tool = Write::new(sample.workspace());
    tool.run(allowed(&tool, args)).unwrap()
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
fn an_existing_file_is_replaced_rather_than_appended_to() {
    let sample = Sample::new("write-replace");
    sample.write("one.txt", "old\n");

    let output = write(&sample, r#"{"path":"one.txt","content":"new\n"}"#);

    assert_eq!(read(&sample, "one.txt"), "new\n");
    assert_eq!(output.text(), "replaced one.txt, 1 lines");
}

#[test]
fn an_empty_file_is_a_thing_that_can_be_asked_for() {
    let sample = Sample::new("write-empty");

    let output = write(&sample, r#"{"path":"one.txt","content":""}"#);

    assert_eq!(read(&sample, "one.txt"), "");
    assert!(!output.is_failed(), "{}", output.text());
}

#[test]
fn the_directories_above_a_new_file_are_made() {
    let sample = Sample::new("write-deep");

    write(
        &sample,
        r#"{"path":"src/cli/parse.rs","content":"fn main() {}\n"}"#,
    );

    assert_eq!(read(&sample, "src/cli/parse.rs"), "fn main() {}\n");
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

    let tool = Write::new(sample.workspace());
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
    let tool = Write::new(sample.workspace());

    let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"one.txt"}"#));

    assert!(matches!(sensitivity, Sensitivity::MutatesFile { .. }));
    assert_eq!(sensitivity.to_string(), "change one.txt");
}

#[test]
fn a_path_that_does_not_resolve_still_says_a_file_is_about_to_change() {
    // No rule matches an unresolved target, so this is asked about rather than
    // waved through by a rule written about somewhere else.
    let sample = Sample::new("write-unresolved");
    let tool = Write::new(sample.workspace());

    let sensitivity = tool.sensitivity(&ToolArgs::new("{}"));

    assert!(matches!(sensitivity, Sensitivity::MutatesFile { .. }));
}
