//! What the box does with a line, on a terminal that records rather than draws.
//!
//! Raw mode is the one thing not asserted here. Entering it reaches the
//! controlling terminal, which under a test harness is the one the tests are
//! being run in — so [`super::ask`] is exercised only where it declines to, and
//! everything after that point is called directly.

use crucible_core::{Mode, Permission, Rules};
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

/// The row a session in this mode is drawn with.
fn settled(mode: Mode) -> Says {
    saying(&engine(mode))
}

/// The same, with the offer to leave standing under it.
fn leaving(mode: Mode) -> Says {
    Says {
        asking: Some(LEAVING),
        ..settled(mode)
    }
}

/// The list `said` opens, pointing where it points before an arrow has moved
/// anything.
fn listing(said: &str) -> Opened {
    Opened::filtered(said, Glyphs::Unicode)
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
        &Opened::default(),
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
        &Opened::default(),
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
        &Opened::default(),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.ends_with("\x1b[2A\x1b[7G"), "{written:?}");
}

#[test]
fn a_finished_line_is_left_in_the_record_and_the_box_is_taken_off() {
    let mut renderer = drawing();
    let mut editor = typed("hi");

    draw(
        &mut renderer,
        &editor,
        Style::plain(),
        &settled(Mode::Ask),
        &Opened::default(),
    )
    .expect("the box to be drawn");
    let boxed = renderer.terminal().written().len();

    let asked = said(
        &mut renderer,
        &mut editor,
        &Opened::default(),
        Style::plain(),
    )
    .expect("the line to be taken");

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

    said(
        &mut renderer,
        &mut editor,
        &Opened::default(),
        Style::plain(),
    )
    .expect("the line to be taken");

    assert!(editor.is_empty());
}

#[test]
fn the_row_is_read_off_the_engine_rather_than_from_a_copy_of_the_mode() {
    // The drift this rules out: a mode kept beside the engine and drawn from
    // there would keep saying what the session started in, and the row that is
    // meant to say what is in force would be the one thing that could be wrong
    // about it.
    let mut runner = engine(Mode::Ask);
    assert_eq!(saying(&runner).mode, "ask mode on");

    runner.cycle();
    assert_eq!(saying(&runner).mode, "allow edits on");
}

#[test]
fn stepping_into_full_access_takes_effect_on_the_press() {
    // It used to go on screen first and wait to be agreed to, which put a
    // confirm on a key nobody reaches for by accident — and a confirm nobody
    // needs is what teaches people to dismiss the ones they do.
    let mut runner = engine(Mode::AllowEdits);
    runner.cycle();

    assert_eq!(runner.mode(), Mode::FullAccess);
    assert_eq!(saying(&runner).mode, "full access mode on");
    assert_eq!(saying(&runner).tone, tone(Mode::FullAccess));
}

#[test]
fn the_row_names_the_key_that_steps_the_mode_in_every_mode_alike() {
    // The row is the only thing that says the mode is a control at all, so it
    // says so wherever the ring has landed rather than in some of it.
    for mode in [Mode::Ask, Mode::AllowEdits, Mode::FullAccess] {
        assert_eq!(settled(mode).keys, CYCLE, "{mode:?}");
    }
}

#[test]
fn the_row_under_the_box_is_drawn_from_characters_every_terminal_has() {
    // The glyph set does not reach this row, so what it says has to be legible
    // in both: a mark added to the sentence or to the key beside it would show
    // as a hollow square on a terminal that asked for `ascii`.
    for mode in [Mode::Ask, Mode::AllowEdits, Mode::FullAccess] {
        let says = settled(mode);

        assert!(says.mode.is_ascii(), "{:?}", says.mode);
        assert!(says.keys.is_ascii(), "{:?}", says.keys);
    }
}

#[test]
fn a_line_beginning_with_a_slash_opens_the_list_above_the_box() {
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("/m"),
        Style::plain(),
        &settled(Mode::Ask),
        &listing("/m"),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    let listed = written.find("/model").expect("the list");

    // The wall is what says this is the box rather than the marked row of the
    // list, which now carries a caret of its own.
    let boxed = written.find("│ › /m").expect("the box");

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
        &listing("/m"),
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
        &Opened::default(),
    )
    .expect("the box to be drawn");

    assert!(
        !renderer.terminal().written().contains("/model"),
        "a list opened over a line that is not a command"
    );
}

#[test]
fn the_offer_to_leave_is_drawn_under_the_mode_and_not_over_it() {
    // Below the bottom row and at the left of it, so nothing that was on screen
    // moves or is covered: the box, the line in it and the mode are all exactly
    // where they were on the keystroke before.
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &Editor::new(),
        Style::plain(),
        &leaving(Mode::Ask),
        &Opened::default(),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    let mode = written.find("ask mode on").expect("the mode");
    let offer = written.find(LEAVING).expect("the offer");

    assert!(mode < offer, "the offer went above the mode: {written:?}");
    assert!(
        written.contains(&format!("\r\n{LEAVING}")),
        "the offer is indented: {written:?}"
    );
}

#[test]
fn a_second_interrupt_leaves_only_while_the_first_is_still_recent() {
    // The press this rules out is a Ctrl-C aimed at a turn that had already
    // finished. The terminal sends it here instead, and a session ended by one
    // stray key is a session nobody asked to end -- so the pair has to read as
    // one gesture, and two presses a minute apart do not.
    let first = Instant::now();

    assert!(!together(None, first), "one press is not a pair");
    assert!(together(Some(first), first + TOGETHER / 2));

    // The window is the time the second press has, so a press at the end of it
    // is a press that arrived after.
    assert!(!together(Some(first), first + TOGETHER));
    assert!(!together(Some(first), first + TOGETHER * 30));
}

#[test]
fn a_list_with_no_room_left_for_it_is_not_opened_at_all() {
    // Cut off at the top it would read as the whole list, and the rewind that
    // takes the region back would reach over rows the terminal has already
    // taken. Neither is worth the rows it would have shown.
    let every = command::filtering("/", Glyphs::Unicode).len();

    for room in 0..every {
        assert!(
            listing("/").rows(60, room, Glyphs::Unicode).is_empty(),
            "a list of {every} opened with room for {room}"
        );
    }

    assert!(!listing("/").rows(60, every, Glyphs::Unicode).is_empty());
}

#[test]
fn return_takes_the_row_the_list_is_pointing_at_and_not_the_letters_typed() {
    // What a list being chosen from is for. A half-typed name names no
    // command, so a line that showed the command and then rejected it would be
    // right and wrong about the same thing on the same screen.
    let mut renderer = drawing();
    let mut editor = typed("/resu");

    let asked = said(
        &mut renderer,
        &mut editor,
        &listing("/resu"),
        Style::plain(),
    )
    .expect("the line to be taken");

    assert!(matches!(asked, Asked::Said(line) if line == "/resume"));
}

#[test]
fn a_line_that_is_no_command_is_taken_exactly_as_it_was_typed() {
    // Nothing filtered means nothing to point at, and a line the list has no
    // opinion about is the line.
    let mut renderer = drawing();
    let mut editor = typed("what does /resume do");

    let asked = said(
        &mut renderer,
        &mut editor,
        &listing("what does /resume do"),
        Style::plain(),
    )
    .expect("the line to be taken");

    assert!(matches!(asked, Asked::Said(line) if line == "what does /resume do"));
}

#[test]
fn the_row_return_takes_is_the_same_rule_for_every_command_there_is() {
    // No command is a case of its own. The list is one list, the mark is one
    // field on it, and both are built by walking the array every command is
    // declared in — so one added later is filtered, marked and runnable
    // without anybody coming back here.
    for one in command::filtering("/", Glyphs::Unicode) {
        // Typed in full it is that command, however many longer names begin
        // with the same letters.
        assert_eq!(listing(one.name).chosen(), Some(one.name), "{}", one.name);

        // Typed as far as the letters that could only be it, the mark is
        // already there and return finishes the name.
        for cut in 1..one.name.len() {
            let Some(said) = one.name.get(..cut) else {
                continue;
            };

            let open = listing(said);

            if open.shown.len() == 1 {
                assert_eq!(open.chosen(), Some(one.name), "{said}");
            }
        }
    }
}

#[test]
fn a_line_naming_a_command_outright_points_at_it_and_not_at_the_longer_one() {
    // `/mode` is a prefix of `/model`, so the first row the filter left is the
    // wrong row: return on a name typed in full would run a different command
    // that merely starts the same way.
    assert_eq!(listing("/mode").chosen(), Some("/mode"));

    // One letter short of it, the first is all there is to go on.
    assert_eq!(listing("/mod").chosen(), Some("/model"));
}

#[test]
fn the_arrows_move_the_mark_and_stop_at_both_ends_of_the_list() {
    // Stopping rather than running round to the other end. A list is short
    // enough to read whole, and what the key returns is what says whether the
    // frame it would cost is worth drawing.
    let mut open = listing("/m");

    assert_eq!(open.chosen(), Some("/model"));
    assert!(!open.up(), "the top row moved back off the list");

    assert!(open.down());
    assert_eq!(open.chosen(), Some("/mode"));
    assert!(!open.down(), "the last row moved on past the end");

    assert!(open.up());
    assert_eq!(open.chosen(), Some("/model"));
}

#[test]
fn a_line_with_no_list_open_has_no_row_for_an_arrow_to_move_to() {
    let mut open = Opened::default();

    assert!(!open.up());
    assert!(!open.down());
    assert_eq!(open.chosen(), None);
}

#[test]
fn the_row_return_would_run_is_the_marked_one_in_the_list_on_screen() {
    // The mark and the row return takes are one fact, read off one field. Two
    // of them would be a list pointing at one command and a key running
    // another.
    let mut open = listing("/m");
    open.down();

    let text: Vec<String> = open
        .rows(60, 10, Glyphs::Unicode)
        .iter()
        .map(Row::text)
        .collect();

    let passed = text.first().expect("a row the mark has left");
    let chosen = text.get(1).expect("the row it moved to");

    assert!(passed.starts_with("  /model"), "{passed:?}");
    assert!(chosen.starts_with("› /mode"), "{chosen:?}");
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
