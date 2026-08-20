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

/// The same, many lines: typed through `press`, which is the only way a newline
/// reaches the text — a `paste` is sanitized, and a `Key::Char('\n')` is a
/// control a one-line editor drops.
fn lines(said: &str) -> Editor {
    let mut editor = Editor::new().multiline();
    for character in said.chars() {
        let key = if character == '\n' {
            Key::Newline
        } else {
            Key::Char(character)
        };
        assert_eq!(editor.press(key), Typed::Changed);
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
fn a_paste_is_inserted_whole_at_the_cursor() {
    let mut editor = back("before after", 5);

    assert_eq!(editor.paste("a large paste "), Typed::Changed);
    assert_eq!(editor.text(), "before a large paste after");
    assert_eq!(editor.column(), 21);
}

#[test]
fn a_large_middle_paste_moves_each_side_once() {
    const SIDE: usize = 256 * 1024;

    let mut editor = Editor {
        said: format!("{}{}", "a".repeat(SIDE), "z".repeat(SIDE)),
        at: SIDE,
        ..Editor::new()
    };
    let pasted = "middle ".repeat(SIDE / 7);

    assert_eq!(editor.paste(&pasted), Typed::Changed);
    assert_eq!(editor.text().len(), SIDE * 2 + pasted.len());
    assert!(editor.text().starts_with(&"a".repeat(SIDE)));
    assert!(editor.text().ends_with(&"z".repeat(SIDE)));
    assert_eq!(editor.at, SIDE + pasted.len());
}

#[test]
fn a_paste_over_the_prompt_bound_is_refused_whole() {
    let mut editor = back("before after", 5);
    let before = editor.text().to_owned();
    let at = editor.at;
    let pasted = "x".repeat(Editor::MAX_BYTES + 1);

    assert_eq!(editor.paste(&pasted), Typed::Refused);
    assert_eq!(editor.text(), before);
    assert_eq!(editor.at, at);
}

#[test]
fn the_last_byte_fits_and_the_next_character_is_refused() {
    let mut editor = Editor::new();

    assert_eq!(editor.paste(&"x".repeat(Editor::MAX_BYTES)), Typed::Changed);
    assert_eq!(editor.press(Key::Char('y')), Typed::Refused);
    assert_eq!(editor.text().len(), Editor::MAX_BYTES);
    assert_eq!(editor.press(Key::Backspace), Typed::Changed);
    assert_eq!(editor.press(Key::Char('日')), Typed::Refused);
    assert_eq!(editor.press(Key::Char('y')), Typed::Changed);
}

#[test]
fn individually_applied_characters_never_retain_more_than_one_mib() {
    let mut editor = Editor::new();
    let mut changes = 0;
    let mut refusals = 0;

    // The terminal layer normally gathers an immediately-ready run before it
    // reaches here. This is the lower-level guarantee: even a caller that
    // applies ordinary characters one at a time cannot cross the retained
    // boundary.
    for _ in 0..Editor::MAX_BYTES + 1024 {
        match editor.press(Key::Char('x')) {
            Typed::Changed => changes += 1,
            Typed::Refused => refusals += 1,
            other => panic!("a character did {other:?}"),
        }
    }

    assert_eq!(editor.text().len(), Editor::MAX_BYTES);
    assert!(editor.said.capacity() <= Editor::MAX_BYTES);
    assert_eq!(changes, Editor::MAX_BYTES);
    assert_eq!(refusals, 1024);
}

#[test]
fn paste_controls_cannot_move_the_terminal_cursor() {
    let mut editor = typed("safe ");

    assert_eq!(editor.paste("text\n\x1b[2J\tstill"), Typed::Changed);
    assert_eq!(editor.text(), "safe text[2J    still");
    assert_eq!(editor.paste("\n\x07"), Typed::Ignored);
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
fn a_newline_is_a_character_only_where_the_editor_is_many_lines() {
    // The prompt is the one caller that asks for it: everywhere else a newline
    // is noise, and the key that sends it is still the line's end.
    let mut single = typed("one");
    assert_eq!(single.press(Key::Newline), Typed::Ignored);
    assert_eq!(single.text(), "one");

    let mut multi = typed("one").multiline();
    assert_eq!(multi.press(Key::Newline), Typed::Changed);
    for character in "two".chars() {
        multi.press(Key::Char(character));
    }
    assert_eq!(multi.text(), "one\ntwo");
    assert_eq!(multi.line(), 1);
    assert_eq!(multi.column(), 3);
}

#[test]
fn a_multi_line_paste_keeps_its_newlines_and_drops_the_rest() {
    let mut editor = Editor::new().multiline();

    assert_eq!(editor.paste("first\nsecond\x07\nthird"), Typed::Changed);
    assert_eq!(editor.text(), "first\nsecond\nthird");
    assert_eq!(editor.line(), 2);

    // And the same paste into a one-line editor still loses the breaks, which is
    // what a permission note or a secret wants of them.
    let mut single = Editor::new();
    assert_eq!(single.paste("first\nsecond"), Typed::Changed);
    assert_eq!(single.text(), "firstsecond");
}

#[test]
fn a_break_is_stored_one_way_whichever_of_the_three_a_paste_arrives_in() {
    // The spelling that matters most is the one this editor does *not* store: a
    // terminal spells the break inside a paste the way Return spells it, so a
    // carriage return is what a real paste is made of and reading only newlines
    // is the same bug as reading none.
    for pasted in ["first\rsecond\rthird", "first\r\nsecond\r\nthird"] {
        let mut editor = Editor::new().multiline();

        assert_eq!(editor.paste(pasted), Typed::Changed, "{pasted:?}");
        assert_eq!(editor.text(), "first\nsecond\nthird", "{pasted:?}");
        assert_eq!(editor.line(), 2, "{pasted:?}");
    }

    // The pair is one break and not two, so a clipboard filled on another
    // platform does not arrive double-spaced.
    let mut editor = Editor::new().multiline();
    assert_eq!(editor.paste("one\r\ntwo"), Typed::Changed);
    assert_eq!(editor.line(), 1);

    // And a one-line editor loses all three, the same as it loses a newline.
    let mut single = Editor::new();
    assert_eq!(single.paste("first\rsecond\r\nthird"), Typed::Changed);
    assert_eq!(single.text(), "firstsecondthird");
}

#[test]
fn up_and_down_cross_lines_and_remember_the_column() {
    let mut editor = lines("long line here\nshort\nlong again");
    // Start at the end of the last line, column 10.
    assert_eq!(editor.line(), 2);
    assert_eq!(editor.column(), 10);

    // Up to the short line: the column is past its end, so the cursor takes the
    // end rather than a column that line does not have.
    assert_eq!(editor.press(Key::Up), Typed::Changed);
    assert_eq!(editor.line(), 1);
    assert_eq!(editor.column(), 5, "the short line's end");

    // Up again to the first line, which has the column.
    assert_eq!(editor.press(Key::Up), Typed::Changed);
    assert_eq!(editor.line(), 0);
    assert_eq!(editor.column(), 10);

    // Down twice returns to where the cursor started: the column is kept across
    // the short line rather than drifting left with it.
    assert_eq!(editor.press(Key::Down), Typed::Changed);
    assert_eq!(editor.line(), 1);
    assert_eq!(editor.press(Key::Down), Typed::Changed);
    assert_eq!(editor.line(), 2);
    assert_eq!(editor.column(), 10, "the column it left from");

    // And past the ends there is nowhere to go, which costs no frame.
    assert_eq!(editor.press(Key::Down), Typed::Ignored);
    let mut top = lines("only");
    assert_eq!(top.press(Key::Up), Typed::Ignored);
}

#[test]
fn home_and_end_reach_the_line_the_cursor_is_on() {
    // On many lines the ends are the line's, not the text's: the cursor crosses
    // a line with the arrows, and Home is still the start of what it is on.
    let mut editor = lines("first line\nsecond line");

    assert_eq!(editor.press(Key::Home), Typed::Changed);
    assert_eq!(editor.line(), 1);
    assert_eq!(editor.column(), 0);

    assert_eq!(editor.press(Key::End), Typed::Changed);
    assert_eq!(editor.line(), 1);
    assert_eq!(editor.column(), 11);
}

#[test]
fn backspace_joins_a_line_onto_the_one_above() {
    let mut editor = lines("one\ntwo");
    editor.press(Key::Home);

    // At the start of the second line, the character behind the cursor is the
    // newline, and taking it joins the two lines into one.
    assert_eq!(editor.press(Key::Backspace), Typed::Changed);
    assert_eq!(editor.text(), "onetwo");
    assert_eq!(editor.line(), 0);
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

#[test]
fn a_pasted_tab_arrives_as_the_columns_it_stood_for() {
    let mut editor = Editor::new().multiline();

    assert_eq!(editor.paste("fn main() {\n\tlet a = 1;\n}"), Typed::Changed);
    assert_eq!(editor.text(), "fn main() {\n    let a = 1;\n}");
}

#[test]
fn return_sends_and_a_modified_return_opens_a_line() {
    // The arrangement almost every reader has, and the one nothing has to be
    // configured for.
    let mut editor = typed("one").multiline();

    assert_eq!(editor.press(Key::Newline), Typed::Changed);
    assert_eq!(editor.text(), "one\n");
    assert_eq!(editor.press(Key::Enter), Typed::Submitted);
}

#[test]
fn the_two_swap_for_a_terminal_that_keeps_the_modified_return() {
    // What a reader asks for when Shift and Return never reach this process:
    // Return opens the line it could not otherwise open, and the press that did
    // arrive is the one that sends.
    let mut editor = typed("one").multiline().sends(Sending::AltEnter);

    assert_eq!(editor.press(Key::Enter), Typed::Changed);
    assert_eq!(editor.text(), "one\n");
    assert_eq!(editor.press(Key::Newline), Typed::Submitted);
}

#[test]
fn the_swap_still_refuses_to_send_nothing() {
    // Whichever press sends, an empty box has nothing to send — otherwise the
    // arrangement below would turn a stray Alt+Return into a turn about
    // nothing.
    let mut editor = Editor::new().multiline().sends(Sending::AltEnter);

    assert_eq!(editor.press(Key::Newline), Typed::Ignored);
    assert_eq!(editor.press(Key::Enter), Typed::Changed);
    assert_eq!(editor.text(), "\n");
}

#[test]
fn the_three_edits_every_shell_answers_to_reach_the_line_here_too() {
    let mut editor = lines("first line\nsecond word here");

    // The cursor is at the end of the last line. A word back, then another.
    assert_eq!(editor.press(Key::RubWord), Typed::Changed);
    assert_eq!(editor.text(), "first line\nsecond word ");
    assert_eq!(editor.press(Key::RubWord), Typed::Changed);
    assert_eq!(editor.text(), "first line\nsecond ");

    // The rest of the line, and only of the line: the one above it stays.
    assert_eq!(editor.press(Key::RubToStart), Typed::Changed);
    assert_eq!(editor.text(), "first line\n");
    assert_eq!(editor.line(), 1);
    assert_eq!(editor.column(), 0);

    // An edit against an end it is already at is nothing happening, which is
    // what keeps a held key from redrawing at the speed of the repeat. The line
    // ahead is empty and the line behind is empty, so two of the three refuse.
    assert_eq!(editor.press(Key::RubToStart), Typed::Ignored);
    assert_eq!(editor.press(Key::Delete), Typed::Ignored);

    // The third does not, and that is the point: a word goes on being a word
    // across a break, the same way Backspace joins the lines rather than
    // stopping at one. Nothing else here treats a break as a wall.
    assert_eq!(editor.press(Key::RubWord), Typed::Changed);
    assert_eq!(editor.text(), "first ");
    assert_eq!(editor.line(), 0);
}

#[test]
fn the_rest_of_a_line_ahead_goes_without_the_line_under_it() {
    let mut editor = lines("keep this\nand this");
    assert_eq!(editor.press(Key::Home), Typed::Changed);

    assert_eq!(editor.press(Key::RubToEnd), Typed::Changed);
    assert_eq!(editor.text(), "keep this\n");

    // The break is not on the line ahead, so the line above survives an edit
    // that took everything the cursor could see.
    assert_eq!(editor.press(Key::RubToEnd), Typed::Ignored);
    assert_eq!(editor.line(), 1);
}

#[test]
fn delete_takes_what_is_ahead_and_leaves_the_cursor_where_it_was() {
    let mut editor = Editor::new();
    editor.paste("abc");
    assert_eq!(editor.press(Key::Home), Typed::Changed);

    assert_eq!(editor.press(Key::Delete), Typed::Changed);
    assert_eq!(editor.text(), "bc");
    assert_eq!(editor.column(), 0);

    // A character wider than a byte goes whole, which is what a rub that
    // counted bytes would get wrong.
    let mut wide = Editor::new();
    wide.paste("日本");
    assert_eq!(wide.press(Key::Home), Typed::Changed);
    assert_eq!(wide.press(Key::Delete), Typed::Changed);
    assert_eq!(wide.text(), "本");
}
