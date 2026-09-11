//! What the two versions of a file say happened between them.

use crucible_core::{Change, Diff};

use super::between;

/// A file of `lines`, each ended, the way one comes off a disk.
fn file(lines: &[&str]) -> String {
    lines.iter().flat_map(|line| [*line, "\n"]).collect()
}

/// `count` lines each saying which one it is, so a test can name a number.
fn run(word: &str, count: usize) -> String {
    let lines: Vec<String> = (1..=count).map(|at| format!("{word} {at}\n")).collect();
    lines.concat()
}

/// The block as a reader reads it: gutter number, mark, words.
fn shown(diff: &Diff) -> Vec<(usize, Change, &str)> {
    diff.lines()
        .iter()
        .map(|line| (line.number(), line.change(), line.text()))
        .collect()
}

/// The file the block above was designed against, with `comment` where the
/// comment block goes, and enough run-up that the numbers are the real ones.
fn release_yml(comment: &[&str]) -> String {
    let mut text = run("line", 302);
    text.push_str("        digest=$(<artifact/digest-linux-x86_64)\n");
    text.push_str("        scripts/smoke.sh --no-provider --checksum \"$digest\"\n\n");
    text.push_str(&file(comment));
    text.push_str("budgets:\n  name: release budgets\n  needs: gate\n");
    text
}

#[test]
fn a_replaced_comment_block_reads_the_way_it_sits_in_the_file() {
    let before = release_yml(&[
        "# Shared-runner numbers are trend data, not the quiet-machine release",
        "# reading required by RELEASING.md. Keeping them with the release still",
        "# makes a future comparison possible, and the environment file says what",
    ]);
    let after = release_yml(&[
        "# What stops a tag whose build got slower: every probe carries its own",
        "# limit and exits non-zero when it is over, so this job failing is the",
        "# release not happening.",
        "#",
        "# The numbers stay on the run rather than on the release. They are",
        "# shared-runner trend data, not the quiet-machine reading RELEASING.md",
        "# requires, and a reading nobody should compare against is worse sitting",
        "# beside the downloads than absent from them. The environment file says",
        "# what produced them, for whoever goes to the run to read the trend.",
    ]);

    let diff = between(&before, &after);

    assert_eq!((diff.added(), diff.removed()), (9, 3));
    assert_eq!(diff.dropped(), 0);

    // Three lines of run-up, what went, what came, three lines after. The two
    // sides of the change both start at 306 because that is where each of them
    // sits in its own version of the file; the lines below it are numbered 315
    // onwards because the file is six lines longer than it was.
    let block = shown(&diff);
    let numbers: Vec<(usize, Change)> = block.iter().map(|(at, how, _)| (*at, *how)).collect();
    assert_eq!(
        numbers,
        [
            (303, Change::Kept),
            (304, Change::Kept),
            (305, Change::Kept),
            (306, Change::Removed),
            (307, Change::Removed),
            (308, Change::Removed),
            (306, Change::Added),
            (307, Change::Added),
            (308, Change::Added),
            (309, Change::Added),
            (310, Change::Added),
            (311, Change::Added),
            (312, Change::Added),
            (313, Change::Added),
            (314, Change::Added),
            (315, Change::Kept),
            (316, Change::Kept),
            (317, Change::Kept),
        ]
    );
    assert_eq!(
        block.first().map(|(_, _, text)| *text),
        Some("        digest=$(<artifact/digest-linux-x86_64)")
    );
    assert_eq!(
        block.last().map(|(_, _, text)| *text),
        Some("  needs: gate")
    );
}

#[test]
fn a_call_that_left_the_file_alone_has_nothing_to_show() {
    let text = file(&["one", "two", "three"]);

    let diff = between(&text, &text);

    assert!(diff.is_empty());
    assert_eq!(diff.lines(), []);
}

#[test]
fn lines_put_in_are_shown_against_the_ones_they_landed_between() {
    let before = file(&["one", "two", "three", "four", "five"]);
    let after = file(&["one", "two", "three", "new", "four", "five"]);

    let diff = between(&before, &after);

    assert_eq!((diff.added(), diff.removed()), (1, 0));
    assert_eq!(
        shown(&diff),
        [
            (1, Change::Kept, "one"),
            (2, Change::Kept, "two"),
            (3, Change::Kept, "three"),
            (4, Change::Added, "new"),
            (5, Change::Kept, "four"),
            (6, Change::Kept, "five"),
        ]
    );
}

#[test]
fn lines_taken_out_keep_the_numbers_they_had_before_they_went() {
    let before = file(&["one", "two", "three", "four", "five"]);
    let after = file(&["one", "two", "five"]);

    let diff = between(&before, &after);

    assert_eq!((diff.added(), diff.removed()), (0, 2));
    assert_eq!(
        shown(&diff),
        [
            (1, Change::Kept, "one"),
            (2, Change::Kept, "two"),
            (3, Change::Removed, "three"),
            (4, Change::Removed, "four"),
            (3, Change::Kept, "five"),
        ]
    );
}

#[test]
fn a_change_at_either_end_of_a_file_has_context_on_one_side_only() {
    let top = between(
        &file(&["one", "two", "three"]),
        &file(&["ONE", "two", "three"]),
    );
    assert_eq!(
        shown(&top),
        [
            (1, Change::Removed, "one"),
            (1, Change::Added, "ONE"),
            (2, Change::Kept, "two"),
            (3, Change::Kept, "three"),
        ]
    );

    let bottom = between(
        &file(&["one", "two", "three"]),
        &file(&["one", "two", "THREE"]),
    );
    assert_eq!(
        shown(&bottom),
        [
            (1, Change::Kept, "one"),
            (2, Change::Kept, "two"),
            (3, Change::Removed, "three"),
            (3, Change::Added, "THREE"),
        ]
    );
}

#[test]
fn a_file_of_repeated_lines_is_never_counted_from_both_ends_at_once() {
    // Every line matches every other, so a scan from the top and a scan from
    // the bottom would each happily claim the whole file. What stops them
    // meeting in the middle and reporting a line as both kept and gone is the
    // room the first scan leaves the second.
    let before = file(&["same", "same", "same", "same"]);
    let after = file(&["same", "same"]);

    let diff = between(&before, &after);

    assert_eq!((diff.added(), diff.removed()), (0, 2));
    assert_eq!(
        shown(&diff),
        [
            (1, Change::Kept, "same"),
            (2, Change::Kept, "same"),
            (3, Change::Removed, "same"),
            (4, Change::Removed, "same"),
        ]
    );
}

#[test]
fn a_change_to_every_line_is_counted_whole_and_shown_as_far_as_it_is_allowed() {
    let before = run("old", 100);
    let after = run("new", 100);

    let diff = between(&before, &after);

    // The counts are of the call. What the block stops short of is said in the
    // same place rather than left for the reader to notice.
    assert_eq!((diff.added(), diff.removed()), (100, 100));
    assert_eq!(diff.lines().len(), Diff::LINES);
    assert_eq!(diff.dropped(), 200 - Diff::LINES);
}
