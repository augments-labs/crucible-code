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
    // Tool output is not trusted to be plain text. A control character kept
    // verbatim would move a cursor this renderer believes it is tracking,
    // and the next frame would erase the wrong lines.
    let mut tail = Tail::new(20, 5);
    push(&mut tail, "a\x1b[2Jb");
    assert_eq!(rows(&tail), ["a[2Jb"]);
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
