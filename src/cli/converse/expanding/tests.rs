use crucible_tui::Recording;

use super::*;

/// A view opened `from` rows down, with `end` rows below it.
fn standing(from: usize, end: usize) -> Window {
    Window {
        from,
        end,
        ..Window::default()
    }
}

/// Results cut down to a row, under the lines of the calls they answer.
fn cut(called: &[&str]) -> Kept {
    let mut kept = Kept::default();

    for one in called {
        kept.calling((*one).to_owned());
        kept.finished(format!("{one} answered this\nand then this\n").into());
    }

    kept
}

/// The window a view is open over, or a failed test if it is closed.
fn opened(standing: &mut Standing) -> &mut Window {
    match standing {
        Standing::Open(window) => window,
        Standing::Closed => panic!("nothing is standing"),
    }
}

/// One result long enough to fill any window it is stood in.
fn overflowing() -> Kept {
    let mut kept = Kept::default();

    kept.calling("Bash(cargo build)".to_owned());
    kept.finished("a line of it\n".repeat(200).into());

    kept
}

#[test]
fn the_view_under_a_turn_leaves_the_tail_a_row_to_go_on_writing_into() {
    // The live region is the tail plus whatever stands under it, and the tail
    // keeps a row whatever is asked of it — so a view as tall as the window
    // would leave a region a row taller than the screen, and the top of that
    // has already scrolled out of reach of the next rewind. The row it gives up
    // is the one the turn goes on writing into.
    //
    // Asserted against the same picture drawn by hand at that height rather
    // than against a count of rows, so the caret is pinned with it: it parks on
    // the view's last row, where the box parks it too.
    let held = overflowing();

    let mut standing = Standing::default();
    standing.open(&held);

    let mut stood = Renderer::new(Recording::new(80, 24));
    assert!(under(&mut stood, Style::plain(), &held, &mut standing).expect("drawn"));

    let mut same = Standing::default();
    same.open(&held);
    let rows = laying(&held, opened(&mut same), Glyphs::Unicode, 80, 23);
    assert_eq!(rows.len(), 23);

    let mut by_hand = Renderer::new(Recording::new(80, 24));
    let caret = Caret {
        row: rows.len().saturating_sub(1),
        column: 0,
    };
    by_hand
        .under(&rows, Some(caret), Style::plain().palette())
        .expect("drawn");

    assert_eq!(stood.terminal().written(), by_hand.terminal().written());
}

#[test]
fn a_window_with_no_room_for_the_view_closes_it_and_gives_the_box_back() {
    // The chrome alone is four rows, so a window this short would stand a
    // header and a footer with none of the text they are about. The caller
    // draws the box when this answers no, which is the same answer the region
    // gives between turns.
    let held = overflowing();

    let mut standing = Standing::default();
    standing.open(&held);

    let mut render = Renderer::new(Recording::new(80, 4));
    assert!(!under(&mut render, Style::plain(), &held, &mut standing).expect("drawn"));

    assert_eq!(standing, Standing::Closed);
    assert_eq!(render.terminal().written(), "");
}

#[test]
fn nothing_opens_where_nothing_was_cut() {
    // The key is offered by the rows that were cut, so a session with none of
    // them has made no offer — and a view put up in answer to a press nobody
    // meant is one that took the prompt away for no reason.
    let mut standing = Standing::default();
    standing.open(&Kept::default());

    assert_eq!(standing, Standing::Closed);
    assert!(!standing.is_open());
}

#[test]
fn a_view_stands_still_while_the_turn_under_it_goes_on_cutting() {
    // What it stands over is what had been cut when it opened. Letting in the
    // results arriving underneath would slide the rows being read down the
    // screen as each one landed, which is unreadable exactly when there is most
    // to read.
    let mut held = cut(&["Bash(cargo build)"]);

    let mut standing = Standing::default();
    standing.open(&held);
    let before = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);

    held.calling("Bash(cargo test)".to_owned());
    held.finished("something else entirely\n".into());

    let after = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);
    assert_eq!(before, after);
}

#[test]
fn opening_it_again_is_what_brings_the_newer_results_in() {
    // Which is why standing still costs the reader nothing: what a turn cut
    // while they were reading is one press away, and the press is the one they
    // already know.
    let mut held = cut(&["Bash(cargo build)"]);

    let mut standing = Standing::default();
    standing.open(&held);
    let before = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);

    held.calling("Bash(cargo test)".to_owned());
    held.finished("something else entirely\n".into());

    standing.open(&held);
    let after = laying(&held, opened(&mut standing), Glyphs::Unicode, 60, 20);
    assert_ne!(before, after);
}

#[test]
fn the_key_that_opened_it_closes_it_under_a_running_turn_too() {
    // The half of the toggle that has no loop of its own: under a turn the view
    // is handed one key at a time by the loop reading for the box, and the way
    // out has to be the same key it is between turns.
    let mut standing = Standing::default();
    standing.open(&cut(&["Bash(cargo build)"]));

    assert!(standing.against(Pressed::Expand));
    assert_eq!(standing, Standing::Closed);
}

#[test]
fn esc_under_a_turn_closes_the_view_rather_than_stopping_the_turn() {
    // Esc belongs to whatever is standing, and while the view is standing that
    // is the view. The turn is still running behind it and the row that offers
    // to interrupt comes back with the box.
    let mut standing = Standing::default();
    standing.open(&cut(&["Bash(cargo build)"]));

    assert!(standing.against(Pressed::Escape));
    assert_eq!(standing, Standing::Closed);
}

#[test]
fn a_key_that_moves_nothing_under_a_turn_owes_no_frame() {
    // The loop this is called from redraws whenever anything says it moved, and
    // a frame here lays the whole of what was cut out again. A key against the
    // top of the view must not be one.
    let mut standing = Standing::Open(standing(0, 20));

    assert!(!standing.against(Pressed::Up));
    assert!(standing.against(Pressed::Down));
    assert!(standing.is_open());
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
