//! What `grep` finds, and what it declines to look at.

use std::path::Path;

use crucible_core::Disposition;

use super::{
    CEILING, Cancel, Found, Grep, Hit, MAX_LINE, NAMED, Partial, RegexMatcherBuilder,
    SearcherBuilder, Sensitivity, Stopping, Tool, ToolArgs, ToolOutput, Top, WIDTH, report, unread,
};
use crate::sample::{Sample, allowed, under};

fn grep(sample: &Sample, args: &str) -> ToolOutput {
    let tool = Grep::new(sample.workspace(), Cancel::new());
    tool.run(allowed(&tool, args)).unwrap()
}

/// The same search, with rules standing about files below the one searched.
fn grep_under(sample: &Sample, args: &str, rules: &[(Disposition, &str)]) -> ToolOutput {
    let tool = Grep::new(sample.workspace(), Cancel::new());
    tool.run(under(&tool, args, rules)).unwrap()
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
    assert!(output.text().contains("showing first 3 matches"));
}

#[test]
fn a_limit_past_the_ceiling_is_the_ceiling() {
    // A number the model wrote is an argument like any other. Left unbounded it
    // decides how many hits the walk holds and how long an answer the next
    // request carries.
    let sample = Sample::new("grep-ceiling");
    sample.write("many.txt", &"needle\n".repeat(CEILING + 50));

    let output = grep(&sample, r#"{"pattern":"needle","limit":100000}"#);

    assert_eq!(
        output
            .text()
            .lines()
            .filter(|line| line.starts_with("many.txt:"))
            .count(),
        CEILING
    );
    assert!(
        output
            .text()
            .contains(&format!("showing first {CEILING} matches")),
        "{}",
        output.text()
    );
}

#[test]
fn an_answer_is_bounded_in_bytes_as_well_as_in_lines() {
    // Two hundred matching lines is the default, and a line is cut at `WIDTH`
    // characters — so the count says how many lines come back and nothing at
    // all about how much text that is.
    let sample = Sample::new("grep-bytes");
    sample.write(
        "wide.txt",
        &format!("needle{}\n", "x".repeat(WIDTH)).repeat(300),
    );

    let output = grep(&sample, r#"{"pattern":"needle","limit":100000}"#);

    assert!(
        output.text().len() < crate::bound::OUTPUT + 200,
        "the answer was {} bytes",
        output.text().len()
    );
    assert!(
        output.text().contains("narrow the pattern"),
        "{}",
        output.text()
    );
}

#[test]
fn a_search_the_user_stopped_never_reaches_the_files() {
    // A walk is the slowest thing this crate does and the one place Ctrl-C had
    // nothing to stop: the tree here holds two matches, and a search that
    // answers with neither of them is one that stopped before it opened
    // anything.
    let sample = tree("grep-stopped");
    let cancel = Cancel::new();
    cancel.request();

    let tool = Grep::new(sample.workspace(), cancel);
    let output = tool.run(allowed(&tool, r#"{"pattern":"needle"}"#)).unwrap();

    assert!(output.is_failed());
    assert!(
        output.text().starts_with("nothing matched needle"),
        "{}",
        output.text()
    );
    assert!(
        output.text().contains("stopped before the walk finished"),
        "{}",
        output.text()
    );
}

#[test]
fn a_search_the_user_stopped_answers_with_what_it_had() {
    // Half a tree searched is half a tree searched. Failing instead would throw
    // away lines the turn had already been spent on — and what the answer owes
    // is that it does not come back looking complete.
    let found = Found {
        hits: vec![Hit {
            path: "src/main.rs".to_owned(),
            line: 2,
            text: "let needle = 1;".to_owned(),
        }],
        more: false,
        partly: Partial::default(),
        stopped: true,
    };

    let output = report(&found, "needle", 200);

    assert!(!output.is_failed());
    assert!(
        output.text().starts_with("src/main.rs:2:let needle = 1;\n"),
        "{}",
        output.text()
    );
    assert!(
        output.text().contains("stopped before the walk finished"),
        "{}",
        output.text()
    );
}

#[test]
fn partial_file_names_are_bounded_while_the_total_keeps_growing() {
    let mut partial = Partial::default();
    for number in (0..100).rev() {
        partial.add(format!("file-{number:03}.txt"));
    }

    assert_eq!(partial.names.len(), NAMED);
    assert_eq!(partial.total, 100);
    let note = unread(&partial);
    assert!(note.contains("file-000.txt, file-001.txt, file-002.txt"));
    assert!(note.contains("and 95 more"));
}

#[test]
fn a_search_stopped_inside_one_file_keeps_what_it_read_and_names_the_file() {
    // The walk answers between files, which leaves one file the size of a tree
    // as the wait a stopped turn would otherwise sit through. Stopping inside
    // one makes it a file the search did not finish, which the answer already
    // has a way to say.
    let sample = Sample::new("grep-stopped-inside");
    sample.write("many.txt", &"needle\n".repeat(50));
    let cancel = Cancel::new();
    cancel.request();

    // Reached from the workspace root rather than from the directory it was
    // made in, which is what the walk does — and on a machine where the one is
    // a symbolic link to the other, a path built the other way is a path the
    // walk could never hand over, so the file would be named in full.
    let workspace = sample.workspace();
    let root = workspace.existing(".").unwrap();
    let named = workspace.root().join("many.txt");
    let reached = root.walked(&named).unwrap();

    let tool = Grep::new(workspace.clone(), cancel);
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let matcher = RegexMatcherBuilder::new().build("needle").unwrap();
    let hits = std::sync::Mutex::new(Top::new(1_000));
    let found = tool.lines(&mut searcher, &matcher, &reached, &hits);
    let hits = hits.into_inner().unwrap();

    assert!(hits.hits.is_empty(), "it read after cancellation");
    assert_eq!(found.partly.as_deref(), Some("many.txt"));
}

#[test]
fn cancellation_is_checked_before_each_file_read() {
    let sample = Sample::new("grep-cancel-reader");
    sample.write("many.txt", &"x".repeat(super::MAX_LINE));
    let file = std::fs::File::open(sample.root().join("many.txt")).unwrap();
    let cancel = Cancel::new();
    let stopped = std::cell::Cell::new(false);
    let mut reader = Stopping {
        file: &file,
        cancel: &cancel,
        stopped: &stopped,
    };
    let mut block = [0_u8; 32];

    assert_eq!(std::io::Read::read(&mut reader, &mut block).unwrap(), 32);
    cancel.request();
    assert_eq!(std::io::Read::read(&mut reader, &mut block).unwrap(), 0);
    assert!(stopped.get());
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
fn a_file_that_is_valid_text_and_still_binary_is_left_alone() {
    // A NUL byte is valid UTF-8, so nothing about decoding stops this one — a
    // checked-in font, `.so` or fixture would otherwise be searched byte for
    // byte and its "lines" sent back to the model with NUL bytes inside them.
    // The sniff `rg` does by default is what draws the line, and the budget is
    // measured against `rg`.
    let sample = Sample::new("grep-binary-text");
    sample.write("one.txt", "needle\n");
    sample.write_bytes("blob.bin", b"\x00\x00needle in the noise\x00\x00");

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert_eq!(output.text(), "one.txt:1:needle\n");
}

#[test]
fn a_bad_byte_later_in_a_file_does_not_take_the_hits_before_it() {
    // Only a NUL stops the searcher, so a Latin-1 file is searched as the text
    // it nearly is. The sink decodes, so the line carrying the odd byte comes
    // back as an error — and that error used to throw away every hit the file
    // had already given up, leaving an answer that said the file held no match.
    let sample = Sample::new("grep-latin1");
    sample.write_bytes("latin1.txt", b"needle one\nneedle caf\xe9\n");

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert!(
        output.text().starts_with("latin1.txt:1:needle one\n"),
        "{}",
        output.text()
    );
    assert!(
        output.text().contains("stopped partway through latin1.txt"),
        "{}",
        output.text()
    );
}

#[test]
fn a_line_too_long_to_hold_stops_that_file_rather_than_the_process() {
    // A committed minified bundle: one ASCII line, no newline. The searcher has
    // to hold a whole line to scan it, so with no limit this file is the heap —
    // 200 MB of it takes the process to 413 MB, against a budget of 35. Bounded,
    // it is one file the answer says it did not finish, which is the note the
    // model can act on.
    let sample = Sample::new("grep-long-line");
    sample.write("src/main.rs", "let needle = 1;\n");
    sample.write_bytes("vendor/bundle.min.js", &b"var a=1;".repeat(MAX_LINE / 4));

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    // The rest of the tree is still searched, and searched to the end.
    assert!(
        output.text().starts_with("src/main.rs:1:let needle = 1;\n"),
        "{}",
        output.text()
    );
    assert!(
        output
            .text()
            .contains("stopped partway through vendor/bundle.min.js"),
        "{}",
        output.text()
    );
}

#[test]
fn a_file_the_search_could_not_finish_is_named_even_when_nothing_matched() {
    // "Nothing matched" is a claim about files that were read to the end, and a
    // file the search stopped partway through is not one of them.
    let sample = Sample::new("grep-latin1-first");
    sample.write_bytes("latin1.txt", b"caf\xe9 needle\nplain needle\n");

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert!(output.is_failed());
    assert!(
        output.text().starts_with("nothing matched needle"),
        "{}",
        output.text()
    );
    assert!(
        output.text().contains("stopped partway through latin1.txt"),
        "{}",
        output.text()
    );
}

#[test]
fn more_files_than_the_note_names_are_counted_instead() {
    // The note is output, and output here is bounded like the rest of it: a
    // tree of files the searcher cannot decode would otherwise put a line in
    // the answer for each of them.
    let sample = Sample::new("grep-latin1-many");
    for which in 0..8 {
        sample.write_bytes(&format!("f{which}.txt"), b"needle one\nneedle caf\xe9\n");
    }

    let output = grep(&sample, r#"{"pattern":"needle"}"#);

    assert!(
        output
            .text()
            .contains("f0.txt, f1.txt, f2.txt, f3.txt, f4.txt and 3 more"),
        "{}",
        output.text()
    );
}

#[test]
fn a_search_that_found_exactly_the_limit_does_not_claim_it_stopped_short() {
    // Whether the answer was cut used to be read off `hits.len() >= limit`
    // *after* the truncation, where "exactly this many exist" and "more exist
    // and were cut" look the same. Telling the model to narrow a pattern that
    // needed no narrowing costs it a turn to rediscover the same lines.
    let sample = Sample::new("grep-exact");
    sample.write("three.txt", &"needle\n".repeat(3));

    let output = grep(&sample, r#"{"pattern":"needle","limit":3}"#);

    assert_eq!(
        output.text(),
        "three.txt:1:needle\nthree.txt:2:needle\nthree.txt:3:needle\n"
    );
}

#[test]
fn a_file_a_deny_rule_names_is_never_searched() {
    // The call was decided about the directory, and a rule about a file below
    // it says nothing at that moment. The walk is where it has to be honoured,
    // or a deny rule protects a file from `read` and hands it back through the
    // search.
    let sample = tree("grep-denied");
    sample.write("private/token.txt", "the needle nobody may see\n");

    let output = grep_under(
        &sample,
        r#"{"pattern":"needle"}"#,
        &[(Disposition::Deny, "grep(private/**)")],
    );

    assert!(!output.text().contains("private/"), "{}", output.text());
    assert!(output.text().contains("src/main.rs:"), "{}", output.text());
}

#[test]
fn an_ask_rule_stops_a_walk_the_way_a_deny_rule_does() {
    // A read is never put to the user, so an `ask` about one is already a
    // refusal by the time the engine is done with it. A walk that honoured
    // only the deny list would leave half of somebody's rules looking like
    // protection while returning the file.
    let sample = tree("grep-ask");
    sample.write("private/token.txt", "the needle nobody may see\n");

    let output = grep_under(
        &sample,
        r#"{"pattern":"needle"}"#,
        &[(Disposition::Ask, "grep(private/**)")],
    );

    assert!(!output.text().contains("private/"), "{}", output.text());
    assert!(output.text().contains("src/main.rs:"), "{}", output.text());
}

#[test]
fn a_path_the_walk_could_not_have_reached_is_refused() {
    // Nothing the walker yields sits outside the root it was given, so this is
    // the answer to a question that cannot arise yet — which is why it is
    // pinned here. The one that costs nothing is the one that keeps a file out
    // of an answer.
    let sample = tree("grep-elsewhere");
    let tool = Grep::new(sample.workspace(), Cancel::new());
    let approved = under(
        &tool,
        r#"{"pattern":"needle"}"#,
        &[(Disposition::Deny, "grep(private/**)")],
    );
    let workspace = sample.workspace();
    let from = workspace
        .existing(".")
        .expect("the root is in the workspace");

    assert!(approved.denies(&workspace, &from, Path::new("/etc/passwd")));
}

#[test]
fn a_rule_about_every_tool_reaches_the_walk_too() {
    // `deny *(private/**)` is how somebody says it once instead of naming
    // `read`, `grep` and `glob` in turn, and the walk is one of the places it
    // has to hold for that to be true.
    let sample = tree("grep-every");
    sample.write("private/token.txt", "the needle nobody may see\n");

    let output = grep_under(
        &sample,
        r#"{"pattern":"needle"}"#,
        &[(Disposition::Deny, "*(private/**)")],
    );

    assert!(!output.text().contains("private/"), "{}", output.text());
    assert!(output.text().contains("src/main.rs:"), "{}", output.text());
}

#[test]
fn a_deny_rule_about_one_tool_is_not_about_another() {
    // A rule names a tool. `grep` skipping what `read` was denied would be a
    // second, invisible rule nobody wrote.
    let sample = tree("grep-denied-elsewhere");
    sample.write("private/token.txt", "a needle in the open\n");

    let output = grep_under(
        &sample,
        r#"{"pattern":"needle"}"#,
        &[(Disposition::Deny, "read(private/**)")],
    );

    assert!(
        output.text().contains("private/token.txt:"),
        "{}",
        output.text()
    );
}

#[test]
fn a_search_with_no_path_named_covers_the_whole_workspace() {
    // The honest answer to what it acts on, and a wider one than a file: see
    // the note on searching in the permissions documentation.
    let sample = Sample::new("grep-sensitivity");
    let tool = Grep::new(sample.workspace(), Cancel::new());

    let sensitivity = tool.sensitivity(&ToolArgs::new("{}"));

    assert!(matches!(sensitivity, Sensitivity::ReadOnly { .. }));
    assert_eq!(sensitivity.to_string(), "read .");
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_to_a_file_outside_the_workspace_is_not_searched() {
    // The walk decides this by not following links at all: a link's own type is
    // not a file, so the entry is skipped before anything opens it. Pinned
    // rather than left to the walker's default, because that default is a
    // setting somebody could turn on for a reason having nothing to do with
    // this — and what a search reads goes back to the model.
    let sample = Sample::new("grep-link-out");
    let secret = sample.outside("secret.txt", "the quick brown fox\n");
    crate::sample::symlink(&secret, sample.root().join("innocent.txt"));

    let output = grep(&sample, r#"{"pattern":"quick brown"}"#);

    assert!(
        !output.text().contains("the quick brown fox"),
        "a link led the search out of the workspace: {}",
        output.text()
    );
    assert!(
        !output.text().contains("innocent"),
        "the link was searched: {}",
        output.text()
    );
}

#[test]
fn a_file_redirected_after_the_walk_reached_it_is_not_searched() {
    let sample = Sample::new("grep-link-race");
    sample.write("innocent.txt", "ordinary contents\n");
    let secret = sample.outside("secret.txt", "the hidden needle\n");
    let workspace = sample.workspace();
    let from = workspace.existing(".").unwrap();
    let named = workspace.root().join("innocent.txt");
    let reached = from.walked(&named).unwrap();

    // This is the interval the old `search_path` left: the walker has already
    // accepted the regular directory entry, and another writer replaces its
    // name before the searcher opens it.
    std::fs::remove_file(&named).unwrap();
    crate::sample::symlink(&secret, &named);

    let tool = Grep::new(workspace, Cancel::new());
    let mut searcher = SearcherBuilder::new().line_number(true).build();
    let matcher = RegexMatcherBuilder::new().build("needle").unwrap();
    let hits = std::sync::Mutex::new(Top::new(1_000));
    let found = tool.lines(&mut searcher, &matcher, &reached, &hits);
    let hits = hits.into_inner().unwrap();

    assert!(hits.hits.is_empty(), "an outside file was searched");
    assert_eq!(found.partly.as_deref(), Some("innocent.txt"));
}

#[test]
fn parallel_search_keeps_the_same_lowest_hits_past_the_limit() {
    let sample = Sample::new("grep-lowest-global");
    for name in ["z.txt", "m.txt", "a.txt", "q.txt"] {
        sample.write(name, "needle\n");
    }

    for _ in 0..8 {
        let output = grep(&sample, r#"{"pattern":"needle","limit":2}"#);
        let lines: Vec<_> = output
            .text()
            .lines()
            .filter(|line| !line.starts_with('[') && !line.is_empty())
            .collect();
        assert_eq!(lines, ["a.txt:1:needle", "m.txt:1:needle"]);
    }
}

#[test]
fn global_hit_retention_never_grows_past_the_requested_limit() {
    let mut top = Top::new(3);
    for number in (0..10_000).rev() {
        top.add(Hit {
            path: format!("{number:05}.txt"),
            line: 1,
            text: "needle".into(),
        });
    }

    assert_eq!(top.hits.len(), 3);
    assert_eq!(top.total, 10_000);
    assert_eq!(
        top.hits
            .iter()
            .map(|hit| hit.path.as_str())
            .collect::<Vec<_>>(),
        ["00000.txt", "00001.txt", "00002.txt"]
    );
}
