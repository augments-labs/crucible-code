//! What the keys do to a list being chosen from.

use crucible_tui::Key;

use super::*;

#[test]
fn the_arrows_walk_the_mark_down_the_list_and_back_up_it() {
    let mut at = 0;

    assert_eq!(moving(Pressed::Down, &mut at, 3), Moved::Redraw);
    assert_eq!(at, 1);
    assert_eq!(moving(Pressed::Down, &mut at, 3), Moved::Redraw);
    assert_eq!(at, 2);
    assert_eq!(moving(Pressed::Up, &mut at, 3), Moved::Redraw);
    assert_eq!(at, 1);
}

#[test]
fn the_mark_stops_at_each_end_rather_than_wrapping_round() {
    // A list read as a ring puts the first entry one key below the last, so the
    // key somebody pressed to go too far is the key they press again to come
    // back — and it lands them at the other end instead.
    let mut at = 0;
    assert_eq!(moving(Pressed::Up, &mut at, 3), Moved::Still);
    assert_eq!(at, 0);

    let mut at = 2;
    assert_eq!(moving(Pressed::Down, &mut at, 3), Moved::Still);
    assert_eq!(at, 2);

    // A list of one is both ends at once.
    let mut at = 0;
    assert_eq!(moving(Pressed::Up, &mut at, 1), Moved::Still);
    assert_eq!(moving(Pressed::Down, &mut at, 1), Moved::Still);
    assert_eq!(at, 0);
}

#[test]
fn return_takes_the_entry_the_mark_is_on() {
    let mut at = 1;

    assert_eq!(moving(Pressed::Key(Key::Enter), &mut at, 3), Moved::Took);
    assert_eq!(at, 1, "the one under the mark, not the one after it");
}

#[test]
fn escape_leaves_and_so_do_the_keys_that_end_a_session() {
    // Ctrl-C is what somebody presses when a panel is up and they did not mean
    // to open one, and Ctrl-D is what they press against a prompt they have
    // nothing to put in. Answering neither leaves them pressing harder.
    for arrived in [
        Pressed::Escape,
        Pressed::Key(Key::Interrupt),
        Pressed::Key(Key::Eof),
    ] {
        let mut at = 1;
        assert_eq!(
            moving(arrived.clone(), &mut at, 3),
            Moved::Left,
            "{arrived:?}"
        );
    }
}

#[test]
fn a_key_the_panel_has_no_meaning_for_costs_no_frame() {
    // A frame each would be a frame per keystroke for somebody typing at a list.
    for arrived in [
        Pressed::Key(Key::Char('a')),
        Pressed::Key(Key::Backspace),
        Pressed::Cycle,
        Pressed::Ignored,
        Pressed::Clicked { row: 4, column: 2 },
    ] {
        let mut at = 1;
        assert_eq!(
            moving(arrived.clone(), &mut at, 3),
            Moved::Still,
            "{arrived:?}"
        );
        assert_eq!(at, 1);
    }
}

#[test]
fn a_window_that_changed_size_is_answered_by_drawing_again() {
    // The panel has to be measured against the new height as well as the new
    // width: it is the component that gives up rows rather than overflowing.
    let mut at = 1;

    assert_eq!(moving(Pressed::Resized, &mut at, 3), Moved::Redraw);
    assert_eq!(at, 1);
}

/// A shelf with three providers over four models and five rungs, marked at the
/// top of each.
///
/// Strings for the models because nothing here is told what a model is: what
/// the keys move is a mark, and what comes back off it is whatever the caller
/// narrowed.
fn standing() -> Standing<&'static str> {
    Standing {
        query: crucible_tui::Editor::new(),
        pane: crucible_tui::Pane::Models,
        provider: 0,
        model: 0,
        rung: 0,
        models: vec!["first", "second", "third", "fourth"],
        providers: 4,
        rungs: 5,
        pointer: None,
        lit: None,
    }
}

#[test]
fn the_down_arrows_walk_whichever_pane_the_mark_is_in() {
    let mut shelf = standing();

    assert_eq!(searching(Pressed::Down, &mut shelf), Moved::Redraw);
    assert_eq!(shelf.model, 1);
    assert_eq!(shelf.provider, 0);

    // Crossing panes moves neither mark. Two panes read as one list would lose
    // the row somebody was on every time they looked at the other pane.
    assert_eq!(searching(Pressed::Tab, &mut shelf), Moved::Redraw);
    assert_eq!(shelf.model, 1);

    assert_eq!(searching(Pressed::Down, &mut shelf), Moved::Redraw);
    assert_eq!(shelf.provider, 1);
}

#[test]
fn the_mark_stops_at_each_end_of_the_pane_it_is_in() {
    let mut shelf = standing();
    assert_eq!(searching(Pressed::Up, &mut shelf), Moved::Still);
    assert_eq!(shelf.model, 0);

    shelf.model = 3;
    assert_eq!(searching(Pressed::Down, &mut shelf), Moved::Still);
    assert_eq!(shelf.model, 3);
}

#[test]
fn walking_the_providers_sends_the_model_and_the_rung_back_to_the_top() {
    // A different provider is a different shelf. A mark left where it was
    // stands on whatever slid under it, and the rung beside it is one the new
    // model may not serve at all.
    let mut shelf = standing();
    shelf.model = 2;
    shelf.rung = 3;
    shelf.pane = crucible_tui::Pane::Providers;

    assert_eq!(searching(Pressed::Down, &mut shelf), Moved::Redraw);
    assert_eq!(shelf.provider, 1);
    assert_eq!(shelf.model, 0);
    assert_eq!(shelf.rung, 0);
}

#[test]
fn the_across_arrows_walk_the_rungs_rather_than_the_query() {
    // The one contested binding, settled by the picture: the strip has a left
    // and a right drawn on it and the search line does not. Home, End and the
    // word keys are still the line's, which is what keeps a long query
    // editable.
    let mut shelf = standing();

    assert_eq!(
        searching(Pressed::Key(Key::Char('o')), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(
        searching(Pressed::Key(Key::Right), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.rung, 1);
    assert_eq!(shelf.query.text(), "o");
    assert_eq!(shelf.query.column(), 1);

    assert_eq!(
        searching(Pressed::Key(Key::Left), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.rung, 0);
    assert_eq!(searching(Pressed::Key(Key::Left), &mut shelf), Moved::Still);
}

#[test]
fn a_model_that_serves_no_rung_has_a_strip_the_arrows_do_not_move() {
    let mut shelf = standing();
    shelf.rungs = 0;

    assert_eq!(
        searching(Pressed::Key(Key::Right), &mut shelf),
        Moved::Still
    );
    assert_eq!(shelf.rung, 0);
}

#[test]
fn a_key_that_changes_the_query_sends_the_marks_back_to_the_top() {
    let mut shelf = standing();
    shelf.model = 3;
    shelf.rung = 2;

    assert_eq!(
        searching(Pressed::Key(Key::Char('k')), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.query.text(), "k");
    assert_eq!(shelf.model, 0);
    assert_eq!(shelf.rung, 0);

    shelf.model = 3;
    assert_eq!(
        searching(Pressed::Key(Key::Backspace), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.query.text(), "");
    assert_eq!(shelf.model, 0);
}

#[test]
fn a_query_typed_against_a_marked_provider_sends_that_mark_back_too() {
    // Somebody standing on one vendor who types another vendor's name has
    // asked two questions with no answer between them, and the pane would win
    // -- leaving a shelf that says nothing matches while the models plainly
    // exist. The line is the thing they can see themselves having done, so the
    // line is the one that has to win.
    let mut shelf = standing();
    shelf.provider = 3;
    shelf.model = 2;
    shelf.rung = 4;

    assert_eq!(
        searching(Pressed::Key(Key::Char('a')), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.provider, 0);
    assert_eq!(shelf.model, 0);
    assert_eq!(shelf.rung, 0);
}

#[test]
fn a_key_that_only_moves_the_cursor_leaves_the_marks_where_they_are() {
    // Somebody pressing Home to fix the front of a word they mistyped has not
    // asked for the row under the mark to move.
    let mut shelf = standing();
    let _ = searching(Pressed::Key(Key::Char('k')), &mut shelf);
    shelf.provider = 2;
    shelf.model = 2;
    shelf.rung = 1;

    assert_eq!(
        searching(Pressed::Key(Key::Home), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.query.text(), "k");
    assert_eq!(shelf.provider, 2);
    assert_eq!(shelf.model, 2);
    assert_eq!(shelf.rung, 1);
}

#[test]
fn pasted_text_goes_to_the_search_line() {
    let mut shelf = standing();
    shelf.model = 2;

    assert_eq!(
        searching(Pressed::Pasted("opus".into()), &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.query.text(), "opus");
    assert_eq!(shelf.model, 0);
}

#[test]
fn return_takes_nothing_where_the_query_left_nothing_on_the_shelf() {
    // Closing on an empty shelf would take the row the mark is nominally on,
    // which is no row at all — and the reader is looking at a search line that
    // says why there is nothing there.
    let mut shelf = standing();
    shelf.models.clear();

    assert_eq!(
        searching(Pressed::Key(Key::Enter), &mut shelf),
        Moved::Still
    );

    shelf.models.push("first");
    assert_eq!(searching(Pressed::Key(Key::Enter), &mut shelf), Moved::Took);
}

#[test]
fn escape_leaves_the_shelf_and_the_wheel_moves_nothing_on_it() {
    let mut shelf = standing();
    assert_eq!(searching(Pressed::Escape, &mut shelf), Moved::Left);
    assert_eq!(
        searching(Pressed::Key(Key::Interrupt), &mut shelf),
        Moved::Left
    );
    assert_eq!(searching(Pressed::Key(Key::Eof), &mut shelf), Moved::Left);

    // A shelf is not a window over more than it holds, so the wheel belongs to
    // the transcript underneath it.
    assert_eq!(
        searching(Pressed::Scrolled { back: true }, &mut shelf),
        Moved::Still
    );
    assert_eq!(shelf.model, 0);
}

#[test]
fn the_pointer_is_remembered_and_only_a_new_place_is_worth_a_frame() {
    // A terminal reporting full motion sends a press per cell crossed, and most
    // of them land where the last one did. Answering every one with a redraw
    // would be a frame per cell for a picture that came back the same.
    let mut shelf = standing();

    assert_eq!(
        searching(Pressed::Hovered { row: 9, column: 5 }, &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.pointer, Some((9, 5)));

    assert_eq!(
        searching(Pressed::Hovered { row: 9, column: 5 }, &mut shelf),
        Moved::Still
    );

    assert_eq!(
        searching(Pressed::Hovered { row: 9, column: 6 }, &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.pointer, Some((9, 6)));
}

#[test]
fn a_pointer_that_has_left_puts_the_row_it_was_lighting_out() {
    // Nowhere is a place too. A pointer gone off the shelf is reported as one
    // past every row it drew, and the row it was lighting has to go out --
    // otherwise a hand at rest on the transcript still claims a model.
    let mut shelf = standing();
    searching(Pressed::Hovered { row: 9, column: 5 }, &mut shelf);

    assert_eq!(
        searching(
            Pressed::Hovered {
                row: usize::MAX,
                column: usize::MAX
            },
            &mut shelf
        ),
        Moved::Redraw
    );
    assert_eq!(shelf.pointer, None);
}

#[test]
fn the_pointer_moves_no_mark_and_takes_nothing() {
    // Two different questions: what a reader is looking at, and what the next
    // key acts on. A hand crossing the window on its way somewhere else has
    // chosen nothing, and enter after it takes what the arrows left.
    let mut shelf = standing();
    searching(Pressed::Down, &mut shelf);
    let walked = (shelf.pane, shelf.provider, shelf.model, shelf.rung);

    searching(Pressed::Hovered { row: 9, column: 5 }, &mut shelf);
    searching(
        Pressed::Hovered {
            row: 11,
            column: 40,
        },
        &mut shelf,
    );

    assert_eq!(
        (shelf.pane, shelf.provider, shelf.model, shelf.rung),
        walked
    );
    assert_eq!(searching(Pressed::Key(Key::Enter), &mut shelf), Moved::Took);
}

#[test]
fn a_click_marks_the_row_it_is_lit_on_and_a_second_one_takes_it() {
    // Two steps, the same two the keys are: the arrows put the mark somewhere
    // and enter takes it. They stay two because the rung is taken with the
    // model, and a click that took on sight would close the shelf before
    // anybody could say how hard to think.
    let mut shelf = standing();
    shelf.pointer = Some((9, 40));
    shelf.lit = Some(Resting::Model(2));

    assert_eq!(
        searching(Pressed::Clicked { row: 9, column: 40 }, &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.model, 2);
    assert_eq!(shelf.pane, Pane::Models);

    assert_eq!(
        searching(Pressed::Clicked { row: 9, column: 40 }, &mut shelf),
        Moved::Took
    );
    assert_eq!(shelf.model, 2);
}

#[test]
fn a_click_takes_what_is_lit_and_lights_a_place_that_is_not() {
    // A terminal that answers motion has already reported the pointer crossing
    // the row, so the two places are one. A terminal that does not would
    // otherwise have a click take whatever the last frame happened to light —
    // which is a row somewhere else on the screen.
    let mut shelf = standing();
    shelf.pointer = Some((9, 40));
    shelf.lit = Some(Resting::Model(3));

    assert_eq!(
        searching(
            Pressed::Clicked {
                row: 14,
                column: 40
            },
            &mut shelf
        ),
        Moved::Redraw
    );
    assert_eq!(shelf.pointer, Some((14, 40)));
    assert_eq!(shelf.model, 0, "nothing was taken off a row nobody lit");
}

#[test]
fn a_click_on_a_provider_narrows_the_pane_beside_it_and_never_takes() {
    // A provider is not an answer. It says which models to show, the way it
    // does under the arrows, and the two marks it governs go back to the top of
    // a pane that is about to hold something else.
    let mut shelf = standing();
    shelf.model = 3;
    shelf.rung = 4;
    shelf.pointer = Some((11, 5));
    shelf.lit = Some(Resting::Provider(2));

    assert_eq!(
        searching(Pressed::Clicked { row: 11, column: 5 }, &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.provider, 2);
    assert_eq!(shelf.pane, Pane::Providers);
    assert_eq!((shelf.model, shelf.rung), (0, 0));

    // And clicking it again says so and stops, rather than taking a row that is
    // not a model.
    assert_eq!(
        searching(Pressed::Clicked { row: 11, column: 5 }, &mut shelf),
        Moved::Still
    );
    assert_eq!(shelf.provider, 2);
}

#[test]
fn a_click_that_crosses_into_a_pane_is_a_frame_even_where_the_mark_is_already_there() {
    // The pane the arrows walk moves with the click, and which pane that is
    // shows: it is the mark drawn strongly. A click landing on the row the mark
    // is already on has still changed the picture.
    let mut shelf = standing();
    shelf.provider = 2;
    shelf.pane = Pane::Models;
    shelf.pointer = Some((11, 5));
    shelf.lit = Some(Resting::Provider(2));

    assert_eq!(
        searching(Pressed::Clicked { row: 11, column: 5 }, &mut shelf),
        Moved::Redraw
    );
    assert_eq!(shelf.pane, Pane::Providers);
    assert_eq!(shelf.model, 0, "the pane was crossed, not renarrowed");
}

#[test]
fn a_click_on_a_row_the_shelf_lit_nothing_on_moves_nothing() {
    // The blanks padding a pane, the line saying a query matched nothing, the
    // count of what is scrolled past. None of them is a row to take, and none
    // of them lit, so a click on one is a click on the frame.
    let mut shelf = standing();
    shelf.pointer = Some((20, 40));
    shelf.lit = None;

    assert_eq!(
        searching(
            Pressed::Clicked {
                row: 20,
                column: 40
            },
            &mut shelf
        ),
        Moved::Still
    );
    assert_eq!((shelf.model, shelf.provider), (0, 0));
}
