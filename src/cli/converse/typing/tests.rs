//! What the box does with a line, on a terminal that records rather than draws.
//!
//! Raw mode is the one thing not asserted here. Entering it reaches the
//! controlling terminal, which under a test harness is the one the tests are
//! being run in — so [`super::ask`] is exercised only where it declines to, and
//! everything after that point is called directly.

use crucible_tui::{Key, Recording};

use super::*;

/// An editor with `said` typed into it and the cursor left at the end.
fn typed(said: &str) -> Editor {
    let mut editor = Editor::new();

    for letter in said.chars() {
        editor.press(Key::Char(letter));
    }

    editor
}

/// A renderer over a terminal 30 columns wide, which is enough for a frame.
fn drawing() -> Renderer<Recording> {
    Renderer::new(Recording::new(30, 24))
}

#[test]
fn a_run_with_nothing_to_type_into_says_so_rather_than_reading_keys() {
    // The test harness captures standard output, so this is the redirected
    // case. It is also every `crucible < script.txt`, and the caller reads a
    // line for itself when it gets this back.
    let mut renderer = drawing();

    let asked = ask(&mut renderer, Style::plain(), "ask").expect("no mode to change");

    assert!(
        matches!(asked, Asked::Untyped),
        "keys were read from a pipe"
    );
    assert_eq!(renderer.terminal().written(), "");
}

#[test]
fn the_box_is_drawn_around_the_line_with_the_mode_under_it() {
    let mut renderer = drawing();

    draw(&mut renderer, &typed("hi"), Style::plain(), "ask").expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.contains("› hi"), "{written:?}");
    assert!(written.contains("ask"), "{written:?}");
}

#[test]
fn the_cursor_ends_up_where_the_line_was_typed_to() {
    // Two rows below the row being typed on — the closing edge and the status
    // row — and then the column outright. Without the move back up, the next
    // character would be drawn under the box rather than in it.
    let mut renderer = drawing();

    draw(&mut renderer, &typed("hi"), Style::plain(), "ask").expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.ends_with("\x1b[2A\x1b[7G"), "{written:?}");
}

#[test]
fn a_finished_line_is_left_in_the_record_and_the_box_is_taken_off() {
    let mut renderer = drawing();
    let mut editor = typed("hi");

    draw(&mut renderer, &editor, Style::plain(), "ask").expect("the box to be drawn");
    let boxed = renderer.terminal().written().len();

    let asked = said(&mut renderer, &mut editor, Style::plain()).expect("the line to be taken");

    assert!(matches!(asked, Asked::Said(line) if line == "hi"));

    // The box goes and the line stays: everything written after the box was on
    // screen is the record of it, with no border and no status row in it.
    let written = renderer.terminal().written();
    let after = written.get(boxed..).unwrap_or_default();

    assert!(after.contains("› hi"), "{after:?}");
    assert!(
        !after.contains('│'),
        "the frame outlived the line: {after:?}"
    );
    assert!(
        !after.contains("ask"),
        "the status row outlived it: {after:?}"
    );
}

#[test]
fn the_editor_is_empty_afterwards_and_ready_for_the_next_line() {
    // It is held for the whole session rather than made per prompt, so a line
    // left in it would be the next prompt's opening text.
    let mut renderer = drawing();
    let mut editor = typed("hi");

    said(&mut renderer, &mut editor, Style::plain()).expect("the line to be taken");

    assert!(editor.is_empty());
}
