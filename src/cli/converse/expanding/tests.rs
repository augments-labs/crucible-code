use super::*;

/// A view opened `from` rows down, with `end` rows below it.
fn standing(from: usize, end: usize) -> Standing {
    Standing { from, end }
}

#[test]
fn the_arrows_walk_the_window_one_row_at_a_time() {
    let mut open = standing(4, 20);

    assert_eq!(moving(Pressed::Down, &mut open), Moved::Redraw);
    assert_eq!(open, standing(5, 20));

    assert_eq!(moving(Pressed::Up, &mut open), Moved::Redraw);
    assert_eq!(open, standing(4, 20));
}

#[test]
fn an_arrow_against_an_end_costs_no_frame() {
    // A key held down against the top or the bottom is the whole of what this
    // saves: the picture has not changed, so nothing is drawn for it.
    let mut top = standing(0, 20);
    assert_eq!(moving(Pressed::Up, &mut top), Moved::Still);
    assert_eq!(top, standing(0, 20));

    let mut bottom = standing(20, 20);
    assert_eq!(moving(Pressed::Down, &mut bottom), Moved::Still);
    assert_eq!(bottom, standing(20, 20));
}

#[test]
fn a_view_with_nothing_below_it_does_not_scroll() {
    // Everything fitted, so the last row is on screen and the footer does not
    // name the arrows. One that moved anyway would be a window walking off the
    // bottom of what it holds.
    let mut whole = standing(0, 0);

    assert_eq!(moving(Pressed::Down, &mut whole), Moved::Still);
    assert_eq!(whole, standing(0, 0));
}

#[test]
fn the_key_that_opened_it_closes_it() {
    // The rows offering it say `ctrl+o to expand` and nothing else, so the same
    // key against what it opened is the whole of the way back.
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Expand, &mut open), Moved::Left);
}

#[test]
fn esc_closes_it_the_way_esc_closes_everything_else() {
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Escape, &mut open), Moved::Left);
}

#[test]
fn the_keys_the_line_underneath_owns_take_the_view_with_them() {
    // Ctrl-C throws the line away and Ctrl-D ends the session, and both reach
    // the line whatever is standing over it. The view goes first so that the
    // key does what it has always done rather than being swallowed here.
    for key in [Key::Interrupt, Key::Eof] {
        let mut open = standing(3, 20);
        assert_eq!(moving(Pressed::Key(key), &mut open), Moved::Left, "{key:?}");
    }
}

#[test]
fn return_scrolls_nothing_and_sends_nothing() {
    // The line under this is not being read. Closing on Return would send
    // whatever is in the box to the model the moment somebody meant to scroll.
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Key(Key::Enter), &mut open), Moved::Still);
    assert_eq!(open, standing(3, 20));
}

#[test]
fn a_resize_owes_the_next_frame() {
    // How many rows the results came to is a fact about the width, so the whole
    // picture is laid out again and `end` is answered again with it.
    let mut open = standing(3, 20);

    assert_eq!(moving(Pressed::Resized, &mut open), Moved::Redraw);
    assert_eq!(open, standing(3, 20));
}

#[test]
fn nothing_else_moves_it() {
    let ignored = [
        Pressed::Cycle,
        Pressed::Explain,
        Pressed::Clicked { row: 2, column: 8 },
        Pressed::Ignored,
        Pressed::Key(Key::Char('q')),
    ];

    for arrived in ignored {
        let mut open = standing(3, 20);
        assert_eq!(moving(arrived, &mut open), Moved::Still, "{arrived:?}");
        assert_eq!(open, standing(3, 20));
    }
}
