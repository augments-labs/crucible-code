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

#[test]
fn restored_preview_keeps_omitted_counts_without_materializing_omitted_lines() {
    let original = added(Diff::LINES + 10);
    let restored = Diff::restored(original.lines().to_vec(), 74, 0, 10).unwrap();
    assert_eq!(restored, original);
}

#[test]
fn restored_preview_rejects_counts_that_cannot_describe_its_lines() {
    let kept = vec![Line::new(1, Change::Kept, "context")];
    assert!(Diff::restored(kept.clone(), 1, 0, 0).is_none());
    assert!(Diff::restored(kept, 0, 0, 1).is_none());
    let full = added(Diff::LINES).lines().to_vec();
    assert!(Diff::restored(full.clone(), Diff::LINES - 1, 0, 0).is_none());
    assert!(Diff::restored(full.clone(), usize::MAX, usize::MAX, usize::MAX).is_none());
    assert!(
        Diff::restored(
            vec![Line::new(1, Change::Kept, ""); Diff::LINES + 1],
            0,
            0,
            0
        )
        .is_none()
    );
}

#[test]
fn a_diff_says_which_file_it_changed_once_told() {
    let diff = added(2);
    assert_eq!(diff.path(), None, "a diff no tool named a file for");

    let named = added(2).in_file("src/main.rs");
    assert_eq!(named.path(), Some("src/main.rs"));
    // Naming the file moves none of what the change was.
    assert_eq!((named.added(), named.lines().len()), (2, 2));

    // A name with nothing in it is no name, and a header drawn from it would
    // open on a gap.
    assert_eq!(added(1).in_file("").path(), None);
}

#[test]
fn a_path_past_its_bound_keeps_its_end_and_never_cuts_inside_a_character() {
    // The end is the file's own name, which is what a reader looks for; the
    // directories in front of it are what can go. A path is model output and
    // so arrives at any length.
    let long = format!("{}/name.rs", "é".repeat(Diff::PATH * 2));

    let diff = added(1).in_file(long);
    let path = diff.path().expect("a path cut, not dropped");

    assert_eq!(path.chars().count(), Diff::PATH);
    assert!(path.ends_with("/name.rs"), "{path}");
    assert!(path.starts_with('é'), "{path}");
}

#[test]
fn the_path_a_diff_names_is_kept_out_of_debug_and_counted_as_retained() {
    // A path can name what the file holds, as a key file's name does, and it
    // is held beside the lines until the change is drawn.
    let path = "secrets/sk-ant-not-a-real-key.env";
    let diff = added(1).in_file(path);

    let said = format!("{diff:?}");
    assert!(!said.contains("sk-ant"), "{said}");

    assert!(diff.retained() >= added(1).retained() + path.len());
}
