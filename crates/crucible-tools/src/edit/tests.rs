//! What `edit` changes, and what it declines to guess at.

use crucible_core::Unwatched;

use std::fs;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use crucible_core::Change;

use super::{Cancel, Edit, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput};
use crate::sample::{Sample, allowed};

fn edit(sample: &Sample, args: &str) -> ToolOutput {
    let tool = Edit::new(sample.workspace(), Cancel::new());
    tool.run(allowed(&tool, args), &Unwatched).unwrap()
}

/// A call the tool cannot read, which ends the turn rather than answering.
fn refuse(sample: &Sample, args: &str) -> ToolError {
    let tool = Edit::new(sample.workspace(), Cancel::new());
    tool.run(allowed(&tool, args), &Unwatched).unwrap_err()
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
fn a_list_of_changes_is_made_in_one_call() {
    // Ten changes to one file are ten turns when each is its own call, and the
    // model has already read the file once for all of them.
    let sample = Sample::new("edit-list");
    sample.write("one.rs", "let a = 1;\nlet b = 2;\nlet b = 2;\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","edits":[
             {"find":"let a = 1;","replace":"let a = 9;"},
             {"find":"let b = 2;","replace":"let b = 7;","all":true}
           ]}"#,
    );

    assert_eq!(
        read(&sample, "one.rs"),
        "let a = 9;\nlet b = 7;\nlet b = 7;\n"
    );
    assert_eq!(output.text(), "changed one.rs, 3 replacements");
}

#[test]
fn each_change_looks_at_what_the_one_before_it_left() {
    let sample = Sample::new("edit-in-order");
    sample.write("one.rs", "old\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","edits":[
             {"find":"old","replace":"middle"},
             {"find":"middle","replace":"new"}
           ]}"#,
    );

    assert!(!output.is_failed(), "{}", output.text());
    assert_eq!(read(&sample, "one.rs"), "new\n");
}

#[test]
fn one_change_that_cannot_be_made_leaves_the_whole_file_as_it_was() {
    // A file half changed is a state nobody asked for, and the model cannot see
    // which half it got without reading the file back.
    let sample = Sample::new("edit-all-or-nothing");
    sample.write("one.rs", "let a = 1;\nlet b = 2;\n");

    let output = edit(
        &sample,
        r#"{"path":"one.rs","edits":[
             {"find":"let a = 1;","replace":"let a = 9;"},
             {"find":"let z = 9;","replace":"x"}
           ]}"#,
    );

    assert!(output.is_failed());
    assert_eq!(
        output.text(),
        "edit 2 of 2 could not be made, so nothing was changed: \
         that text does not appear in one.rs"
    );
    assert_eq!(read(&sample, "one.rs"), "let a = 1;\nlet b = 2;\n");
}

#[test]
fn a_call_that_sends_both_shapes_is_asked_to_pick_one() {
    // Taking one and dropping the other would do half of what the call said,
    // and say it succeeded.
    let sample = Sample::new("edit-both-shapes");
    sample.write("one.rs", "a\n");

    let problem = refuse(
        &sample,
        r#"{"path":"one.rs","find":"a","replace":"b","edits":[{"find":"a","replace":"c"}]}"#,
    );

    assert_eq!(
        problem.to_string(),
        "edit: send find and replace, or edits, but not both"
    );
    assert_eq!(read(&sample, "one.rs"), "a\n");
}

#[test]
fn a_list_that_is_not_a_list_of_changes_says_what_it_should_be() {
    let sample = Sample::new("edit-list-shape");

    let empty = refuse(&sample, r#"{"path":"one.rs","edits":[]}"#);
    let text = refuse(&sample, r#"{"path":"one.rs","edits":"find a, replace b"}"#);
    let element = refuse(&sample, r#"{"path":"one.rs","edits":["a"]}"#);

    assert_eq!(empty.to_string(), "edit: edits is empty");
    assert_eq!(text.to_string(), "edit: edits must be a list");
    assert_eq!(element.to_string(), "edit: edits[0] must be a JSON object");
}

#[test]
fn a_list_longer_than_the_ceiling_is_refused_before_any_scanning() {
    // The defect this catches: `edits` was the one list in the crate with no
    // ceiling, and each entry costs a scan of the whole file — so one bounded
    // call could buy unbounded work.
    let sample = Sample::new("edit-too-many");
    sample.write("one.rs", "a\n");
    // Alternating so that every entry succeeds on its own: the refusal this
    // asserts has to come from the count, not from a change mid-list finding
    // nothing left to change.
    let edits = (0..=super::MOST_EDITS)
        .map(|n| {
            if n % 2 == 0 {
                r#"{"find":"a","replace":"b"}"#
            } else {
                r#"{"find":"b","replace":"a"}"#
            }
        })
        .collect::<Vec<_>>()
        .join(",");

    let problem = refuse(
        &sample,
        &format!(r#"{{"path":"one.rs","edits":[{edits}]}}"#),
    );

    assert!(problem.to_string().contains("at most"), "{problem}");
}

#[test]
fn a_change_in_the_list_missing_a_field_says_which_one_it_was() {
    let sample = Sample::new("edit-list-nofind");

    let problem = refuse(
        &sample,
        r#"{"path":"one.rs","edits":[{"find":"a","replace":"b"},{"replace":"c"}]}"#,
    );

    assert_eq!(problem.to_string(), "edit: edits[1] find is required");
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
        .run(
            allowed(
                &tool,
                r#"{"path":"one.txt","find":"a","replace":"b","all":true}"#,
            ),
            &Unwatched,
        )
        .unwrap_err();

    assert!(matches!(
        problem,
        crucible_core::ToolError::Cancelled(ref tool) if &**tool == "edit"
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
        .run(
            allowed(&tool, r#"{"path":"one.rs","replace":"b"}"#),
            &Unwatched,
        )
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

#[test]
fn a_change_leaves_the_lines_it_moved_behind_for_whoever_is_watching() {
    let sample = Sample::new("edit-shown");
    sample.write("show.rs", "let a = 1;\nlet b = 2;\nlet c = 3;\n");

    let output = edit(
        &sample,
        r#"{"path":"show.rs","find":"let b = 2;","replace":"let b = 3;"}"#,
    );

    // The one thing an edit answers with that the model is never sent. The
    // result text says how many replacements were made; which lines they landed
    // on is a question only the two versions of the file can answer, and this
    // call is the last place both of them exist.
    let diff = output.diff().expect("the change the call made");
    let block: Vec<(usize, Change, &str)> = diff
        .lines()
        .iter()
        .map(|line| (line.number(), line.change(), line.text()))
        .collect();

    assert_eq!((diff.added(), diff.removed()), (1, 1));
    assert_eq!(
        block,
        [
            (1, Change::Kept, "let a = 1;"),
            (2, Change::Removed, "let b = 2;"),
            (2, Change::Added, "let b = 3;"),
            (3, Change::Kept, "let c = 3;"),
        ]
    );
}

#[test]
fn a_call_that_changed_nothing_leaves_nothing_to_draw() {
    let sample = Sample::new("edit-nothing-shown");
    sample.write("show.rs", "let a = 1;\n");

    let output = edit(
        &sample,
        r#"{"path":"show.rs","find":"let z = 9;","replace":"let z = 8;"}"#,
    );

    assert!(output.is_failed());
    assert!(output.diff().is_none());
}
