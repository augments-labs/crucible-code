use super::*;

/// Reads one delta and returns every run with the slot it was said under.
fn read(markdown: &mut Markdown, delta: &str) -> Vec<(Slot, String)> {
    let mut said = Vec::new();
    markdown.read(delta, &mut |slot, text| said.push((slot, text.to_owned())));
    said
}

/// Everything a scan drew, with the markers gone.
fn drawn(said: &[(Slot, String)]) -> String {
    said.iter().map(|(_, text)| text.as_str()).collect()
}

/// The slots a scan used, in order, with runs of the same one collapsed.
fn slots(said: &[(Slot, String)]) -> Vec<Slot> {
    said.iter().fold(Vec::new(), |mut worn, (slot, _)| {
        if worn.last() != Some(slot) {
            worn.push(*slot);
        }
        worn
    })
}

/// One whole answer, read as one delta.
fn whole(answer: &str) -> Vec<(Slot, String)> {
    read(&mut Markdown::default(), answer)
}

#[test]
fn text_with_no_markers_in_it_is_one_run_of_plain() {
    let said = whole("the answer, as it arrived");

    assert_eq!(
        said.first(),
        Some(&(Slot::Plain, "the answer, as it arrived".to_owned()))
    );
    assert_eq!(said.len(), 1, "a delta with no markers is one run");
}

#[test]
fn the_marker_is_dropped_and_the_run_it_covered_wears_the_slot() {
    let said = whole("a **loud** word");

    assert_eq!(drawn(&said), "a loud word");
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Strong, Slot::Plain]);
}

#[test]
fn one_marker_and_two_say_the_same_thing() {
    assert_eq!(
        slots(&whole("a *loud* word")),
        slots(&whole("a **loud** word"))
    );
}

#[test]
fn a_heading_loses_its_hashes_and_keeps_its_words() {
    let said = whole("### What it costs\nthe paragraph under it");

    assert_eq!(drawn(&said), "What it costs\nthe paragraph under it");
    assert_eq!(slots(&said), vec![Slot::Strong, Slot::Plain]);
}

#[test]
fn a_hash_that_no_space_follows_is_a_hash() {
    assert_eq!(drawn(&whole("#3 in the list")), "#3 in the list");
}

#[test]
fn a_hash_partway_along_a_line_is_a_hash() {
    assert_eq!(drawn(&whole("issue # 12")), "issue # 12");
}

#[test]
fn inline_code_is_quiet_and_loses_its_backticks() {
    let said = whole("call `read` for it");

    assert_eq!(drawn(&said), "call read for it");
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Quiet, Slot::Plain]);
}

#[test]
fn a_fence_and_the_language_written_on_it_take_no_row_of_their_own() {
    let said = whole("before\n```rust\nlet it = 1;\n```\nafter\n");

    assert_eq!(drawn(&said), "before\nlet it = 1;\nafter\n");
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Quiet, Slot::Plain]);
}

#[test]
fn everything_inside_a_fence_is_code_however_it_is_punctuated() {
    let said = whole("```\n# not a heading **not bold** _not_\n```\n");

    assert_eq!(drawn(&said), "# not a heading **not bold** _not_\n");
    assert_eq!(slots(&said), vec![Slot::Quiet]);
}

#[test]
fn one_backtick_inside_a_fence_is_a_backtick() {
    let said = whole("```\n`quoted` in a shell\n```\n");

    assert_eq!(drawn(&said), "`quoted` in a shell\n");
    assert_eq!(slots(&said), vec![Slot::Quiet]);
}

#[test]
fn a_bullet_stays_a_bullet_rather_than_opening_emphasis() {
    let said = whole("* first\n* second\n");

    assert_eq!(drawn(&said), "* first\n* second\n");
    assert_eq!(slots(&said), vec![Slot::Plain]);
}

#[test]
fn an_underscore_inside_a_word_is_part_of_the_word() {
    let said = whole("call read_to_string on it");

    assert_eq!(drawn(&said), "call read_to_string on it");
    assert_eq!(slots(&said), vec![Slot::Plain]);
}

#[test]
fn an_underscore_around_a_word_is_emphasis() {
    let said = whole("it is _yours_ now");

    assert_eq!(drawn(&said), "it is yours now");
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Strong, Slot::Plain]);
}

#[test]
fn a_marker_that_never_closes_costs_its_own_line_and_no_more() {
    let said = whole("**never closed\nthe line after it\n");

    assert_eq!(drawn(&said), "never closed\nthe line after it\n");
    assert_eq!(slots(&said), vec![Slot::Strong, Slot::Plain]);
}

#[test]
fn a_run_that_meant_nothing_is_put_back_where_it_was() {
    let said = whole("2 * 3 * 4");

    assert_eq!(drawn(&said), "2 * 3 * 4");
    assert_eq!(slots(&said), vec![Slot::Plain]);
}

#[test]
fn a_marker_split_across_two_deltas_is_still_one_marker() {
    let mut markdown = Markdown::default();

    let first = read(&mut markdown, "a *");
    let second = read(&mut markdown, "*loud** word");

    assert_eq!(drawn(&first), "a ");
    assert_eq!(drawn(&second), "loud word");
    assert_eq!(slots(&second), vec![Slot::Strong, Slot::Plain]);
}

#[test]
fn a_fence_split_across_three_deltas_is_still_one_fence() {
    let mut markdown = Markdown::default();

    let first = read(&mut markdown, "``");
    let second = read(&mut markdown, "`rust\nlet it");
    let third = read(&mut markdown, " = 1;\n```\n");

    assert_eq!(drawn(&first), "");
    assert_eq!(drawn(&second), "let it");
    assert_eq!(drawn(&third), " = 1;\n");
    assert_eq!(slots(&third), vec![Slot::Quiet]);
}
