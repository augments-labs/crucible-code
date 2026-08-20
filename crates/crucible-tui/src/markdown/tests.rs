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
fn one_marker_is_emphasis_and_two_are_weight() {
    assert_eq!(
        slots(&whole("a *leant on* word")),
        vec![Slot::Plain, Slot::Emphasis, Slot::Plain]
    );
    assert_eq!(
        slots(&whole("a **loud** word")),
        vec![Slot::Plain, Slot::Strong, Slot::Plain]
    );
    assert_eq!(
        slots(&whole("a ***both*** word")),
        vec![Slot::Plain, Slot::Strong, Slot::Plain],
        "three markers are both, and weight is the louder"
    );
}

#[test]
fn the_two_are_told_apart_inside_one_line() {
    let said = whole("*this* and **that**");

    assert_eq!(drawn(&said), "this and that");
    assert_eq!(
        slots(&said),
        vec![Slot::Emphasis, Slot::Plain, Slot::Strong]
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

    // The fence and the word after it are markers, so neither is drawn and
    // neither costs a row. What is between them is the block.
    assert_eq!(drawn(&said), "before\nlet it = 1;\nafter\n");

    // The block is read rather than quiet, because the fence named a language
    // this build knows. What it is *not* is prose: nothing inside it comes back
    // Plain-and-nothing-else the way the words on either side do.
    let inside: Vec<Slot> = said
        .iter()
        .skip_while(|(_, text)| !text.contains("let"))
        .take_while(|(_, text)| !text.contains("after"))
        .map(|(slot, _)| *slot)
        .collect();

    assert!(inside.contains(&Slot::Keyword), "{said:?}");
    assert_eq!(
        slots(&said).first().copied(),
        Some(Slot::Plain),
        "the prose before it moved"
    );
    assert_eq!(
        slots(&said).last().copied(),
        Some(Slot::Plain),
        "the prose after it moved"
    );
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
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Emphasis, Slot::Plain]);
}

#[test]
fn two_underscores_around_a_word_are_weight() {
    let said = whole("it is __yours__ now");

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

    // Nothing yet: a block being read is held until its line is whole, because
    // a highlighter reads lines and a delta is a piece of the wire. So code
    // arrives on screen a line at a time where prose arrives a character at a
    // time — bounded by one line, and the price of reading it at all.
    assert_eq!(drawn(&second), "");
    assert_eq!(drawn(&third), "let it = 1;\n");

    let read_as: Vec<Slot> = third.iter().map(|(slot, _)| *slot).collect();
    assert!(read_as.contains(&Slot::Keyword), "{third:?}");
}

#[test]
fn a_fenced_block_in_a_language_this_build_knows_is_read() {
    // The whole point of the fence keeping its language.
    let said = whole("```rust\nlet x = 1; // hi\n```\n");
    let kinds: Vec<Slot> = said.iter().map(|(slot, _)| *slot).collect();

    assert!(kinds.contains(&Slot::Keyword), "no keyword: {said:?}");
    assert!(kinds.contains(&Slot::Comment), "no comment: {said:?}");
}

#[test]
fn a_fence_that_named_nothing_is_the_block_it_always_was() {
    // The fallback, unchanged: quiet and whole.
    let said = whole("```\nlet x = 1;\n```\n");

    assert!(
        said.iter().all(|(slot, _)| *slot == Slot::Quiet),
        "something was read: {said:?}"
    );
}

#[test]
fn a_fence_naming_something_nothing_knows_is_the_block_it_always_was() {
    let said = whole("```wingdings\nlet x = 1;\n```\n");

    assert!(
        said.iter().all(|(slot, _)| *slot == Slot::Quiet),
        "something was read: {said:?}"
    );
}

#[test]
fn a_block_that_was_read_says_exactly_what_arrived() {
    // Every byte of the code comes back, once, in order — the fence's own two
    // lines being the markers they always were.
    let code = "fn main() {\n    let x = 1; // hi\n    println!(\"{x}\");\n}\n";
    let said = whole(&format!("```rust\n{code}```\n"));
    let text: String = said.iter().map(|(_, text)| text.as_str()).collect();

    assert_eq!(text, code);
}

#[test]
fn a_block_read_a_character_at_a_time_says_the_same_thing() {
    // A delta is a piece of the wire, so a fence, a language and a line of code
    // all arrive split as often as not.
    let streamed = "```rust\nlet x = 1; // hi\nlet y = 2;\n```\n";
    let together = self::whole(streamed);

    let mut markdown = Markdown::default();
    let mut apart = Vec::new();
    for character in streamed.chars() {
        let mut piece = [0u8; 4];
        markdown.read(character.encode_utf8(&mut piece), &mut |slot, text| {
            apart.push((slot, text.to_owned()));
        });
    }

    let joined = |runs: &[(Slot, String)]| -> Vec<(Slot, String)> {
        let mut out: Vec<(Slot, String)> = Vec::new();
        for (slot, text) in runs {
            match out.last_mut() {
                Some((last, held)) if last == slot => held.push_str(text),
                _ => out.push((*slot, text.clone())),
            }
        }
        out
    };

    assert_eq!(joined(&apart), joined(&together));
}

#[test]
fn a_link_is_its_words_and_the_address_after_them() {
    let said = whole("see [the guide](https://example.com/guide) for more");

    assert_eq!(
        drawn(&said),
        "see the guide (https://example.com/guide) for more"
    );
    assert_eq!(
        slots(&said),
        vec![Slot::Plain, Slot::Link, Slot::Quiet, Slot::Plain]
    );
}

#[test]
fn a_link_whose_words_are_its_address_says_it_once() {
    let said = whole("[https://example.com](https://example.com)");

    assert_eq!(drawn(&said), "https://example.com");
    assert_eq!(slots(&said), vec![Slot::Link]);
}

#[test]
fn a_link_with_no_words_is_its_address() {
    let said = whole("[](https://example.com)");

    assert_eq!(drawn(&said), "https://example.com");
    assert_eq!(slots(&said), vec![Slot::Link]);
}

#[test]
fn the_title_after_an_address_is_not_part_of_it() {
    let said = whole("[a](<https://example.com> \"The title\")");

    assert_eq!(drawn(&said), "a (https://example.com)");
}

#[test]
fn a_bracket_that_was_never_a_link_is_left_exactly_as_it_was() {
    for written in [
        "[TODO] fix this",
        "an array is arr[0] and no more",
        "[unclosed and then the line ends\n",
        "[half](and then the line ends\n",
    ] {
        assert_eq!(drawn(&whole(written)), written, "{written:?}");
    }
}

#[test]
fn a_bracket_inside_a_span_of_code_is_an_index() {
    let said = whole("`arr[0](x)` and arr[1](y)");

    assert_eq!(drawn(&said), "arr[0](x) and arr1 (y)");
}

#[test]
fn a_link_split_across_deltas_is_still_one_link() {
    // Every boundary a socket could put in the middle of one, one at a time.
    let whole_link = "see [the guide](https://example.com) now";
    for at in 0..whole_link.len() {
        if !whole_link.is_char_boundary(at) {
            continue;
        }

        let mut markdown = Markdown::default();
        let mut said = read(&mut markdown, &whole_link[..at]);
        said.extend(read(&mut markdown, &whole_link[at..]));
        markdown.finish(&mut |slot, text| said.push((slot, text.to_owned())));

        assert_eq!(
            drawn(&said),
            "see the guide (https://example.com) now",
            "split at {at}"
        );
    }
}

#[test]
fn a_message_that_ended_inside_a_link_keeps_what_it_wrote() {
    let mut markdown = Markdown::default();
    let mut said = read(&mut markdown, "see [the guide");
    markdown.finish(&mut |slot, text| said.push((slot, text.to_owned())));

    assert_eq!(drawn(&said), "see [the guide");
}

#[test]
fn a_link_in_a_fenced_block_is_code() {
    let said = whole("```\n[a](b)\n```\n");

    assert_eq!(drawn(&said), "[a](b)\n");
}
