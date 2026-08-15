//! What `edit` changes, and what it declines to guess at.

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use super::{Cancel, Edit, Sensitivity, Tool, ToolArgs, ToolOutput};
use crate::sample::{Sample, allowed};

fn edit(sample: &Sample, args: &str) -> ToolOutput {
    let tool = Edit::new(sample.workspace(), Cancel::new());
    tool.run(allowed(&tool, args)).unwrap()
}

fn read(sample: &Sample, at: &str) -> String {
    fs::read_to_string(sample.root().join(at)).expect("the file under edit")
}

#[test]
fn the_text_found_once_is_replaced() {
    let sample = Sample::new("edit-one");
    sample.write("one.rs", "let a = 1;\nlet b = 2;\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","find":"let b = 2;","replace":"let b = 3;"}"#,
    );

    assert_eq!(read(&sample, "one.rs"), "let a = 1;\nlet b = 3;\n");
    assert_eq!(output.text(), "changed one.rs, 1 replacements");
}

#[test]
fn text_that_appears_twice_is_refused_rather_than_guessed_at() {
    // Replacing the first would change a line the model never looked at, and
    // it has no way to find out which one it got.
    let sample = Sample::new("edit-ambiguous");
    sample.write("one.rs", "x = 1;\nx = 1;\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","find":"x = 1;","replace":"x = 2;"}"#,
    );

    assert!(output.is_failed());
    assert!(
        output.text().contains("appears 2 times"),
        "{}",
        output.text()
    );
    assert_eq!(read(&sample, "one.rs"), "x = 1;\nx = 1;\n");
}

#[test]
fn every_occurrence_goes_when_the_call_asks_for_it() {
    let sample = Sample::new("edit-all");
    sample.write("one.rs", "x = 1;\nx = 1;\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","find":"x = 1;","replace":"x = 2;","all":true}"#,
    );

    assert_eq!(read(&sample, "one.rs"), "x = 2;\nx = 2;\n");
    assert_eq!(output.text(), "changed one.rs, 2 replacements");
}

#[test]
fn an_empty_replacement_deletes_the_text() {
    let sample = Sample::new("edit-delete");
    sample.write("one.rs", "keep\nremove\n");

    edit(
        &sample,
        r#"{"path":"one.rs","find":"remove\n","replace":""}"#,
    );

    assert_eq!(read(&sample, "one.rs"), "keep\n");
}

#[test]
fn text_that_is_not_there_is_a_result_the_model_can_act_on() {
    let sample = Sample::new("edit-absent");
    sample.write("one.rs", "let a = 1;\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","find":"let z = 9;","replace":"x"}"#,
    );

    assert!(output.is_failed());
    assert_eq!(output.text(), "that text does not appear in one.rs");
}

#[test]
fn replacing_text_with_itself_is_refused_before_the_file_is_touched() {
    let sample = Sample::new("edit-noop");
    sample.write("one.rs", "same\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","find":"same","replace":"same"}"#,
    );

    assert!(output.is_failed());
    assert_eq!(read(&sample, "one.rs"), "same\n");
}

#[test]
fn indentation_is_part_of_the_text_that_has_to_match() {
    let sample = Sample::new("edit-indent");
    sample.write("one.rs", "fn f() {\n    let a = 1;\n}\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","find":"let a = 1;","replace":"let a = 2;"}"#,
    );

    // The quoted text is inside the line, so it still matches once — what this
    // pins is that the surrounding spaces survive untouched.
    assert_eq!(read(&sample, "one.rs"), "fn f() {\n    let a = 2;\n}\n");
    assert!(!output.is_failed());
}

#[test]
fn a_path_outside_the_workspace_is_refused_without_reading_it() {
    let sample = Sample::new("edit-escape");
    let outside = sample.outside("secret.txt", "classified");

    let output = edit(
        &sample,
        &format!(r#"{{"path":"{outside}","find":"classified","replace":"x"}}"#),
    );

    assert!(output.is_failed());
    assert!(!output.text().contains("changed"));
}

#[test]
fn a_missing_file_says_so() {
    let sample = Sample::new("edit-missing");

    let output = edit(&sample, r#"{"path":"absent.rs","find":"a","replace":"b"}"#);

    assert!(output.is_failed());
    assert!(
        output.text().contains("does not exist"),
        "{}",
        output.text()
    );
}

#[test]
fn something_that_is_not_text_says_so_instead_of_corrupting_it() {
    let sample = Sample::new("edit-binary");
    sample.write_bytes("blob.bin", &[0xff, 0xfe, 0x00, 0x01]);

    let output = edit(&sample, r#"{"path":"blob.bin","find":"a","replace":"b"}"#);

    assert!(output.is_failed());
    assert_eq!(output.text(), "blob.bin is not a text file");
}

#[test]
fn a_file_over_the_input_ceiling_is_refused_without_reading_it_whole() {
    let sample = Sample::new("edit-large-input");
    let path = sample.root().join("large.txt");
    let mut source = fs::File::create(&path).unwrap();
    std::io::Write::write_all(&mut source, b"a").unwrap();
    source.set_len((super::FILE_LIMIT + 1) as u64).unwrap();
    drop(source);

    let output = edit(&sample, r#"{"path":"large.txt","find":"a","replace":"b"}"#);

    assert!(output.is_failed());
    assert!(output.text().contains("too large to edit safely"));
    assert_eq!(fs::metadata(&path).unwrap().len(), 1_000_001);
    assert_eq!(fs::read(&path).unwrap().first(), Some(&b'a'));
}

#[test]
fn a_stopped_turn_does_not_scan_or_change_the_file() {
    let sample = Sample::new("edit-cancelled");
    sample.write("one.txt", &"a".repeat(super::FILE_LIMIT));
    let cancel = Cancel::new();
    cancel.request();
    let tool = Edit::new(sample.workspace(), cancel);

    let problem = tool
        .run(allowed(
            &tool,
            r#"{"path":"one.txt","find":"a","replace":"b","all":true}"#,
        ))
        .unwrap_err();

    assert!(matches!(
        problem,
        crucible_core::ToolError::Cancelled("edit")
    ));
    assert_eq!(
        fs::metadata(sample.root().join("one.txt")).unwrap().len(),
        1_000_000
    );
}

#[cfg(unix)]
#[test]
fn a_fifo_is_refused_before_the_edit_can_wait_for_input() {
    let sample = Sample::new("edit-fifo");
    let made = std::process::Command::new("mkfifo")
        .arg(sample.root().join("waiting"))
        .status()
        .unwrap();
    assert!(made.success());

    let output = edit(&sample, r#"{"path":"waiting","find":"a","replace":"b"}"#);

    assert!(output.is_failed());
    assert!(output.text().contains("is not a regular file"));
}

#[test]
fn a_replacement_over_the_output_ceiling_leaves_the_original_whole() {
    let sample = Sample::new("edit-large-output");
    sample.write("one.txt", "a");
    let replacement = "b".repeat(super::FILE_LIMIT + 1);

    let output = edit(
        &sample,
        &format!(r#"{{"path":"one.txt","find":"a","replace":"{replacement}"}}"#),
    );

    assert!(output.is_failed());
    assert!(output.text().contains("too large to edit safely"));
    assert_eq!(read(&sample, "one.txt"), "a");
}

#[cfg(unix)]
#[test]
fn an_edit_preserves_the_existing_file_mode() {
    let sample = Sample::new("edit-mode");
    let path = sample.root().join("one.txt");
    sample.write("one.txt", "before\n");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

    let output = edit(
        &sample,
        r#"{"path":"one.txt","find":"before","replace":"after"}"#,
    );

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

#[test]
fn a_call_with_no_find_says_what_is_missing() {
    let sample = Sample::new("edit-nofind");

    let tool = Edit::new(sample.workspace(), Cancel::new());
    let problem = tool
        .run(allowed(&tool, r#"{"path":"one.rs","replace":"b"}"#))
        .unwrap_err();

    assert_eq!(problem.to_string(), "edit: find is required");
}

#[test]
fn editing_names_the_file_it_would_change() {
    let sample = Sample::new("edit-sensitivity");
    sample.write("one.rs", "a\n");
    let tool = Edit::new(sample.workspace(), Cancel::new());

    let sensitivity = tool.sensitivity(&ToolArgs::new(r#"{"path":"one.rs"}"#));

    assert!(matches!(sensitivity, Sensitivity::MutatesFile { .. }));
    assert_eq!(sensitivity.to_string(), "change one.rs");
}
