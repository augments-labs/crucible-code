use super::*;

/// A diff of `count` lines that all went in, numbered from one.
fn added(count: usize) -> Diff {
    Diff::new((1..=count).map(|number| Line::new(number, Change::Added, format!("line {number}"))))
}

#[test]
fn a_diff_counts_the_lines_that_moved_and_not_the_ones_it_kept() {
    let diff = Diff::new([
        Line::new(303, Change::Kept, "digest=$(<artifact/digest)"),
        Line::new(306, Change::Removed, "# Shared-runner numbers are trend"),
        Line::new(306, Change::Added, "# What stops a tag whose build"),
        Line::new(307, Change::Added, "# got slower is the budget"),
        Line::new(315, Change::Kept, "budgets:"),
    ]);

    assert_eq!((diff.added(), diff.removed()), (2, 1));
    assert_eq!(diff.lines().len(), 5);
    assert_eq!(diff.dropped(), 0);
    assert!(!diff.is_empty());
}

#[test]
fn retained_bytes_include_line_text_and_storage() {
    let diff = Diff::new([
        Line::new(1, Change::Removed, "old"),
        Line::new(1, Change::Added, "new text"),
    ]);

    assert!(diff.retained() >= "old".len() + "new text".len());
}

#[test]
fn a_diff_that_moved_nothing_is_empty_however_many_lines_it_carries() {
    // Which is what a call that found what it was looking for and replaced it
    // with itself produces. The lines around it are still lines.
    let unchanged = Diff::new([Line::new(1, Change::Kept, "budgets:")]);

    assert!(unchanged.is_empty());
    assert!(Diff::new([]).is_empty());
}

#[test]
fn a_diff_past_its_bound_keeps_the_first_of_it_and_says_what_it_dropped() {
    // The bound is on what crosses a thread and waits to be drawn, so it is the
    // *front* that is kept: a reader scrolling back to a change reads it from
    // the top, and a block that kept the tail would start in the middle of one.
    let diff = added(Diff::LINES + 10);

    assert_eq!(diff.lines().len(), Diff::LINES);
    assert_eq!(diff.dropped(), 10);
    assert_eq!(diff.lines().first().map(Line::number), Some(1));
}

#[test]
fn the_count_a_diff_reports_is_of_the_call_and_not_of_what_survived_the_cut() {
    // The row above the block says how many lines went in. It is answering "what
    // did this call do", so a cut further down may not change it -- a reader
    // told that seventy-four lines went in has been told the truth, whether or
    // not all seventy-four are under it.
    let diff = added(Diff::LINES + 10);

    assert_eq!(diff.added(), Diff::LINES + 10);
    assert!(diff.lines().len() < diff.added());
}

#[test]
fn a_line_past_its_bound_is_cut_and_never_inside_a_character() {
    // A minified file is one line and a megabyte of it, and it arrives here the
    // same as any other. Cutting by bytes would leave the last one split down
    // the middle, which is a panic in a `String` and a replacement character
    // everywhere else.
    let long = Line::new(1, Change::Added, "é".repeat(Line::TEXT * 2));

    assert_eq!(long.text().chars().count(), Line::TEXT);
    assert!(long.text().chars().all(|character| character == 'é'));

    // And a line under the bound is left exactly as it arrived.
    let short = Line::new(2, Change::Removed, "  budgets:");
    assert_eq!(short.text(), "  budgets:");
}

#[test]
fn a_line_keeps_the_number_and_the_direction_it_was_given() {
    let line = Line::new(306, Change::Removed, "# Shared-runner numbers");

    assert_eq!(line.number(), 306);
    assert_eq!(line.change(), Change::Removed);
}

#[test]
fn nothing_a_diff_read_out_of_a_file_reaches_a_debug_line() {
    // A diff of a file holding a key is a key. `Debug` is what a log line, an
    // error and a panic payload all reach for, so it is the one that has to be
    // safe rather than the one that has to be useful.
    let secret = "sk-ant-not-a-real-key";
    let diff = Diff::new([
        Line::new(1, Change::Removed, format!("key = \"{secret}\"")),
        Line::new(1, Change::Added, "key = { env = \"ANTHROPIC_API_KEY\" }"),
    ]);

    let said = format!("{diff:?} {:?}", diff.lines());

    assert!(!said.contains(secret), "{said}");
    assert!(!said.contains("ANTHROPIC_API_KEY"), "{said}");

    // And what is left is still worth printing: the shape of the change, and
    // the numbers a reader would be looking at the diff to check.
    assert!(said.contains("added: 1"), "{said}");
    assert!(said.contains("Removed"), "{said}");
}
