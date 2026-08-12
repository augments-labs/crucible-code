//! What the box does with a line, on a terminal that records rather than draws.
//!
//! Raw mode is the one thing not asserted here. Entering it reaches the
//! controlling terminal, which under a test harness is the one the tests are
//! being run in — so [`super::ask`] is exercised only where it declines to, and
//! everything after that point is called directly.

use crucible_core::{Permission, Rules};
use crucible_runner::{Model, Session, Tools};
use crucible_tui::{Key, Recording};

use super::*;
use crate::cli::fake::Script;

/// An engine that holds one mode and would run nothing.
///
/// The mode is what this file is after: it is the one thing the box reads from
/// the session rather than from its own state, and the row under the box is
/// what proves it read it rather than a copy.
fn engine(mode: Mode) -> Runner {
    Runner::new(
        Box::new(Script::new(vec![])),
        Tools::new(),
        Model {
            name: "script".into(),
            max_tokens: 64,
            system: None,
        },
        Session::nowhere(),
    )
    .permitting(Permission::with(mode, Rules::new()))
}

/// The row a session in this mode is drawn with, nothing waiting to be agreed
/// to.
fn settled(mode: Mode) -> Says {
    saying(&engine(mode), None, Glyphs::Unicode)
}

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

/// The same, wide enough for the status row to hold the keys as well as the
/// mode.
fn roomy() -> Renderer<Recording> {
    Renderer::new(Recording::new(60, 24))
}

#[test]
fn a_run_with_nothing_to_type_into_says_so_rather_than_reading_keys() {
    // The test harness captures standard output, so this is the redirected
    // case. It is also every `crucible < script.txt`, and the caller reads a
    // line for itself when it gets this back.
    let mut renderer = drawing();
    let mut runner = engine(Mode::Ask);

    let asked = ask(&mut renderer, Style::plain(), &mut runner).expect("no keys to read");

    assert!(
        matches!(asked, Asked::Untyped),
        "keys were read from a pipe"
    );
    assert_eq!(renderer.terminal().written(), "");

    // And nothing was stepped on the way out of a call that read no key.
    assert_eq!(runner.mode(), Mode::Ask);
}

#[test]
fn the_box_is_drawn_around_the_line_with_the_mode_under_it() {
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("hi"),
        Style::plain(),
        &settled(Mode::Ask),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.contains("› hi"), "{written:?}");
    assert!(written.contains("ask mode on"), "{written:?}");
    assert!(written.contains(CYCLE), "{written:?}");
}

#[test]
fn a_window_with_room_for_one_of_them_keeps_the_mode_and_drops_the_keys() {
    // Which is the right way round: the mode is what is in force, and the keys
    // are how to change it. A row that wrapped to fit both would move the box
    // by a line every time somebody narrowed the window.
    let mut renderer = drawing();

    draw(
        &mut renderer,
        &typed("hi"),
        Style::plain(),
        &settled(Mode::Ask),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.contains("ask mode on"), "{written:?}");
    assert!(!written.contains(CYCLE), "{written:?}");
}

#[test]
fn the_cursor_ends_up_where_the_line_was_typed_to() {
    // Two rows below the row being typed on — the closing edge and the status
    // row — and then the column outright. Without the move back up, the next
    // character would be drawn under the box rather than in it.
    let mut renderer = drawing();

    draw(
        &mut renderer,
        &typed("hi"),
        Style::plain(),
        &settled(Mode::Ask),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.ends_with("\x1b[2A\x1b[7G"), "{written:?}");
}

#[test]
fn a_finished_line_is_left_in_the_record_and_the_box_is_taken_off() {
    let mut renderer = drawing();
    let mut editor = typed("hi");

    draw(&mut renderer, &editor, Style::plain(), &settled(Mode::Ask)).expect("the box to be drawn");
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
        !after.contains("ask mode on"),
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

#[test]
fn the_row_is_read_off_the_engine_rather_than_from_a_copy_of_the_mode() {
    // The drift this rules out: a mode kept beside the engine and drawn from
    // there would keep saying what the session started in, and the row that is
    // meant to say what is in force would be the one thing that could be wrong
    // about it.
    let mut runner = engine(Mode::Ask);
    assert_eq!(saying(&runner, None, Glyphs::Unicode).mode, "ask mode on");

    runner.cycle();
    assert_eq!(
        saying(&runner, None, Glyphs::Unicode).mode,
        "allow edits on"
    );
}

#[test]
fn a_mode_waiting_to_be_agreed_to_is_the_one_on_screen_and_not_the_one_in_force() {
    // What the box is drawn in while a step is standing is the mode being
    // offered, so agreeing to it changes what is in force and not what is
    // being looked at. The engine is still in the mode it was in until the key
    // that agrees arrives.
    let runner = engine(Mode::AllowEdits);
    let says = saying(&runner, Some(Mode::FullAccess), Glyphs::Unicode);

    assert!(says.mode.contains("full access mode on"), "{:?}", says.mode);
    assert!(
        says.mode.contains("nothing will be asked"),
        "the step says what it means: {:?}",
        says.mode
    );
    assert_eq!(says.tone, tone(Mode::FullAccess));
    assert_eq!(runner.mode(), Mode::AllowEdits);
}

#[test]
fn the_row_names_the_keys_that_answer_whatever_it_is_showing() {
    // One key while there is nothing standing, two while there is. A row that
    // named the step and not the answer to it would leave somebody looking at
    // a mode they cannot get out of.
    assert_eq!(settled(Mode::Ask).keys, CYCLE);

    let waiting = saying(
        &engine(Mode::AllowEdits),
        Some(Mode::FullAccess),
        Glyphs::Unicode,
    );
    assert!(waiting.keys.contains("confirm"), "{:?}", waiting.keys);
    assert!(waiting.keys.contains("esc"), "{:?}", waiting.keys);
}

#[test]
fn a_terminal_with_no_box_drawing_font_gets_the_same_sentence_in_its_own_characters() {
    // The marks in the unicode row are the two the confirm is answered with,
    // so a terminal that would draw them as hollow squares is one where the
    // keys stop being legible rather than merely looking plain.
    let says = saying(
        &engine(Mode::AllowEdits),
        Some(Mode::FullAccess),
        Glyphs::Ascii,
    );

    assert!(says.mode.is_ascii(), "{:?}", says.mode);
    assert!(says.keys.is_ascii(), "{:?}", says.keys);
    assert!(
        says.mode.contains("nothing will be asked"),
        "{:?}",
        says.mode
    );
    assert!(says.keys.contains("enter"), "{:?}", says.keys);
}

#[test]
fn the_only_step_that_is_agreed_to_first_is_the_one_that_stops_asking() {
    // The other two still ask about something, so a step into either is
    // answerable by stepping again. This one is the end of the ring in that
    // sense: nothing after it will ask.
    assert!(!agreed_first(Mode::Ask));
    assert!(!agreed_first(Mode::AllowEdits));
    assert!(agreed_first(Mode::FullAccess));
}

#[test]
fn a_line_beginning_with_a_slash_opens_the_list_above_the_box() {
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("/m"),
        Style::plain(),
        &settled(Mode::Ask),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    let listed = written.find("/model").expect("the list");
    let boxed = written.find("› /m").expect("the box");

    assert!(listed < boxed, "the list is under the box: {written:?}");
}

#[test]
fn the_box_stays_where_it_was_while_the_list_is_open() {
    // The whole reason it opens upwards. The cursor is parked by counting back
    // from the bottom of the region, so rows added above the box leave the same
    // two below the line — the box and the row under it are exactly where they
    // were on the keystroke before, and the mode is neither covered nor pushed
    // down the screen.
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("/m"),
        Style::plain(),
        &settled(Mode::Ask),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.ends_with("\x1b[2A\x1b[7G"), "{written:?}");
}

#[test]
fn a_prompt_is_drawn_in_the_rows_the_box_has_always_been() {
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("hi"),
        Style::plain(),
        &settled(Mode::Ask),
    )
    .expect("the box to be drawn");

    assert!(
        !renderer.terminal().written().contains("/model"),
        "a list opened over a line that is not a command"
    );
}

#[test]
fn a_list_with_no_room_left_for_it_is_not_opened_at_all() {
    // Cut off at the top it would read as the whole list, and the rewind that
    // takes the region back would reach over rows the terminal has already
    // taken. Neither is worth the rows it would have shown.
    for room in 0..4 {
        assert!(
            opened("/", 60, room, Glyphs::Unicode).is_empty(),
            "a list of four opened with room for {room}"
        );
    }

    assert!(!opened("/", 60, 4, Glyphs::Unicode).is_empty());
}

#[test]
fn no_two_modes_are_drawn_in_the_same_colour() {
    // The colour is the part read before the sentence is, and two modes
    // sharing one would make the border say less than nothing — it would say
    // the mode had not changed.
    let (quiet, edits, full) = (
        tone(Mode::Ask),
        tone(Mode::AllowEdits),
        tone(Mode::FullAccess),
    );

    assert_ne!(quiet, edits);
    assert_ne!(edits, full);
    assert_ne!(quiet, full);
}
