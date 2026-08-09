//! What `grep` finds, and what it declines to look at.

use super::{Grep, Sensitivity, Tool, ToolArgs, ToolOutput, WIDTH};
use crate::sample::{Sample, allowed};

fn grep(sample: &Sample, args: &str) -> ToolOutput {
    let tool = Grep::new(sample.workspace());
    tool.run(allowed(&tool, args)).unwrap()
}

/// A tree with something to find in two files and something to skip.
fn tree(name: &str) -> Sample {
    let sample = Sample::new(name);
    sample.write("src/main.rs", "fn main() {\n    let needle = 1;\n}\n");
    sample.write("src/lib.rs", "// needle in a comment\npub fn other() {}\n");
    sample.write("notes.md", "no match here\n");
    sample
}

#[test]
fn a_match_comes_back_as_path_line_and_text() {
    let sample = tree("grep-basic");

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert_eq!(
        output.text(),
        "src/lib.rs:1:// needle in a comment\nsrc/main.rs:2:    let needle = 1;\n"
    );
}

#[test]
fn the_same_search_answers_the_same_way_twice() {
    // The walk is parallel, so without the sort the order is whichever
    // thread finished first.
    let sample = tree("grep-stable");

    let first = grep(&sample, r#"{"pattern":"needle"}"#);
    let again = grep(&sample, r#"{"pattern":"needle"}"#);

    assert_eq!(first.text(), again.text());
}

#[test]
fn nothing_matching_is_a_result_and_not_an_empty_answer() {
    let sample = tree("grep-empty");

    let output = grep(&sample, r#"{"pattern":"haystack"}"#);

    assert!(output.is_failed());
    assert_eq!(output.text(), "nothing matched haystack");
}

#[test]
fn a_glob_narrows_the_search_to_the_files_it_names() {
    let sample = tree("grep-glob");

    let output = grep(&sample, r#"{"pattern":"needle","glob":"**/main.rs"}"#);

    assert_eq!(output.text(), "src/main.rs:2:    let needle = 1;\n");
}

#[test]
fn a_path_narrows_the_search_to_one_directory() {
    let sample = tree("grep-path");
    sample.write("docs/guide.md", "needle in the docs\n");

    let output = grep(&sample, r#"{"pattern":"needle","path":"docs"}"#);

    assert_eq!(output.text(), "docs/guide.md:1:needle in the docs\n");
}

#[test]
fn a_gitignored_file_is_not_searched() {
    // It is generated or vendored, so a match in it is one the user cannot
    // act on — and searching it is most of what makes a naive walk slow.
    let sample = tree("grep-ignored");
    sample.write(".gitignore", "target/\n");
    sample.write("target/debug/build.rs", "let needle = 2;\n");

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert!(!output.text().contains("target/"), "{}", output.text());
}

#[test]
fn case_can_be_ignored_when_the_call_asks_for_it() {
    let sample = Sample::new("grep-case");
    sample.write("one.txt", "Needle\n");

    assert!(grep(&sample, r#"{"pattern":"needle"}"#).is_failed());
    assert_eq!(
        grep(&sample, r#"{"pattern":"needle","ignore_case":true}"#).text(),
        "one.txt:1:Needle\n"
    );
}

#[test]
fn a_limit_stops_and_says_that_it_did() {
    let sample = Sample::new("grep-limit");
    sample.write("many.txt", &"needle\n".repeat(20));

    let output = grep(&sample, r#"{"pattern":"needle","limit":3}"#);

    assert_eq!(
        output
            .text()
            .lines()
            .filter(|l| l.contains("needle"))
            .count(),
        3
    );
    assert!(output.text().contains("stopped at 3 matches"));
}

#[test]
fn a_pattern_that_is_not_a_regular_expression_says_so() {
    let sample = tree("grep-badre");

    let output = grep(&sample, r#"{"pattern":"([unclosed"}"#);

    assert!(output.is_failed());
    assert!(output.text().contains("not a valid regular expression"));
}

#[test]
fn a_path_outside_the_workspace_is_refused() {
    let sample = tree("grep-escape");
    let outside = sample.outside("secret.txt", "needle");

    let output = grep(
        &sample,
        &format!(r#"{{"pattern":"needle","path":"{outside}"}}"#),
    );

    assert!(output.is_failed());
    assert!(!output.text().contains("secret.txt:"));
}

#[test]
fn a_very_long_matching_line_is_cut() {
    let sample = Sample::new("grep-wide");
    sample.write("wide.txt", &format!("needle{}\n", "x".repeat(WIDTH * 2)));

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert!(output.text().ends_with("…\n"));
    assert!(output.text().len() < WIDTH * 2);
}

#[test]
fn something_that_is_not_text_contributes_nothing() {
    let sample = Sample::new("grep-binary");
    sample.write("one.txt", "needle\n");
    sample.write_bytes("blob.bin", b"needle\x00\xff\xfe");

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert_eq!(output.text(), "one.txt:1:needle\n");
}

#[test]
fn a_search_with_no_path_named_covers_the_whole_workspace() {
    // The honest answer to what it acts on, and a wider one than a file: see
    // the note on searching in the permissions documentation.
    let sample = Sample::new("grep-sensitivity");
    let tool = Grep::new(sample.workspace());

    let sensitivity = tool.sensitivity(&ToolArgs::new("{}"));

    assert!(matches!(sensitivity, Sensitivity::ReadOnly { .. }));
    assert_eq!(sensitivity.to_string(), "read .");
}
