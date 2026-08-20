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
        assert_eq!(moving(arrived.clone(), &mut at, 3), Moved::Left, "{arrived:?}");
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
        assert_eq!(moving(arrived.clone(), &mut at, 3), Moved::Still, "{arrived:?}");
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
