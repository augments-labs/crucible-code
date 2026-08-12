use crate::color::Palette;

use super::*;

/// The widths worth walking: every one the component changes shape at, and a
/// stretch on either side of each.
const WIDTHS: std::ops::RangeInclusive<usize> = 1..=200;

/// What the status row says while this release is being typed into.
const MODE: &str = "ask before edits";

/// What is said quietly after it.
const HINT: &str = "(shift+tab to cycle)";

/// Lines worth drawing at every width: nothing, something short, something far
/// longer than any box, and something that is two columns per character.
const SAID: [&str; 4] = [
    "",
    "rename the tail's bound",
    "why does the grep probe walk the whole tree before it reports the first hit",
    "日本語のテキストを入れてみる",
];

/// A prompt with `said` typed into it and the cursor `column` columns along.
fn typing(said: &str, column: usize) -> Prompt<'_> {
    Prompt {
        said,
        column,
        mode: MODE,
        tone: Slot::Accent,
        hint: HINT,
    }
}

/// The same, with the cursor at the end of what was typed.
fn typed(said: &str) -> Prompt<'_> {
    typing(said, width::columns(said))
}

/// What the component says, with no colour in it.
fn drawn(prompt: &Prompt<'_>, columns: usize, glyphs: Glyphs) -> Vec<String> {
    prompt.rows(columns, glyphs).iter().map(Row::text).collect()
}

/// One row of that, by its place from the top.
fn row(prompt: &Prompt<'_>, at: usize, columns: usize, glyphs: Glyphs) -> String {
    drawn(prompt, columns, glyphs)
        .get(at)
        .cloned()
        .expect("a row the component drew")
}

/// A palette that writes every hue it has.
fn colourful() -> Palette {
    Palette::resolve(true, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

#[test]
fn a_framed_terminal_gets_four_rows_and_the_box_ends_at_its_last_column() {
    // The property the component is built to hold. A row past the last column
    // is one the terminal wraps itself, which leaves the cursor a row below
    // where the next frame expects it -- so the next frame erases somebody
    // else's line.
    for columns in WIDTHS.filter(|columns| *columns >= FRAMED_AT) {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for said in SAID {
                let rows = typed(said).rows(columns, glyphs);
                assert_eq!(rows.len(), 4, "{glyphs:?} at {columns}");

                for row in rows.iter().take(3) {
                    assert_eq!(
                        row.columns(),
                        columns,
                        "{glyphs:?} at {columns}: {:?}",
                        row.text()
                    );
                }
            }
        }
    }
}

#[test]
fn nothing_is_ever_drawn_past_the_last_column() {
    // Including the status row, which is never padded out to the width, and
    // including every terminal too narrow for a frame at all.
    for columns in WIDTHS {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for said in SAID {
                for row in typed(said).rows(columns, glyphs) {
                    assert!(
                        row.columns() <= columns,
                        "{glyphs:?} at {columns}: {:?}",
                        row.text()
                    );
                }
            }
        }
    }
}

#[test]
fn the_box_is_the_same_height_however_long_the_line_is() {
    // A box that grew a row as the line got longer would push everything above
    // it up the screen on a keystroke, and the reader would be typing into
    // something that moves.
    for columns in [FRAMED_AT, 40, 80] {
        let heights: Vec<usize> = SAID
            .iter()
            .map(|said| typed(said).rows(columns, Glyphs::Unicode).len())
            .collect();

        assert_eq!(heights, vec![4; SAID.len()], "at {columns}");
    }
}

#[test]
fn the_line_is_typed_after_the_mark_inside_the_frame() {
    let rows = drawn(&typed("hello"), 30, Glyphs::Unicode);

    assert_eq!(
        rows,
        vec![
            format!("╭{}╮", "─".repeat(28)),
            "│ › hello                    │".to_owned(),
            format!("╰{}╯", "─".repeat(28)),
            MODE.to_owned(),
        ]
    );
}

#[test]
fn a_font_with_no_box_drawing_in_it_gets_a_box_of_the_same_shape() {
    // Same width, same rows, same columns held for the mark: the set changes
    // what a border is drawn with and nothing about where anything sits.
    let rows = drawn(&typed("hello"), 30, Glyphs::Ascii);

    assert_eq!(
        rows,
        vec![
            format!("+{}+", "-".repeat(28)),
            "| > hello                    |".to_owned(),
            format!("+{}+", "-".repeat(28)),
            MODE.to_owned(),
        ]
    );
}

#[test]
fn a_terminal_too_narrow_for_a_frame_gets_the_mark_and_the_mode() {
    // The border would cost a quarter of the screen to say what the mark
    // already says.
    let rows = drawn(&typed("hello"), 20, Glyphs::Unicode);

    assert_eq!(rows, vec!["› hello".to_owned(), MODE.to_owned()]);
}

#[test]
fn the_cursor_sits_where_the_line_was_typed_to() {
    // Counted from the top left of the rows rather than from the terminal's
    // origin, which an inline renderer never learns.
    let prompt = typed("hello");

    assert_eq!(
        prompt.caret(80),
        Caret {
            row: FRAMED_ROW,
            column: FRAMED + 5,
        }
    );

    // And where there is no frame, on the only row there is.
    assert_eq!(
        prompt.caret(20),
        Caret {
            row: 0,
            column: BARE + 5,
        }
    );
}

#[test]
fn a_wide_character_costs_the_cursor_two_columns() {
    // Placing it by counting characters would land it three columns short of
    // where the reader can see their own text end.
    let said = "日本語";
    let caret = typed(said).caret(80);
    assert_eq!(caret.column, FRAMED + 6);

    // Which is exactly where the row that was drawn runs out of text.
    let row = row(&typed(said), FRAMED_ROW, 80, Glyphs::Unicode);
    let upto = width::cut(&row, caret.column).and_then(|at| row.get(..at));

    assert_eq!(upto, Some("│ › 日本語"));
}

#[test]
fn the_cursor_is_never_left_standing_on_the_border() {
    // A line that filled the box exactly would put it there, which is what the
    // column the window holds back is for.
    for columns in WIDTHS.filter(|columns| *columns >= FRAMED_AT) {
        for said in SAID {
            let caret = typed(said).caret(columns);

            assert!(
                (FRAMED..columns - CLOSING).contains(&caret.column),
                "at {columns}: the cursor sat at {}",
                caret.column
            );
        }
    }
}

#[test]
fn a_line_longer_than_the_box_is_windowed_onto_its_end() {
    // Windowed rather than wrapped: the box is a fixed number of rows, and a
    // wrap would need another one. Eighteen columns inside the frame at this
    // width, one of them held back so the cursor is not on the edge.
    let said = "abcdefghijklmnopqrstuvwxyz";

    assert_eq!(
        row(&typed(said), FRAMED_ROW, FRAMED_AT, Glyphs::Unicode),
        "│ › jklmnopqrstuvwxyz  │"
    );
    assert_eq!(typed(said).caret(FRAMED_AT).column, FRAMED + 17);
}

#[test]
fn the_window_follows_the_cursor_back_up_the_line() {
    // Worked out from the cursor every time rather than remembered: a kept
    // scroll position is a second piece of state the line can get out of step
    // with.
    let said = "abcdefghijklmnopqrstuvwxyz";

    assert_eq!(
        row(&typing(said, 0), FRAMED_ROW, FRAMED_AT, Glyphs::Unicode),
        "│ › abcdefghijklmnopqr │"
    );
    assert_eq!(typing(said, 0).caret(FRAMED_AT).column, FRAMED);
}

#[test]
fn a_window_never_cuts_a_wide_character_in_half() {
    // Half of one drawn is a column the border was already counted for, so the
    // frame would close a column early on the row being typed on and nowhere
    // else.
    let said = "日本語のテキストを入れてみる";

    for columns in WIDTHS.filter(|columns| *columns >= FRAMED_AT) {
        for column in 0..=width::columns(said) {
            let typed = typing(said, column);
            let rows = typed.rows(columns, Glyphs::Unicode);
            let line = rows.get(FRAMED_ROW).map(Row::columns);

            assert_eq!(line, Some(columns), "at {columns}, cursor at {column}");
        }
    }
}

#[test]
fn the_status_row_says_the_mode_and_then_the_keys_that_change_it() {
    assert_eq!(
        row(&typed(""), 3, 80, Glyphs::Unicode),
        format!("{MODE} {HINT}")
    );
}

#[test]
fn a_hint_is_drawn_only_where_the_whole_of_it_fits() {
    // Half of the keys to press is not half as useful as all of them.
    let together = width::columns(MODE) + 1 + width::columns(HINT);

    assert_eq!(
        row(&typed(""), 3, together, Glyphs::Unicode),
        format!("{MODE} {HINT}")
    );
    assert_eq!(row(&typed(""), 3, together - 1, Glyphs::Unicode), MODE);
}

#[test]
fn the_status_row_is_not_padded_out_to_the_width() {
    // It holds no edge up, and trailing spaces on it are bytes written every
    // keystroke to draw nothing.
    let status = row(&typed(""), 3, 80, Glyphs::Unicode);

    assert!(status.len() < 80);
    assert!(!status.ends_with(' '), "{status:?}");
}

#[test]
fn the_mode_is_readable_with_no_colour_at_all() {
    // The border carries it as a hue and this row carries it as words, which is
    // what keeps the colour from being the only thing that says it.
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        assert!(row(&typed(""), 3, 80, glyphs).contains(MODE));
    }
}

#[test]
fn the_border_is_drawn_in_the_tone_the_mode_was_given() {
    // Every part of it, so a mode that changed cannot leave one edge behind in
    // the last mode's colour.
    let prompt = Prompt {
        tone: Slot::FullAccess,
        ..typed("")
    };

    let opened = colourful().open(Slot::FullAccess);
    assert!(!opened.is_empty(), "the palette had no hue to test with");

    for row in prompt.rows(80, Glyphs::Unicode).iter().take(3) {
        assert!(
            row.paint(colourful()).starts_with(opened),
            "{:?}",
            row.text()
        );
    }
}

#[test]
fn a_line_that_was_committed_keeps_the_mark_and_is_not_clipped() {
    // Nothing is ever drawn over a settled row, so a line longer than the
    // terminal is the terminal's to wrap and costs no count this process keeps.
    let said = "why does the grep probe walk the whole tree before it reports";

    assert_eq!(
        Prompt::committed(said, Glyphs::Unicode).text(),
        format!("› {said}")
    );
    assert_eq!(
        Prompt::committed(said, Glyphs::Ascii).text(),
        format!("> {said}")
    );
}
