use super::*;
use crate::width;

/// A window wide enough that nothing here is laid out against its edge.
const ROOM: usize = 80;

/// Reads one delta and returns every run with the slot it was said under.
fn read(markdown: &mut Markdown, delta: &str) -> Vec<(Slot, String)> {
    let mut said = Vec::new();
    markdown.read(delta, ROOM, &mut |slot, text| {
        said.push((slot, text.to_owned()));
    });
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
    // The star that opens a list and the star that opens a phrase are the same
    // character; the space after it is the whole of the difference.
    let said = whole("* first\n* second\n");

    assert_eq!(drawn(&said), "· first\n· second\n");
    assert_eq!(
        slots(&said),
        vec![Slot::Quiet, Slot::Plain, Slot::Quiet, Slot::Plain]
    );
}

#[test]
fn an_underscore_inside_a_word_is_part_of_the_word() {
    let said = whole("call read_to_string on it");

    assert_eq!(drawn(&said), "call read_to_string on it");
    assert_eq!(slots(&said), vec![Slot::Plain]);
}

#[test]
fn an_underscore_standing_in_for_a_value_is_not_emphasis() {
    let said = whole("match it { Ok(_) | Err(_) => () }");

    assert_eq!(drawn(&said), "match it { Ok(_) | Err(_) => () }");
    assert_eq!(slots(&said), vec![Slot::Plain]);
}

#[test]
fn an_underscore_a_word_does_not_follow_is_not_emphasis() {
    let said = whole("let (_, rest) = it.split_at(1);");

    assert_eq!(drawn(&said), "let (_, rest) = it.split_at(1);");
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
fn a_marker_that_never_closes_costs_its_own_paragraph_and_no_more() {
    let said = whole("**never closed\nthe line after it\n\na new paragraph\n");

    assert_eq!(
        drawn(&said),
        "never closed\nthe line after it\n\na new paragraph\n"
    );
    assert_eq!(
        slots(&said),
        vec![Slot::Strong, Slot::Plain, Slot::Strong, Slot::Plain]
    );
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
        markdown.read(
            character.encode_utf8(&mut piece),
            ROOM,
            &mut |slot, text| {
                apart.push((slot, text.to_owned()));
            },
        );
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
        markdown.finish(ROOM, &mut |slot, text| said.push((slot, text.to_owned())));

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
    markdown.finish(ROOM, &mut |slot, text| said.push((slot, text.to_owned())));

    assert_eq!(drawn(&said), "see [the guide");
}

#[test]
fn a_link_in_a_fenced_block_is_code() {
    let said = whole("```\n[a](b)\n```\n");

    assert_eq!(drawn(&said), "[a](b)\n");
}

#[test]
fn every_spelling_of_a_bullet_is_drawn_as_one_mark() {
    // A model writes all three and means the same thing by each. A reader
    // meets one list, so one mark.
    let said = whole("- dash\n* star\n+ plus\n");

    assert_eq!(drawn(&said), "· dash\n· star\n· plus\n");
}

#[test]
fn a_bullet_keeps_the_indentation_that_nests_it() {
    // The spaces before the marker are the whole of what says a list is inside
    // another one, and they arrive before anything is decided about the line.
    let said = whole("- one\n  - under it\n    - under that\n");

    assert_eq!(drawn(&said), "· one\n  · under it\n    · under that\n");
}

#[test]
fn the_mark_is_quiet_and_the_item_is_not() {
    // The words are what somebody is reading; the mark is what tells them
    // where one item stops.
    let said = whole("- an item\n");

    assert_eq!(slots(&said), vec![Slot::Quiet, Slot::Plain]);
}

#[test]
fn a_dash_that_is_not_a_bullet_is_left_exactly_where_it_was() {
    // Only at the start of a line, and only with a space after it. Everything
    // else is arithmetic, a flag, or a word somebody hyphenated.
    for prose in ["5 - 3 = 2", "pass --colour never", "-–—", "-no space"] {
        assert_eq!(drawn(&whole(prose)), prose, "{prose:?}");
    }
}

#[test]
fn a_star_at_the_start_of_a_line_is_still_emphasis_where_it_opens_one() {
    // The two are told apart by the space: `* item` is a list and `*word*` is
    // a phrase, wherever on the line either of them starts.
    let said = whole("*loud* at the start\n");

    assert_eq!(drawn(&said), "loud at the start\n");
    assert_eq!(slots(&said), vec![Slot::Emphasis, Slot::Plain]);
}

#[test]
fn a_quote_is_a_bar_and_the_words_beside_it() {
    let said = whole("> somebody else said this\n");

    assert_eq!(drawn(&said), "│ somebody else said this\n");
    // The break itself is written after the line's state is dropped, so that no
    // row carries a slot into the one below it.
    assert_eq!(slots(&said), vec![Slot::Quiet, Slot::Plain]);
}

#[test]
fn a_quote_ends_where_the_line_does() {
    // Like every other thing this reader opens: a model that quoted one line
    // has not put the rest of the answer in somebody else's mouth.
    let said = whole("> quoted\nplain again\n");

    assert_eq!(drawn(&said), "│ quoted\nplain again\n");
    assert_eq!(slots(&said), vec![Slot::Quiet, Slot::Plain]);
}

#[test]
fn a_greater_than_sign_in_the_middle_of_a_line_is_a_greater_than_sign() {
    for prose in ["if a > b", "a -> b", ">no space"] {
        assert_eq!(drawn(&whole(prose)), prose, "{prose:?}");
    }
}

#[test]
fn a_bullet_split_across_two_deltas_is_still_one_bullet() {
    // Which is how it arrives: a marker and the space after it land either
    // side of a delta boundary as often as not.
    let mut markdown = Markdown::default();

    let opened = read(&mut markdown, "-");
    let rest = read(&mut markdown, " an item");

    assert_eq!(drawn(&opened), "");
    assert_eq!(drawn(&rest), "· an item");
}

#[test]
fn nothing_inside_a_fence_is_read_as_a_bullet_or_a_quote() {
    // A block of code is full of both, and neither means anything there.
    let said = whole("```sh\n- not a bullet\n> not a quote\n```\n");

    assert!(
        drawn(&said).contains("- not a bullet"),
        "{:?}",
        drawn(&said)
    );
    assert!(drawn(&said).contains("> not a quote"), "{:?}", drawn(&said));
}

#[test]
fn the_marks_come_out_of_the_set_the_terminal_can_draw() {
    // A font without the box-drawing characters has neither of these, and a
    // bullet that arrives as a hollow square is worse than the dash it
    // replaced.
    let said = read(&mut Markdown::new(Glyphs::Ascii), "- item\n> quoted\n");

    assert_eq!(drawn(&said), "- item\n| quoted\n");
}

#[test]
fn a_mark_costs_the_line_the_columns_the_marker_would_have() {
    // Two columns either way, in both sets. A mark that cost a third would
    // push every nested list one column right of where the model put it.
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let said = read(&mut Markdown::new(glyphs), "- item");
        assert_eq!(
            unicode_width::UnicodeWidthStr::width(drawn(&said).as_str()),
            "- item".len(),
            "{glyphs:?}"
        );
    }
}

#[test]
fn two_tildes_take_a_phrase_back() {
    let said = whole("the ~~first~~ second answer");

    assert_eq!(drawn(&said), "the first second answer");
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Struck, Slot::Plain]);
}

#[test]
fn a_retraction_outranks_the_weight_it_was_written_in() {
    // Both are true of the phrase and a slot says one thing. What a reader
    // needs first is that it was taken back.
    assert_eq!(
        slots(&whole("**~~loud and wrong~~** and quiet and right")),
        vec![Slot::Struck, Slot::Plain]
    );
}

#[test]
fn a_tilde_that_is_not_a_pair_is_left_exactly_where_it_was() {
    // A home directory, a fence somebody wrote with tildes, and an
    // approximation. None of them is a retraction and all of them are text.
    assert_eq!(drawn(&whole("look in ~/Projects")), "look in ~/Projects");
    assert_eq!(drawn(&whole("~~~\ncode\n~~~\n")), "~~~\ncode\n~~~\n");
    assert_eq!(drawn(&whole("about ~40 lines")), "about ~40 lines");
}

#[test]
fn a_pair_left_dangling_at_the_end_of_a_line_strikes_nothing() {
    assert_eq!(
        drawn(&whole("nothing to strike ~~\n")),
        "nothing to strike ~~\n"
    );
    assert_eq!(slots(&whole("nothing to strike ~~\n")), vec![Slot::Plain]);
}

#[test]
fn a_retraction_ends_where_the_paragraph_does() {
    // Emphasis of every kind is a paragraph's, not a message's: a model that
    // opens one and never closes it costs the reader that paragraph and no
    // more.
    let said = whole("~~opened here\nand struck here\n\nand plain here\n");

    assert_eq!(
        slots(&said),
        vec![Slot::Struck, Slot::Plain, Slot::Struck, Slot::Plain]
    );
}

#[test]
fn a_retraction_split_across_two_deltas_is_still_one_retraction() {
    let mut markdown = Markdown::default();

    let first = read(&mut markdown, "the ~");
    let second = read(&mut markdown, "~first~~ answer");

    assert_eq!(drawn(&first), "the ");
    assert_eq!(drawn(&second), "first answer");
    assert_eq!(slots(&second), vec![Slot::Struck, Slot::Plain]);
}

#[test]
fn nothing_inside_a_fence_is_read_as_a_retraction() {
    let said = whole("```python\nx = ~~y\n```\n");

    assert!(drawn(&said).contains("x = ~~y"), "{}", drawn(&said));
}

/// One whole answer, read as one delta, laid out against `room` columns.
fn narrowed(answer: &str, room: usize) -> Vec<(Slot, String)> {
    let mut markdown = Markdown::default();
    let mut said = Vec::new();
    markdown.read(answer, room, &mut |slot, text| {
        said.push((slot, text.to_owned()));
    });
    markdown.finish(room, &mut |slot, text| said.push((slot, text.to_owned())));
    said
}

#[test]
fn a_table_is_drawn_as_one_rather_than_as_the_bars_it_was_written_with() {
    let said = whole("| file | lines |\n| --- | --- |\n| main.rs | 42 |\n\n");

    assert_eq!(
        drawn(&said),
        "file    │ lines\n\
         ────────┼──────\n\
         main.rs │ 42   \n\n"
    );
}

#[test]
fn a_column_is_as_wide_as_what_is_drawn_in_it() {
    // The markers inside a cell are read like any others, so the column is
    // measured against what reaches the screen. Counting what the model wrote
    // would leave every column after this one a place out.
    let said = whole("| file |\n| --- |\n| `main.rs` |\n\n");

    assert_eq!(drawn(&said), "file   \n───────\nmain.rs\n\n");
}

#[test]
fn a_delimiter_row_says_which_side_a_column_is_drawn_against() {
    let said = whole("| a | b | c |\n| :-- | --: | :-: |\n| x | x | x |\n\n");

    assert_eq!(
        drawn(&said),
        "a │ b │ c\n\
         ──┼───┼──\n\
         x │ x │ x\n\n"
    );
}

#[test]
fn bars_with_no_delimiter_row_under_them_are_left_exactly_where_they_were() {
    let written = "| not | a table |\n| still | not one |\n\n";

    assert_eq!(drawn(&whole(written)), written);
}

#[test]
fn a_table_split_across_deltas_is_still_one_table() {
    let mut markdown = Markdown::default();
    let mut said = Vec::new();

    for piece in ["| file |\n| -", "-- |\n| main", ".rs |\n\n"] {
        markdown.read(piece, ROOM, &mut |slot, text| {
            said.push((slot, text.to_owned()));
        });
    }

    assert_eq!(drawn(&said), "file   \n───────\nmain.rs\n\n");
}

#[test]
fn a_table_the_window_cannot_hold_gives_up_its_widest_column_first() {
    let said = narrowed(
        "| a | a very long cell indeed |\n| --- | --- |\n| b | c |\n\n",
        16,
    );

    assert_eq!(
        drawn(&said),
        "a │ a very long…\n\
         ──┼─────────────\n\
         b │ c           \n\n"
    );

    // The point of giving a column up: every row is exactly the window, so the
    // terminal never wraps one and the columns stay under each other.
    for row in drawn(&said).lines().filter(|row| !row.is_empty()) {
        assert_eq!(crate::width::columns(row), 16, "{row:?}");
    }
}

#[test]
fn a_window_too_narrow_for_a_table_at_all_gets_it_as_it_was_written() {
    let written = "| a | b | c | d |\n| --- | --- | --- | --- |\n| w | x | y | z |\n\n";

    assert_eq!(drawn(&narrowed(written, 6)), written);
}

#[test]
fn a_table_the_answer_ended_in_the_middle_of_is_still_drawn() {
    let mut markdown = Markdown::default();
    let mut said = Vec::new();

    markdown.read("| file |\n| --- |\n| main.rs |", ROOM, &mut |slot, text| {
        said.push((slot, text.to_owned()));
    });
    markdown.finish(ROOM, &mut |slot, text| said.push((slot, text.to_owned())));

    assert_eq!(drawn(&said), "file   \n───────\nmain.rs\n");
}

#[test]
fn nothing_inside_a_fence_is_read_as_a_table() {
    // The fence's own lines go the way every fence's do; the bars between them
    // are code and stay exactly as they were written.
    let said = whole("```\n| a | b |\n| --- | --- |\n```\n");

    assert_eq!(drawn(&said), "| a | b |\n| --- | --- |\n");
}

#[test]
fn the_header_is_raised_and_the_bars_are_quiet() {
    // Reading across: the header's two cells raised with the bar between them
    // quiet, the break, the rule, the break, then a body row whose cells are
    // the prose they were written as with the bar and the padding quiet.
    let said = whole("| file | lines |\n| --- | --- |\n| main.rs | 42 |\n\n");

    assert_eq!(
        slots(&said),
        vec![
            Slot::Strong,
            Slot::Quiet,
            Slot::Strong,
            Slot::Plain,
            Slot::Quiet,
            Slot::Plain,
            Slot::Quiet,
            Slot::Plain,
            Slot::Quiet,
            Slot::Plain,
        ]
    );
}

#[test]
fn a_block_of_bars_that_never_ends_is_written_out_rather_than_held() {
    // A table is drawn only once the last of it has arrived, so a block held is
    // a block not on screen. Past the cap it goes out as the model wrote it,
    // which is what it would have done had the reader never gathered it.
    let row = "| a | b |\n";
    let written = format!("| a | b |\n| --- | --- |\n{}", row.repeat(2000));

    let drawn = drawn(&whole(&written));

    assert!(drawn.contains(row), "the rows reach the screen");
    assert!(
        drawn.len() >= written.len() / 2,
        "held {} of {} written",
        drawn.len(),
        written.len()
    );
}

#[test]
fn a_task_a_bullet_opens_is_a_box_rather_than_the_brackets_it_was_written_with() {
    let said = whole("- [ ] a task not done\n- [x] one that is\n");

    assert_eq!(drawn(&said), "□ a task not done\n✓ one that is\n");
}

#[test]
fn a_finished_task_is_drawn_as_one_that_is_behind_you() {
    let said = whole("- [x] one that is\n");

    assert_eq!(
        slots(&said),
        vec![Slot::DoneMark, Slot::Quiet, Slot::Done, Slot::Plain],
        "the mark, the space after it, the words, and the break"
    );
}

#[test]
fn a_task_nobody_has_started_reads_as_the_prose_around_it() {
    let said = whole("- [ ] a task not done\n");

    assert_eq!(slots(&said), vec![Slot::Quiet, Slot::Plain]);
}

#[test]
fn a_bracket_that_is_not_a_box_leaves_the_bullet_it_follows_alone() {
    let said = whole("- see [TODO] in the grammar\n");

    assert_eq!(drawn(&said), "· see [TODO] in the grammar\n");
}

#[test]
fn a_box_that_does_not_open_an_item_is_the_brackets_somebody_wrote() {
    let said = whole("the set is [ ] until something is in it\n");

    assert_eq!(drawn(&said), "the set is [ ] until something is in it\n");
}

#[test]
fn a_link_that_opens_an_item_still_gets_its_bullet() {
    let said = whole("- [the page](https://example.invalid)\n");

    assert_eq!(drawn(&said), "· the page (https://example.invalid)\n");
}

#[test]
fn a_task_split_across_two_deltas_is_still_a_task() {
    let mut markdown = Markdown::default();
    let mut said = read(&mut markdown, "- [x");
    said.extend(read(&mut markdown, "] one that is\n"));

    assert_eq!(drawn(&said), "✓ one that is\n");
}

#[test]
fn a_line_of_dashes_alone_is_a_rule_between_the_blocks_either_side_of_it() {
    let said = narrowed("Here.\n\n---\n\nNext.\n", 20);

    assert_eq!(
        drawn(&said),
        format!("Here.\n\n{}\n\nNext.\n", "─".repeat(20))
    );
}

#[test]
fn a_rule_is_as_wide_as_the_window_it_is_drawn_in() {
    for room in [1, 12, 80] {
        let said = narrowed("---\n", room);

        assert_eq!(
            width::columns(&drawn(&said).replace('\n', "")),
            room,
            "a rule fills the room it was given and no more"
        );
    }
}

#[test]
fn a_rule_can_be_written_with_any_of_the_three_markers_that_mean_one() {
    for written in ["---\n", "***\n", "___\n"] {
        let said = narrowed(written, 8);

        assert_eq!(drawn(&said), "────────\n", "{written:?} is a rule");
    }
}

#[test]
fn a_rule_is_quiet_and_the_break_after_it_is_not() {
    let said = narrowed("---\n", 4);

    assert_eq!(slots(&said), vec![Slot::Quiet, Slot::Plain]);
}

#[test]
fn two_markers_alone_on_a_line_are_the_characters_somebody_wrote() {
    let said = narrowed("--\n", 20);

    assert_eq!(drawn(&said), "--\n");
}

#[test]
fn a_line_of_dashes_inside_a_fence_is_code_rather_than_a_rule() {
    let said = narrowed("```\n---\n```\n", 20);

    assert_eq!(drawn(&said), "---\n");
}

#[test]
fn dashes_partway_along_a_line_are_left_exactly_where_they_were() {
    let said = narrowed("a --- b\n", 20);

    assert_eq!(drawn(&said), "a --- b\n");
}

#[test]
fn a_message_that_ended_on_a_marker_ends_it_the_way_a_line_break_would() {
    for answer in [
        "one last thing*",
        "a path under ~",
        "**loud and never closed, and never ended**",
        "a trailing tick`",
        "## a heading with no break after it",
        "- an item with no break after it",
        "---",
    ] {
        let ended = narrowed(answer, ROOM);
        let broken = narrowed(&format!("{answer}\n"), ROOM);

        assert_eq!(
            drawn(&ended),
            drawn(&broken).trim_end_matches('\n'),
            "{answer:?} reads the same whether or not a break followed it"
        );
    }
}

#[test]
fn a_marker_a_message_ended_on_and_that_meant_nothing_is_put_back() {
    for answer in ["one last thing*", "a path under ~", "two hashes ##"] {
        let said = narrowed(answer, ROOM);

        assert_eq!(drawn(&said), answer, "every byte comes back exactly once");
    }
}

#[test]
fn a_marker_inside_a_code_span_is_left_where_it_was() {
    for (answer, drawn_text) in [
        ("call `_private` first", "call _private first"),
        ("the `*ptr` it points at", "the *ptr it points at"),
        ("`**kwargs` and the rest", "**kwargs and the rest"),
        ("a `~~draft~~` name", "a ~~draft~~ name"),
    ] {
        assert_eq!(drawn(&whole(answer)), drawn_text, "{answer:?}");
    }
}

#[test]
fn a_code_span_is_still_toned_down_for_its_length() {
    let said = whole("call `*ptr` first");

    assert_eq!(slots(&said), [Slot::Plain, Slot::Quiet, Slot::Plain]);
}

#[test]
fn emphasis_around_a_code_span_still_closes_after_it() {
    let said = whole("*the `*ptr` it points at*, then");

    assert_eq!(drawn(&said), "the *ptr it points at, then");
    assert_eq!(
        slots(&said),
        [Slot::Emphasis, Slot::Quiet, Slot::Emphasis, Slot::Plain]
    );
}

/// One whole answer, read as one delta and then ended.
fn ended(answer: &str) -> Vec<(Slot, String)> {
    let mut markdown = Markdown::default();
    let mut said = read(&mut markdown, answer);
    markdown.finish(ROOM, &mut |slot, text| said.push((slot, text.to_owned())));
    said
}

#[test]
fn a_marker_the_answer_escaped_is_drawn_as_itself() {
    for (answer, drawn_text) in [
        ("a \\*literal\\* star", "a *literal* star"),
        (
            "two \\_\\_underscores\\_\\_ here",
            "two __underscores__ here",
        ),
        ("a \\| pipe", "a | pipe"),
        ("a \\\\ backslash", "a \\ backslash"),
        ("\\- not an item", "- not an item"),
    ] {
        assert_eq!(drawn(&ended(answer)), drawn_text, "{answer:?}");
    }
}

#[test]
fn an_escaped_marker_marks_nothing() {
    let said = ended("a \\*literal\\* star");

    assert_eq!(slots(&said), [Slot::Plain]);
}

#[test]
fn a_backslash_before_anything_else_keeps_it() {
    for answer in [
        "C:\\Users\\name",
        "the \\d+ in a pattern",
        "a message that ended on one \\",
    ] {
        assert_eq!(drawn(&ended(answer)), answer, "{answer:?}");
    }
}

#[test]
fn a_backslash_inside_a_code_span_is_left_where_it_was() {
    assert_eq!(drawn(&ended("the `\\d+` in it")), "the \\d+ in it");
}

#[test]
fn a_bare_address_arrives_exactly_as_it_was_written() {
    for answer in [
        "see https://example.com/a/_private for more",
        "see https://docs.rs/crate/*/latest here",
        "at http://localhost:8080/a~b end",
        "see https://example.com",
    ] {
        assert_eq!(drawn(&ended(answer)), answer, "{answer:?}");
    }
}

#[test]
fn a_bare_address_is_drawn_as_the_link_it_is() {
    let said = ended("see https://example.com/docs for more");

    assert_eq!(slots(&said), [Slot::Plain, Slot::Link, Slot::Plain]);
}

#[test]
fn a_stop_after_an_address_is_not_part_of_it() {
    let said = ended("see https://example.com/docs.");

    assert_eq!(drawn(&said), "see https://example.com/docs.");
    assert_eq!(slots(&said), [Slot::Plain, Slot::Link, Slot::Plain]);
}

#[test]
fn an_address_split_across_two_deltas_is_still_one_address() {
    let mut markdown = Markdown::default();
    let mut said = read(&mut markdown, "see https://exa");
    said.extend(read(&mut markdown, "mple.com/docs here"));

    assert_eq!(drawn(&said), "see https://example.com/docs here");
    assert_eq!(slots(&said), [Slot::Plain, Slot::Link, Slot::Plain]);
}

#[test]
fn a_word_that_only_starts_like_an_address_is_left_where_it_was() {
    for answer in [
        "here is the answer",
        "the http/2 protocol",
        "https not a link",
        "hypertext, https, and http",
    ] {
        assert_eq!(drawn(&ended(answer)), answer, "{answer:?}");
        assert_eq!(slots(&ended(answer)), [Slot::Plain], "{answer:?}");
    }
}

#[test]
fn an_address_inside_a_code_span_is_code_like_the_rest_of_it() {
    let said = ended("run `curl https://example.com/a_b`");

    assert_eq!(drawn(&said), "run curl https://example.com/a_b");
    assert_eq!(slots(&said), [Slot::Plain, Slot::Quiet]);
}

#[test]
fn an_address_a_link_names_is_still_the_link_it_names() {
    let said = ended("[the docs](https://example.com)");

    assert_eq!(drawn(&said), "the docs (https://example.com)");
    assert_eq!(slots(&said), [Slot::Link, Slot::Quiet]);
}

/// Every shape this file reads, as one answer each.
const SHAPES: [&str; 12] = [
    "**strong** and *leaning* words",
    "a [link](https://example.com) here",
    "| a | b |\n|---|---|\n| 1 | 2 |\n",
    "- [x] done\n- [ ] not\n",
    "```rust\nfn main() {}\n```\n",
    "see https://example.com/a_b for more",
    "a \\*literal\\* star",
    "---\n\ntext\n",
    "> quoted\n",
    "a `*ptr*` span",
    "~~struck~~ words",
    "# heading\n",
];

#[test]
fn where_a_delta_ends_changes_nothing_about_what_is_drawn() {
    for answer in SHAPES {
        let whole = ended(answer);

        let mut markdown = Markdown::default();
        let mut said = Vec::new();
        for character in answer.chars() {
            let mut delta = [0; 4];
            said.extend(read(&mut markdown, character.encode_utf8(&mut delta)));
        }
        markdown.finish(ROOM, &mut |slot, text| said.push((slot, text.to_owned())));

        assert_eq!(drawn(&said), drawn(&whole), "{answer:?}");
        assert_eq!(slots(&said), slots(&whole), "{answer:?}");
    }
}

#[test]
fn emphasis_that_opened_on_one_line_closes_on_the_next() {
    let said = ended("**a phrase over\ntwo lines** and prose\n");

    assert_eq!(drawn(&said), "a phrase over\ntwo lines and prose\n");
    assert_eq!(
        slots(&said),
        vec![Slot::Strong, Slot::Plain, Slot::Strong, Slot::Plain]
    );
}

#[test]
fn emphasis_nobody_closed_ends_with_the_paragraph() {
    let said = ended("**opened and left open\n\na new paragraph\n");

    assert_eq!(drawn(&said), "opened and left open\n\na new paragraph\n");
    assert_eq!(slots(&said), vec![Slot::Strong, Slot::Plain]);
}

#[test]
fn emphasis_nobody_closed_ends_where_the_next_block_starts() {
    let said = ended("- **opened and left open\n- a second item\n");

    assert_eq!(drawn(&said), "· opened and left open\n· a second item\n");
    assert_eq!(
        slots(&said),
        vec![
            Slot::Quiet,
            Slot::Strong,
            Slot::Plain,
            Slot::Quiet,
            Slot::Plain
        ]
    );
}
