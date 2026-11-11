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
