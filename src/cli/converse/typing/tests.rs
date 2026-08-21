//! What the box does with a line, on a terminal that records rather than draws.
//!
//! Raw mode is the one thing not asserted here. Entering it reaches the
//! controlling terminal, which under a test harness is the one the tests are
//! being run in — so [`super::ask`] is exercised only where it declines to, and
//! everything after that point is called directly.

use crucible_core::{Mode, Permission, Rules, ToolArgs};
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
            window: None,
            system: None,
            effort: None,
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

/// No plan above the box, which is what a session has until the agent writes
/// one — and what every test in this file is drawn with but the last two, since
/// none of the others is about the panel.
fn nothing() -> Planning {
    Planning::new(crucible_tools::Plan::new())
}

/// A plan of `count` open tasks, each named after where it is in the list.
///
/// Written through the tool the way the model writes one, because that is the
/// only way anything gets into a plan and the panel is drawn from what came out
/// the other side.
fn planned(count: usize) -> Planning {
    let said = (0..count)
        .map(|at| format!(r#"{{"task":"Task {at}","state":"open"}}"#))
        .collect::<Vec<_>>()
        .join(",");

    let plan = crucible_tools::Plan::new();
    plan.replay(&ToolArgs::new(format!(r#"{{"tasks":[{said}]}}"#)));

    Planning::new(plan)
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
    // The keyboard is held by the loop above for the whole session now, so what
    // reaches here is whether there was one to hold. `false` is every
    // `crucible < script.txt` and every redirected run, and the caller reads a
    // line for itself when it gets this back.
    let mut renderer = drawing();
    let mut runner = engine(Mode::Ask);
    let mut editor = crucible_tui::Editor::new();

    let asked = ask(
        &mut renderer,
        Style::plain(),
        Between {
            runner: &mut runner,
            editor: &mut editor,
            planning: &mut nothing(),
            left: &crucible_tools::Background::new(),
            keys: false,
        },
    )
    .expect("no keys to read");

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
        around(&nothing(), &Opened::default(), &settled(Mode::Ask)),
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
        around(&nothing(), &Opened::default(), &settled(Mode::Ask)),
    )
    .expect("the box to be drawn");

    let written = renderer.terminal().written();
    assert!(written.contains("ask mode on"), "{written:?}");
    assert!(!written.contains(CYCLE), "{written:?}");
}

#[test]
fn the_cursor_ends_up_where_the_line_was_typed_to() {
    // On the row the line is on and at the end of what has been typed, rather
    // than wherever drawing the rows below it happened to leave the cursor.
    // Parked anywhere else, the next character is drawn under the box rather
    // than in it.
    let mut renderer = drawing();

    draw(
        &mut renderer,
        &typed("hi"),
        Style::plain(),
        around(&nothing(), &Opened::default(), &settled(Mode::Ask)),
    )
    .expect("the box to be drawn");

    let shown = renderer.terminal().picture();
    let rows = shown.rows();
    let (row, column) = shown.caret();

    assert!(
        rows.get(row).is_some_and(|row| row.contains("› hi")),
        "the cursor is not on the row being typed on: {rows:?}"
    );
    let before: Option<String> = rows.get(row).map(|row| row.chars().take(column).collect());

    assert_eq!(
        before.as_deref(),
        Some("│ › hi"),
        "the cursor is not at the end of the line: {rows:?}"
    );
}

#[test]
fn a_finished_line_is_left_in_the_record_and_the_box_is_taken_off() {
    let mut renderer = drawing();
    let mut editor = typed("hi");

    draw(
        &mut renderer,
        &editor,
        Style::plain(),
        around(&nothing(), &Opened::default(), &settled(Mode::Ask)),
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
fn a_blank_row_parts_the_prompt_from_the_answer_above_it() {
    // The transcript is a column of blocks and what separates one from the next
    // is a row of nothing. Every block asks for that row on its way in; this is
    // the one caller that was not asking, so a prompt landed against the last
    // row of the answer it was replying to.
    let mut renderer = drawing();
    let mut editor = typed("hi");

    renderer
        .commit("whatever the last turn said")
        .expect("a row to be committed");
    let before = renderer.lines();

    said(
        &mut renderer,
        &mut editor,
        &Opened::default(),
        Style::plain(),
    )
    .expect("the line to be taken");

    assert_eq!(
        renderer.lines() - before,
        2,
        "the blank and the prompt, in that order"
    );
}

#[test]
fn a_wrapped_prompt_grows_the_record_by_every_row_it_actually_drew() {
    // The defect this closes: the row was handed over unwrapped, the terminal
    // broke it, and the record counted one where the screen showed several. A
    // caller that means to point at a row later reads this number, so a count
    // short by two points two rows off.
    let mut renderer = Renderer::new(Recording::new(24, 24));
    let line = "why does the grep probe walk the whole tree before it reports";
    let mut editor = typed(line);

    let drawn = Prompt::committed(line, 24, Style::plain().glyphs(), true).len();
    assert!(drawn > 1, "the fixture has to wrap to be testing anything");

    said(
        &mut renderer,
        &mut editor,
        &Opened::default(),
        Style::plain(),
    )
    .expect("the line to be taken");

    assert_eq!(renderer.lines(), drawn);
}

#[test]
fn no_blank_row_is_spent_at_the_top_of_a_session() {
    // The boundary there is the start of the session, and a transcript that
    // opens on an empty row has spent one for nothing.
    let mut renderer = drawing();
    let mut editor = typed("hi");

    said(
        &mut renderer,
        &mut editor,
        &Opened::default(),
        Style::plain(),
    )
    .expect("the line to be taken");

    assert_eq!(renderer.lines(), 1, "the prompt and nothing above it");
}

#[test]
fn a_second_prompt_in_a_row_is_parted_from_the_first_by_one_blank_and_no_more() {
    // Two prompts with nothing between them: the rhythm is one blank row, never
    // two, and `apart` is what holds that rather than each caller counting.
    let mut renderer = drawing();

    for _ in 0..2 {
        let mut editor = typed("hi");
        said(
            &mut renderer,
            &mut editor,
            &Opened::default(),
            Style::plain(),
        )
        .expect("the line to be taken");
    }

    assert_eq!(renderer.lines(), 3, "prompt, blank, prompt");
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
    // The words are the same in both sets — only the marks between them come
    // out of the setting — so what a run that asked for `ascii` is left with
    // has to be drawable there. A word added to the sentence or to the keys
    // beside it would show as a hollow square on the terminal least able to
    // spare one.
    for mode in [Mode::Ask, Mode::AllowEdits, Mode::FullAccess] {
        let says = settled(mode);

        assert!(says.mode.is_ascii(), "{:?}", says.mode);
        assert!(says.keys.is_ascii(), "{:?}", says.keys);

        let running = under(&engine(mode), Glyphs::Ascii);
        assert!(running.keys.is_ascii(), "{:?}", running.keys);
    }
}

#[test]
fn a_line_beginning_with_a_slash_opens_the_list_above_the_box() {
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("/m"),
        Style::plain(),
        around(&nothing(), &listing("/m"), &settled(Mode::Ask)),
    )
    .expect("the box to be drawn");

    // Asked of the picture rather than of the bytes. The box and what stands
    // over it are two frames now — they go into two bands and the box goes
    // first — so the order they were written in is not the order they are read
    // in, and only the window says where each of them ended up.
    let shown = renderer.terminal().picture();
    let rows = shown.rows();
    let listed = at(&rows, "/model");

    // The wall is what says this is the box rather than the marked row of the
    // list, which now carries a caret of its own.
    let boxed = at(&rows, "│ › /m");

    assert!(listed < boxed, "the list is under the box: {rows:?}");
}

/// Which row of a window `looked` is on.
///
/// Panics where nothing on screen holds it, which is the failure every caller
/// would otherwise have written for itself.
fn at(rows: &[String], looked: &str) -> usize {
    rows.iter()
        .position(|row| row.contains(looked))
        .unwrap_or_else(|| panic!("nothing on screen says {looked:?}: {rows:?}"))
}

#[test]
fn the_box_stays_where_it_was_while_the_list_is_open() {
    // The whole reason it opens upwards. The box has the rows above the status
    // row whether or not anything is standing over it, so a list arriving takes
    // rows off the transcript rather than pushing the box down the screen: the
    // box, the line in it and the mode are where they were on the keystroke
    // before, and so is the cursor.
    let shown = |opened: &Opened| {
        let mut renderer = roomy();

        draw(
            &mut renderer,
            &typed("/m"),
            Style::plain(),
            around(&nothing(), opened, &settled(Mode::Ask)),
        )
        .expect("the box to be drawn");

        let shown = renderer.terminal().picture();
        (shown.rows().split_off(19), shown.caret())
    };

    let (was, parked) = shown(&Opened::default());
    let (now, moved) = shown(&listing("/m"));

    assert_eq!(now, was, "the box moved when the list opened");
    assert_eq!(moved, parked, "the cursor moved when the list opened");
}

#[test]
fn a_prompt_is_drawn_in_the_rows_the_box_has_always_been() {
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("hi"),
        Style::plain(),
        around(&nothing(), &Opened::default(), &settled(Mode::Ask)),
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
        around(&nothing(), &Opened::default(), &leaving(Mode::Ask)),
    )
    .expect("the box to be drawn");

    let shown = renderer.terminal().picture();
    let rows = shown.rows();
    let mode = rows
        .iter()
        .position(|row| row.contains("ask mode on"))
        .expect("the mode");

    assert_eq!(
        rows.get(mode + 1).map(String::as_str),
        Some(LEAVING),
        "the offer is not the row under the mode: {rows:?}"
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
    // Cut off at the top it would read as the whole list, which is a worse
    // answer than no list at all: nothing is what a reader can tell is nothing.
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
fn esc_is_what_stops_a_turn_and_ctrl_c_is_the_line_s_own_in_both_loops() {
    // The two halves of one change. Esc was a key with nothing to act on while
    // a turn ran, which is what made it the free slot; Ctrl-C was caught here
    // before the editor saw it, which is what made it mean one thing at the
    // prompt and another mid turn. Swapping them is what leaves it meaning the
    // same thing in both loops.
    assert_eq!(meant(Pressed::Escape), Meant::Interrupt);
    assert_eq!(
        meant(Pressed::Key(Key::Interrupt)),
        Meant::Editing(Key::Interrupt)
    );
}

#[test]
fn every_other_key_read_during_a_turn_keeps_the_meaning_it_had() {
    // The rest of the table, pinned so that the swap above is the whole change.
    // Ctrl-D still reaches the editor.
    assert_eq!(meant(Pressed::Resized), Meant::Resized);
    assert_eq!(meant(Pressed::Key(Key::Char('a'))), Meant::Typing('a'));
    assert_eq!(meant(Pressed::Key(Key::Eof)), Meant::Editing(Key::Eof));
    assert_eq!(meant(Pressed::Key(Key::Left)), Meant::Editing(Key::Left));

    for arrived in [Pressed::Up, Pressed::Down, Pressed::Cycle, Pressed::Ignored] {
        assert_eq!(meant(arrived.clone()), Meant::Ignored, "{arrived:?}");
    }
}

#[test]
fn neither_spelling_of_return_is_decided_here() {
    // Which press finishes a line and which opens one under it is `input.send`,
    // and the editor is the one thing that reads it. Answering Return here
    // instead read it for the reader: on a terminal keeping the modified Return
    // for itself, the arrangement that exists for exactly that case had the
    // bare key queueing a line the reader meant to break, and the modified key
    // — which the editor did answer, with a line to send — dropped on the way
    // back. Both spellings go to the editor now, and what comes back says which
    // it was.
    assert_eq!(meant(Pressed::Key(Key::Enter)), Meant::Editing(Key::Enter));
    assert_eq!(
        meant(Pressed::Key(Key::Newline)),
        Meant::Editing(Key::Newline)
    );
}

#[test]
fn a_click_read_during_a_turn_keeps_both_the_row_and_the_column() {
    // A click means more while a turn runs rather than less, and it means two
    // things: the row is what a result the transcript cut short is opened by,
    // and the column is where the cursor goes in a line being written under the
    // answer. Dropping either would leave one of the two unanswerable.
    assert_eq!(
        meant(Pressed::Clicked { row: 4, column: 2 }),
        Meant::Clicked(Pointed { row: 4, column: 2 })
    );
}

#[test]
fn ctrl_o_does_what_the_row_offering_it_says_while_the_turn_is_still_running() {
    // The row saying `ctrl+o to expand` is drawn by the turn that cut the
    // result, and the moment somebody reads it is the moment it goes past. A
    // key that waited for the turn to yield would be one whose offer is on
    // screen for the whole stretch it does nothing.
    assert_eq!(meant(Pressed::Expand), Meant::Expand);
}

#[test]
fn the_row_under_a_running_turn_names_the_key_that_interrupts_it() {
    // The one place the key is printed beside the thing it acts on while a turn
    // is the thing on screen. It earns the room the same way the mode's key
    // does: nothing else says the row is a control at all.
    let says = under(&engine(Mode::Ask), Glyphs::Unicode);

    assert_eq!(says.keys, interrupting(Glyphs::Unicode));
    assert!(says.keys.contains("esc"), "{:?}", says.keys);

    // And says nothing about the key that no longer stops a turn. Naming it
    // here is what would teach it twice: it does the same thing under a turn
    // that it does at the prompt, and a row that mentions it mid turn is a row
    // claiming otherwise.
    assert!(!says.keys.contains("ctrl"), "{:?}", says.keys);
}

#[test]
fn what_parts_the_two_keys_under_a_running_turn_comes_out_of_the_glyph_set() {
    // The row above the box parts its own segments with this mark, two rows
    // away and out of the setting already. One of the two drawn from the
    // setting and one written down would show as a hollow square beside a mark
    // that came out right, on a screen where they are meant to read as a pair.
    for (glyphs, said) in [
        (Glyphs::Unicode, "(enter queues it · esc to interrupt)"),
        (Glyphs::Ascii, "(enter queues it - esc to interrupt)"),
    ] {
        assert_eq!(under(&engine(Mode::Ask), glyphs).keys, said, "{glyphs:?}");
    }
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

#[test]
fn the_plan_stands_above_the_box_between_turns() {
    // The same panel a running turn draws over the box, in the same place while
    // nothing is running. What the agent was working to is what the next prompt
    // is typed against, so it does not come down with the turn that wrote it.
    let mut renderer = roomy();

    draw(
        &mut renderer,
        &typed("hi"),
        Style::plain(),
        around(&planned(3), &Opened::default(), &settled(Mode::Ask)),
    )
    .expect("the box to be drawn");

    let shown = renderer.terminal().picture();
    let rows = shown.rows();
    assert!(at(&rows, "3 tasks") < at(&rows, "› hi"), "{rows:?}");
}

#[test]
fn the_list_a_slash_opened_takes_its_rows_before_the_plan_does() {
    // The list is the shorter of the two and it was opened by the character
    // last typed, which is a stronger claim on a short window than a panel that
    // was already standing. So the panel is what gives way, and it gives way a
    // task at a time rather than all at once.
    let short = || Renderer::new(Recording::new(60, 14));
    let plan = planned(6);

    let mut alone = short();
    draw(
        &mut alone,
        &typed("hi"),
        Style::plain(),
        around(&plan, &Opened::default(), &settled(Mode::Ask)),
    )
    .expect("the box to be drawn");

    let mut beside = short();
    draw(
        &mut beside,
        &typed("/m"),
        Style::plain(),
        around(&plan, &listing("/m"), &settled(Mode::Ask)),
    )
    .expect("the box to be drawn");

    let tasks = |written: &str| written.matches("Task ").count();
    let (alone, beside) = (alone.terminal().written(), beside.terminal().written());

    assert!(tasks(alone) > 0, "{alone:?}");
    assert!(tasks(beside) < tasks(alone), "{beside:?} against {alone:?}");
    assert!(beside.contains("/model"), "{beside:?}");
}

#[test]
fn the_key_that_copies_takes_the_line_and_not_the_picture_of_it() {
    // What a drag over the box takes is the border, the padding and the ground
    // between them, which is why the key exists. So what goes out is the text
    // the editor holds -- no box, no mode row, no trailing spaces, and the
    // break between two rows of one line still a break.
    let mut renderer = roomy();
    let mut editor = Editor::new().multiline();
    assert_eq!(
        editor.paste("cargo test --all\nand a second line"),
        Typed::Changed
    );

    assert_eq!(
        copy(&mut renderer, &editor).expect("the line to go"),
        Some(COPIED)
    );

    // The line, encoded, and nothing around it. Written out rather than encoded
    // here, so that what this asserts is what a terminal will read.
    assert_eq!(
        renderer.terminal().written(),
        "\x1b]52;c;Y2FyZ28gdGVzdCAtLWFsbAphbmQgYSBzZWNvbmQgbGluZQ==\x07"
    );
}

#[test]
fn a_box_with_nothing_in_it_says_nothing_and_leaves_the_clipboard_alone() {
    // A reader who pressed the key over an empty box has been answered by the
    // box; a row saying so would be noise. And emptying the clipboard is the
    // one thing a copy must never be mistaken for.
    let mut renderer = roomy();

    assert_eq!(
        copy(&mut renderer, &Editor::new()).expect("no request"),
        None
    );
    assert_eq!(renderer.terminal().written(), "");
}
