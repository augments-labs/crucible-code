use super::*;
use crate::color::{Design, Palette, Theme};
use crate::width;

/// A window wide enough that nothing here is laid out against its edge.
const ROOM: usize = 80;

/// What a run that takes the open line back is written as, where a test reads
/// the runs as text.
const RETRACTED: &str = "<retracted>";

/// The reader as runs alone: a block it hands on is laid out at the room it
/// was read in, and its rows said in its place, each ending its line.
///
/// What the record is handed and lays itself, said the way the runs around
/// it are, so a test about the runs reads a block as the rows it draws.
trait Runs {
    fn read_runs(
        &mut self,
        delta: &str,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    );
    fn finish_runs(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>));
}

impl Runs for Markdown {
    fn read_runs(
        &mut self,
        delta: &str,
        room: usize,
        say: &mut dyn FnMut(Slot, &str, Option<&str>),
    ) {
        let glyphs = self.glyphs;
        self.read(delta, room, &mut |said| runs(said, room, glyphs, say));
    }

    fn finish_runs(&mut self, room: usize, say: &mut dyn FnMut(Slot, &str, Option<&str>)) {
        let glyphs = self.glyphs;
        self.finish(room, &mut |said| runs(said, room, glyphs, say));
    }
}

/// One thing the reader said, as runs.
fn runs(
    said: Said<'_>,
    room: usize,
    glyphs: Glyphs,
    say: &mut dyn FnMut(Slot, &str, Option<&str>),
) {
    match said {
        Said::Run { slot, text, link } => say(slot, text, link),
        Said::Block(block) => {
            for row in lay(&block, room, glyphs, &Links::default()) {
                for (slot, text) in row.spans() {
                    say(slot, text, None);
                }
                say(Slot::Plain, "\n", None);
            }
        }
        Said::Retract => say(Slot::Plain, RETRACTED, None),
    }
}

/// Reads one delta and returns every run with the slot it was said under.
fn read(markdown: &mut Markdown, delta: &str) -> Vec<(Slot, String)> {
    let mut said = Vec::new();
    markdown.read_runs(delta, ROOM, &mut |slot, text, _| {
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

/// What is left of `said` once every taken-back line is gone: the runs since
/// the last line break, which is where the line being written began.
fn standing(said: Vec<(Slot, String)>) -> Vec<(Slot, String)> {
    let mut left: Vec<(Slot, String)> = Vec::new();
    for (slot, text) in said {
        if text != RETRACTED {
            left.push((slot, text));
            continue;
        }
        while let Some((slot, text)) = left.pop() {
            if let Some(at) = text.rfind('\n') {
                left.push((slot, text.get(..=at).unwrap_or_default().to_owned()));
                break;
            }
        }
    }
    left
}

/// The same for runs already put together as text.
fn standing_text(text: &str) -> String {
    let mut left = String::new();
    for part in text
        .split(RETRACTED)
        .enumerate()
        .map(|(at, part)| (at > 0, part))
    {
        if part.0 {
            left.truncate(left.rfind('\n').map_or(0, |at| at + 1));
        }
        left.push_str(part.1);
    }
    left
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
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Bold, Slot::Plain]);
}

#[test]
fn one_marker_is_emphasis_and_two_are_weight() {
    assert_eq!(
        slots(&whole("a *leant on* word")),
        vec![Slot::Plain, Slot::Emphasis, Slot::Plain]
    );
    assert_eq!(
        slots(&whole("a **loud** word")),
        vec![Slot::Plain, Slot::Bold, Slot::Plain]
    );
    assert_eq!(
        slots(&whole("a ***both*** word")),
        vec![Slot::Plain, Slot::Bold, Slot::Plain],
        "three markers are both, and weight is the louder"
    );
}

#[test]
fn the_two_are_told_apart_inside_one_line() {
    let said = whole("*this* and **that**");

    assert_eq!(drawn(&said), "this and that");
    assert_eq!(slots(&said), vec![Slot::Emphasis, Slot::Plain, Slot::Bold]);
}

#[test]
fn a_heading_loses_its_hashes_and_keeps_its_words() {
    let said = whole("### What it costs\nthe paragraph under it");

    assert_eq!(drawn(&said), "What it costs\nthe paragraph under it");
    assert_eq!(slots(&said), vec![Slot::Heading, Slot::Plain]);
}

#[test]
fn a_hash_that_no_space_follows_is_a_hash() {
    assert_eq!(drawn(&whole("#3 in the list")), "#3 in the list");
}

#[test]
fn a_hash_partway_along_a_line_is_a_hash() {
    // Ended, since a number at the end of a delta is held until its word is.
    assert_eq!(drawn(&ended("issue # 12")), "issue # 12");
}

#[test]
fn inline_code_wears_its_own_slot_and_loses_its_backticks() {
    // A command, a path, a name — what backticks hold is what the reader is
    // being handed, and the grey a hint wears says it can be skipped.
    let said = whole("call `read` for it");

    assert_eq!(drawn(&said), "call read for it");
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Code, Slot::Plain]);
}

#[test]
fn a_fence_and_the_language_written_on_it_are_the_edges_of_its_frame() {
    let said = ended("before\n```rust\nlet it = 1;\n```\nafter\n");

    // The fence and the word after it are not drawn as they were written: the
    // fence's two lines become the two edges of a frame, and the word after the
    // first names it. What is between them is the block.
    assert_eq!(drawn(&said), "before\n┌─ rust\n│ let it = 1;\n└─\nafter\n");

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

    assert_eq!(
        drawn(&said),
        "┌─\n│ # not a heading **not bold** _not_\n└─\n"
    );
    assert_eq!(slots(&said), vec![Slot::Quiet]);
}

#[test]
fn one_backtick_inside_a_fence_is_a_backtick() {
    let said = whole("```\n`quoted` in a shell\n```\n");

    assert_eq!(drawn(&said), "┌─\n│ `quoted` in a shell\n└─\n");
    assert_eq!(slots(&said), vec![Slot::Quiet]);
}

#[test]
fn a_bullet_stays_a_bullet_rather_than_opening_emphasis() {
    // The star that opens a list and the star that opens a phrase are the same
    // character; the space after it is the whole of the difference.
    let said = whole("* first\n* second\n");

    assert_eq!(drawn(&said), "• first\n• second\n");
    assert_eq!(
        slots(&said),
        vec![
            Slot::Bullet,
            Slot::Quiet,
            Slot::Plain,
            Slot::Bullet,
            Slot::Quiet,
            Slot::Plain
        ]
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
    assert_eq!(slots(&said), vec![Slot::Plain, Slot::Bold, Slot::Plain]);
}

#[test]
fn a_marker_that_never_closes_costs_its_own_paragraph_and_no_more() {
    let said = ended("**never closed\nthe line after it\n\na new paragraph\n");

    assert_eq!(
        drawn(&said),
        "never closed\nthe line after it\n\na new paragraph\n"
    );
    assert_eq!(
        slots(&said),
        vec![Slot::Bold, Slot::Plain, Slot::Bold, Slot::Plain]
    );
}

#[test]
fn a_run_that_meant_nothing_is_put_back_where_it_was() {
    // Ended, since a number at the end of a delta is held until its word is.
    let said = ended("2 * 3 * 4");

    // Nothing opened emphasis: the figures are figures and the stars prose.
    assert_eq!(drawn(&said), "2 * 3 * 4");
    assert!(
        said.iter()
            .all(|(slot, _)| matches!(slot, Slot::Plain | Slot::Figure)),
        "{said:?}"
    );
}

#[test]
fn a_marker_split_across_two_deltas_is_still_one_marker() {
    let mut markdown = Markdown::default();

    let first = read(&mut markdown, "a *");
    let second = read(&mut markdown, "*loud** word");

    assert_eq!(drawn(&first), "a ");
    assert_eq!(drawn(&second), "loud word");
    assert_eq!(slots(&second), vec![Slot::Bold, Slot::Plain]);
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
    // The frame is up as soon as the fence's line is, and the gutter with it.
    assert_eq!(drawn(&second), "┌─ rust\n│ ");
    assert_eq!(drawn(&third), "let it = 1;\n└─\n");

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
    // Every byte of the code comes back, once, in order, each line behind the
    // gutter of its frame.
    let code = "fn main() {\n    let x = 1; // hi\n    println!(\"{x}\");\n}\n";
    let said = whole(&format!("```rust\n{code}```\n"));
    let text: String = said.iter().map(|(_, text)| text.as_str()).collect();
    let inside = text
        .lines()
        .skip(1)
        .filter_map(|line| line.strip_prefix("│ "))
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";

    assert_eq!(inside, code);
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
        markdown.read_runs(
            character.encode_utf8(&mut piece),
            ROOM,
            &mut |slot, text, _| {
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

/// One whole answer with no repository behind it, with where each run points.
fn pointing(answer: &str) -> Vec<(Slot, String, Option<String>)> {
    let mut markdown = Markdown::default();
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    markdown.read_runs(answer, ROOM, &mut into);
    markdown.finish_runs(ROOM, &mut into);
    said
}

#[test]
fn a_link_is_its_words_carrying_the_address_rather_than_showing_it() {
    // The sentence reads as the sentence the answer wrote. The address is not
    // lost for that: it rides on the words, for a terminal that opens links.
    let said = pointing("see [the guide](https://example.com/guide) for more");

    assert_eq!(wrote(&said), "see the guide for more");
    assert_eq!(
        said.iter().map(|(slot, ..)| *slot).collect::<Vec<_>>(),
        [Slot::Plain, Slot::Link, Slot::Plain]
    );
    assert_eq!(points(&said), [("the guide", "https://example.com/guide")]);
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
    let said = pointing("[a](<https://example.com> \"The title\")");

    assert_eq!(wrote(&said), "a");
    assert_eq!(points(&said), [("a", "https://example.com")]);
}

#[test]
fn a_bracket_that_was_never_a_link_is_left_exactly_as_it_was() {
    for written in [
        "[TODO] fix this",
        "an array is arr[0] and no more",
        "[unclosed and then the line ends\n",
        "[half](and then the line ends\n",
    ] {
        assert_eq!(drawn(&ended(written)), written, "{written:?}");
    }
}

#[test]
fn a_bracket_inside_a_span_of_code_is_an_index() {
    let said = whole("`arr[0](x)` and arr[1](y)");

    assert_eq!(drawn(&said), "arr[0](x) and arr1");
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
        markdown.finish_runs(ROOM, &mut |slot, text, _| {
            said.push((slot, text.to_owned()));
        });

        assert_eq!(drawn(&said), "see the guide now", "split at {at}");
    }
}

#[test]
fn a_message_that_ended_inside_a_link_keeps_what_it_wrote() {
    let mut markdown = Markdown::default();
    let mut said = read(&mut markdown, "see [the guide");
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });

    assert_eq!(drawn(&said), "see [the guide");
}

#[test]
fn a_link_in_a_fenced_block_is_code() {
    let said = whole("```\n[a](b)\n```\n");

    assert_eq!(drawn(&said), "┌─\n│ [a](b)\n└─\n");
}

#[test]
fn every_spelling_of_a_bullet_is_drawn_as_one_mark() {
    // A model writes all three and means the same thing by each. A reader
    // meets one list, so one mark.
    let said = whole("- dash\n* star\n+ plus\n");

    assert_eq!(drawn(&said), "• dash\n• star\n• plus\n");
}

#[test]
fn a_bullet_keeps_the_indentation_that_nests_it() {
    // The spaces before the marker are the whole of what says a list is inside
    // another one, and they arrive before anything is decided about the line.
    let said = whole("- one\n  - under it\n    - under that\n");

    assert_eq!(drawn(&said), "• one\n  • under it\n    • under that\n");
}

#[test]
fn the_mark_is_quiet_and_the_item_is_not() {
    // The words are what somebody is reading; the mark is what tells them
    // where one item stops.
    let said = whole("- an item\n");

    assert_eq!(slots(&said), vec![Slot::Bullet, Slot::Quiet, Slot::Plain]);
}

#[test]
fn a_dash_that_is_not_a_bullet_is_left_exactly_where_it_was() {
    // Only at the start of a line, and only with a space after it. Everything
    // else is arithmetic, a flag, or a word somebody hyphenated.
    // Ended, since a number at the end of a delta is held until its word is.
    for prose in ["5 - 3 = 2", "pass --colour never", "-–—", "-no space"] {
        assert_eq!(drawn(&ended(prose)), prose, "{prose:?}");
    }
}

#[test]
fn a_star_at_the_start_of_a_line_is_still_emphasis_where_it_opens_one() {
    // The two are told apart by the space: `* item` is a list and `*word*` is
    // a phrase, wherever on the line either of them starts.
    let said = ended("*loud* at the start\n");

    assert_eq!(drawn(&said), "loud at the start\n");
    assert_eq!(slots(&said), vec![Slot::Emphasis, Slot::Plain]);
}

#[test]
fn a_quote_is_a_bar_and_the_words_beside_it() {
    let said = whole("> somebody else said this\n");

    assert_eq!(drawn(&said), "│ somebody else said this\n");
    // The break itself is written after the line's state is dropped, so that no
    // row carries a slot into the one below it.
    assert_eq!(slots(&said), vec![Slot::Quiet, Slot::Quote, Slot::Plain]);
}

#[test]
fn a_quote_ends_where_the_line_does() {
    // Like every other thing this reader opens: a model that quoted one line
    // has not put the rest of the answer in somebody else's mouth.
    let said = ended("> quoted\nplain again\n");

    assert_eq!(drawn(&said), "│ quoted\nplain again\n");
    assert_eq!(slots(&said), vec![Slot::Quiet, Slot::Quote, Slot::Plain]);
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
    assert_eq!(drawn(&rest), "• an item");
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
    // A home directory and an approximation. Neither is a retraction and both
    // are text: one tilde is a character somebody wrote, and it takes exactly
    // two to put a phrase under one.
    assert_eq!(drawn(&whole("look in ~/Projects")), "look in ~/Projects");
    assert_eq!(drawn(&whole("about ~40 lines")), "about ~40 lines");
}

#[test]
fn a_pair_left_dangling_at_the_end_of_a_line_strikes_nothing() {
    assert_eq!(
        drawn(&ended("nothing to strike ~~\n")),
        "nothing to strike ~~\n"
    );
    assert_eq!(slots(&ended("nothing to strike ~~\n")), vec![Slot::Plain]);
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
    markdown.read_runs(answer, room, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });
    markdown.finish_runs(room, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });
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
        markdown.read_runs(piece, ROOM, &mut |slot, text, _| {
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

    // The cell the column no longer holds is wrapped inside it rather than cut:
    // what a column gives up is width, never words.
    assert_eq!(
        drawn(&said),
        "a │ a very long \n\
         \u{20} │ cell indeed \n\
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
fn a_wrapped_header_cell_is_the_header_on_every_line_of_it() {
    let said = narrowed("| a | one two |\n| --- | --- |\n| b | c |\n\n", 9);
    let header: Vec<&str> = said
        .iter()
        .filter(|(slot, text)| *slot == Slot::Bold && !text.trim().is_empty())
        .map(|(_, text)| text.as_str())
        .collect();

    assert_eq!(header, ["a", "one", "two"]);
    assert_eq!(
        drawn(&said),
        "a │ one  \n\
         \u{20} │ two  \n\
         ──┼──────\n\
         b │ c    \n\n"
    );
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

    markdown.read_runs(
        "| file |\n| --- |\n| main.rs |",
        ROOM,
        &mut |slot, text, _| {
            said.push((slot, text.to_owned()));
        },
    );
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });

    assert_eq!(drawn(&said), "file   \n───────\nmain.rs\n");
}

#[test]
fn nothing_inside_a_fence_is_read_as_a_table() {
    // The fence's own lines go the way every fence's do; the bars between them
    // are code and stay exactly as they were written.
    let said = whole("```\n| a | b |\n| --- | --- |\n```\n");

    assert_eq!(drawn(&said), "┌─\n│ | a | b |\n│ | --- | --- |\n└─\n");
}

#[test]
fn the_header_is_raised_and_the_bars_are_quiet() {
    // Reading across: the header's two cells raised with the bar between them
    // quiet, the break, the rule, the break, then a body row whose cells are
    // the prose they were written as with the bar and the padding quiet, and
    // the figure in the second of them told as one.
    let said = whole("| file | lines |\n| --- | --- |\n| main.rs | 42 |\n\n");

    assert_eq!(
        slots(&said),
        vec![
            Slot::Bold,
            Slot::Quiet,
            Slot::Bold,
            Slot::Plain,
            Slot::Quiet,
            Slot::Plain,
            Slot::Quiet,
            Slot::Figure,
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

    assert_eq!(drawn(&said), "• see [TODO] in the grammar\n");
}

#[test]
fn a_box_that_does_not_open_an_item_is_the_brackets_somebody_wrote() {
    let said = ended("the set is [ ] until something is in it\n");

    assert_eq!(drawn(&said), "the set is [ ] until something is in it\n");
}

#[test]
fn a_link_that_opens_an_item_still_gets_its_bullet() {
    let said = whole("- [the page](https://example.invalid)\n");

    assert_eq!(drawn(&said), "• the page\n");
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

    assert_eq!(drawn(&said), "┌─\n│ ---\n└─\n");
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

        // Trimmed on both sides, because a rule is a block, and a block ends
        // its own line whether or not a break followed it.
        assert_eq!(
            drawn(&ended).trim_end_matches('\n'),
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

    assert_eq!(slots(&said), [Slot::Plain, Slot::Code, Slot::Plain]);
}

#[test]
fn emphasis_around_a_code_span_still_closes_after_it() {
    let said = whole("*the `*ptr` it points at*, then");

    assert_eq!(drawn(&said), "the *ptr it points at, then");
    assert_eq!(
        slots(&said),
        [Slot::Emphasis, Slot::Code, Slot::Emphasis, Slot::Plain]
    );
}

/// One whole answer, read as one delta and then ended.
fn ended(answer: &str) -> Vec<(Slot, String)> {
    ended_in(&[answer])
}

/// An answer read as these deltas, one after another, and then ended.
fn ended_in(deltas: &[&str]) -> Vec<(Slot, String)> {
    let mut markdown = Markdown::default();
    let mut said = Vec::new();
    for delta in deltas {
        said.extend(read(&mut markdown, delta));
    }
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });
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
    assert_eq!(slots(&said), [Slot::Plain, Slot::Code]);
}

#[test]
fn an_address_a_link_names_is_still_the_link_it_names() {
    let said = pointing("[the docs](https://example.com)");

    assert_eq!(wrote(&said), "the docs");
    assert_eq!(points(&said), [("the docs", "https://example.com")]);
}

/// Every shape this file reads, as one answer each.
const SHAPES: [&str; 19] = [
    "**strong** and *leaning* words",
    "a [link](https://example.com) here",
    "| a | b |\n|---|---|\n| 1 | 2 |\n",
    "a | b\n--|--\n1 | 2\n\nafter\n",
    "a | b\n--|---|--\n1 | 2\n",
    "a | b\n---\n1 | 2\n",
    "- [x] done\n- [ ] not\n",
    "```rust\nfn main() {}\n```\n",
    "see https://example.com/a_b for more",
    "a \\*literal\\* star",
    "---\n\ntext\n",
    "> quoted\n",
    "a `*ptr*` span",
    "~~struck~~ words",
    "# heading\n",
    "para\n\n    let x = 1;\n\n    y\n---\n",
    "> [!WARNING]\n> a *loud* body\n> and more\n\nafter\n",
    "> [!NOTE] not an alert\n> [!TIP]\n",
    "> [!CAUTION]",
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
        markdown.finish_runs(ROOM, &mut |slot, text, _| {
            said.push((slot, text.to_owned()));
        });

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
        vec![Slot::Bold, Slot::Plain, Slot::Bold, Slot::Plain]
    );
}

#[test]
fn emphasis_nobody_closed_ends_with_the_paragraph() {
    let said = ended("**opened and left open\n\na new paragraph\n");

    assert_eq!(drawn(&said), "opened and left open\n\na new paragraph\n");
    assert_eq!(slots(&said), vec![Slot::Bold, Slot::Plain]);
}

#[test]
fn emphasis_nobody_closed_ends_where_the_next_block_starts() {
    let said = ended("- **opened and left open\n- a second item\n");

    assert_eq!(drawn(&said), "• opened and left open\n• a second item\n");
    assert_eq!(
        slots(&said),
        vec![
            Slot::Bullet,
            Slot::Quiet,
            Slot::Bold,
            Slot::Plain,
            Slot::Bullet,
            Slot::Quiet,
            Slot::Plain
        ]
    );
}

#[test]
fn the_spaces_in_front_of_a_fence_go_with_it() {
    let said = ended("- item\n  ```rs\n  let a = 1;\n  ```\n- next\n");

    assert_eq!(drawn(&said), "• item\n┌─ rs\n│   let a = 1;\n└─\n• next\n");
}

#[test]
fn the_spaces_in_front_of_a_nested_item_stay_where_they_were() {
    let said = ended("- item\n  - nested\n");

    assert_eq!(drawn(&said), "• item\n  • nested\n");
}

#[test]
fn a_block_fenced_with_tildes_is_a_block() {
    let said = ended("~~~\nlet a = **1**;\n~~~\n");

    assert_eq!(drawn(&said), "┌─\n│ let a = **1**;\n└─\n");
    assert_eq!(slots(&said), vec![Slot::Quiet]);
}

#[test]
fn a_block_is_closed_by_the_marker_that_opened_it() {
    let said = ended("~~~\n```\nstill code\n~~~\n");

    assert_eq!(drawn(&said), "┌─\n│ ```\n│ still code\n└─\n");
}

/// The repository the references below are counted against.
fn forge() -> Forge {
    Forge::new(
        "https://github.com",
        "augments-labs/crucible-code",
        "/issues/",
    )
}

/// One whole answer, read against a repository, with where each run points.
fn counted(answer: &str) -> Vec<(Slot, String, Option<String>)> {
    let mut markdown = Markdown::default().counting(Some(forge()));
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    markdown.read_runs(answer, ROOM, &mut into);
    markdown.finish_runs(ROOM, &mut into);
    said
}

/// What `answer` hands on, read with links to paths opened in `/repo`.
fn opened(answer: &str) -> Vec<(Slot, String, Option<String>)> {
    let files = Files::new(std::path::Path::new("/repo"), crate::files::Line::Fragment);
    let mut markdown = Markdown::default().opening(Some(files));
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    markdown.read_runs(answer, ROOM, &mut into);
    markdown.finish_runs(ROOM, &mut into);
    said
}

#[test]
fn a_link_to_a_path_points_at_the_file_in_the_checkout() {
    // A terminal opens an address, and `src/a.rs:3` is not one until it is
    // read against the directory the session is in. The words are what the
    // answer wrote; only where they point changes.
    let said =
        opened("see [a](src/a.rs:3), [g](docs/My%20Guide.md) and [docs](https://example.test/d)");

    assert_eq!(wrote(&said), "see a, g and docs");
    assert_eq!(
        points(&said),
        [
            ("a", "file:///repo/src/a.rs#3"),
            ("g", "file:///repo/docs/My%20Guide.md"),
            ("docs", "https://example.test/d")
        ]
    );
}

/// Everything one of those answers drew, with the markers gone.
fn wrote(said: &[(Slot, String, Option<String>)]) -> String {
    said.iter().map(|(_, text, _)| text.as_str()).collect()
}

/// The runs of one of those answers that point somewhere, and where to.
fn points(said: &[(Slot, String, Option<String>)]) -> Vec<(&str, &str)> {
    said.iter()
        .filter_map(|(_, text, link)| Some((text.as_str(), link.as_deref()?)))
        .collect()
}

#[test]
fn a_number_the_answer_wrote_points_at_the_repository_in_hand() {
    let said = counted("landed in #487 yesterday");

    assert_eq!(wrote(&said), "landed in #487 yesterday");
    assert_eq!(
        points(&said),
        [(
            "#487",
            "https://github.com/augments-labs/crucible-code/issues/487"
        )]
    );
}

#[test]
fn a_number_that_named_a_repository_points_at_that_one() {
    let said = counted("see someone/else#12 for why");

    assert_eq!(wrote(&said), "see someone/else#12 for why");
    assert_eq!(
        points(&said),
        [(
            "someone/else#12",
            "https://github.com/someone/else/issues/12"
        )]
    );
}

#[test]
fn the_word_saying_what_a_number_counts_is_part_of_the_reference() {
    // `PR #435` is one thing to the reader, and the words they click are the
    // words the answer wrote it with. Whatever the case, and however the word
    // is spelled out.
    for (answer, words) in [
        ("landed in PR #435 yesterday", "PR #435"),
        ("landed in pr #435 yesterday", "pr #435"),
        ("see issue #12 for why", "issue #12"),
        ("see Issue #12 for why", "Issue #12"),
        ("the pull request #7 that did it", "pull request #7"),
    ] {
        let said = counted(answer);

        assert_eq!(wrote(&said), answer, "{answer:?}");
        assert_eq!(
            points(&said)
                .iter()
                .map(|(words, _)| *words)
                .collect::<Vec<_>>(),
            [words],
            "{answer:?}"
        );
    }
}

#[test]
fn a_word_that_only_ends_in_a_lead_is_not_one() {
    // `prepr #12` ends in `pr`, and is not about a pull request.
    let said = counted("run prepr #12 first");

    assert_eq!(wrote(&said), "run prepr #12 first");
    assert_eq!(
        points(&said),
        [(
            "#12",
            "https://github.com/augments-labs/crucible-code/issues/12"
        )]
    );
}

#[test]
fn a_lead_with_no_repository_behind_the_session_is_left_where_it_was() {
    let said = ended("landed in PR #435 yesterday");

    assert_eq!(drawn(&said), "landed in PR #435 yesterday");
    assert_eq!(slots(&said), [Slot::Plain]);
}

#[test]
fn a_number_wears_the_slot_a_link_wears() {
    let said = counted("landed in #487 yesterday");

    let slots: Vec<_> = said.iter().map(|(slot, _, _)| *slot).collect();
    assert_eq!(slots, [Slot::Plain, Slot::Link, Slot::Plain]);
}

#[test]
fn a_number_with_no_repository_behind_the_session_is_left_where_it_was() {
    // The whole reason the forge is optional: a checkout with no remote has
    // nowhere for this to point, and somewhere wrong is worse than nowhere.
    let said = ended("landed in #487 yesterday");

    assert_eq!(drawn(&said), "landed in #487 yesterday");
    assert_eq!(slots(&said), [Slot::Plain]);
}

#[test]
fn a_hash_in_front_of_something_that_is_not_a_number_is_left_where_it_was() {
    for answer in [
        "the #include at the top",
        "a colour like #fff there",
        "the ## marker itself",
        "trailing hash here #",
        "an id like abc#def in it",
    ] {
        let said = counted(answer);
        assert_eq!(wrote(&said), answer, "{answer:?}");
        assert!(points(&said).is_empty(), "{answer:?}");
    }
}

#[test]
fn a_number_inside_code_is_code_like_the_rest_of_it() {
    let said = counted("run `git show #487` first");

    assert_eq!(wrote(&said), "run git show #487 first");
    assert!(points(&said).is_empty());
}

#[test]
fn a_number_split_across_two_deltas_is_still_one_number() {
    let mut markdown = Markdown::default().counting(Some(forge()));
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    markdown.read_runs("landed in #4", ROOM, &mut into);
    markdown.read_runs("87 yesterday", ROOM, &mut into);
    markdown.finish_runs(ROOM, &mut into);

    assert_eq!(wrote(&said), "landed in #487 yesterday");
    assert_eq!(
        points(&said),
        [(
            "#487",
            "https://github.com/augments-labs/crucible-code/issues/487"
        )]
    );
}

#[test]
fn a_number_that_ends_the_message_is_still_a_number() {
    // The character that would have ended it never arrives, exactly as it
    // never does for an address.
    let said = counted("landed in #487");

    assert_eq!(wrote(&said), "landed in #487");
    assert_eq!(points(&said).len(), 1);
}

#[test]
fn what_ends_a_sentence_after_a_number_is_not_part_of_it() {
    let said = counted("landed in #487.");

    assert_eq!(wrote(&said), "landed in #487.");
    assert_eq!(
        points(&said),
        [(
            "#487",
            "https://github.com/augments-labs/crucible-code/issues/487"
        )]
    );
}

#[test]
fn a_heading_is_still_a_heading_and_not_a_number() {
    let said = counted("# 487 of them\n");

    // The words, and the rule a heading of the first level is drawn with.
    assert_eq!(wrote(&said), "487 of them\n━━━━━━━━━━━\n");
    assert!(points(&said).is_empty());
}

#[test]
fn a_calm_reply_spends_the_theme_colour_on_code_alone() {
    // Headings, raised phrases and a table's header are weight in the reader's
    // own foreground; the theme's colour is kept for what the reader copies,
    // and a link is the second colour.
    let said = whole(
        "# Fixed\n\nSee **the note**, `wait_for_index` and \
         [the issue](https://example.com/1).\n\n\
         | file | lines |\n| --- | --- |\n| main.rs | 42 |\n\n",
    );
    let slot_of = |wanted: &str| {
        said.iter()
            .find(|(_, text)| text.trim() == wanted)
            .map(|(slot, _)| *slot)
    };

    assert_eq!(slot_of("Fixed"), Some(Slot::Heading), "a heading: {said:?}");
    assert_eq!(slot_of("the note"), Some(Slot::Bold), "bold: {said:?}");
    assert_eq!(slot_of("file"), Some(Slot::Bold), "a header cell: {said:?}");
    assert_eq!(
        slot_of("lines"),
        Some(Slot::Bold),
        "a header cell: {said:?}"
    );
    assert_eq!(slot_of("wait_for_index"), Some(Slot::Code), "{said:?}");
    assert_eq!(slot_of("the issue"), Some(Slot::Link), "{said:?}");
    assert!(
        said.iter().all(|(slot, _)| *slot != Slot::Strong),
        "the accent, bold, in a reply: {said:?}"
    );

    // And code still wears the accent's ink, a heading weight alone, and a
    // link a line under an ink that is not the accent's.
    for theme in [Theme::Dark, Theme::Light, Theme::ColourblindDark] {
        let palette = Palette::resolve(true, theme, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        });
        let accent = palette.open(Slot::Accent);
        let accent = accent.as_str().trim_start_matches("\x1b[");
        let link = palette.open(Slot::Link);

        assert_eq!(palette.open(Slot::Code).as_str(), format!("\x1b[{accent}"));
        assert_eq!(palette.open(Slot::Heading).as_str(), "\x1b[1m");
        assert!(link.as_str().starts_with("\x1b[4;38;2;"), "{theme:?}");
        assert_ne!(link.as_str(), format!("\x1b[4;{accent}"), "{theme:?}");
    }
}

#[test]
fn model_prose_is_plain_and_follows_the_colour_rule() {
    // What a model says is read, not landed on: its prose is plain, and what
    // it marks up takes the slots that mean something without the accent.
    let said = whole(
        "# Fixed\n\nThe test waits on a file the picker writes after it returns.\n\
         I moved the wait into `wait_for_index`, see **the note** and \
         [the issue](https://example.com/1).\n\n- one\n- two\n",
    );
    let prose = said
        .iter()
        .find(|(_, text)| text.contains("The test waits"))
        .expect("the prose");
    assert_eq!(prose.0, Slot::Plain, "{said:?}");

    let mut rows = vec![crate::row::Row::new()];
    for (slot, text) in &said {
        for (at, line) in text.split('\n').enumerate() {
            if at > 0 {
                rows.push(crate::row::Row::new());
            }
            if let Some(row) = rows.last_mut() {
                row.push(*slot, line);
            }
        }
    }

    crate::colour_rule::holds("model text", &rows, |_| false);
}

/// One whole answer, read and finished, with the slot each run was said under.
fn finished(answer: &str) -> Vec<(Slot, String)> {
    let mut markdown = Markdown::default();
    let mut said = Vec::new();
    let mut into = |slot, text: &str, _: Option<&str>| said.push((slot, text.to_owned()));
    markdown.read_runs(answer, ROOM, &mut into);
    markdown.finish_runs(ROOM, &mut into);
    said
}

/// The slot the run spelling exactly `text` was said under, where one was.
fn kind_of(said: &[(Slot, String)], text: &str) -> Option<Slot> {
    said.iter()
        .find(|(_, run)| run == text)
        .map(|(slot, _)| *slot)
}

/// An answer naming one of each kind a design can colour.
const KINDS: &str = "See [the pull request](https://example.test/1) and \
                     https://example.test/2, then `src/cli/draw.rs`: it ships \
                     in 0.45.0 as 3436980f, and 641 passed.\n";

#[test]
fn what_an_answer_names_takes_the_slot_for_its_kind() {
    // Which kind a run is was decided here, once, and only picks its slot:
    // the characters are exactly the ones the answer wrote.
    let said = finished(KINDS);

    assert_eq!(
        kind_of(&said, "the pull request"),
        Some(Slot::Link),
        "{said:?}"
    );
    assert_eq!(
        kind_of(&said, "https://example.test/2"),
        Some(Slot::Link),
        "{said:?}"
    );
    assert_eq!(
        kind_of(&said, "src/cli/draw.rs"),
        Some(Slot::Path),
        "{said:?}"
    );
    assert_eq!(kind_of(&said, "0.45.0"), Some(Slot::Revision), "{said:?}");
    assert_eq!(kind_of(&said, "3436980f"), Some(Slot::Revision), "{said:?}");
    assert_eq!(kind_of(&said, "641"), Some(Slot::Figure), "{said:?}");
    assert_eq!(
        drawn(&said),
        "See the pull request and https://example.test/2, then src/cli/draw.rs: \
         it ships in 0.45.0 as 3436980f, and 641 passed.\n"
    );
}

#[test]
fn each_design_draws_an_answers_kinds_in_its_own_inks() {
    // The same answer under each design, in the Dark theme at the exact rung.
    let truecolor = |name: &str| (name == "COLORTERM").then(|| "truecolor".to_owned());
    let said = finished(KINDS);
    let underlined = "\x1b[4;38;2;97;145;230m";
    let accent = "\x1b[38;2;18;137;127m";
    let third = "\x1b[38;2;165;130;235m";
    let fourth = "\x1b[38;2;218;114;166m";

    for (design, path, revision, figure) in [
        (Design::Calm, accent, "", ""),
        (Design::Balanced, third, fourth, ""),
        (Design::Rich, third, fourth, fourth),
    ] {
        let palette = Palette::resolve(true, Theme::Dark, None, &truecolor).designing(design);
        let ink =
            |text: &str| kind_of(&said, text).map(|slot| palette.open(slot).as_str().to_owned());

        assert_eq!(
            ink("the pull request").as_deref(),
            Some(underlined),
            "{design:?}"
        );
        assert_eq!(
            ink("https://example.test/2").as_deref(),
            Some(underlined),
            "{design:?}"
        );
        assert_eq!(ink("src/cli/draw.rs").as_deref(), Some(path), "{design:?}");
        assert_eq!(ink("0.45.0").as_deref(), Some(revision), "{design:?}");
        assert_eq!(ink("3436980f").as_deref(), Some(revision), "{design:?}");
        assert_eq!(ink("641").as_deref(), Some(figure), "{design:?}");
    }
}

#[test]
fn a_version_split_across_deltas_is_still_one_version() {
    let mut markdown = Markdown::default();
    let mut said = read(&mut markdown, "ships in 0.4");
    said.extend(read(&mut markdown, "5.0 today"));
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });

    assert_eq!(kind_of(&said, "0.45.0"), Some(Slot::Revision), "{said:?}");
    assert_eq!(drawn(&said), "ships in 0.45.0 today");
}

#[test]
fn code_that_is_not_a_path_stays_code() {
    // A command, a name and a field are copied rather than opened, and a dot
    // between two words is not a file's extension.
    for code in [
        "cargo test -p crucible-tui",
        "wait_for_index",
        "self.line",
        "*.rs",
    ] {
        let said = finished(&format!("run `{code}` now"));

        assert_eq!(kind_of(&said, code), Some(Slot::Code), "{said:?}");
    }
    for path in ["src/cli/draw.rs", "Cargo.toml", "docs/", "./install"] {
        let said = finished(&format!("open `{path}` now"));

        assert_eq!(kind_of(&said, path), Some(Slot::Path), "{said:?}");
    }
}

#[test]
fn a_heading_a_quote_a_bullet_and_a_number_each_say_what_they_are() {
    let said = finished("## Why\n\n> the picker\n\n- one\n1. first\n");

    assert_eq!(kind_of(&said, "Why"), Some(Slot::Heading), "{said:?}");
    assert_eq!(kind_of(&said, "the picker"), Some(Slot::Quote), "{said:?}");
    assert_eq!(
        kind_of(&said, Glyphs::default().bullet()),
        Some(Slot::Bullet),
        "{said:?}"
    );
    assert_eq!(kind_of(&said, "1."), Some(Slot::Ordinal), "{said:?}");
    assert_eq!(drawn(&said).lines().last(), Some("1. first"));
}

#[test]
fn a_hash_is_a_hash_in_capitals_too() {
    let said = finished("landed as 3436980F, then 9FCEB02D0AE598E95DC970B74767F19372D61AF8.");

    assert_eq!(kind_of(&said, "3436980F"), Some(Slot::Revision), "{said:?}");
    assert_eq!(
        kind_of(&said, "9FCEB02D0AE598E95DC970B74767F19372D61AF8"),
        Some(Slot::Revision),
        "{said:?}"
    );
}

#[test]
fn hex_digits_that_spell_a_word_or_a_number_are_not_a_hash() {
    // Seven to forty hex digits would take in all of these. A word made of the
    // letters a to f is still a word, in either case, and a run of digits is
    // a figure, so a hash needs a digit and a letter, in one case.
    let said = finished("the defaced and effaced DEFACED wall had 1234567 bricks, aBc1234 too");

    for word in ["defaced", "effaced", "DEFACED", "aBc1234"] {
        let holding: Vec<Slot> = said
            .iter()
            .filter(|(_, text)| text.contains(word))
            .map(|(slot, _)| *slot)
            .collect();
        assert_eq!(holding, [Slot::Plain], "{word}: {said:?}");
    }
    assert_eq!(kind_of(&said, "1234567"), Some(Slot::Figure), "{said:?}");
}

#[test]
fn a_word_that_only_looks_like_a_kind_is_prose() {
    // Too short to be a hash, letters only, and a number inside a word.
    let said = finished("abc123 x86_64 deadbeef 2026-10-03 v2");

    assert!(
        said.iter().all(|(slot, _)| *slot == Slot::Plain),
        "{said:?}"
    );
}

/// What the reader said about `answer` read in one delta and finished: the
/// blocks it handed on, and the text of every run around them.
fn blocks(answer: &str) -> (Vec<Block>, String) {
    let mut markdown = Markdown::default();
    let mut blocks = Vec::new();
    let mut runs = String::new();
    let mut say = |said: Said<'_>| match said {
        Said::Run { text, .. } => runs.push_str(text),
        Said::Block(block) => blocks.push(block),
        Said::Retract => runs.push_str(RETRACTED),
    };
    markdown.read(answer, ROOM, &mut say);
    markdown.finish(ROOM, &mut say);
    (blocks, runs)
}

/// What a reader that holds nothing draws of `answer`, as text.
fn as_written(answer: &str) -> String {
    let mut markdown = Markdown::default().as_written();
    let mut runs = String::new();
    let mut say = |said: Said<'_>| {
        if let Said::Run { text, .. } = said {
            runs.push_str(text);
        }
    };
    markdown.read(answer, ROOM, &mut say);
    markdown.finish(ROOM, &mut say);
    runs
}

/// What is held while `answer` is read a character at a time, after each.
fn holdings(answer: &str) -> Vec<Option<Holding>> {
    let mut markdown = Markdown::default();
    let mut held = Vec::new();
    for (at, character) in answer.char_indices() {
        let piece = answer
            .get(at..at + character.len_utf8())
            .unwrap_or_default();
        markdown.read(piece, ROOM, &mut |_| {});
        held.push(markdown.holding());
    }
    held
}

#[test]
fn a_table_is_handed_on_whole_with_the_source_it_was_written_as() {
    let table = "| a | b |\n|---|---|\n| 1 | 2 |\n";
    let (blocks, runs) = blocks(&format!("before\n{table}\nafter\n"));

    assert_eq!(
        blocks,
        [Block {
            kind: Kind::Table,
            source: table.to_owned(),
            closed: true,
        }]
    );
    assert_eq!(runs, "before\n\nafter\n");
}

#[test]
fn lines_with_bars_and_no_delimiter_row_are_not_a_table() {
    let (blocks, runs) = blocks("| a | b |\n| 1 | 2 |\n\nafter\n");

    assert!(blocks.is_empty(), "{blocks:?}");
    assert!(runs.contains("| a | b |"), "{runs:?}");
}

#[test]
fn a_rule_is_handed_on_as_a_block_of_its_own() {
    for rule in ["---", "***", "___"] {
        let (blocks, runs) = blocks(&format!("above\n\n{rule}\n\nbelow\n"));

        assert_eq!(
            blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
            [Kind::Rule],
            "{rule}"
        );
        assert_eq!(runs, "above\n\n\nbelow\n", "{rule}");
    }
}

#[test]
fn display_math_is_held_from_its_opener_to_the_end_of_its_closer_line() {
    for (math, closer) in [
        ("$$\nx^2 + y^2\n$$\n", "$$"),
        ("$$ x = 1 $$\n", "$$ on one line"),
        ("\\[\nx = \\frac{1}{2}\n\\]\n", "\\]"),
        (
            "\\begin{align}\na &= b \\\\\nc &= d\n\\end{align}\n",
            "\\end{align}",
        ),
        ("\\begin{align*}\na &= b\n\\end{align*}\n", "\\end{align*}"),
    ] {
        let (blocks, runs) = blocks(&format!("before\n\n{math}after\n"));

        assert_eq!(
            blocks,
            [Block {
                kind: Kind::DisplayMath,
                source: math.to_owned(),
                closed: true,
            }],
            "{closer}"
        );
        assert_eq!(runs, "before\n\nafter\n", "{closer}");
    }
}

#[test]
fn a_dollar_or_a_backslash_that_opens_no_formula_is_read_as_it_always_was() {
    for answer in [
        "$5 and more\n",
        "it costs $$ 5, after a word\n",
        "  $5 under an indent\n",
        "\\begin{document}\n",
        "\\section{one}\n",
        "\\\\ a break\n",
    ] {
        let (blocks, runs) = blocks(answer);

        assert!(blocks.is_empty(), "{answer:?}: {blocks:?}");
        assert_eq!(runs, as_written(answer), "{answer:?}");
    }
}

#[test]
fn a_mermaid_diff_or_math_fence_is_held_whole() {
    for (fence, kind) in [
        ("```mermaid\ngraph TD\n  A --> B\n```\n", Kind::Mermaid),
        ("~~~mermaid\ngraph TD\n~~~\n", Kind::Mermaid),
        ("```diff\n-old\n+new\n```\n", Kind::Diff),
        ("```patch\n@@ -1 +1 @@\n-a\n+b\n```\n", Kind::Diff),
        ("```math\nx^2\n```\n", Kind::DisplayMath),
        ("````latex\nx^2\n````\n", Kind::DisplayMath),
    ] {
        let (blocks, runs) = blocks(&format!("before\n{fence}after\n"));

        assert_eq!(
            blocks,
            [Block {
                kind,
                source: fence.to_owned(),
                closed: true,
            }],
            "{fence:?}"
        );
        assert_eq!(runs, "before\nafter\n", "{fence:?}");
    }
}

#[test]
fn a_fence_in_another_language_is_not_held() {
    let (blocks, runs) = blocks("```rust\nfn main() {}\n```\nafter\n");

    assert!(blocks.is_empty(), "{blocks:?}");
    assert!(runs.contains("fn main() {}"), "{runs:?}");
}

#[test]
fn a_held_block_still_open_when_the_message_ends_is_handed_on_unclosed() {
    for (answer, kind) in [
        ("$$\nx^2\n", Kind::DisplayMath),
        ("```mermaid\ngraph TD\n", Kind::Mermaid),
        ("```diff\n-old\n", Kind::Diff),
    ] {
        let (blocks, _) = blocks(answer);

        assert_eq!(
            blocks,
            [Block {
                kind,
                source: answer.to_owned(),
                closed: false,
            }],
            "{answer:?}"
        );
    }
}

#[test]
fn a_held_block_past_its_cap_is_shown_as_written_and_never_handed_on() {
    for (opener, line, closer, cap) in [
        ("$$\n", "x + ", "$$\n", MATH_CAP),
        ("```mermaid\n", "A --> B\n", "```\n", MERMAID_CAP),
        ("```diff\n", "+added\n", "```\n", DIFF_CAP),
    ] {
        let body = line.repeat(cap / line.len() + 1);
        let answer = format!("{opener}{body}\n{closer}after\n");
        let (blocks, runs) = blocks(&answer);

        assert!(blocks.is_empty(), "{opener:?}: {} blocks", blocks.len());

        // The same rows as the block read as it was written, except for the
        // top edge of a fence, which says why it was not drawn.
        let written = as_written(&answer);
        if let Some(kind) = opener.strip_prefix("```") {
            let kind = kind.trim_end();
            let (first, rest) = runs.split_once('\n').expect("a row");
            assert_eq!(
                first,
                format!("┌─ {kind} · longer than this can hold · shown as written"),
                "{opener:?}"
            );
            let (was, rest_was) = written.split_once('\n').expect("a row");
            assert_eq!(was, format!("┌─ {kind}"), "{opener:?}");
            assert_eq!(rest, rest_was, "{opener:?}");
        } else {
            assert_eq!(runs, written, "{opener:?}");
        }
    }
}

#[test]
fn a_table_past_its_cap_is_drawn_as_it_always_was() {
    let row = "| a cell | another cell |\n";
    let table = format!(
        "| a | b |\n|---|---|\n{}",
        row.repeat(TABLE_CAP / row.len() + 1)
    );
    let (blocks, _) = blocks(&format!("{table}\nafter\n"));

    assert!(blocks.is_empty(), "{} blocks", blocks.len());
}

/// A table written without its outer bars.
const BARE: &str = "name | since\n--|--\nalpha | 0.1\nbeta | 0.2\n";

#[test]
fn a_table_without_outer_bars_is_handed_on_whole_with_the_source_it_was_written_as() {
    let (blocks, runs) = blocks(&format!("before\n{BARE}\nafter\n"));

    assert_eq!(
        blocks,
        [Block {
            kind: Kind::Table,
            source: BARE.to_owned(),
            closed: true,
        }]
    );
    assert_eq!(standing_text(&runs), "before\n\nafter\n");
}

#[test]
fn a_table_without_outer_bars_is_drawn_as_the_table_it_is() {
    let said = standing(whole(&format!("{BARE}\n")));

    assert_eq!(
        drawn(&said),
        "name  │ since\n\
         ──────┼──────\n\
         alpha │ 0.1  \n\
         beta  │ 0.2  \n\n"
    );
}

#[test]
fn a_table_without_outer_bars_is_drawn_as_the_one_with_them_is() {
    // A body row with fewer or more cells than the header is padded or cut
    // by the one reader of tables, whichever way the table was written.
    let bare = "a | b\n:--|--:\n1\n2 | 3 | 4\n`x` | **y**\n\n";
    let barred = "| a | b |\n|:--|--:|\n| 1 |\n| 2 | 3 | 4 |\n| `x` | **y** |\n\n";

    assert_eq!(standing(whole(bare)), whole(barred));
    assert!(drawn(&standing(whole(bare))).contains('│'));
}

#[test]
fn a_table_without_outer_bars_runs_to_the_first_blank_line() {
    let table = "a | b\n--|--\n1 | 2\nno bars in this row\n";
    let (blocks, runs) = blocks(&format!("{table}\nafter | this\n"));

    assert_eq!(
        blocks,
        [Block {
            kind: Kind::Table,
            source: table.to_owned(),
            closed: true,
        }]
    );
    assert_eq!(standing_text(&runs), "\nafter | this\n");
}

#[test]
fn a_table_without_outer_bars_may_open_in_the_middle_of_a_paragraph() {
    let (blocks, runs) = blocks("one line of prose\na | b\n--|--\n1 | 2\n");

    assert_eq!(
        blocks
            .iter()
            .map(|block| block.source.as_str())
            .collect::<Vec<_>>(),
        ["a | b\n--|--\n1 | 2\n"]
    );
    assert_eq!(standing_text(&runs), "one line of prose\n");
}

#[test]
fn a_header_is_not_announced_as_a_table_until_its_delimiter_row_has_arrived() {
    let answer = "name | since\n--|--\nalpha | 0.1\n";
    let held = holdings(answer);
    let delimited = "name | since\n--|--\n".len();

    assert!(
        held.iter().take(delimited - 1).all(Option::is_none),
        "{held:?}"
    );
    assert_eq!(
        held.get(delimited - 1),
        Some(&Some(Holding {
            held: Held::Table,
            lines: 2,
        }))
    );
    assert_eq!(
        held.last(),
        Some(&Some(Holding {
            held: Held::Table,
            lines: 3,
        }))
    );
}

#[test]
fn lines_that_only_look_like_a_table_without_outer_bars_stay_as_they_were_written() {
    for answer in [
        // The delimiter row has more cells than the header, or fewer.
        "a | b\n--|---|--\n1 | 2\n",
        "a | b | c\n--|--\n1 | 2\n",
        // A delimiter row whose cells are not dashes, or are only colons.
        "a | b\n--|x\n1 | 2\n",
        "a | b\n:|:\n1 | 2\n",
        // A header with one cell, or one whose bar is escaped.
        "no bars\n--|--\n1 | 2\n",
        "a \\| b\n--|--\n1 | 2\n",
        // A row of dashes in the middle of prose.
        "a | b\nthen -- | --\n1 | 2\n",
        // Nothing under the header at all, or the message ending in the
        // middle of the delimiter row.
        "a | b\n",
        "a | b\n--|",
    ] {
        let (blocks, _) = blocks(answer);
        assert!(blocks.is_empty(), "{answer:?}: {blocks:?}");
        assert_eq!(
            drawn(&ended(answer)),
            answer.replace("\\|", "|"),
            "{answer:?}"
        );
    }
}

#[test]
fn a_lone_row_of_dashes_under_a_line_is_not_a_table() {
    for answer in ["a | b\n---\n1 | 2\n", "one cell\n---\n", "one cell\n:-:\n"] {
        let (blocks, runs) = blocks(answer);

        assert!(
            blocks.iter().all(|block| block.kind != Kind::Table),
            "{answer:?}: {blocks:?}"
        );
        assert!(runs.starts_with(answer.lines().next().unwrap_or_default()));
    }

    // What the line is instead: the title of a level-2 heading, or prose.
    for (answer, title) in [
        ("one cell\n---\n", "one cell"),
        ("a | b\n---\n1 | 2\n", "a | b"),
    ] {
        let (blocks, _) = blocks(answer);

        assert_eq!(
            blocks.first().map(|block| block.kind),
            Some(Kind::Heading(Level::Two)),
            "{answer:?}: {blocks:?}"
        );
        assert!(
            blocks
                .first()
                .is_some_and(|block| block.source.starts_with(title)),
            "{answer:?}: {blocks:?}"
        );
    }
    let (blocks, _) = blocks("one cell\n:-:\n");
    assert!(blocks.is_empty(), "{blocks:?}");
}

#[test]
fn a_header_longer_than_a_header_is_held_for_is_read_as_the_prose_it_is() {
    let header = format!("{} | b", "a".repeat(HEADER_CAP));
    let answer = format!("{header}\n--|--\n1 | 2\n");
    let (blocks, _) = blocks(&answer);

    assert!(blocks.is_empty(), "{blocks:?}");
    assert_eq!(drawn(&ended(&answer)), answer);
}

#[test]
fn a_line_past_the_cap_on_a_header_is_said_without_waiting_for_its_end() {
    let line = "a".repeat(HEADER_CAP + 8);
    let said = read(&mut Markdown::default(), &line);

    assert_eq!(drawn(&said), line);
}

#[test]
fn a_table_without_outer_bars_past_its_cap_is_drawn_as_it_always_was() {
    let row = "a cell | another cell\n";
    let table = format!("a | b\n--|--\n{}", row.repeat(TABLE_CAP / row.len() + 1));
    let (blocks, _) = blocks(&format!("{table}\nafter\n"));

    assert!(blocks.is_empty(), "{} blocks", blocks.len());
}

#[test]
fn a_reader_that_holds_nothing_draws_a_table_without_outer_bars_as_written() {
    assert_eq!(as_written(BARE), BARE);
}

#[test]
fn a_message_that_ends_in_a_table_without_outer_bars_draws_it_as_one() {
    let (blocks, _) = blocks("a | b\n--|--\n1 | 2");

    assert_eq!(
        blocks
            .iter()
            .map(|block| block.source.as_str())
            .collect::<Vec<_>>(),
        ["a | b\n--|--\n1 | 2"]
    );
}

/// What the reader said while `answer` was read a character at a time and
/// finished, as one line per thing: `run <text>`, `retract` and `block`.
fn told(answer: &str) -> Vec<String> {
    let mut markdown = Markdown::default();
    let mut told = Vec::new();
    let mut say = |said: Said<'_>| match said {
        Said::Run { text, .. } => told.push(format!("run {text}")),
        Said::Retract => told.push("retract".to_owned()),
        Said::Block(_) => told.push("block".to_owned()),
    };
    for (at, character) in answer.char_indices() {
        let piece = answer
            .get(at..at + character.len_utf8())
            .unwrap_or_default();
        markdown.read(piece, ROOM, &mut say);
    }
    markdown.finish(ROOM, &mut say);
    told
}

/// The text of the runs among `told`.
fn said_text(told: &[String]) -> String {
    told.iter()
        .filter_map(|thing| thing.strip_prefix("run "))
        .collect()
}

#[test]
fn a_header_is_said_as_the_prose_it_is_so_far_and_taken_back_when_the_delimiter_row_decides() {
    let told = told("before\nname | since\n--|--\nalpha | 0.1\n\nafter");

    let retract = told.iter().position(|thing| thing == "retract");
    let block = told.iter().position(|thing| thing == "block");
    assert!(retract.is_some() && retract < block, "{told:?}");
    assert_eq!(told.iter().filter(|thing| *thing == "retract").count(), 1);
    // The header is on its way out before the line under it has arrived.
    let header = told.iter().position(|thing| thing == "run e");
    assert!(header.is_some() && header < retract, "{told:?}");
}

#[test]
fn a_line_that_stops_looking_like_a_header_is_never_taken_back() {
    for answer in [
        "a | b\nthen -- | --\n1 | 2\n",
        "a | b\n--|x\n1 | 2\n",
        "a | b\n--|",
        "a | b\n",
        "a | b",
        "no bars\n--|--\n",
    ] {
        let told = told(answer);

        assert!(
            !told.iter().any(|thing| thing == "retract"),
            "{answer:?}: {told:?}"
        );
        assert_eq!(said_text(&told), answer, "{answer:?}");
    }
}

#[test]
fn a_header_is_in_doubt_exactly_until_the_delimiter_row_has_arrived() {
    let answer = "name | since\n--|--\nalpha | 0.1\n\nafter";
    let mut markdown = Markdown::default();
    let mut doubt = Vec::new();
    for (at, character) in answer.char_indices() {
        let piece = answer
            .get(at..at + character.len_utf8())
            .unwrap_or_default();
        markdown.read(piece, ROOM, &mut |_| {});
        doubt.push(markdown.in_doubt());
    }

    // From the first character, through the header's break and the start of
    // the line under it, up to the last character of the delimiter row; the
    // break that ends that row decides it, and the rows of the table are no
    // longer a line that could be a header.
    let row = answer.find("--|--").unwrap_or_default() + "--|--".len();
    let table = row + "\nalpha | 0.1".len();
    assert!(doubt.iter().take(row).all(|doubt| *doubt), "{doubt:?}");
    assert!(
        doubt.iter().take(table + 1).skip(row).all(|doubt| !*doubt),
        "{doubt:?}"
    );
}

#[test]
fn a_line_that_cannot_be_a_header_is_not_in_doubt() {
    let long = format!("{} | b\n", "x".repeat(HEADER_CAP));
    for answer in [
        "no bars here\n\n",
        "a | b\nthen no bars\n\n",
        "- a | b\n",
        long.as_str(),
    ] {
        let mut markdown = Markdown::default();
        markdown.read(answer, ROOM, &mut |_| {});
        assert!(!markdown.in_doubt(), "{answer:?}");
    }
}

#[test]
fn a_paragraph_line_is_in_doubt_while_it_is_open_and_right_after_its_break() {
    for line in [
        "no bars here",
        "a | b",
        "  indented",
        "**bold**",
        "$5",
        "\\*x",
        "[a link](https://example.test)",
        "1.5 is a number",
    ] {
        let mut markdown = Markdown::default();
        for (at, character) in line.char_indices() {
            let piece = line.get(at..at + character.len_utf8()).unwrap_or_default();
            markdown.read(piece, ROOM, &mut |_| {});
            assert!(markdown.in_doubt(), "{line:?}: after {piece:?}");
        }
        markdown.read("\n", ROOM, &mut |_| {});
        assert!(markdown.in_doubt(), "{line:?}: right after its break");
    }
}

#[test]
fn a_line_that_opens_something_else_is_not_in_doubt() {
    for line in [
        "# a heading\n",
        "- an item\n",
        "* an item\n",
        "> a quote\n",
        "```rust\n",
        "1. an item\n",
        "| a | b |\n",
        "---\n",
        "\n",
    ] {
        let mut markdown = Markdown::default();
        markdown.read(line, ROOM, &mut |_| {});
        assert!(!markdown.in_doubt(), "{line:?}");
    }
}

#[test]
fn the_start_of_the_next_line_rules_an_underline_out() {
    for rest in ["x", "-=", "=-", "\n", "=x", "- x", "***"] {
        let mut markdown = Markdown::default();
        let before = drawn(&read(&mut markdown, "Title\n"));
        assert!(markdown.in_doubt(), "{rest:?}");
        let after = drawn(&read(&mut markdown, rest));

        // The break that was held comes out with the title, and nothing the
        // title was can be taken back.
        assert_eq!(before, "Title", "{rest:?}");
        assert!(after.starts_with('\n'), "{rest:?}: {after:?}");
        let (blocks, _) = blocks(&format!("Title\n{rest}\n"));
        assert!(
            !blocks.iter().any(|block| block.source.starts_with("Title")),
            "{rest:?}: {blocks:?}"
        );
    }
    // A line that is not a paragraph line is not in doubt either.
    for rest in ["\n", "- x"] {
        let mut markdown = Markdown::default();
        markdown.read("Title\n", ROOM, &mut |_| {});
        markdown.read(rest, ROOM, &mut |_| {});
        assert!(!markdown.in_doubt(), "{rest:?}");
    }
    for rest in ["=", "==", "-", "---", "  ---", "=== ", "---\t"] {
        let mut markdown = Markdown::default();
        markdown.read("Title\n", ROOM, &mut |_| {});
        markdown.read(rest, ROOM, &mut |_| {});
        assert!(markdown.in_doubt(), "{rest:?}");
    }
}

#[test]
fn an_underline_past_the_cap_rules_the_hold_out() {
    let mut markdown = Markdown::default();
    markdown.read(
        &format!("Title\n{}", "=".repeat(HEADER_CAP)),
        ROOM,
        &mut |_| {},
    );
    assert!(markdown.in_doubt());
    markdown.read("=", ROOM, &mut |_| {});
    assert!(!markdown.in_doubt());
}

#[test]
fn the_end_of_the_message_ends_the_hold() {
    let mut markdown = Markdown::default();
    markdown.read("Title\n==", ROOM, &mut |_| {});
    assert!(markdown.in_doubt());
    markdown.finish(ROOM, &mut |_| {});
    assert!(!markdown.in_doubt());
}

#[test]
fn a_header_is_taken_back_once_however_the_answer_is_cut_into_deltas() {
    let answer = "name | since\n--|--\nalpha | 0.1\n\nafter\n";
    let whole = drawn(&standing(ended(answer)));

    for cut in 0..answer.len() {
        let (first, second) = answer.split_at(cut);
        assert_eq!(
            drawn(&standing(ended_in(&[first, second]))),
            whole,
            "cut at {cut}"
        );
    }
}

#[test]
fn what_is_held_says_its_kind_and_how_many_lines_have_arrived() {
    let answer = "$$\nx^2\n";
    let held = holdings(answer);

    assert_eq!(held.first(), Some(&None), "one dollar is not yet a formula");
    assert_eq!(
        held.get(2),
        Some(&Some(Holding {
            held: Held::DisplayMath,
            lines: 1,
        }))
    );
    assert_eq!(
        held.last(),
        Some(&Some(Holding {
            held: Held::DisplayMath,
            lines: 2,
        }))
    );

    let table = holdings("| a | b |\n|---|---|\n");
    assert_eq!(
        table.last(),
        Some(&Some(Holding {
            held: Held::Table,
            lines: 2,
        }))
    );
}

#[test]
fn nothing_is_held_once_a_block_closes() {
    let held = holdings("```mermaid\ngraph TD\n```\nafter");

    assert!(held.iter().any(Option::is_some));
    assert_eq!(held.last(), Some(&None));
}

#[test]
fn the_status_row_names_the_kind_and_counts_lines_in_either_glyph_set() {
    let row = |held, lines, glyphs| Holding { held, lines }.row(64, glyphs).text();

    assert_eq!(
        row(Held::Table, 1, Glyphs::Unicode),
        "◇ drawing a table · 1 line so far"
    );
    assert_eq!(
        row(Held::DisplayMath, 12, Glyphs::Unicode),
        "◇ drawing a formula · 12 lines so far"
    );
    assert_eq!(
        row(Held::Mermaid, 3, Glyphs::Ascii),
        "o drawing a mermaid diagram - 3 lines so far"
    );
    assert_eq!(
        row(Held::Diff, 2, Glyphs::Ascii),
        "o drawing a diff - 2 lines so far"
    );

    let narrow = Holding {
        held: Held::Mermaid,
        lines: 3,
    }
    .row(12, Glyphs::Unicode);
    assert!(width::columns(&narrow.text()) <= 12, "{:?}", narrow.text());
    assert!(
        narrow.spans().all(|(slot, _)| slot == Slot::Quiet),
        "{narrow:?}"
    );
}

#[test]
fn a_held_block_laid_out_is_what_it_would_have_streamed_as() {
    for answer in [
        "$$\nx^2 + y^2\n$$\n",
        "```mermaid\ngraph TD\n  A --> B\n```\n",
        "```diff\n-old\n+new\n```\n",
    ] {
        let (blocks, _) = blocks(answer);
        let Some(block) = blocks.first() else {
            panic!("{answer:?} was not held");
        };
        let mut laid = String::new();
        for row in lay(block, ROOM, Glyphs::Unicode, &Links::default()) {
            laid.push_str(&row.text());
            laid.push('\n');
        }

        assert_eq!(laid, as_written(answer), "{answer:?}");
    }
}

/// The first line of `answer`, as the row the reader drew it as.
fn row_of(answer: &str, glyphs: Glyphs) -> Row {
    let said = read(&mut Markdown::new(glyphs), answer);
    let mut row = Row::new();
    for (slot, text) in said {
        if text.contains('\n') {
            break;
        }
        row.push(slot, text);
    }
    row
}

/// What a row after the first stands in front of its words with.
fn hung_under(answer: &str, glyphs: Glyphs) -> Option<(usize, usize, String)> {
    hang(&row_of(answer, glyphs)).map(|hang| (hang.lead, hang.columns, hang.prefix.text()))
}

#[test]
fn a_bullet_hangs_under_the_column_its_words_begin_in() {
    // The mark is one column and the space after it another, in both sets.
    assert_eq!(
        hung_under("- an item that wraps\n", Glyphs::Unicode),
        Some(("• ".len(), 2, "  ".to_owned()))
    );
    assert_eq!(
        hung_under("- an item that wraps\n", Glyphs::Ascii),
        Some((2, 2, "  ".to_owned()))
    );
}

#[test]
fn a_task_hangs_under_its_words_whether_it_is_done_or_not() {
    for (answer, lead) in [
        ("- [ ] a task\n", "□ ".len()),
        ("- [x] a task\n", "✓ ".len()),
    ] {
        assert_eq!(
            hung_under(answer, Glyphs::Unicode),
            Some((lead, 2, "  ".to_owned())),
            "{answer:?}"
        );
    }
    for answer in ["- [ ] a task\n", "- [x] a task\n"] {
        assert_eq!(
            hung_under(answer, Glyphs::Ascii),
            Some((2, 2, "  ".to_owned())),
            "{answer:?}"
        );
    }
}

#[test]
fn a_numbered_item_hangs_at_the_width_of_its_own_number() {
    // Three columns for `1. ` and four for `10. `: what the words are hung
    // under is where they began, not a column the list agreed on.
    assert_eq!(
        hung_under("1. first\n", Glyphs::Unicode),
        Some((3, 3, "   ".to_owned()))
    );
    assert_eq!(
        hung_under("10. tenth\n", Glyphs::Unicode),
        Some((4, 4, "    ".to_owned()))
    );
}

#[test]
fn the_indentation_that_nests_an_item_is_kept_in_its_hang() {
    assert_eq!(
        hung_under("  - nested\n", Glyphs::Unicode),
        Some(("  • ".len(), 4, "    ".to_owned()))
    );
    assert_eq!(
        hung_under("   1. nested\n", Glyphs::Unicode),
        Some((6, 6, "      ".to_owned()))
    );
}

#[test]
fn a_quote_stands_its_bar_again_in_front_of_every_row_it_wraps_onto() {
    let hang = hang(&row_of("> somebody else\n", Glyphs::Unicode)).expect("a quote hangs");

    assert_eq!((hang.lead, hang.columns), ("│ ".len(), 2));
    assert_eq!(
        hang.prefix.spans().collect::<Vec<_>>(),
        vec![(Slot::Quiet, "│"), (Slot::Quiet, " ")],
        "the bar is as quiet on the rows after the first as on the first"
    );
    assert_eq!(
        hung_under("> somebody else\n", Glyphs::Ascii),
        Some((2, 2, "| ".to_owned()))
    );
}

#[test]
fn a_bar_and_a_mark_in_one_row_hang_by_both() {
    // The reader does not yet read an item inside a quote, so the row is built
    // as the pieces it would be drawn from: the hang must compose them.
    let mut row = Row::new();
    row.push(Slot::Quiet, "│");
    row.push(Slot::Quiet, " ");
    row.push(Slot::Bullet, "•");
    row.push(Slot::Quiet, " ");
    row.push(Slot::Plain, "an item");

    let hang = hang(&row).expect("a quoted item hangs");

    assert_eq!((hang.lead, hang.columns), ("│ • ".len(), 4));
    assert_eq!(hang.prefix.text(), "│   ");
}

#[test]
fn prose_and_things_that_only_look_like_marks_hang_under_nothing() {
    // The same lines the reader leaves alone: arithmetic, a figure, and a
    // number in the middle of a sentence.
    for answer in [
        "plain words that wrap\n",
        "  indented prose\n",
        "5 - 3 = 2\n",
        "1.5 is a figure\n",
        "word 1. later\n",
    ] {
        let row = row_of(answer, Glyphs::Unicode);

        assert_eq!(hang(&row), None, "{answer:?} draws {row:?}");
    }
}

// A fence in its frame.

/// The runs `answer` comes to, read and finished in `glyphs`.
fn framed(answer: &str, glyphs: Glyphs) -> Vec<(Slot, String)> {
    let mut markdown = Markdown::new(glyphs);
    let mut said = read(&mut markdown, answer);
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });
    said
}

#[test]
fn a_fence_is_said_between_a_top_edge_and_a_bottom_edge() {
    let said = framed("```rust\nlet x = 1;\n```\nafter\n", Glyphs::Unicode);

    assert_eq!(drawn(&said), "┌─ rust\n│ let x = 1;\n└─\nafter\n");
    assert_eq!(said.first(), Some(&(Slot::Quiet, "┌─ rust".to_owned())));
    assert!(said.contains(&(Slot::Quiet, "│ ".to_owned())), "{said:?}");
    assert!(said.contains(&(Slot::Quiet, "└─".to_owned())), "{said:?}");
}

#[test]
fn a_fence_is_said_with_the_ascii_edges_in_the_ascii_set() {
    let said = framed("```rust\nlet x = 1;\n```\n", Glyphs::Ascii);

    assert_eq!(drawn(&said), "+- rust\n| let x = 1;\n+-\n");
}

#[test]
fn every_line_of_a_fence_wears_one_gutter_even_a_blank_one() {
    for language in ["", "rust"] {
        let said = framed(
            &format!("```{language}\none\n\n  two\n```\n"),
            Glyphs::Unicode,
        );

        let gutters = said
            .iter()
            .filter(|(slot, text)| *slot == Slot::Quiet && text == "│ ")
            .count();
        assert_eq!(gutters, 3, "{language:?}: {said:?}");
        assert!(drawn(&said).contains("│ \n"), "{language:?}: {said:?}");
        assert!(drawn(&said).contains("│   two\n"), "{language:?}: {said:?}");
    }
}

#[test]
fn a_fence_the_message_ends_inside_is_never_given_a_bottom() {
    let said = framed("```rust\nlet x = 1;\n", Glyphs::Unicode);

    assert_eq!(drawn(&said), "┌─ rust\n│ let x = 1;\n");
}

#[test]
fn a_fence_holding_the_other_marker_is_framed_once() {
    let said = framed("~~~\n```\ninside\n~~~\n", Glyphs::Unicode);

    assert_eq!(drawn(&said), "┌─\n│ ```\n│ inside\n└─\n");
}

/// What a label makes of the info string that was written for it.
fn labelled(info: &str) -> String {
    let said = framed(&format!("```{info}\ncode\n```\n"), Glyphs::Unicode);
    drawn(&said)
        .lines()
        .next()
        .and_then(|top| top.strip_prefix("┌─"))
        .map(|label| label.trim_start().to_owned())
        .unwrap_or_default()
}

#[test]
fn a_label_is_the_first_word_of_the_info_string_as_written() {
    assert_eq!(labelled("rust"), "rust");
    assert_eq!(labelled("Rust,ignore title=\"a.rs\""), "Rust,ignore");
    assert_eq!(labelled("c++"), "c++");
    assert_eq!(labelled("  python  "), "python");
    assert_eq!(labelled("{.rust .numberLines}"), "{.rust");
    assert_eq!(labelled(""), "");
}

#[test]
fn a_label_is_cleaned_before_it_is_drawn() {
    for (info, label) in [
        ("\u{1b}]0;a title\u{7}rust", "rust"),
        ("\u{1b}[31mrust", "rust"),
        ("ru\u{7}st", "rust"),
        ("ru\u{9b}st", "rust"),
        ("\u{202e}tsur", "tsur"),
        ("\u{2066}rust\u{2069}", "rust"),
        ("\u{10eeee}\u{305}\u{305}rust", "rust"),
        ("ru\u{200b}st", "rust"),
        ("\u{1b}", ""),
    ] {
        assert_eq!(labelled(info), label, "{info:?}");
    }
}

#[test]
fn a_label_wider_than_a_label_may_be_ends_in_an_ellipsis() {
    let long = "x".repeat(200);

    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let said = framed(&format!("```{long}\ncode\n```\n"), glyphs);
        let top = drawn(&said).lines().next().expect("a top edge").to_owned();

        assert!(top.ends_with(glyphs.ellipsis()), "{top:?}");
        assert!(width::columns(&top) < 48, "{top:?}");
    }
}

/// A fence named `kind` with the source of a held block past its cap.
fn past_its_cap(kind: &str, line: &str, cap: usize) -> String {
    format!(
        "```{kind}\n{}\n```\nafter\n",
        line.repeat(cap / line.len() + 1)
    )
}

#[test]
fn a_held_fence_past_its_cap_says_why_in_the_label_of_its_frame() {
    for (kind, line, cap) in [
        ("mermaid", "A --> B\n", MERMAID_CAP),
        ("diff", "+added\n", DIFF_CAP),
        ("math", "x + y\n", MATH_CAP),
    ] {
        let said = framed(&past_its_cap(kind, line, cap), Glyphs::Unicode);

        assert_eq!(
            drawn(&said).lines().next(),
            Some(format!("┌─ {kind} · longer than this can hold · shown as written").as_str()),
            "{kind}"
        );
        assert!(drawn(&said).ends_with("└─\nafter\n"), "{kind}");
    }
}

#[test]
fn a_fence_that_is_not_held_says_nothing_of_a_cap() {
    for answer in [
        "```rust\nfn main() {}\n```\n",
        "```mermaid\ngraph TD\n```\n",
        "```\ntext\n```\n",
    ] {
        let said = framed(answer, Glyphs::Unicode);

        assert!(
            !drawn(&said).contains("longer than this can hold"),
            "{answer:?}"
        );
    }
}

#[test]
fn a_closing_fence_the_message_ends_in_the_middle_of_still_shuts_its_frame() {
    for answer in ["```rust\nlet x = 1;\n```", "~~~\nx\n~~~"] {
        let said = ended(answer);

        assert!(
            drawn(&said).ends_with("\n└─\n"),
            "{answer:?}: {:?}",
            drawn(&said)
        );
    }
}

// A block indented four spaces after a blank line, in the frame a fence has.

#[test]
fn a_block_indented_after_a_blank_line_is_quiet_code_in_a_frame() {
    let said = ended("para\n\n    let x = 1;\n    let y = 2;\n\nafter\n");

    assert_eq!(
        drawn(&said),
        "para\n\n┌─\n│ let x = 1;\n│ let y = 2;\n└─\n\nafter\n"
    );
    assert!(
        !said.iter().any(|(slot, _)| *slot == Slot::Figure),
        "{said:?}"
    );
    assert!(
        said.iter()
            .filter(|(_, text)| text.contains("let"))
            .all(|(slot, _)| *slot == Slot::Quiet),
        "{said:?}"
    );
}

#[test]
fn a_dashed_row_under_an_indented_block_is_a_rule_across_the_room() {
    let answer = "para\n\n    code\n---\n";
    let (blocks, runs) = blocks(answer);

    assert_eq!(runs, "para\n\n┌─\n│ code\n└─\n");
    assert_eq!(
        blocks.iter().map(|block| &block.kind).collect::<Vec<_>>(),
        [&Kind::Rule]
    );
    assert_eq!(
        drawn(&ended(answer)),
        format!("para\n\n┌─\n│ code\n└─\n{}\n", "─".repeat(ROOM))
    );
}

#[test]
fn a_row_of_equals_under_an_indented_block_is_prose() {
    let (blocks, runs) = blocks("para\n\n    code\n===\n");

    assert_eq!(runs, "para\n\n┌─\n│ code\n└─\n===\n");
    assert!(blocks.is_empty(), "{blocks:?}");
}

#[test]
fn a_blank_line_between_two_indented_lines_stays_in_the_frame() {
    let said = ended("para\n\n    one\n\n    two\n\nafter\n");

    assert_eq!(drawn(&said), "para\n\n┌─\n│ one\n│ \n│ two\n└─\n\nafter\n");
}

#[test]
fn the_spaces_past_the_fourth_and_the_markers_are_the_codes_own() {
    let said = ended("para\n\n    if *a* {\n        `b` # c\n    }\n");

    assert_eq!(
        drawn(&said),
        "para\n\n┌─\n│ if *a* {\n│     `b` # c\n│ }\n└─\n"
    );
}

#[test]
fn an_indented_block_the_message_ends_in_is_shut() {
    for (answer, drawn_text) in [
        ("para\n\n    code", "para\n\n┌─\n│ code\n└─\n"),
        ("para\n\n    code\n", "para\n\n┌─\n│ code\n└─\n"),
        ("para\n\n    code\n\n", "para\n\n┌─\n│ code\n└─\n\n"),
    ] {
        assert_eq!(drawn(&ended(answer)), drawn_text, "{answer:?}");
    }
}

#[test]
fn an_indented_line_in_a_list_item_or_a_quote_or_a_paragraph_is_read_as_before() {
    for (answer, drawn_text) in [
        ("- item\n\n    more\n", "• item\n\n    more\n"),
        ("1. step\n\n    more\n", "1. step\n\n    more\n"),
        ("- item\nlazy\n\n    more\n", "• item\nlazy\n\n    more\n"),
        (
            "- item\n\n  more\n\n    deeper\n",
            "• item\n\n  more\n\n    deeper\n",
        ),
        ("> quote\n\n>     code\n", "│ quote\n\n│     code\n"),
        ("para\n    more\n", "para\n    more\n"),
    ] {
        assert_eq!(drawn(&ended(answer)), drawn_text, "{answer:?}");
    }
}

// A table and a heading underlined with dashes each end on a line the reader
// never sees end, since the block took its break: the next line starts where
// the block let go of it, and is read from its first character all the same.
// A table written without outer bars ends only at a blank line, which the
// reader does see, and takes every other line under it as a row.
const ENDED_BY_THE_LINE_UNDER: [&str; 2] = ["| a |\n| - |\n| 1 |\n", "Title\n-----\n"];

#[test]
fn an_item_straight_under_a_table_or_a_heading_keeps_its_indented_line() {
    let mut wrong = Vec::new();
    for block in ENDED_BY_THE_LINE_UNDER {
        let answer = format!("{block}1. item\n\n    more\n");
        let (_, runs) = headed(&answer);
        let whole = drawn(&ended(&answer));
        if !runs.ends_with("1. item\n\n    more\n")
            || !whole.ends_with("1. item\n\n    more\n")
            || whole.contains("│ more")
        {
            wrong.push((answer, runs, whole));
        }
    }

    assert_eq!(wrong, []);
}

// The line above it was written, so it opens no block of code, however far
// back the last blank line was.
#[test]
fn a_line_indented_straight_under_a_table_or_a_heading_is_not_code() {
    let mut wrong = Vec::new();
    for block in ENDED_BY_THE_LINE_UNDER {
        let answer = format!("para\n\n{block}    code\n");
        let (_, runs) = headed(&answer);
        let whole = drawn(&ended(&answer));
        if runs.contains("┌─") || !runs.ends_with("    code\n") || whole.contains("┌─") {
            wrong.push((answer, runs, whole));
        }
    }

    assert_eq!(wrong, []);
}

#[test]
fn a_paragraph_after_a_blank_line_ends_the_list_an_indented_block_continued() {
    let said = ended("- item\n\npara\n\n    code\n");

    assert_eq!(drawn(&said), "• item\n\npara\n\n┌─\n│ code\n└─\n");
}

/// What `answer` read to its end handed on: the blocks, and the runs with
/// every line taken back gone.
fn headed(answer: &str) -> (Vec<Block>, String) {
    let (blocks, runs) = blocks(answer);
    (blocks, standing_text(&runs))
}

/// The heading block `title` is, with the lines it was written as.
fn heading(level: Level, source: &str) -> Block {
    Block {
        kind: Kind::Heading(level),
        source: source.to_owned(),
        closed: true,
    }
}

#[test]
fn an_atx_heading_of_the_first_two_levels_is_followed_by_its_rule() {
    for (answer, block, words) in [
        ("# One\n", heading(Level::One, "# One"), "One\n"),
        ("## Two\n", heading(Level::Two, "## Two"), "Two\n"),
        ("# One", heading(Level::One, "# One"), "One\n"),
    ] {
        let (blocks, runs) = headed(answer);

        assert_eq!(blocks, [block], "{answer:?}");
        assert_eq!(runs, words, "{answer:?}");
    }
}

#[test]
fn an_atx_heading_of_the_third_level_or_deeper_wears_no_rule() {
    for answer in [
        "### Three\n",
        "#### Four\n",
        "##### Five\n",
        "###### Six\n",
        "####### Seven\n",
    ] {
        let (blocks, _) = headed(answer);

        assert!(blocks.is_empty(), "{answer:?}: {blocks:?}");
    }
}

#[test]
fn an_atx_heading_streams_its_words_and_is_never_taken_back() {
    let told = told("# One\nafter\n");

    assert!(!told.iter().any(|thing| thing == "retract"), "{told:?}");
    assert_eq!(said_text(&told), "One\nafter\n");
    let words = told.iter().position(|thing| thing == "run O");
    let block = told.iter().position(|thing| thing == "block");
    assert!(words.is_some() && words < block, "{told:?}");
}

#[test]
fn an_underline_makes_the_line_above_it_a_heading_and_is_never_drawn() {
    for (answer, block, words) in [
        (
            "Title\n=====\n",
            heading(Level::One, "Title\n====="),
            "Title\n",
        ),
        ("Sub\n-----\n", heading(Level::Two, "Sub\n-----"), "Sub\n"),
        ("Title\n=\n", heading(Level::One, "Title\n="), "Title\n"),
        ("Sub\n-\n", heading(Level::Two, "Sub\n-"), "Sub\n"),
        (
            "Title\n   ===\n",
            heading(Level::One, "Title\n   ==="),
            "Title\n",
        ),
        (
            "Title\n===\t\n",
            heading(Level::One, "Title\n===\t"),
            "Title\n",
        ),
        ("Sub\n---  \n", heading(Level::Two, "Sub\n---  "), "Sub\n"),
        (
            "Title\n=====",
            heading(Level::One, "Title\n====="),
            "Title\n",
        ),
        ("Sub\n---", heading(Level::Two, "Sub\n---"), "Sub\n"),
    ] {
        let (blocks, runs) = headed(answer);

        assert_eq!(blocks, [block], "{answer:?}");
        assert_eq!(runs, words, "{answer:?}");
    }
}

#[test]
fn a_heading_that_an_underline_made_wears_the_slot_an_atx_heading_does() {
    let atx = finished("# Title\n");
    let setext = standing(finished("Title\n=====\n"));

    assert_eq!(slots(&atx).first(), Some(&Slot::Heading));
    assert_eq!(slots(&setext).first(), Some(&Slot::Heading), "{setext:?}");
    // Its words, and the rule the block under them is laid out as.
    assert_eq!(drawn(&setext), "Title\n━━━━━\n");
}

#[test]
fn only_the_last_line_of_a_paragraph_is_the_heading() {
    let (blocks, runs) = headed("one\ntwo\nthree\n===\n");

    assert_eq!(blocks, [heading(Level::One, "three\n===")]);
    assert_eq!(runs, "one\ntwo\nthree\n");
}

#[test]
fn a_title_is_said_as_prose_and_taken_back_once_the_underline_has_settled_it() {
    let told = told("Title\n=====\nafter\n");

    let retract = told.iter().position(|thing| thing == "retract");
    let block = told.iter().position(|thing| thing == "block");
    assert!(retract.is_some() && retract < block, "{told:?}");
    assert_eq!(told.iter().filter(|thing| *thing == "retract").count(), 1);
    let title = told.iter().position(|thing| thing == "run T");
    assert!(title.is_some() && title < retract, "{told:?}");
}

#[test]
fn a_dashed_row_that_follows_something_other_than_a_paragraph_is_a_rule() {
    for answer in [
        "- item\n---\n",
        "> quote\n---\n",
        "# Foo\n---\n",
        "```\ncode\n```\n---\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n---\n",
        "1. item\n---\n",
        "\n---\n",
        "---\n",
        "text\n\n---\n",
    ] {
        let (blocks, _) = headed(answer);

        assert!(
            blocks.iter().any(|block| block.kind == Kind::Rule),
            "{answer:?}: {blocks:?}"
        );
        assert!(
            !blocks
                .iter()
                .any(|block| block.kind == Kind::Heading(Level::Two)),
            "{answer:?}: {blocks:?}"
        );
    }
}

#[test]
fn a_row_of_equals_signs_that_follows_something_other_than_a_paragraph_is_prose() {
    for answer in [
        "- item\n===\n",
        "> quote\n===\n",
        "# Foo\n===\n",
        "\n===\n",
        "===\n",
    ] {
        let (blocks, runs) = headed(answer);

        // The one heading there may be is the `#` line's own, which has no
        // row of `=` in it.
        assert!(
            !blocks.iter().any(|block| block.source.contains('=')),
            "{answer:?}: {blocks:?}"
        );
        assert!(runs.contains("==="), "{answer:?}: {runs:?}");
    }
}

#[test]
fn a_line_of_cells_over_a_delimiter_row_of_as_many_is_still_a_table() {
    let (blocks, _) = headed("a | b\n--|--\n1 | 2\n");

    assert_eq!(
        blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
        [Kind::Table]
    );
}

#[test]
fn an_underline_that_is_not_one_leaves_both_lines_as_they_were_written() {
    for answer in [
        "Title\n-=\n",
        "Title\n=-\n",
        "Title\n= =\n",
        "Title\n    ===\n",
        "Title\n=== x\n",
        "Title\n:-:\n",
        "Title\n***\n",
    ] {
        let (blocks, runs) = headed(answer);

        assert!(
            !blocks
                .iter()
                .any(|block| matches!(block.kind, Kind::Heading(_))),
            "{answer:?}: {blocks:?}"
        );
        if !blocks.iter().any(|block| block.kind == Kind::Rule) {
            assert_eq!(runs, answer, "{answer:?}");
        }
    }
}

#[test]
fn an_underline_of_a_hundred_and_twenty_eight_marks_is_read_and_one_more_is_not() {
    for (mark, level) in [('-', Level::Two), ('=', Level::One)] {
        let held = mark.to_string().repeat(HEADER_CAP);
        let (blocks, _) = headed(&format!("Title\n{held}\n"));
        assert_eq!(
            blocks,
            [heading(level, &format!("Title\n{held}"))],
            "{mark}"
        );

        let past = mark.to_string().repeat(HEADER_CAP + 1);
        let (blocks, runs) = headed(&format!("Title\n{past}\n"));
        if mark == '-' {
            assert_eq!(
                blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
                [Kind::Rule]
            );
        } else {
            assert!(blocks.is_empty(), "{blocks:?}");
            assert_eq!(runs, format!("Title\n{past}\n"));
        }
    }
}

#[test]
fn a_title_of_a_hundred_and_twenty_eight_bytes_is_a_heading_and_one_more_is_prose() {
    for (underline, level) in [("---", Level::Two), ("===", Level::One)] {
        let title = "t".repeat(HEADER_CAP);
        let (blocks, _) = headed(&format!("{title}\n{underline}\n"));
        assert_eq!(
            blocks,
            [heading(level, &format!("{title}\n{underline}"))],
            "{underline}"
        );

        let title = "t".repeat(HEADER_CAP + 1);
        let (blocks, runs) = headed(&format!("{title}\n{underline}\n"));
        assert!(
            !blocks
                .iter()
                .any(|block| matches!(block.kind, Kind::Heading(_))),
            "{underline}: {blocks:?}"
        );
        if underline == "===" {
            assert_eq!(runs, format!("{title}\n{underline}\n"));
        } else {
            assert_eq!(
                blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
                [Kind::Rule]
            );
        }
    }
}

#[test]
fn a_title_of_nothing_but_markers_is_a_heading() {
    for title in ["**", "--"] {
        let (blocks, runs) = headed(&format!("{title}\n---\n"));

        assert_eq!(
            blocks,
            [heading(Level::Two, &format!("{title}\n---"))],
            "{title}"
        );
        assert_eq!(runs, format!("{title}\n"), "{title}");
    }
}

#[test]
fn a_title_that_opens_with_a_dollar_or_a_backslash_is_held_and_read() {
    for (answer, level, words) in [
        ("$5\n---\n", Level::Two, "$5\n"),
        ("\\*x\n===\n", Level::One, "*x\n"),
        ("$x$ is math\n===\n", Level::One, "$x$ is math\n"),
    ] {
        let (blocks, runs) = headed(answer);

        assert_eq!(
            blocks.iter().map(|block| block.kind).collect::<Vec<_>>(),
            [Kind::Heading(level)],
            "{answer:?}"
        );
        assert_eq!(runs, words, "{answer:?}");
    }
}

#[test]
fn a_title_that_opens_with_a_dollar_or_a_backslash_is_in_doubt_from_its_first_character() {
    for line in ["$5", "\\*x", "$x$ is math"] {
        let mut markdown = Markdown::default();
        for (at, character) in line.char_indices() {
            let piece = line.get(at..at + character.len_utf8()).unwrap_or_default();
            markdown.read(piece, ROOM, &mut |_| {});
            assert!(
                markdown.in_doubt() || markdown.holding().is_some(),
                "{line:?}: {piece:?}"
            );
        }
        markdown.read("\n", ROOM, &mut |_| {});
        assert!(markdown.in_doubt(), "{line:?}");
    }
}

#[test]
fn a_heading_is_the_same_however_the_answer_is_cut_into_deltas() {
    for answer in [
        "Title\n=====\nafter\n",
        "**Sub**\n---\nafter\n",
        "one\ntwo\nthree\n===\n\nnext\n",
        "a | b\n---\n1 | 2\n\nx | y\n:|:\nend\n",
        "$5\n---\n",
        "\\*x\n===\n",
        "Title\n=====",
    ] {
        let whole = drawn(&standing(ended(answer)));
        let blocks_whole = blocks(answer).0;

        for cut in 0..answer.len() {
            if !answer.is_char_boundary(cut) {
                continue;
            }
            let (first, second) = answer.split_at(cut);
            assert_eq!(
                drawn(&standing(ended_in(&[first, second]))),
                whole,
                "{answer:?} cut at {cut}"
            );

            let mut markdown = Markdown::default();
            let mut cut_blocks = Vec::new();
            let mut say = |said: Said<'_>| {
                if let Said::Block(block) = said {
                    cut_blocks.push(block);
                }
            };
            markdown.read(first, ROOM, &mut say);
            markdown.read(second, ROOM, &mut say);
            markdown.finish(ROOM, &mut say);
            assert_eq!(cut_blocks, blocks_whole, "{answer:?} cut at {cut}");
        }
    }
}

#[test]
fn a_reader_that_holds_nothing_draws_a_title_and_its_underline_as_written() {
    assert_eq!(as_written("Title\n=====\n"), "Title\n=====\n");
    assert_eq!(as_written("# Title\n"), "Title\n");
}

#[test]
fn a_heading_block_is_laid_out_as_a_rule_as_wide_as_its_words() {
    let links = Links::default();
    for (block, glyphs, rule) in [
        (
            heading(Level::One, "Title\n====="),
            Glyphs::Unicode,
            "━━━━━",
        ),
        (heading(Level::Two, "Title\n---"), Glyphs::Unicode, "─────"),
        (heading(Level::One, "# Title"), Glyphs::Ascii, "====="),
        (heading(Level::Two, "## Title"), Glyphs::Ascii, "-----"),
        (heading(Level::Two, "**\n---"), Glyphs::Unicode, "──"),
    ] {
        let rows = lay(&block, ROOM, glyphs, &links);

        assert_eq!(
            rows.iter().map(Row::text).collect::<Vec<_>>(),
            [rule],
            "{block:?} in {glyphs:?}"
        );
    }
}

#[test]
fn a_heading_that_wraps_wears_a_rule_as_wide_as_its_last_row() {
    let links = Links::default();
    let block = heading(Level::One, "# aaa bbb ccc");

    let rows = lay(&block, 8, Glyphs::Unicode, &links);

    assert_eq!(rows.iter().map(Row::text).collect::<Vec<_>>(), ["━━━"]);
}

#[test]
fn a_heading_block_names_no_source_to_list() {
    assert_eq!(Kind::Heading(Level::One).named(), None);
    assert_eq!(Kind::Heading(Level::Two).named(), None);
}

const ATX_HEADINGS: &str = "# One\n\n## Two\n\n### Three\n\nA paragraph under them.\n";
const SETEXT_HEADINGS: &str = "One\n===\n\nTwo\n---\n\nA paragraph under them.\n";

#[test]
fn atx_headings_64_unicode() {
    insta::assert_snapshot!(corpus_pictures::picture(ATX_HEADINGS, 64, Glyphs::Unicode));
}

#[test]
fn atx_headings_64_ascii() {
    insta::assert_snapshot!(corpus_pictures::picture(ATX_HEADINGS, 64, Glyphs::Ascii));
}

#[test]
fn setext_headings_64_unicode() {
    insta::assert_snapshot!(corpus_pictures::picture(
        SETEXT_HEADINGS,
        64,
        Glyphs::Unicode
    ));
}

#[test]
fn setext_headings_64_ascii() {
    insta::assert_snapshot!(corpus_pictures::picture(SETEXT_HEADINGS, 64, Glyphs::Ascii));
}

const INDENTED_CODE: &str = "A block indented four spaces:\n\n    let total = 1 + 2;\n\n    \
    let message = \"a line of code long enough to wrap at sixty-four columns\";\n\nAfter it.\n";

#[test]
fn indented_code_64_unicode() {
    insta::assert_snapshot!(corpus_pictures::picture(INDENTED_CODE, 64, Glyphs::Unicode));
}

#[test]
fn indented_code_64_ascii() {
    insta::assert_snapshot!(corpus_pictures::picture(INDENTED_CODE, 64, Glyphs::Ascii));
}

// GitHub alerts: a quote whose first line is nothing but `[!KIND]`.

/// Each alert kind as written, the word it is shown as, the slot its mark
/// wears, and that mark in the unicode set and in the ascii one.
const ALERTS: [(&str, &str, Slot, &str, &str); 5] = [
    ("NOTE", "Note", Slot::Link, "◆", "*"),
    ("TIP", "Tip", Slot::Done, "✓", "+"),
    ("IMPORTANT", "Important", Slot::Accent, "■", "#"),
    ("WARNING", "Warning", Slot::Doing, "▲", "!"),
    ("CAUTION", "Caution", Slot::Trouble, "✗", "x"),
];

/// One whole answer in `glyphs`, read as these deltas and then ended.
fn ended_as(glyphs: Glyphs, deltas: &[&str]) -> Vec<(Slot, String)> {
    let mut markdown = Markdown::new(glyphs);
    let mut said = Vec::new();
    for delta in deltas {
        said.extend(read(&mut markdown, delta));
    }
    markdown.finish_runs(ROOM, &mut |slot, text, _| {
        said.push((slot, text.to_owned()));
    });
    said
}

#[test]
fn an_alert_is_a_quiet_bar_on_each_line_of_its_quote_with_its_mark_and_its_word_on_the_first() {
    for (marker, word, slot, unicode, ascii) in ALERTS {
        for (glyphs, bar, mark) in [(Glyphs::Unicode, "▎", unicode), (Glyphs::Ascii, "|", ascii)]
        {
            let said = ended_as(glyphs, &[&format!("> [!{marker}]\n> first\n> second\n")]);
            let run = |slot: Slot, text: &str| (slot, text.to_owned());

            assert_eq!(
                said,
                vec![
                    run(Slot::Quiet, bar),
                    run(Slot::Quiet, " "),
                    run(slot, mark),
                    run(Slot::Plain, " "),
                    run(Slot::Bold, word),
                    run(Slot::Plain, "\n"),
                    run(Slot::Quiet, bar),
                    run(Slot::Quiet, " "),
                    run(Slot::Plain, "first"),
                    run(Slot::Plain, "\n"),
                    run(Slot::Quiet, bar),
                    run(Slot::Quiet, " "),
                    run(Slot::Plain, "second"),
                    run(Slot::Plain, "\n"),
                ],
                "{marker} in {glyphs:?}"
            );
        }
    }
}

#[test]
fn an_alert_never_shows_its_marker() {
    for (marker, ..) in ALERTS {
        let text = drawn(&ended(&format!("> [!{marker}]\n> body\n")));

        assert!(!text.contains("[!") && !text.contains(marker), "{text:?}");
    }
}

#[test]
fn an_alert_is_known_by_its_kind_in_any_case() {
    assert_eq!(drawn(&ended("> [!tip]\n> body\n")), "▎ ✓ Tip\n▎ body\n");
    assert_eq!(
        drawn(&ended("> [!Caution]\n> body\n")),
        "▎ ✗ Caution\n▎ body\n"
    );
}

#[test]
fn an_alert_that_ends_the_message_has_its_header_and_no_break() {
    assert_eq!(drawn(&ended("> [!WARNING]")), "▎ ▲ Warning");
}

#[test]
fn an_alert_with_nothing_under_it_is_its_header_row() {
    assert_eq!(drawn(&ended("> [!NOTE]\n\nafter\n")), "▎ ◆ Note\n\nafter\n");
}

#[test]
fn an_empty_row_of_an_alert_keeps_its_bar() {
    assert_eq!(
        drawn(&ended("> [!NOTE]\n> one\n> \n> two\n")),
        "▎ ◆ Note\n▎ one\n▎ \n▎ two\n"
    );
}

#[test]
fn a_quote_with_an_unknown_kind_or_more_on_its_line_stays_a_quote() {
    for answer in [
        "> [!SECRET]\n> body\n",
        "> [!NOTES]\n> body\n",
        "> [! NOTE]\n> body\n",
        "> [!NOTE] with words after it\n> body\n",
        "> [!NOTE]x\n> body\n",
        "> [!]\n> body\n",
        "> [NOTE]\n> body\n",
        "> a [!NOTE] in a sentence\n> body\n",
    ] {
        let text = drawn(&ended(answer));

        assert_eq!(text, answer.replace("> ", "│ "), "{answer:?}");
    }
}

#[test]
fn a_marker_with_an_address_after_it_is_the_link_it_was() {
    let said = ended("> [!NOTE](https://example.com)\n");

    assert_eq!(drawn(&said), "│ !NOTE\n");
}

#[test]
fn an_alert_marker_is_the_first_line_of_its_quote_only() {
    assert_eq!(
        drawn(&ended("> some words\n> [!NOTE]\n> more\n")),
        "│ some words\n│ [!NOTE]\n│ more\n"
    );
    assert_eq!(
        drawn(&ended("> [!NOTE]\n> [!TIP]\n")),
        "▎ ◆ Note\n▎ [!TIP]\n"
    );
}

#[test]
fn a_marker_outside_a_quote_is_prose() {
    assert_eq!(drawn(&ended("[!NOTE]\nbody\n")), "[!NOTE]\nbody\n");
    assert_eq!(drawn(&ended("- [!NOTE]\n")), "• [!NOTE]\n");
    assert_eq!(
        drawn(&ended("- > [!NOTE]\n  > a\n")),
        "• > [!NOTE]\n  │ a\n"
    );
}

#[test]
fn an_alert_ends_where_its_quote_does() {
    let said = ended("> [!NOTE]\n> body\nplain again\n> second\n\n> [!TIP]\n> third\n");

    assert_eq!(
        drawn(&said),
        "▎ ◆ Note\n▎ body\nplain again\n│ second\n\n▎ ✓ Tip\n▎ third\n"
    );
}

#[test]
fn an_alert_ends_at_any_line_that_is_not_a_row_of_its_quote() {
    // A bare `>` and a `>` with no space after it are not rows of a quote, and
    // neither is a table whose first line opens with `|`, a heading's rule or
    // display math, each of them also written after spaces, where lines that
    // open with `|` are drawn as the lines they are and not as a table: none of
    // them is part of the alert, so the quote row after one is a quote of its
    // own and draws the plain bar.
    for between in [
        ">\n",
        ">a\n",
        "| a | b |\n| - | - |\n| 1 | 2 |\n",
        "Title\n===\n",
        "$$\nx^2\n$$\n",
        "  | a | b |\n  | - | - |\n",
        "  Title\n  ===\n",
        "  $$\n  x^2\n  $$\n",
    ] {
        let answer = format!("> [!NOTE]\n> a\n{between}> b\n");
        let text = drawn(&ended(&answer));

        assert!(text.starts_with("▎ ◆ Note\n▎ a\n"), "{answer:?}: {text:?}");
        assert!(text.ends_with("│ b\n"), "{answer:?}: {text:?}");
    }
}

#[test]
fn a_quote_row_straight_after_a_table_not_opened_by_a_bar_is_a_row_of_it() {
    // The one block that does not hand a quote row on as a quote of its own: a
    // table whose first line does not open with `|` goes on until a blank line,
    // so the row is one more of its rows, whatever its delimiter row opens
    // with. Such a first line is a header written without outer bars, with or
    // without spaces before it, or one written with them after a tab.
    for (answer, text) in [
        (
            "> [!NOTE]\n> a\na | b\n- | -\n1 | 2\n> b\n",
            "▎ ◆ Note\n▎ a\na   │ b\n────┼──\n1   │ 2\n│ b │  \n",
        ),
        (
            "> [!NOTE]\n> a\na | b\n| - | - |\n> b\n",
            "▎ ◆ Note\n▎ a\na   │ b\n────┼──\n│ b │  \n",
        ),
        (
            "> [!NOTE]\n> a\na | b |\n| - | - |\n> b\n",
            "▎ ◆ Note\n▎ a\na   │ b\n────┼──\n│ b │  \n",
        ),
        (
            "> [!NOTE]\n> a\n  a | b\n  - | -\n> b\n",
            "▎ ◆ Note\n▎ a\na   │ b\n────┼──\n│ b │  \n",
        ),
        (
            "> [!NOTE]\n> a\n\t| a | b |\n\t| - | - |\n> b\n",
            "▎ ◆ Note\n▎ a\na   │ b\n────┼──\n│ b │  \n",
        ),
        (
            "> [!NOTE]\n> a\na | b\n- | -\n1 | 2\n\n> b\n",
            "▎ ◆ Note\n▎ a\na │ b\n──┼──\n1 │ 2\n\n│ b\n",
        ),
    ] {
        assert_eq!(standing_text(&drawn(&ended(answer))), text, "{answer:?}");
    }
}

#[test]
fn a_row_of_an_alert_after_spaces_is_still_a_row_of_its_quote() {
    // The quote mark is the first character that is not a space, so an alert
    // under a list item, an alert indented itself and a row indented more than
    // its header each keep the alert bar on every row.
    for (answer, text) in [
        (
            "- item\n  > [!NOTE]\n  > a\n  > b\n",
            "• item\n  ▎ ◆ Note\n  ▎ a\n  ▎ b\n",
        ),
        (
            "- item\n\n  > [!NOTE]\n  > a\n  > b\n",
            "• item\n\n  ▎ ◆ Note\n  ▎ a\n  ▎ b\n",
        ),
        (
            "1. item\n   > [!NOTE]\n   > a\n",
            "1. item\n   ▎ ◆ Note\n   ▎ a\n",
        ),
        ("  > [!NOTE]\n  > a\n  > b\n", "  ▎ ◆ Note\n  ▎ a\n  ▎ b\n"),
        ("> [!NOTE]\n  > a\n", "▎ ◆ Note\n  ▎ a\n"),
        ("> [!NOTE]\n> a\n > b\n", "▎ ◆ Note\n▎ a\n ▎ b\n"),
        ("> [!NOTE]\n> a\n  > b\n", "▎ ◆ Note\n▎ a\n  ▎ b\n"),
        ("> [!NOTE]\n> a\n   > b\n", "▎ ◆ Note\n▎ a\n   ▎ b\n"),
        ("> [!NOTE]\n> a\n    > b\n", "▎ ◆ Note\n▎ a\n    ▎ b\n"),
    ] {
        assert_eq!(drawn(&ended(answer)), text, "{answer:?}");
    }
}

#[test]
fn a_marker_on_an_indented_row_of_an_ordinary_quote_stays_as_written() {
    // A marker is read on the first row of a quote only, so one on a later row
    // is text however far the row is indented, and the quote keeps its plain
    // bar.
    for (answer, text) in [
        ("> a\n  > [!NOTE]\n  > b\n", "│ a\n  │ [!NOTE]\n  │ b\n"),
        ("> a\n > [!NOTE]\n", "│ a\n │ [!NOTE]\n"),
        ("- a\n  > x\n  > [!NOTE]\n", "• a\n  │ x\n  │ [!NOTE]\n"),
    ] {
        assert_eq!(drawn(&ended(answer)), text, "{answer:?}");
    }
}

#[test]
fn an_alert_after_spaces_cut_anywhere_draws_what_the_whole_alert_draws() {
    // What is drawn, and the slot of everything that shows. Spaces are left
    // out of the slots: the indentation before a bar is said under the quote's
    // slot when the answer arrives whole and the plain one when it is cut
    // inside it, for a quote that is no alert as well, and no ink shows on it.
    let shown = |said: &[(Slot, String)]| {
        slots(
            &said
                .iter()
                .filter(|(_, text)| !text.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>(),
        )
    };

    for answer in [
        "> [!NOTE]\n> a\n  > b\n",
        "- item\n  > [!NOTE]\n  > a\n  > b\n",
        "> a\n  > [!NOTE]\n  > b\n",
        "> [!NOTE]\n> a\n  Title\n  ===\n> b\n",
    ] {
        let whole = ended(answer);

        for cut in 1..answer.len() {
            let (first, second) = answer.split_at(cut);
            let said = ended_in(&[first, second]);

            assert_eq!(drawn(&said), drawn(&whole), "{answer:?} cut at {cut}");
            assert_eq!(shown(&said), shown(&whole), "{answer:?} cut at {cut}");
        }
    }
}

#[test]
fn the_words_of_an_alert_keep_what_the_answer_gave_them() {
    let said = ended("> [!NOTE]\n> a **loud** word, `code`, [the guide](https://a.io)\n");

    assert_eq!(
        slots(&said)
            .into_iter()
            .skip_while(|slot| *slot != Slot::Bold)
            .collect::<Vec<_>>(),
        [
            Slot::Bold,
            Slot::Plain,
            Slot::Quiet,
            Slot::Plain,
            Slot::Bold,
            Slot::Plain,
            Slot::Code,
            Slot::Plain,
            Slot::Link,
            Slot::Plain
        ]
    );
    assert!(
        said.iter()
            .any(|(slot, text)| *slot == Slot::Link && text == "the guide"),
        "{said:?}"
    );
}

#[test]
fn an_alert_hangs_under_its_bar() {
    for (glyphs, bar) in [(Glyphs::Unicode, "▎ "), (Glyphs::Ascii, "| ")] {
        assert_eq!(
            hung_under("> [!NOTE]\n", glyphs),
            Some((bar.len(), 2, bar.to_owned())),
            "{glyphs:?}"
        );
    }
}

#[test]
fn an_alert_cut_anywhere_draws_what_the_whole_alert_draws() {
    // Inside its marker, between the marker and its break, and in the rows
    // under it: the marker is held until the break says it was alone on its
    // line, so a cut is never drawn as a quote that turned out to be an alert.
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        for (marker, ..) in ALERTS {
            let answer = format!("> [!{marker}]\n> body *words*\n\n> [!{marker}] no\n> after\n");
            let whole = ended_as(glyphs, &[&answer]);
            assert!(
                drawn(&whole).starts_with(&format!("{} ", glyphs.alerted())),
                "{marker} drew no alert: {whole:?}"
            );

            for cut in 1..answer.len() {
                let (first, second) = answer.split_at(cut);
                let said = ended_as(glyphs, &[first, second]);

                assert_eq!(drawn(&said), drawn(&whole), "{marker} cut at {cut}");
                assert_eq!(slots(&said), slots(&whole), "{marker} cut at {cut}");
            }
        }
    }
}

#[test]
fn nothing_of_an_alert_is_drawn_before_its_break_says_it_was_one() {
    let mut markdown = Markdown::default();

    for delta in ["> [", "!", "WARN", "ING", "]"] {
        assert_eq!(drawn(&read(&mut markdown, delta)), "", "after {delta:?}");
    }
    assert_eq!(drawn(&read(&mut markdown, "\n")), "▎ ▲ Warning\n");
}

#[test]
fn an_alert_mark_carries_its_kinds_ink_and_its_word_only_weight_in_every_theme() {
    // The first row of each kind, as the slots it was said under: a bar and a
    // mark, then the word. Everything but the mark is quiet, plain or bold,
    // which are the same in every theme by design, so whatever differs from
    // one kind to the next is the mark.
    let mut rows = Vec::new();
    for (marker, word, slot, ..) in ALERTS {
        let said = whole(&format!("> [!{marker}]\n> body\n"));
        let first: Vec<(Slot, &str)> = said
            .iter()
            .take_while(|(_, text)| text != "\n")
            .map(|(slot, text)| (*slot, text.as_str()))
            .collect();
        assert_eq!(
            first.iter().map(|(slot, _)| *slot).collect::<Vec<_>>(),
            [Slot::Quiet, Slot::Quiet, slot, Slot::Plain, Slot::Bold],
            "{marker}: {first:?}"
        );
        assert_eq!(first.last().map(|(_, text)| *text), Some(word));

        let mut row = crate::row::Row::new();
        for (slot, text) in &said {
            if text != "\n" {
                row.push(*slot, text);
            }
        }
        rows.push(row);
    }
    crate::colour_rule::holds("an alert", &rows, |_| false);

    for theme in every_theme() {
        let palette = Palette::resolve(true, theme, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        });
        let ink = |slot| palette.open(slot).as_str().to_owned();

        assert_eq!(ink(Slot::Bold), "\x1b[1m", "{theme:?}: weight and no hue");
        assert!(
            !ink(Slot::Plain).contains('3'),
            "{theme:?}: plain has a hue"
        );
        for (marker, _, slot, ..) in ALERTS {
            assert_ne!(ink(slot), ink(Slot::Plain), "{theme:?} {marker}");
            assert_ne!(ink(slot), ink(Slot::Quiet), "{theme:?} {marker}");
        }

        // Of the four kinds the design draws, it gives the marks of Note and
        // Caution a colour of their own. It draws no Important alert: the
        // accent Important's mark wears is this crate's choice. Each of those
        // three marks carries a foreground colour. The design gives Warning
        // and Tip no colour of their own: Warning is the word's weight and
        // nothing more, and Tip is a strike through the bar's quiet grey.
        for (marker, _, slot, ..) in ALERTS {
            let hued = has_hue(&ink(slot));
            match marker {
                "NOTE" | "IMPORTANT" | "CAUTION" => {
                    assert!(hued, "{theme:?} {marker} has no hue: {:?}", ink(slot));
                }
                "WARNING" => {
                    assert_eq!(ink(slot), ink(Slot::Bold), "{theme:?} {marker}");
                }
                "TIP" => assert_eq!(ink(slot), "\x1b[9;90m", "{theme:?} {marker}"),
                other => assert_eq!(other, "", "no ink is asked of the kind {other}"),
            }
        }
    }
}

/// Every theme the chain below names, in the order the enum lists them.
///
/// The match has no wildcard, so a theme added to the enum stops this
/// compiling until it has an arm. It is walked only once an arm before it
/// names it: a new theme given `None` compiles and is left out, so whoever adds
/// one puts it in the chain.
fn every_theme() -> Vec<Theme> {
    let mut every = vec![Theme::Dark];
    while let Some(next) = every.last().and_then(|theme| match theme {
        Theme::Dark => Some(Theme::Light),
        Theme::Light => Some(Theme::ColourblindDark),
        Theme::ColourblindDark => Some(Theme::ColourblindLight),
        Theme::ColourblindLight => Some(Theme::Ansi),
        Theme::Ansi => None,
    }) {
        every.push(next);
    }
    every
}

/// Whether an opening escape sets a foreground colour: one of the parameters
/// 30 to 38, which excludes the weight, the strike, the underline and the
/// quiet grey (90).
fn has_hue(open: &str) -> bool {
    open.trim_start_matches("\x1b[")
        .trim_end_matches('m')
        .split(';')
        .filter_map(|part| part.parse::<u8>().ok())
        .any(|part| (30..=38).contains(&part))
}

const ALERT_KINDS: &str = "> [!NOTE]\n> Useful information.\n\n> [!TIP]\n> A shortcut worth taking.\n\n\
> [!IMPORTANT]\n> Something that must not be missed.\n\n> [!WARNING]\n> Running `cargo clean` \
deletes the build cache; the next build takes minutes.\n\n> [!CAUTION]\n> This deletes the session \
log; it cannot be undone.\n";

#[test]
fn alerts_64_unicode() {
    insta::assert_snapshot!(corpus_pictures::picture(ALERT_KINDS, 64, Glyphs::Unicode));
}

#[test]
fn alerts_64_ascii() {
    insta::assert_snapshot!(corpus_pictures::picture(ALERT_KINDS, 64, Glyphs::Ascii));
}

const SMALL_CONSTRUCTS: &str = concat!(
    "Crucible bounds every buffer it keeps[^1] and draws nothing a model wrote as an\n",
    "escape[^2]. Press <kbd>Ctrl</kbd>+<kbd>O</kbd> to see a cut result; ",
    "H<sub>2</sub>O and x<sup>2</sup> read as written.\n\n",
    "<details>\n<summary>Why the record is bounded</summary>\n",
    "A store that grew with the session would spend the RSS budget.\n</details>\n\n",
    "[^1]: CLAUDE.md, \"Everything kept is bounded\"\n",
    "[^2]: markdown.rs, the rule the reader opens with\n",
);

#[test]
fn small_constructs_80_unicode() {
    insta::assert_snapshot!(corpus_pictures::picture(
        SMALL_CONSTRUCTS,
        80,
        Glyphs::Unicode
    ));
}

#[test]
fn small_constructs_80_ascii() {
    insta::assert_snapshot!(corpus_pictures::picture(
        SMALL_CONSTRUCTS,
        80,
        Glyphs::Ascii
    ));
}

/// The slots the runs written exactly `text` were said under, in order.
fn worn(said: &[(Slot, String)], text: &str) -> Vec<Slot> {
    said.iter()
        .filter(|(_, run)| run == text)
        .map(|(slot, _)| *slot)
        .collect()
}

/// `answer` read whole in both glyph sets and ended, as the text drawn.
fn drawn_in(glyphs: Glyphs, answer: &str) -> String {
    drawn(&ended_as(glyphs, &[answer]))
}

#[test]
fn a_footnote_reference_is_a_plain_superscript_number_and_its_definition_a_quiet_note() {
    let said = ended_as(Glyphs::Unicode, &["A claim.[^1]\n\n[^1]: the source\n"]);

    assert_eq!(drawn(&said), "A claim.¹\n\n¹ the source\n");
    assert_eq!(worn(&said, "¹"), [Slot::Plain, Slot::Quiet]);
    assert_eq!(worn(&said, " the source"), [Slot::Quiet]);
}

#[test]
fn under_ascii_a_footnote_reference_and_its_note_are_a_quiet_bracketed_number() {
    let said = ended_as(Glyphs::Ascii, &["A claim.[^1]\n\n[^1]: the source\n"]);

    assert_eq!(drawn(&said), "A claim.[1]\n\n[1] the source\n");
    assert_eq!(worn(&said, "[1]"), [Slot::Quiet, Slot::Quiet]);
    assert_eq!(worn(&said, " the source"), [Slot::Quiet]);
}

#[test]
fn a_footnote_label_is_the_same_label_in_any_letter_case() {
    let answer = "A claim[^Note], again[^NOTE] and[^other].\n\n[^note]: the note\n";
    assert_eq!(
        drawn_in(Glyphs::Unicode, answer),
        "A claim¹, again¹ and².\n\n¹ the note\n"
    );
    assert_eq!(
        drawn_in(Glyphs::Ascii, answer),
        "A claim[1], again[1] and[2].\n\n[1] the note\n"
    );
}

#[test]
fn a_note_is_its_number_one_space_and_its_text_whatever_followed_the_colon() {
    for written in ["[^1]:no space\n", "[^1]: no space\n", "[^1]:   no space\n"] {
        let said = ended_as(Glyphs::Unicode, &[written]);
        assert_eq!(drawn(&said), "¹ no space\n", "{written:?}");
        assert_eq!(worn(&said, " no space"), [Slot::Quiet], "{written:?}");
        assert_eq!(
            drawn_in(Glyphs::Ascii, written),
            "[1] no space\n",
            "{written:?}"
        );
    }
}

#[test]
fn footnotes_are_numbered_in_the_order_their_labels_first_appear() {
    // A definition after its reference takes the reference's number, and a
    // label used twice is one note.
    let answer = "a[^why] b[^ex] c[^why]\n\n[^ex]: example\n[^why]: reason\n";

    assert_eq!(
        drawn_in(Glyphs::Unicode, answer),
        "a¹ b² c¹\n\n² example\n¹ reason\n"
    );
    assert_eq!(
        drawn_in(Glyphs::Ascii, answer),
        "a[1] b[2] c[1]\n\n[2] example\n[1] reason\n"
    );
}

#[test]
fn a_footnote_reference_with_no_definition_and_a_definition_with_no_reference_are_both_drawn() {
    // Nothing looks ahead, so a reference is drawn as the number it is given
    // whether or not its definition ever arrives.
    assert_eq!(drawn_in(Glyphs::Unicode, "a[^gone] b\n"), "a¹ b\n");
    assert_eq!(
        drawn_in(Glyphs::Unicode, "[^alone]: a note\n"),
        "¹ a note\n"
    );
}

#[test]
fn a_second_definition_of_one_label_is_shown_as_written() {
    assert_eq!(
        drawn_in(Glyphs::Unicode, "[^1]: one\n[^1]: two\n"),
        "¹ one\n[^1]: two\n"
    );
}

#[test]
fn a_footnote_past_the_cap_is_shown_as_written() {
    let cited = (1..=FOOTNOTE_CAP)
        .map(|n| format!("[^{n}]"))
        .collect::<Vec<_>>()
        .concat();
    let answer = format!(
        "{cited}[^{}][^1]\n\n[^{}]: late\n",
        FOOTNOTE_CAP + 1,
        FOOTNOTE_CAP + 2
    );

    let drawn = drawn_in(Glyphs::Ascii, &answer);
    let numbered = (1..=FOOTNOTE_CAP)
        .map(|n| format!("[{n}]"))
        .collect::<Vec<_>>()
        .concat();
    assert_eq!(
        drawn,
        format!(
            "{numbered}[^{}][1]\n\n[^{}]: late\n",
            FOOTNOTE_CAP + 1,
            FOOTNOTE_CAP + 2
        )
    );
}

#[test]
fn a_footnote_label_longer_than_the_label_cap_is_shown_as_written() {
    let longest = "x".repeat(FOOTNOTE_LABEL_CAP);
    let longer = "x".repeat(FOOTNOTE_LABEL_CAP + 1);

    assert_eq!(
        drawn_in(Glyphs::Ascii, &format!("a[^{longest}]\n")),
        "a[1]\n"
    );
    assert_eq!(
        drawn_in(Glyphs::Ascii, &format!("a[^{longer}]\n")),
        format!("a[^{longer}]\n")
    );
}

#[test]
fn a_bracket_that_is_almost_a_footnote_is_shown_as_written() {
    for written in ["[^]", "[^a b]", "[^\u{1b}]", "[ ^1]", "[^1"] {
        let answer = format!("a {written} b\n");
        assert_eq!(drawn_in(Glyphs::Unicode, &answer), answer, "{written:?}");
    }

    // A link's words that open with a caret are still a link's.
    let said = ended_as(Glyphs::Unicode, &["a [^1](https://example.test/) b\n"]);
    assert_eq!(drawn(&said), "a ^1 b\n");
    assert_eq!(worn(&said, "^1"), [Slot::Link]);
}

#[test]
fn a_footnote_is_shown_as_written_in_a_heading_a_span_of_code_a_fence_and_a_table_cell() {
    let heading = ended_as(Glyphs::Unicode, &["# Title[^1]\n"]);
    assert!(drawn(&heading).starts_with("Title[^1]\n"), "{heading:?}");

    let code = ended_as(Glyphs::Unicode, &["a `[^1]` b\n"]);
    assert_eq!(worn(&code, "[^1]"), [Slot::Code]);

    let fence = drawn_in(Glyphs::Unicode, "```\n[^1]: x\n```\n");
    assert!(fence.contains("[^1]: x"), "{fence:?}");

    let table = drawn_in(Glyphs::Unicode, "| a[^1] |\n|---|\n| b |\n");
    assert!(table.contains("a[^1]"), "{table:?}");
}

#[test]
fn a_footnote_is_read_in_a_quote_a_list_item_and_emphasis_where_it_is_only_ever_a_reference() {
    // A definition is a line that opens with its label, so after a quote's
    // mark or a bullet the label is a reference and the colon is prose.
    assert_eq!(drawn_in(Glyphs::Ascii, "> a[^1]\n"), "| a[1]\n");
    assert_eq!(drawn_in(Glyphs::Ascii, "> [^1]: x\n"), "| [1]: x\n");
    assert_eq!(drawn_in(Glyphs::Ascii, "- [^1]: x\n"), "- [1]: x\n");

    let said = ended_as(Glyphs::Unicode, &["**bold[^1]**\n"]);
    assert_eq!(drawn(&said), "bold¹\n");
    assert_eq!(worn(&said, "¹"), [Slot::Bold]);
}

#[test]
fn a_kbd_tag_draws_its_key_in_the_code_slot_bare_and_in_brackets_under_ascii() {
    let answer = "Press <kbd>Ctrl</kbd>+<kbd>O</kbd>.\n";

    let unicode = ended_as(Glyphs::Unicode, &[answer]);
    assert_eq!(drawn(&unicode), "Press Ctrl+O.\n");
    assert_eq!(worn(&unicode, "Ctrl"), [Slot::Code]);
    assert_eq!(worn(&unicode, "O"), [Slot::Code]);

    let ascii = ended_as(Glyphs::Ascii, &[answer]);
    assert_eq!(drawn(&ascii), "Press [Ctrl]+[O].\n");
    assert_eq!(worn(&ascii, "[Ctrl]"), [Slot::Code]);
    assert_eq!(worn(&ascii, "[O]"), [Slot::Code]);
}

#[test]
fn what_a_kbd_tag_holds_is_drawn_as_written_in_the_code_slot() {
    let inside = "*a* &amp; `b` [c](d) \\x <sub>2</sub> [^1]";
    let said = ended_as(Glyphs::Unicode, &[&format!("<kbd>{inside}</kbd>\n")]);

    assert_eq!(drawn(&said), format!("{inside}\n"));
    assert!(
        said.iter()
            .filter(|(_, text)| text != "\n")
            .all(|(slot, _)| *slot == Slot::Code),
        "{said:?}"
    );
}

#[test]
fn a_kbd_tag_left_open_ends_with_its_line() {
    assert_eq!(drawn_in(Glyphs::Ascii, "a <kbd>b\nc\n"), "a [b]\nc\n");
    let said = ended_as(Glyphs::Unicode, &["a <kbd>b\nc\n"]);
    assert_eq!(worn(&said, "c"), [Slot::Plain]);
}

#[test]
fn a_tag_is_read_in_any_letter_case_with_attributes_and_self_closed() {
    assert_eq!(drawn_in(Glyphs::Ascii, "<KBD>x</Kbd>\n"), "[x]\n");
    assert_eq!(
        drawn_in(Glyphs::Unicode, "a<br/>b<br />c<BR>d\n"),
        "a\nb\nc\nd\n"
    );
    assert_eq!(
        drawn_in(
            Glyphs::Unicode,
            "<details open>\n<summary class=\"x\">S</summary>\nbody\n</details>\n"
        ),
        "▸ S\n  body\n"
    );
}

#[test]
fn sub_and_superscript_runs_are_quiet_unicode_characters_in_both_glyph_sets() {
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let said = ended_as(glyphs, &["H<sub>2</sub>O and x<sup>n+1</sup>\n"]);
        assert_eq!(drawn(&said), "H₂O and xⁿ⁺¹\n", "{glyphs:?}");
        assert_eq!(worn(&said, "₂"), [Slot::Quiet], "{glyphs:?}");
        assert_eq!(worn(&said, "ⁿ⁺¹"), [Slot::Quiet], "{glyphs:?}");
    }
}

#[test]
fn a_script_run_with_a_character_that_has_no_such_form_is_drawn_as_its_plain_text() {
    // Whole or not at all: a run is never half converted.
    let said = ended_as(
        Glyphs::Unicode,
        &["the 1<sup>st</sup> and H<sub>2 a</sub>\n"],
    );
    assert_eq!(drawn(&said), "the 1st and H2 a\n");
    assert_eq!(slots(&said), [Slot::Plain]);
}

#[test]
fn a_script_run_that_is_empty_crossed_unclosed_or_past_its_cap_is_drawn_as_its_plain_text() {
    assert_eq!(drawn_in(Glyphs::Unicode, "x<sup></sup>y\n"), "xy\n");
    assert_eq!(drawn_in(Glyphs::Unicode, "x<sub>2</sup>y\n"), "x2y\n");
    assert_eq!(drawn_in(Glyphs::Unicode, "x<sup>2\ny\n"), "x2\ny\n");
    assert_eq!(drawn_in(Glyphs::Unicode, "x<sup>2"), "x2");
    assert_eq!(
        drawn_in(Glyphs::Unicode, "<sub><sup>2</sup></sub>\n"),
        "²\n"
    );

    let long = "1".repeat(SCRIPT_CAP + 1);
    assert_eq!(
        drawn_in(Glyphs::Unicode, &format!("x<sup>{long}</sup>y\n")),
        format!("x{long}y\n")
    );
}

#[test]
fn a_tag_crossed_with_a_kbd_tag_is_a_character_of_the_key() {
    let said = ended_as(Glyphs::Unicode, &["<kbd><sub></kbd></sub>2\n"]);
    assert_eq!(drawn(&said), "<sub>2\n");
    assert_eq!(worn(&said, "<sub>"), [Slot::Code]);
}

#[test]
fn a_line_break_tag_breaks_the_line_and_a_quote_keeps_its_bar_on_the_row_after_it() {
    assert_eq!(drawn_in(Glyphs::Unicode, "one<br>two\n"), "one\ntwo\n");
    assert_eq!(drawn_in(Glyphs::Ascii, "> one<br>two\n"), "| one\n| two\n");
}

#[test]
fn a_line_break_tag_with_nothing_after_it_on_its_line_breaks_it_once() {
    assert_eq!(drawn_in(Glyphs::Unicode, "one<br>\ntwo\n"), "one\ntwo\n");
    assert_eq!(drawn_in(Glyphs::Unicode, "one<br>"), "one");
    assert_eq!(
        drawn_in(Glyphs::Unicode, "one<br><br>two\n"),
        "one\n\ntwo\n"
    );
    assert_eq!(drawn_in(Glyphs::Unicode, "one<br>[two\n"), "one\n[two\n");
}

#[test]
fn a_line_break_tag_in_a_heading_or_a_table_cell_is_a_space() {
    let heading = drawn_in(Glyphs::Unicode, "# one<br>two\n");
    assert!(heading.starts_with("one two\n"), "{heading:?}");

    let table = drawn_in(Glyphs::Unicode, "| a<br>b |\n|---|\n| c |\n");
    assert!(table.contains("a b"), "{table:?}");
}

#[test]
fn a_line_a_break_or_details_tag_has_broken_is_neither_a_bare_tables_header_nor_a_title() {
    // The row the break ended is out and cannot be taken back, so the line is
    // prose and the line under it is read as the line it would be after one.
    assert_eq!(
        drawn_in(Glyphs::Unicode, "a<br>b | c\n- | -\n"),
        "a\nb | c\n• | -\n"
    );
    assert_eq!(drawn_in(Glyphs::Unicode, "a<br>b\n===\n"), "a\nb\n===\n");
    assert_eq!(
        drawn_in(Glyphs::Unicode, "<details> | c\n- | -\n"),
        "   | c\n• | -\n"
    );
}

#[test]
fn a_details_tag_draws_its_summary_bold_behind_a_quiet_mark_and_its_body_two_columns_in() {
    let answer = "<details><summary>Why it is</summary>\nthe body\n</details>\nafter\n";

    let unicode = ended_as(Glyphs::Unicode, &[answer]);
    assert_eq!(drawn(&unicode), "▸ Why it is\n  the body\nafter\n");
    assert_eq!(worn(&unicode, "▸"), [Slot::Quiet]);
    assert_eq!(worn(&unicode, "Why it is"), [Slot::Bold]);

    let ascii = ended_as(Glyphs::Ascii, &[answer]);
    assert_eq!(drawn(&ascii), "Details: Why it is\n  the body\nafter\n");
    assert_eq!(worn(&ascii, "Details:"), [Slot::Quiet]);
    assert_eq!(worn(&ascii, "Why it is"), [Slot::Bold]);

    // A body line wider than the window, and the row a `<br>` breaks it onto,
    // hang under the two columns on every row they fold onto.
    let wide = "a body line long enough that it has to wrap onto another row";
    for body in [wide.to_owned(), format!("the body<br>{wide}")] {
        let answer = format!("<details><summary>Why</summary>\n{body}\n</details>\nafter\n");
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let rows: Vec<String> = corpus_pictures::laid(&answer, 30, glyphs)
                .iter()
                .map(Row::text)
                .collect();
            let body = rows
                .get(1..rows.len().saturating_sub(1))
                .unwrap_or_default();
            assert!(body.len() > 2, "{glyphs:?} drew {rows:#?}");
            for row in body {
                let indent = row.len() - row.trim_start().len();
                assert_eq!(indent, 2, "{glyphs:?} drew {row:?} in {rows:#?}");
            }
            assert_eq!(rows.last().map(String::as_str), Some("after"), "{rows:#?}");
        }
    }
}

#[test]
fn a_closing_details_tag_alone_on_its_line_ends_a_bare_table_and_is_never_one_of_its_rows() {
    let table = "<details>\n<summary>S</summary>\n\na | b\n--|--\n1 | 2\n</details>";
    for answer in [
        format!("{table}\nafter\n"),
        format!("{table}\n"),
        table.to_owned(),
    ] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let rows: Vec<String> = corpus_pictures::laid(&answer, 40, glyphs)
                .iter()
                .map(|row| row.text().trim_end().to_owned())
                .collect();
            assert!(
                rows.iter().all(|row| !row.contains("details")),
                "{glyphs:?} drew {rows:#?} for {answer:?}"
            );
            assert!(
                rows.iter().any(|row| row.contains('2')),
                "{glyphs:?} drew {rows:#?} for {answer:?}"
            );
            if answer.contains("after") {
                assert_eq!(rows.last().map(String::as_str), Some("after"), "{rows:#?}");
            }

            for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
                let pieces = [&answer[..cut], &answer[cut..]];
                assert_eq!(
                    drawn(&ended_as(glyphs, &pieces)),
                    drawn(&ended_as(glyphs, &[&answer])),
                    "{glyphs:?} cut at {cut} of {answer:?}"
                );
            }
        }
    }
}

#[test]
fn a_list_quote_heading_table_fence_or_formula_inside_details_stays_at_the_margin() {
    // The paragraph after each is set in, so the block is still open there.
    for (block, word) in [
        ("- item", "item"),
        ("> quoted", "quoted"),
        ("# Head", "Head"),
        ("| a | b |\n| - | - |\n| 1 | 2 |", "1"),
        ("```\ncode\n```", "code"),
        ("$$\nx\n$$", "x"),
    ] {
        let answer = format!("<details>\n{block}\n\nbody\n</details>\n");
        let drawn = drawn_in(Glyphs::Ascii, &answer);
        let (body, rest): (Vec<&str>, Vec<&str>) = drawn.lines().partition(|row| *row == "  body");
        assert_eq!(body, ["  body"], "{answer:?} drew {drawn:?}");
        assert!(rest.iter().all(|row| !row.starts_with(' ')), "{drawn:?}");
        assert!(rest.iter().any(|row| row.contains(word)), "{drawn:?}");
    }
}

#[test]
fn details_left_open_run_to_the_end_of_the_message_and_the_first_close_ends_nested_ones() {
    assert_eq!(
        drawn_in(Glyphs::Unicode, "<details>\na\n\nb\n"),
        "  a\n\n  b\n"
    );
    assert_eq!(
        drawn_in(Glyphs::Unicode, "<details>\n<details>\na\n</details>\nb\n"),
        "  a\nb\n"
    );
}

#[test]
fn other_tags_and_near_misses_of_the_six_are_shown_as_written() {
    let answer = "An <article> tag, a <b>bold</b>, <detail>, <kbdx>, <subs>, <br-x>, a < b, a<3\n";
    assert_eq!(drawn_in(Glyphs::Unicode, answer), answer);

    // An address in angle brackets is no tag: it is read as the address it is,
    // without its brackets.
    let said = ended_as(Glyphs::Unicode, &["<https://example.test/a>\n"]);
    assert_eq!(drawn(&said), "https://example.test/a\n");
    assert_eq!(worn(&said, "https://example.test/a"), [Slot::Link]);
}

#[test]
fn a_tag_longer_than_the_tag_cap_is_shown_as_written() {
    let attribute = "x".repeat(TAG_CAP);
    let answer = format!("<kbd data=\"{attribute}\">k</kbd>\n");
    // The opening tag is written out; its closing one, with nothing open, is
    // read away.
    assert_eq!(
        drawn_in(Glyphs::Ascii, &answer),
        format!("<kbd data=\"{attribute}\">k\n")
    );
}

#[test]
fn the_tag_cap_counts_characters_so_wide_ones_inside_a_tag_spend_one_each() {
    // `<kbd data="` and `">` are thirteen characters of the tag.
    let fits = "é".repeat(TAG_CAP - 13);
    assert_eq!(
        drawn_in(Glyphs::Ascii, &format!("<kbd data=\"{fits}\">k</kbd>\n")),
        "[k]\n"
    );

    let over = "é".repeat(TAG_CAP - 12);
    assert_eq!(
        drawn_in(Glyphs::Ascii, &format!("<kbd data=\"{over}\">k</kbd>\n")),
        format!("<kbd data=\"{over}\">k\n")
    );
}

#[test]
fn a_bracket_inside_a_quoted_attribute_value_neither_ends_nor_spoils_the_tag() {
    for value in ["\"a>b\"", "'a>b'", "\"a<b\"", "'a<b'"] {
        let answer = format!("<kbd title={value}>K</kbd>\n");
        assert_eq!(drawn_in(Glyphs::Ascii, &answer), "[K]\n", "{answer:?}");
    }

    // A quote left open runs to the end of the line, so the tag never ends
    // and is shown as written.
    let open = "<kbd title=\"a>K</kbd>\n";
    assert_eq!(drawn_in(Glyphs::Ascii, open), open);
}

#[test]
fn a_tag_is_shown_as_written_in_a_span_of_code_and_a_fence() {
    let said = ended_as(Glyphs::Unicode, &["`<kbd>x</kbd>`\n"]);
    assert_eq!(worn(&said, "<kbd>x</kbd>"), [Slot::Code]);

    let fence = drawn_in(Glyphs::Unicode, "```\n<br>\n```\n");
    assert!(fence.contains("<br>"), "{fence:?}");
}

#[test]
fn a_line_of_tags_alone_written_with_spaces_in_front_is_an_empty_row_whole_or_cut() {
    // The spaces go out as they arrive when the answer is cut in front of the
    // tag, so the row they are on stands; whole, it stands as well. Rows are
    // compared without their trailing spaces, which are all that differs.
    let rows = |drawn: &str| -> Vec<String> {
        drawn.lines().map(|row| row.trim_end().to_owned()).collect()
    };
    for (answer, expected) in [
        ("b\n   <details>\nc\n", ["b", "", "  c"].as_slice()),
        ("# h\n   <details>\nc\n", &["h", "━", "", "  c"]),
        ("> q\n   <details>\nc\n", &["│ q", "", "  c"]),
        ("b\n\n   <details>\nc\n", &["b", "", "", "  c"]),
    ] {
        assert_eq!(
            rows(&drawn_in(Glyphs::Unicode, answer)),
            expected,
            "{answer:?} whole"
        );
        let pieces: Vec<String> = answer.chars().map(String::from).collect();
        let pieces: Vec<&str> = pieces.iter().map(String::as_str).collect();
        let cut = drawn(&ended_as(Glyphs::Unicode, &pieces));
        assert_eq!(
            rows(&cut),
            expected,
            "{answer:?} cut before every character"
        );
    }
}

#[test]
fn an_answer_cut_anywhere_draws_the_footnotes_and_tags_the_whole_answer_draws() {
    // Inside a footnote's label, inside each tag, and between a definition's
    // bracket and its colon: each is held until the character that decides it.
    let answer = "A claim[^1] with <kbd>Ctrl</kbd>, H<sub>2</sub>O<br>and x<sup>2</sup>.\n\n\
                  <details><summary>Why</summary>\nthe body\n</details>\n\n[^1]: the note\n";
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let whole = ended_as(glyphs, &[answer]);
        assert!(
            !drawn(&whole).contains('<'),
            "{glyphs:?} drew a tag: {whole:?}"
        );
        assert!(
            !drawn(&whole).contains("[^"),
            "{glyphs:?} drew a label: {whole:?}"
        );

        for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
            let (first, second) = answer.split_at(cut);
            let said = ended_as(glyphs, &[first, second]);

            assert_eq!(drawn(&said), drawn(&whole), "{glyphs:?} cut at {cut}");
            assert_eq!(slots(&said), slots(&whole), "{glyphs:?} cut at {cut}");
        }
    }
}

/// Where an answer's links point, as one answer cut into these deltas draws
/// them.
fn pointing_in(deltas: &[&str]) -> Vec<(Slot, String, Option<String>)> {
    let mut markdown = Markdown::default();
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    for delta in deltas {
        markdown.read_runs(delta, ROOM, &mut into);
    }
    markdown.finish_runs(ROOM, &mut into);
    said
}

#[test]
fn a_reference_link_defined_earlier_reads_as_its_words() {
    let said =
        pointing("[docs]: https://example.test/d\nsee [the guide][docs], [docs][] and [Docs].\n");

    assert_eq!(
        wrote(&said),
        "https://example.test/d\nsee the guide, docs and Docs.\n"
    );
    assert_eq!(
        points(&said),
        [
            ("https://example.test/d", "https://example.test/d"),
            ("the guide", "https://example.test/d"),
            ("docs", "https://example.test/d"),
            ("Docs", "https://example.test/d"),
        ]
    );
}

#[test]
fn a_reference_link_is_read_in_a_quote_a_list_item_and_emphasis() {
    let said = pointing("[d]: https://example.test/d\n> a [d]\n- a *[d][d]*\n");

    assert_eq!(wrote(&said), "https://example.test/d\n│ a d\n• a d\n");
    assert_eq!(
        points(&said)
            .iter()
            .filter(|(text, _)| *text == "d")
            .count(),
        2
    );
}

#[test]
fn the_first_definition_of_a_label_wins() {
    let said = pointing("[a]: https://one.test\n[A]: https://two.test\nsee [a]\n");

    assert_eq!(wrote(&said), "https://one.test\nhttps://two.test\nsee a\n");
    assert_eq!(
        points(&said),
        [
            ("https://one.test", "https://one.test"),
            ("https://two.test", "https://two.test"),
            ("a", "https://one.test"),
        ]
    );
}

#[test]
fn brackets_with_no_earlier_definition_stay_as_written() {
    for written in [
        "[TODO] fix this\n",
        "an array is arr[0] and no more\n",
        "see [guide][g] and [guide][] and [guide]\n",
        "[x]: not an address\n",
        "[x]: https://example.test is an address and some prose\n",
        "[x]:\n",
    ] {
        let said = pointing(written);
        assert_eq!(wrote(&said), written, "{written:?}");
        assert!(
            points(&said)
                .iter()
                .all(|(text, _)| text.starts_with("http")),
            "{written:?} linked {:?}",
            points(&said)
        );
    }

    // A definition after its reference ties nothing up: the reference was
    // drawn before the definition was read.
    let said = pointing("see [guide]\n\n[guide]: https://example.test/g\n");
    assert!(wrote(&said).starts_with("see [guide]\n\n"), "{said:?}");
    assert_eq!(
        points(&said),
        [("https://example.test/g", "https://example.test/g")]
    );
}

#[test]
fn a_label_nobody_defined_stays_written_beside_one_somebody_did() {
    let said = pointing("[a]: https://example.test/a\nsee [b] and [b][a] and [a][b]\n");

    assert_eq!(
        wrote(&said),
        "https://example.test/a\nsee [b] and b and [a][b]\n"
    );
}

#[test]
fn a_definition_that_is_no_single_address_is_shown_as_written() {
    // A title or prose after the address, an address in angle brackets and a
    // target that is no address make no definition: the line is as it was
    // before definitions were read, and nothing is tied to it.
    for written in [
        "[a]: https://example.test/a \"The title\"\nsee [a]\n",
        "[a]: some prose\nsee [a]\n",
        "[a]: https://\nsee [a]\n",
        "[a]:https://example.test/a extra\nsee [a]\n",
    ] {
        assert_eq!(wrote(&pointing(written)), written, "{written:?}");
    }

    // The same line with its blanks only is one.
    let said = pointing("[a]:   https://example.test/a  \t\nsee [a]\n");
    assert_eq!(wrote(&said), "https://example.test/a\nsee a\n");
}

#[test]
fn a_definition_line_keeps_the_markup_a_line_that_is_not_one_would_have() {
    let said = pointing("[TODO]: **fix** the `thing`\n");

    assert_eq!(wrote(&said), "[TODO]: fix the thing\n");
    assert_eq!(
        slots_of(&said),
        [
            Slot::Plain,
            Slot::Bold,
            Slot::Plain,
            Slot::Code,
            Slot::Plain
        ]
    );
}

/// The slots a run of `pointing`'s said, with runs of the same one collapsed.
fn slots_of(said: &[(Slot, String, Option<String>)]) -> Vec<Slot> {
    said.iter().fold(Vec::new(), |mut worn, (slot, ..)| {
        if worn.last() != Some(slot) {
            worn.push(*slot);
        }
        worn
    })
}

#[test]
fn a_definition_past_the_definition_cap_links_nothing_and_is_shown_as_written() {
    let defined: Vec<String> = (0..=DEFINITION_CAP)
        .map(|at| format!("[l{at}]: https://example.test/{at}\n"))
        .collect();
    let answer = format!("{}see [l0] and [l{DEFINITION_CAP}]\n", defined.concat());

    let said = pointing(&answer);
    let last = format!("[l{DEFINITION_CAP}]: https://example.test/{DEFINITION_CAP}\n");
    assert!(wrote(&said).contains(&last), "{said:?}");
    assert!(
        wrote(&said).ends_with(&format!("see l0 and [l{DEFINITION_CAP}]\n")),
        "{said:?}"
    );
    assert!(
        points(&said)
            .iter()
            .any(|(text, link)| *text == "l0" && *link == "https://example.test/0")
    );
    assert!(
        points(&said)
            .iter()
            .all(|(text, _)| *text != format!("l{DEFINITION_CAP}")),
        "{said:?}"
    );
}

#[test]
fn definitions_past_the_byte_cap_link_nothing_and_are_shown_as_written() {
    let path = "p".repeat(1000);
    let each = DEFINITION_BYTES / 1000 + 1;
    let defined: Vec<String> = (0..each)
        .map(|at| format!("[b{at}]: https://example.test/{path}\n"))
        .collect();
    let answer = format!("{}see [b0] and [b{}]\n", defined.concat(), each - 1);

    let said = pointing(&answer);
    let last = format!("[b{}]: https://example.test/{path}\n", each - 1);
    assert!(
        wrote(&said).contains(&last),
        "past the cap it is written out"
    );
    assert!(
        wrote(&said).ends_with(&format!("see b0 and [b{}]\n", each - 1)),
        "the first is tied and the last is not"
    );
}

#[test]
fn an_angle_bracket_address_drops_its_brackets_from_text_and_link() {
    let said = pointing(
        "see <https://example.test/a>, <http://example.test/b?x=1> and <me@example.test>.\n",
    );

    assert_eq!(
        wrote(&said),
        "see https://example.test/a, http://example.test/b?x=1 and me@example.test.\n"
    );
    assert_eq!(
        points(&said),
        [
            ("https://example.test/a", "https://example.test/a"),
            ("http://example.test/b?x=1", "http://example.test/b?x=1"),
            ("me@example.test", "mailto:me@example.test"),
        ]
    );
}

#[test]
fn an_angle_bracket_that_is_no_address_is_shown_as_written() {
    for written in [
        "<https://>\n",
        "<https://x y>\n",
        "<https://x\n",
        "<me@>\n",
        "<@example.test>\n",
        "<a@b@c.test>\n",
        "<a b@c.test>\n",
        "<foo>\n",
        "a < b and c > d\n",
        "Vec<String> and x<y\n",
        "<<https\n",
        "<sub@\n",
    ] {
        let said = pointing(written);
        assert_eq!(wrote(&said), written, "{written:?}");
        assert!(
            points(&said)
                .iter()
                .all(|(_, link)| !link.starts_with("mailto:")),
            "{written:?} linked {:?}",
            points(&said)
        );
    }

    // What the line held is still read as the line would have been: the
    // address inside it is as bare as any other.
    let said = pointing("<https://x.test/a and more\n");
    assert_eq!(points(&said), [("https://x.test/a", "https://x.test/a")]);
}

#[test]
fn an_angle_bracket_address_is_shown_as_written_in_code_and_inside_a_key_or_script() {
    for written in [
        "`<https://x.test>`\n",
        "```\n<https://x.test>\n```\n",
        "<kbd><https://x.test></kbd>\n",
    ] {
        let said = pointing(written);
        assert!(wrote(&said).contains("https://x.test"), "{written:?}");
        assert!(
            points(&said).iter().all(|(_, link)| !link.is_empty()),
            "{written:?}"
        );
        assert!(
            points(&said).iter().all(|(text, _)| !text.contains('>')),
            "{written:?}"
        );
    }
    assert!(wrote(&pointing("`<https://x.test>`\n")).starts_with("<https://x.test>"));
}

#[test]
fn an_angle_bracket_address_past_the_most_a_hold_may_keep_is_shown_as_written() {
    let long = format!("<https://example.test/{}>\n", "a".repeat(MOST));
    let said = pointing(&long);

    assert_eq!(wrote(&said), long);
    assert!(points(&said).iter().all(|(text, _)| !text.contains('>')));
}

#[test]
fn a_www_address_in_prose_links_to_its_https_address() {
    let said = pointing("see www.example.test/docs, (www.example.test) and\nwww.example.test.\n");

    assert_eq!(
        wrote(&said),
        "see www.example.test/docs, (www.example.test) and\nwww.example.test.\n"
    );
    assert_eq!(
        points(&said),
        [
            ("www.example.test/docs", "https://www.example.test/docs"),
            ("www.example.test", "https://www.example.test"),
            ("www.example.test", "https://www.example.test"),
        ]
    );
}

#[test]
fn a_word_that_only_looks_like_a_www_address_is_left_where_it_was() {
    for written in [
        "the www is wide\n",
        "awww.example.test\n",
        "see www. and www.-x and www\n",
        "x.www.example.test\n",
        "`www.example.test` and [www.example.test](https://other.test)\n",
    ] {
        let said = pointing(written);
        assert!(
            points(&said)
                .iter()
                .all(|(text, link)| !text.starts_with("www") || *link == "https://other.test"),
            "{written:?} linked {:?}",
            points(&said)
        );
    }
    assert_eq!(
        wrote(&pointing("awww.example.test\n")),
        "awww.example.test\n"
    );
}

#[test]
fn an_answer_cut_anywhere_draws_the_reference_links_and_angle_addresses_the_whole_answer_draws() {
    // Inside a definition's label, between its bracket and its colon, inside a
    // reference's two labels, and inside each angle address.
    let answer = "[Docs]: https://example.test/d\n\
                  see [the guide][docs] and [docs][] and [docs] and [no][docs.\n\
                  at <https://example.test/a>, <me@example.test>, <https://x y> and www.example.test.\n";
    let whole = pointing_in(&[answer]);
    assert!(
        points(&whole)
            .iter()
            .any(|(text, link)| { *text == "the guide" && *link == "https://example.test/d" }),
        "{whole:?}"
    );
    assert!(
        points(&whole)
            .iter()
            .any(|(_, link)| *link == "mailto:me@example.test")
    );

    for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
        let (first, second) = answer.split_at(cut);
        let said = pointing_in(&[first, second]);
        assert_eq!(wrote(&said), wrote(&whole), "cut at {cut}");
        assert_eq!(points(&said), points(&whole), "cut at {cut}");
        assert_eq!(slots_of(&said), slots_of(&whole), "cut at {cut}");
    }

    let mut markdown = Markdown::default();
    let mut said = Vec::new();
    let mut into = |slot, text: &str, link: Option<&str>| {
        said.push((slot, text.to_owned(), link.map(str::to_owned)));
    };
    for character in answer.chars() {
        let mut delta = [0; 4];
        markdown.read_runs(character.encode_utf8(&mut delta), ROOM, &mut into);
    }
    markdown.finish_runs(ROOM, &mut into);
    assert_eq!(wrote(&said), wrote(&whole));
    assert_eq!(points(&said), points(&whole));
}

/// The blocks and the standing text of an answer arriving as these deltas.
fn headed_in(deltas: &[&str]) -> (Vec<Block>, String) {
    let mut markdown = Markdown::default();
    let (mut blocks, mut runs) = (Vec::new(), String::new());
    let mut say = |said: Said<'_>| match said {
        Said::Run { text, .. } => runs.push_str(text),
        Said::Block(block) => blocks.push(block),
        Said::Retract => runs.push_str(RETRACTED),
    };
    for delta in deltas {
        markdown.read(delta, ROOM, &mut say);
    }
    markdown.finish(ROOM, &mut say);
    (blocks, standing_text(&runs))
}

#[test]
fn a_title_or_a_header_keeps_every_character_of_the_address_or_bracket_it_held() {
    // The block is laid out from the source the line was written as, so what
    // a held address, definition or bracket took must be in it, however the
    // answer is cut. Each answer names its last block: a heading over the
    // lines after the first `skip`, or the table that is the whole answer.
    let headings = [
        ("See <https://a.test/x> now\n===\n", Level::One, 0),
        ("[a]: https://a.test/x\n===\n", Level::One, 0),
        ("<me@a.test> now\n---\n", Level::Two, 0),
        ("# See <https://a b c>\n", Level::One, 0),
        ("See <https://a b c>\n===\n", Level::One, 0),
        ("[a]: https://a.test/x and\n---\n", Level::Two, 0),
        ("[d]: https://d.test\n[a][b] [c][d\n===\n", Level::One, 1),
        ("[a]: see <https://x.test/p> now\n===\n", Level::One, 0),
        ("[1]: <https://x.test/p>\n---\n", Level::Two, 0),
        ("[a]: x <yy z\n===\n", Level::One, 0),
    ];
    let tables = [
        "<https://a.test/x> | b\n--- | ---\n1 | 2\n",
        "[a]: https://a.test/x | b\n--- | ---\n1 | 2\n",
        "<https://a b | c\n--- | ---\n1 | 2\n",
        "[a]: <https://x.test/p> | b\n--- | ---\n1 | 2\n",
        "[a]: me <me@a.test> | b\n--- | ---\n1 | 2\n",
        "[a]: x <yy | z\n--- | ---\n1 | 2\n",
    ];
    let wanted = headings
        .map(|(answer, level, skip)| {
            let lines: Vec<&str> = answer.lines().skip(skip).collect();
            (answer, heading(level, &lines.join("\n")))
        })
        .into_iter()
        .chain(tables.map(|answer| {
            let kind = Kind::Table;
            let source = answer.to_owned();
            (
                answer,
                Block {
                    kind,
                    source,
                    closed: true,
                },
            )
        }));
    for (answer, block) in wanted {
        let whole = headed_in(&[answer]);
        assert_eq!(whole.0.last(), Some(&block), "{answer:?}");
        for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
            let (first, second) = answer.split_at(cut);
            assert_eq!(
                headed_in(&[first, second]),
                whole,
                "{answer:?} cut at {cut}"
            );
        }
    }
}

#[test]
fn a_link_or_a_footnote_reference_after_brackets_reads_as_it_does_with_no_definition_above() {
    for written in [
        "see [a][b](https://x.test/p) now\n",
        "A claim [x][^1].\n\n[^1]: note\n",
        "see [a][b] and [c][^1]\n\n[^1]: note\n",
    ] {
        let with = pointing(&format!("[d]: https://d.test\n{written}"));
        let without = pointing(written);
        assert_eq!(
            wrote(&with).strip_prefix("https://d.test\n"),
            Some(wrote(&without).as_str()),
            "{written:?}"
        );
        assert_eq!(
            points(&with)
                .into_iter()
                .filter(|(_, link)| *link != "https://d.test")
                .collect::<Vec<_>>(),
            points(&without),
            "{written:?}"
        );

        let answer = format!("[d]: https://d.test\n{written}");
        for cut in (1..answer.len()).filter(|cut| answer.is_char_boundary(*cut)) {
            let (first, second) = answer.split_at(cut);
            let said = pointing_in(&[first, second]);
            assert_eq!(wrote(&said), wrote(&with), "{written:?} cut at {cut}");
            assert_eq!(points(&said), points(&with), "{written:?} cut at {cut}");
        }
    }
}
