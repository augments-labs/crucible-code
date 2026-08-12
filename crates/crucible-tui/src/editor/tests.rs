//! What each key does to a line, and what it costs the screen.

use super::*;

/// An editor that has been typed into, cursor left at the end.
fn typed(said: &str) -> Editor {
    let mut editor = Editor::new();
    for character in said.chars() {
        assert_eq!(editor.press(Key::Char(character)), Typed::Changed);
    }

    editor
}

/// The same, with `back` presses of the left arrow after it.
fn back(said: &str, back: usize) -> Editor {
    let mut editor = typed(said);
    for _ in 0..back {
        editor.press(Key::Left);
    }

    editor
}

#[test]
fn a_new_line_is_empty_and_its_cursor_is_at_the_start() {
    let editor = Editor::new();

    assert_eq!(editor.text(), "");
    assert_eq!(editor.column(), 0);
    assert!(editor.is_empty());
}

#[test]
fn typing_puts_characters_where_the_cursor_is() {
    let mut editor = back("cont", 2);

    assert_eq!(editor.press(Key::Char('u')), Typed::Changed);

    assert_eq!(editor.text(), "count");
    // After what was typed, not after what was there: the next character goes
    // beside this one rather than back where the cursor started.
    assert_eq!(editor.column(), 3);

    assert_eq!(editor.press(Key::Char('!')), Typed::Changed);
    assert_eq!(editor.text(), "cou!nt");
}

#[test]
fn backspace_takes_the_character_behind_the_cursor_and_not_the_one_on_it() {
    let mut editor = back("count", 1);

    assert_eq!(editor.press(Key::Backspace), Typed::Changed);

    assert_eq!(editor.text(), "cout");
    assert_eq!(editor.column(), 3);
}

#[test]
fn backspace_at_the_start_of_a_line_costs_no_frame() {
    // Held down, this is the key that arrives at the repeat rate of the
    // keyboard. Every one of them redrawing the same row is the whole reason a
    // key says whether it changed anything.
    let mut editor = back("count", 5);

    assert_eq!(editor.press(Key::Backspace), Typed::Ignored);
    assert_eq!(editor.text(), "count");
}

#[test]
fn the_arrows_stop_at_both_ends() {
    let mut editor = typed("ab");

    assert_eq!(editor.press(Key::Right), Typed::Ignored);
    assert_eq!(editor.column(), 2);

    for _ in 0..2 {
        assert_eq!(editor.press(Key::Left), Typed::Changed);
    }
    assert_eq!(editor.press(Key::Left), Typed::Ignored);
    assert_eq!(editor.column(), 0);
}

#[test]
fn home_and_end_reach_the_ends_and_then_cost_nothing() {
    let mut editor = typed("a search that stops partway");

    assert_eq!(editor.press(Key::Home), Typed::Changed);
    assert_eq!(editor.press(Key::Home), Typed::Ignored);
    assert_eq!(editor.column(), 0);

    assert_eq!(editor.press(Key::End), Typed::Changed);
    assert_eq!(editor.press(Key::End), Typed::Ignored);
    assert_eq!(editor.column(), 27);
}

#[test]
fn a_word_either_way_crosses_the_whole_word_and_stops_where_it_ends() {
    // A path is one word. What is usually wrong at the far end of a long line
    // is a path or a name, and a rule that stopped at every punctuation mark
    // inside one would need the key four more times to finish crossing it.
    let mut editor = typed("crates/crucible-tui/src/editor.rs holds the line");

    for left in [
        "crates/crucible-tui/src/editor.rs holds the ",
        "crates/crucible-tui/src/editor.rs holds ",
        "crates/crucible-tui/src/editor.rs ",
        "",
    ] {
        assert_eq!(editor.press(Key::WordLeft), Typed::Changed);
        assert_eq!(editor.before(), left);
    }

    assert_eq!(editor.press(Key::WordLeft), Typed::Ignored);

    for right in [
        "crates/crucible-tui/src/editor.rs",
        "crates/crucible-tui/src/editor.rs holds",
        "crates/crucible-tui/src/editor.rs holds the",
        "crates/crucible-tui/src/editor.rs holds the line",
    ] {
        assert_eq!(editor.press(Key::WordRight), Typed::Changed);
        assert_eq!(editor.before(), right);
    }

    assert_eq!(editor.press(Key::WordRight), Typed::Ignored);
}

#[test]
fn the_spaces_beside_the_cursor_are_crossed_along_with_the_word() {
    // Sitting on the far side of a space, a press has to reach the word past
    // it. Landing on the near edge of that space instead would mean the cursor
    // one press leaves is a press short of the next word, and one word back
    // would cost two keystrokes for the rest of the line.
    let mut editor = typed("read   the  tail");

    for left in ["read   the  ", "read   ", ""] {
        assert_eq!(editor.press(Key::WordLeft), Typed::Changed);
        assert_eq!(editor.before(), left);
    }

    for right in ["read", "read   the", "read   the  tail"] {
        assert_eq!(editor.press(Key::WordRight), Typed::Changed);
        assert_eq!(editor.before(), right);
    }
}

#[test]
fn a_cursor_is_counted_in_columns_rather_than_characters() {
    // The reason this crate has a width module at all. Two characters that a
    // terminal draws four cells wide, and a cursor placed three cells early is
    // a cursor inside a glyph.
    let mut editor = typed("日本");

    assert_eq!(editor.column(), 4);

    editor.press(Key::Left);
    assert_eq!(editor.column(), 2);
}

#[test]
fn a_character_no_terminal_can_place_a_cursor_after_is_still_edited_whole() {
    // A combining mark is a character that costs no columns, so the cursor does
    // not move for one -- but backspace still takes the whole of it, and the
    // string is still the string that was typed.
    let mut editor = typed("e\u{301}");

    assert_eq!(editor.text(), "e\u{301}");
    assert_eq!(editor.column(), 1);

    assert_eq!(editor.press(Key::Backspace), Typed::Changed);
    assert_eq!(editor.text(), "e");
}

#[test]
fn a_character_that_is_several_bytes_is_one_press_of_backspace() {
    // The cursor is a byte offset, and taking one byte off it would leave it
    // inside a character -- where the next slice is a panic rather than a bug
    // somebody notices later.
    let mut editor = typed("héllo");

    for _ in 0..5 {
        assert_eq!(editor.press(Key::Backspace), Typed::Changed);
    }

    assert!(editor.is_empty());
    assert_eq!(editor.press(Key::Backspace), Typed::Ignored);
}

#[test]
fn a_character_a_terminal_would_act_on_is_not_a_character_that_was_typed() {
    // Everything with a meaning arrives as its own key, so what is left here is
    // what a paste drags in: a tab, a bell, the start of an escape sequence.
    // Stored, they would be drawn -- and an escape drawn is a cursor moved out
    // of the box the renderer is counting rows for.
    let mut editor = Editor::new();

    for character in ['\t', '\x07', '\x1b', '\u{0}'] {
        assert_eq!(editor.press(Key::Char(character)), Typed::Ignored);
    }

    assert!(editor.is_empty());
}

#[test]
fn return_on_a_line_that_was_typed_submits_it() {
    let mut editor = typed("what changed in the tail");

    assert_eq!(editor.press(Key::Enter), Typed::Submitted);

    // Still there afterwards: submitting says a line is ready, and taking it is
    // what empties the editor.
    assert_eq!(editor.text(), "what changed in the tail");
}

#[test]
fn return_on_an_empty_line_asks_for_nothing() {
    let mut editor = Editor::new();

    assert_eq!(editor.press(Key::Enter), Typed::Ignored);
}

#[test]
fn taking_a_line_leaves_an_empty_one_ready() {
    let mut editor = back("count the columns", 6);

    assert_eq!(editor.take(), "count the columns");

    assert!(editor.is_empty());
    assert_eq!(editor.column(), 0, "the cursor was left in the old line");
    assert_eq!(editor.press(Key::Char('a')), Typed::Changed);
    assert_eq!(editor.text(), "a");
}

#[test]
fn interrupt_throws_away_a_line_before_it_ends_anything() {
    // The key is how a half-typed command is called off, so it must not be the
    // key that runs one. The first press empties, and only the second -- now
    // against an empty line -- is aimed anywhere else.
    let mut editor = typed("rm -rf /");

    assert_eq!(editor.press(Key::Interrupt), Typed::Changed);
    assert!(editor.is_empty());
    assert_eq!(editor.column(), 0);

    assert_eq!(editor.press(Key::Interrupt), Typed::Interrupted);
}

#[test]
fn interrupt_against_an_empty_line_reports_the_press_rather_than_ending() {
    // The difference between this and the end of input is the whole reason the
    // two are separate variants. Ctrl-D says what it means once; Ctrl-C is the
    // key somebody hits at a turn that has already finished, and a session it
    // ended on its own would be one nobody asked to end. What to do about that
    // needs a clock, which is above here.
    let mut editor = Editor::new();

    assert_eq!(editor.press(Key::Interrupt), Typed::Interrupted);
    assert_eq!(editor.press(Key::Interrupt), Typed::Interrupted);
    assert!(editor.is_empty(), "nothing was typed by either press");
}

#[test]
fn the_end_of_input_is_only_read_from_an_empty_line() {
    // One key away from the ones that edit, and what it would end is a prompt
    // somebody is still writing.
    let mut editor = typed("half a thought");

    assert_eq!(editor.press(Key::Eof), Typed::Ignored);
    assert_eq!(editor.text(), "half a thought");

    editor.clear();
    assert_eq!(editor.press(Key::Eof), Typed::Ended);
}

#[test]
fn every_edit_leaves_the_cursor_somewhere_the_line_actually_ends() {
    // The invariant the whole file rests on, asserted against a sequence rather
    // than against one key: the offset is on a character boundary and never
    // past the end, whatever order the keys arrived in.
    let mut editor = Editor::new();
    let keys = [
        Key::Char('日'),
        Key::Left,
        Key::Char('e'),
        Key::Char('\u{301}'),
        Key::Right,
        Key::Char('本'),
        Key::Home,
        Key::Backspace,
        Key::Char('x'),
        Key::End,
        Key::Backspace,
        Key::Left,
        Key::Backspace,
        Key::WordLeft,
        Key::Char(' '),
        Key::WordRight,
        Key::WordLeft,
    ];

    for key in keys {
        editor.press(key);

        assert!(
            editor.text().is_char_boundary(editor.at),
            "{:?} at {}",
            editor.text(),
            editor.at
        );
        assert!(editor.at <= editor.text().len());
        assert_eq!(editor.column(), width::columns(editor.before()));
    }
}
