use crate::color::Theme;
use unicode_width::UnicodeWidthStr;

use super::*;

/// One column as text, two once the selector follows it. Spelled out
/// because a selector is invisible in a source file.
const WARNING: &str = "\u{26A0}\u{FE0F}";

/// Pushes and returns what overflowed, so a test reads as one call.
fn push(tail: &mut Tail, delta: &str) -> Vec<String> {
    let mut overflow = Vec::new();
    tail.push(delta, &mut overflow);
    overflow
}

fn rows(tail: &Tail) -> Vec<&str> {
    tail.rows().collect()
}

#[test]
fn text_shorter_than_the_width_stays_on_one_row() {
    let mut tail = Tail::new(20, 5);
    assert!(push(&mut tail, "hello").is_empty());
    assert_eq!(rows(&tail), ["hello"]);
}

#[test]
fn a_delta_split_mid_word_still_lands_on_one_row() {
    // Providers split wherever the socket did, so the tail must not treat a
    // delta boundary as anything at all.
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "hel");
    push(&mut tail, "lo wor");
    push(&mut tail, "ld");
    assert_eq!(rows(&tail), ["hello world"]);
}

#[test]
fn a_newline_starts_a_row() {
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "one\ntwo");
    assert_eq!(rows(&tail), ["one", "two"]);
}

#[test]
fn crlf_does_not_leave_a_blank_row() {
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "one\r\ntwo");
    assert_eq!(rows(&tail), ["one", "two"]);
}

#[test]
fn text_wider_than_the_width_wraps() {
    let mut tail = Tail::new(4, 5);
    push(&mut tail, "abcdefghij");
    assert_eq!(rows(&tail), ["abcd", "efgh", "ij"]);
}

#[test]
fn a_wide_character_takes_two_columns_when_wrapping() {
    // The bug this catches is counting characters instead of columns: five
    // of these fit in a five-column row only if each is one wide, and they
    // are not.
    let mut tail = Tail::new(5, 5);
    push(&mut tail, "日本語です");
    assert_eq!(rows(&tail), ["日本", "語で", "す"]);
}

#[test]
fn a_combining_mark_does_not_take_a_column() {
    // "e" plus a combining acute is two chars and one column, so it must
    // not push the row over the edge.
    let mut tail = Tail::new(2, 5);
    push(&mut tail, "e\u{301}x");
    assert_eq!(rows(&tail), ["e\u{301}x"]);
}

#[test]
fn an_emoji_presentation_selector_takes_the_column_it_asks_for() {
    // The base is one column alone and two once the selector follows, so
    // three do not fit on a four-column row. Counted per character it is 45
    // of them on an 80-column row the terminal lays out at 90.
    let mut tail = Tail::new(4, 5);
    push(&mut tail, &WARNING.repeat(3));

    assert_eq!(rows(&tail), [WARNING.repeat(2), WARNING.to_owned()]);
}

#[test]
fn a_selector_arriving_in_the_next_delta_still_widens_the_pair() {
    // Providers split wherever the socket did, so the row was measured
    // before the selector turned up. The pair moves down together: one
    // parted from its base stops being a selector at all.
    let mut tail = Tail::new(3, 5);
    push(&mut tail, "ab\u{26A0}");
    push(&mut tail, "\u{FE0F}");

    assert_eq!(rows(&tail), ["ab", WARNING]);
}

#[test]
fn an_escape_sequence_from_a_tool_cannot_move_the_cursor() {
    // Tool output is not trusted to be plain text. A sequence kept verbatim
    // would move a cursor this renderer believes it is tracking, and the next
    // frame would erase the wrong lines. Dropping only the escape byte is not
    // enough either: the parameters are printable, so `[2J` would be drawn and
    // the row counted three columns wider than the terminal shows it.
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "a\x1b[2Jb");
    assert_eq!(rows(&tail), ["ab"]);
}

#[test]
fn a_sequence_split_across_two_deltas_is_still_dropped_whole() {
    // A delta is a piece of the wire rather than a piece of the output, so a
    // sequence arrives cut as often as not.
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "a\x1b[38;5");
    push(&mut tail, ";214mb");

    assert_eq!(rows(&tail), ["ab"]);
}

#[test]
fn a_turn_that_ended_mid_sequence_does_not_swallow_the_next_one() {
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "a\x1b[38");
    tail.clear();
    push(&mut tail, "next answer");

    assert_eq!(rows(&tail), ["next answer"]);
}

#[test]
fn a_tab_advances_to_the_next_stop() {
    let mut tail = Tail::new(40, 5);
    push(&mut tail, "ab\tc");
    assert_eq!(rows(&tail), ["ab      c"]);
}

#[test]
fn a_tab_past_the_edge_wraps_instead_of_overflowing_the_row() {
    let mut tail = Tail::new(6, 5);
    push(&mut tail, "abcde\tx");
    assert_eq!(rows(&tail), ["abcde", "x"]);
}

#[test]
fn rows_past_the_bound_overflow_oldest_first() {
    let mut tail = Tail::new(20, 2);
    let out = push(&mut tail, "one\ntwo\nthree\nfour");

    assert_eq!(out, ["one", "two"]);
    assert_eq!(rows(&tail), ["three", "four"]);
}

#[test]
fn the_tail_never_grows_past_its_bound() {
    // This is the property the whole type exists for: memory must not grow
    // with how long the session has run.
    let mut tail = Tail::new(20, 3);
    let mut overflow = Vec::new();

    for turn in 0..10_000 {
        tail.push(&format!("line {turn}\n"), &mut overflow);
        overflow.clear();
        assert!(tail.len() <= 3, "tail grew to {} rows", tail.len());
    }
}

#[test]
fn wrapped_rows_count_against_the_bound_too() {
    // One logical line can be many display rows, so the bound has to be
    // about rows or a single long paragraph would blow past it.
    let mut tail = Tail::new(4, 2);
    let out = push(&mut tail, "abcdefghijklmnop");

    assert_eq!(out, ["abcd", "efgh"]);
    assert_eq!(rows(&tail), ["ijkl", "mnop"]);
}

#[test]
fn the_row_the_cursor_sits_on_is_not_content() {
    // Otherwise every answer ending in a newline settles a blank line.
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "one\ntwo\n");

    assert_eq!(rows(&tail), ["one", "two", ""]);
    assert_eq!(tail.content().collect::<Vec<_>>(), ["one", "two"]);
}

#[test]
fn a_blank_line_somebody_wrote_is_content() {
    // Only the *last* empty row is the cursor's. The rest are output.
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "one\n\ntwo");

    assert_eq!(tail.content().collect::<Vec<_>>(), ["one", "", "two"]);
}

#[test]
fn clearing_leaves_one_empty_row() {
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "one\ntwo");
    tail.clear();

    assert_eq!(rows(&tail), [""]);
    assert!(tail.is_empty());
}

#[test]
fn a_zero_width_does_not_wrap_forever() {
    // A terminal reports zero columns while a pane is being dragged, and a
    // literal zero here would loop pushing empty rows.
    let mut tail = Tail::new(0, 3);
    push(&mut tail, "abc");
    assert_eq!(rows(&tail), ["a", "b", "c"]);
}

#[test]
fn a_selector_is_dropped_when_the_row_cannot_hold_the_pair() {
    // The width a dragged pane reports, where the pair asks for two columns of
    // one. The base keeps its column and draws as text; a row counted at two
    // would be wrapped by the terminal, one below where the next frame rewinds.
    let mut tail = Tail::new(0, 3);
    push(&mut tail, WARNING);

    assert_eq!(rows(&tail), ["\u{26A0}"]);
}

#[test]
fn a_character_wider_than_the_row_is_dropped_rather_than_drawn() {
    // Nowhere to put it: a fresh row would be counted one column short and the
    // one after it would land on top of a row already committed to scrollback.
    let mut tail = Tail::new(1, 3);
    push(&mut tail, "a日b");

    assert_eq!(rows(&tail), ["a", "b"]);
}

#[test]
fn no_row_is_ever_wider_than_the_tail() {
    // The invariant the rewind arithmetic rests on, and the one every
    // placement path has to keep rather than most of them. Both halves matter:
    // what the tail counted is what it moves back over, and what it drew is
    // what the terminal lays out.
    let warnings = WARNING.repeat(3);
    let hostile = [
        "abcdef",
        "日本語です",
        warnings.as_str(),
        "e\u{301}xyz",
        "ab\tcd\te",
        "a\x1b[2Jb",
        "\u{1F468}\u{200D}\u{1F469} 1\u{FE0F}\u{20E3}",
        "\u{26A0}\u{FE0F}日\u{FE0F}x\n\u{26A0}\u{FE0F}",
    ];

    for width in 1..=6 {
        for text in hostile {
            let mut tail = Tail::new(width, 20);
            push(&mut tail, text);

            for row in &tail.rows {
                let drawn = UnicodeWidthStr::width(row.text.as_str());
                assert!(row.width <= width, "{text:?} at {width}: {row:?}");
                assert!(drawn <= width, "{text:?} at {width}: {row:?} drew {drawn}");
            }
        }
    }
}

/// A palette that writes every hue it has, without an environment to say so.
fn colourful() -> Palette {
    Palette::resolve(true, Theme::Dark, None, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

/// How many rows carry an unbalanced sequence.
///
/// A row that opened a slot and did not close it sets that attribute on
/// everything drawn after it, which for the last row before scrollback is the
/// reader's own terminal for the rest of the day.
fn unbalanced(tail: &Tail) -> Vec<&str> {
    let open = colourful().open(Slot::Quiet).as_str().to_owned();
    let close = colourful().close();

    tail.rows()
        .filter(|row| row.matches(open.as_str()).count() != row.matches(close).count())
        .collect()
}

#[test]
fn a_slot_costs_the_row_it_is_worn_on_no_column_at_all() {
    // The property the whole arrangement rests on. Width is counted as
    // characters are placed and a sequence is never placed, so the same answer
    // wraps in the same column whether it wears a slot or not — and the rewind
    // that follows moves back over the same number of rows.
    let mut plain = Tail::new(8, 5);
    let mut worn = Tail::new(8, 5);
    worn.wear(Slot::Quiet, &colourful());

    push(&mut plain, "one two three four");
    push(&mut worn, "one two three four");

    assert_eq!(plain.len(), worn.len());
    assert_eq!(plain.column(), worn.column());
    assert_eq!(
        rows(&plain),
        rows(&worn)
            .iter()
            .map(|row| row
                .replace(colourful().open(Slot::Quiet).as_str(), "")
                .replace(colourful().close(), ""))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_row_that_wrapped_opens_the_slot_again_rather_than_carrying_it_over() {
    // The rows do not stay together: the oldest overflow into scrollback one at
    // a time, so a row holding half of a pair would leave the other half
    // unwritten for good.
    let mut tail = Tail::new(4, 5);
    tail.wear(Slot::Quiet, &colourful());
    push(&mut tail, "abcdefgh");

    assert_eq!(tail.len(), 2);
    assert!(unbalanced(&tail).is_empty(), "{:?}", rows(&tail));
    for row in rows(&tail) {
        assert!(
            row.starts_with(colourful().open(Slot::Quiet).as_str()),
            "{row:?}"
        );
        assert!(row.ends_with(colourful().close()), "{row:?}");
    }
}

#[test]
fn the_row_still_being_written_is_closed_like_every_other() {
    // A slot is worn across deltas, and the tail is read between every pair of
    // them. Left open, the attribute would be set on the footing under the tail
    // and on the prompt after it.
    let mut tail = Tail::new(20, 5);
    tail.wear(Slot::Quiet, &colourful());
    push(&mut tail, "half a sen");
    assert!(unbalanced(&tail).is_empty(), "{:?}", rows(&tail));

    push(&mut tail, "tence\nand another");
    assert!(unbalanced(&tail).is_empty(), "{:?}", rows(&tail));
}

#[test]
fn a_slot_worn_and_changed_before_anything_is_written_leaves_nothing_behind() {
    // An opening sequence around no text is bytes for nothing, and the pair a
    // reader would find if they looked at the row.
    let mut tail = Tail::new(20, 5);
    tail.wear(Slot::Quiet, &colourful());
    tail.wear(Slot::Plain, &colourful());
    push(&mut tail, "text");

    assert_eq!(rows(&tail), ["text"]);
}

#[test]
fn a_palette_with_no_colour_in_it_puts_no_bytes_in_a_row() {
    // What a redirected run gets. The slot is still read and still meant; there
    // is simply nothing to write it with, and an escape byte here would end up
    // in whatever kept the output.
    let mut tail = Tail::new(20, 5);
    tail.wear(Slot::Quiet, &Palette::plain());
    push(&mut tail, "text\nmore");

    assert_eq!(rows(&tail), ["text", "more"]);
}

#[test]
fn a_row_holding_a_slot_and_no_text_is_still_an_empty_row() {
    // Measured in columns like everything else here. Counted as content, it
    // would settle into the record as a blank line nobody wrote.
    let mut tail = Tail::new(20, 5);
    tail.wear(Slot::Quiet, &colourful());
    push(&mut tail, "said\n");

    assert!(!tail.is_empty());
    assert_eq!(tail.content().collect::<Vec<_>>().len(), 1);
}

#[test]
fn the_slot_in_force_does_not_survive_the_turn() {
    // An answer that ended inside a code block ended there. Carried over, the
    // whole of the next answer would be painted as code.
    let mut tail = Tail::new(20, 5);
    tail.wear(Slot::Quiet, &colourful());
    push(&mut tail, "code");
    tail.clear();
    push(&mut tail, "plain again");

    assert_eq!(rows(&tail), ["plain again"]);
}

#[test]
fn a_tail_that_has_written_nothing_has_put_no_row_under_anything() {
    let tail = Tail::new(20, 1);
    assert!(tail.parted());
}

#[test]
fn the_blank_row_a_paragraph_ended_on_is_remembered_after_it_has_gone() {
    // The row is in the terminal's scrollback by then and cannot be looked at
    // again. Measuring it on the way out is the only chance there is.
    let mut tail = Tail::new(20, 1);
    push(&mut tail, "one\n\n");
    assert!(tail.parted());

    push(&mut tail, "two");
    assert!(!tail.parted(), "a row is being written into");

    push(&mut tail, "\n");
    assert!(!tail.parted(), "the row that left drew something");
}
