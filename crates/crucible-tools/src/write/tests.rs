//! What `write` puts down, and what it refuses to put down.

use std::fs;

use super::{Sensitivity, Tool, ToolArgs, ToolOutput, Write};
use crate::sample::{Sample, call, permitted};

fn write(sample: &Sample, args: &str) -> ToolOutput {
    Write::new(sample.workspace())
        .run(call(args), permitted(&Sensitivity::MutatesFile))
        .unwrap()
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
    let outside = format!("{}/../outside/secret.txt", sample.root().display());

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

    let problem = Write::new(sample.workspace())
        .run(
            call(r#"{"path":"one.txt"}"#),
            permitted(&Sensitivity::MutatesFile),
        )
        .unwrap_err();

    assert_eq!(problem.to_string(), "write: content is required");
}

#[test]
fn writing_is_always_put_to_the_user() {
    let sample = Sample::new("write-sensitivity");
    let tool = Write::new(sample.workspace());

    assert_eq!(
        tool.sensitivity(&ToolArgs::new("{}")),
        Sensitivity::MutatesFile
    );
}
