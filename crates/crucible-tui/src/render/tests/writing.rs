//! The cursor while a line standing over the box is being written: it stands
//! in that line, and only in a line that is drawn.

use super::*;

/// Stands the box under `over` while a turn runs, with the cursor where
/// `writing` says a line over the box is being written.
fn writing_over(drawn: &mut Drawn, over: &[Row], writing: Caret) {
    let (rows, caret) = boxed();
    let prompt = PromptRows::new(&rows, caret, None)
        .unwrap()
        .writing_over(Some(writing));
    drawn
        .replace_running(prompt, &standing(), over, Palette::plain())
        .unwrap();
}

#[test]
fn a_line_written_over_the_box_takes_the_cursor() {
    // A question docked over the box with a note being written in it: the
    // keys are typing into the note, not the box.
    let mut drawn = Drawn::new(40, 10);
    let over = vec![Row::plain("Which?"), Row::plain("Note: hi")];
    writing_over(&mut drawn, &over, Caret { row: 1, column: 8 });

    let (row, column) = drawn.screen().caret();
    assert_eq!(drawn.screen().row(row), "Note: hi");
    assert_eq!(column, 8);
}

#[test]
fn a_caret_off_the_rows_over_the_box_leaves_the_cursor_in_the_box() {
    let over = vec![Row::plain("Which?"), Row::plain("Note: hi")];
    for off in [Caret { row: 2, column: 0 }, Caret { row: 1, column: 9 }] {
        let mut drawn = Drawn::new(40, 10);
        writing_over(&mut drawn, &over, off);

        assert_eq!(drawn.screen().caret(), (8, 4), "{off:?}");
    }
}

#[test]
fn a_line_written_over_the_box_past_the_rows_the_window_draws_leaves_the_cursor_in_the_box() {
    // The turn's row and four over the box, on a window with room for three
    // over it: the note being written is not drawn, so the keys have nowhere
    // on the screen to stand but the box.
    let mut drawn = Drawn::new(40, 6);
    let over = vec![
        Row::plain("Which?"),
        Row::plain("  1. this"),
        Row::plain("  2. that"),
        Row::plain("Note: hi"),
    ];
    writing_over(&mut drawn, &over, Caret { row: 3, column: 8 });

    let screen = drawn.screen();
    assert!(
        (0..6).all(|row| screen.row(row) != "Note: hi"),
        "the note is drawn: {screen:?}"
    );
    let (row, column) = screen.caret();
    assert_eq!((screen.row(row), column), ("│ ›  │", 4), "{screen:?}");
}

#[test]
fn a_line_written_on_the_first_row_the_window_does_not_draw_leaves_the_cursor_in_the_box() {
    // The turn's row and three over the box, on a window with room for three:
    // the note is the first row past those drawn, so the cursor stays in the
    // box rather than on the row the box begins on.
    let mut drawn = Drawn::new(40, 6);
    let over = vec![
        Row::plain("Which?"),
        Row::plain("  1. this"),
        Row::plain("Note: hi"),
    ];
    writing_over(&mut drawn, &over, Caret { row: 2, column: 8 });

    let screen = drawn.screen();
    assert!(
        (0..6).all(|row| screen.row(row) != "Note: hi"),
        "the note is drawn: {screen:?}"
    );
    let (row, column) = screen.caret();
    assert_eq!((screen.row(row), column), ("│ ›  │", 4), "{screen:?}");
}
