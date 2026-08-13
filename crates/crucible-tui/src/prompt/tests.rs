use crate::color::Palette;
use crate::dump::dump;

use super::*;

/// The widths worth walking: every one the component changes shape at, and a
/// stretch on either side of each.
const WIDTHS: std::ops::RangeInclusive<usize> = 1..=200;

/// What the status row says while this release is being typed into.
const MODE: &str = "ask before edits";

/// What is said quietly after it.
const HINT: &str = "(shift+tab to cycle)";

/// The three modes as the row under the box spells them, the colour each puts
/// on the border, and the name of the picture each is checked against.
///
/// The words a session actually shows, unlike [`MODE`] above, which is a
/// fixture for the tests that are about the row rather than about the mode.
const MODES: [(&str, Slot, &str); 3] = [
    ("ask mode on", Slot::Quiet, "ask"),
    ("allow edits on", Slot::AllowEdits, "allow_edits"),
    ("full access mode on", Slot::FullAccess, "full_access"),
];

/// What something waiting on the very next key says while it waits.
const ASKING: &str = "press ctrl-c again to leave";

/// How many rows of line the tests give a box, unless one is about the ceiling.
///
/// Generous, so that every line below is drawn whole and the assertions are
/// about the wrapping rather than about what the ceiling took away.
const ROOM: usize = 8;

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
        asking: None,
        room: ROOM,
    }
}

/// How many rows of a drawn component the line itself took.
fn lines(prompt: &Prompt<'_>, columns: usize) -> usize {
    let chrome = if columns < FRAMED_AT { 1 } else { 3 };

    prompt.rows(columns, Glyphs::Unicode).len() - chrome
}

/// The same, with the cursor at the end of what was typed.
fn typed(said: &str) -> Prompt<'_> {
    typing(said, width::columns(said))
}

/// An empty box with something waiting on the very next key under it.
fn asked() -> Prompt<'static> {
    Prompt {
        asking: Some(ASKING),
        ..typed("")
    }
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

/// The component at `columns`, against the picture checked in beside it under
/// `name@columns`.
///
/// The width is the suffix rather than something written into the name, so that
/// a picture cannot end up checked against a drawing of some other terminal:
/// one argument decides both what was drawn and which file it is read from.
fn pictured(name: &str, prompt: &Prompt<'_>, columns: usize, glyphs: Glyphs) {
    insta::with_settings!({snapshot_suffix => columns.to_string()}, {
        insta::assert_snapshot!(name, dump(&prompt.rows(columns, glyphs), columns));
    });
}

/// A palette that writes every hue it has.
fn colourful() -> Palette {
    Palette::resolve(true, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

#[test]
fn every_row_of_a_framed_box_ends_at_its_last_column() {
    // The property the component is built to hold. A row past the last column
    // is one the terminal wraps itself, which leaves the cursor a row below
    // where the next frame expects it -- so the next frame erases somebody
    // else's line.
    for columns in WIDTHS.filter(|columns| *columns >= FRAMED_AT) {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for said in SAID {
                let rows = typed(said).rows(columns, glyphs);
                let framed = rows.len() - 1;

                for row in rows.iter().take(framed) {
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
fn the_box_grows_a_row_for_every_row_the_line_takes() {
    // A prompt is written and read at the same time. Scrolled sideways instead,
    // a paragraph being written is a paragraph nobody can see.
    let said = "abcdefghijklmnopqrstuvwxyz";

    // Eighteen columns inside the frame at this width, so twenty-six characters
    // are two rows.
    assert_eq!(lines(&typed(said), FRAMED_AT), 2);
    assert_eq!(lines(&typed(""), FRAMED_AT), 1);

    // And wider, where the same line fits on one.
    assert_eq!(lines(&typed(said), 80), 1);
}

#[test]
fn a_line_that_exactly_fills_a_row_takes_the_next_one_as_well() {
    // The cursor after the last character would otherwise stand on the padding
    // beside the border, which is not where the next character appears.
    let filling = "a".repeat(inner(FRAMED_AT));

    assert_eq!(lines(&typed(&filling), FRAMED_AT), 2);
    assert_eq!(typed(&filling).caret(FRAMED_AT).column, FRAMED);
}

#[test]
fn the_box_stops_growing_at_the_room_it_was_given() {
    // The region is taken back by moving the cursor up over it, so a box taller
    // than the screen is one that could not be taken back at all.
    let said = "a".repeat(1000);
    let capped = Prompt {
        room: 3,
        ..typed(&said)
    };

    assert_eq!(lines(&capped, FRAMED_AT), 3);
}

#[test]
fn how_much_room_a_window_gives_a_box_is_about_half_of_it() {
    // Enough to write a paragraph in, and never so much that what the prompt is
    // a reply to is pushed off the screen.
    assert_eq!(Prompt::room(24), 9);
    assert_eq!(Prompt::room(48), 21);

    // And never nothing, however short the window is: a box with no row to type
    // on is not a box.
    assert_eq!(Prompt::room(6), 1);
    assert_eq!(Prompt::room(1), 1);
}

#[test]
fn the_line_is_typed_after_the_mark_inside_the_frame() {
    pictured("short", &typed("hello"), 30, Glyphs::Unicode);
}

#[test]
fn a_box_nothing_has_been_typed_into_is_the_same_box() {
    // The one on screen for longer than any other, and the one every keystroke
    // is drawn over. It is a row of line either way: an empty box that closed
    // up would open again on the first character and move what is above it.
    pictured("empty", &typed(""), FRAMED_AT, Glyphs::Unicode);
    pictured("empty", &typed(""), 80, Glyphs::Unicode);
}

#[test]
fn a_font_with_no_box_drawing_in_it_gets_a_box_of_the_same_shape() {
    // Same width, same rows, same columns held for the mark: the set changes
    // what a border is drawn with and nothing about where anything sits.
    pictured("short_ascii", &typed("hello"), 30, Glyphs::Ascii);
}

#[test]
fn a_terminal_too_narrow_for_a_frame_gets_the_mark_and_the_mode() {
    // The border would cost a quarter of the screen to say what the mark
    // already says. Both sets, because the mark is the last chrome left and it
    // is drawn out of whichever one is in force.
    pictured("bare", &typed("hello"), FRAMED_AT - 1, Glyphs::Unicode);
    pictured("bare_ascii", &typed("hello"), FRAMED_AT - 1, Glyphs::Ascii);
}

#[test]
fn a_mode_is_a_colour_on_the_border_and_a_sentence_under_the_box() {
    // Three pictures rather than three assertions about a slot: what the reader
    // meets is the border and the row under it changing together, and a mode
    // given one and not the other is a screen that says two things.
    for (mode, tone, name) in MODES {
        let prompt = Prompt {
            mode,
            tone,
            ..typed("")
        };

        pictured(name, &prompt, 80, Glyphs::Unicode);
    }
}

#[test]
fn a_question_waiting_on_the_next_key_is_drawn_under_the_status_row() {
    pictured("asking", &asked(), 80, Glyphs::Unicode);
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
fn a_line_longer_than_the_box_wraps_onto_the_next_row() {
    // Eighteen columns inside the frame at this width. The rows under the first
    // are indented to match it, so a line that wrapped reads as one line.
    let said = "abcdefghijklmnopqrstuvwxyz";
    pictured("wrapped", &typed(said), FRAMED_AT, Glyphs::Unicode);

    assert_eq!(
        typed(said).caret(FRAMED_AT),
        Caret {
            row: FRAMED_ROW + 1,
            column: FRAMED + 8,
        }
    );
}

#[test]
fn the_window_follows_the_cursor_back_up_the_line() {
    // Worked out from the cursor every time rather than remembered: a kept
    // scroll position is a second piece of state the line can get out of step
    // with. With one row of room the cursor at the start brings the first row
    // back into view.
    let said = "abcdefghijklmnopqrstuvwxyz";
    let capped = Prompt {
        room: 1,
        ..typing(said, 0)
    };

    pictured("held_at_the_top", &capped, FRAMED_AT, Glyphs::Unicode);
    assert_eq!(capped.caret(FRAMED_AT).column, FRAMED);

    // And the cursor at the end of it scrolls the first row back under the top
    // edge.
    let capped = Prompt {
        room: 1,
        ..typed(said)
    };

    pictured("scrolled", &capped, FRAMED_AT, Glyphs::Unicode);
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
            let framed = rows.len() - 1;

            for (at, row) in rows.iter().enumerate().take(framed) {
                assert_eq!(
                    row.columns(),
                    columns,
                    "at {columns}, cursor at {column}, row {at}"
                );
            }
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
fn a_question_takes_a_row_of_its_own_under_the_status_and_starts_at_the_left() {
    // Under rather than beside: the mode holds until somebody changes it and
    // this holds until the next keystroke, so a row shared between the two
    // would have one of them read as the other.
    for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
        let rows = drawn(&asked(), 80, glyphs);

        assert_eq!(rows.len(), 5, "{rows:?}");
        assert!(
            rows.get(3).is_some_and(|status| status.starts_with(MODE)),
            "the status row moved: {rows:?}"
        );
        assert_eq!(rows.get(4).map(String::as_str), Some(ASKING));
    }
}

#[test]
fn nothing_asking_takes_no_row_at_all() {
    // The ordinary state, and the one every other test here is drawn in: the
    // component is the height it has always been, so the box does not sit a row
    // higher for the whole session.
    assert_eq!(drawn(&typed(""), 80, Glyphs::Unicode).len(), 4);
    assert_eq!(drawn(&typed(""), 10, Glyphs::Unicode).len(), 2);
}

#[test]
fn a_question_is_clipped_to_the_width_rather_than_dropped() {
    // Unlike the keys after the mode: half of this still names the key that is
    // waiting, and the row is only there because somebody has just pressed it.
    for columns in WIDTHS {
        let rows = asked().rows(columns, Glyphs::Unicode);
        let last = rows.last().expect("a row the component drew");

        assert!(last.columns() <= columns, "at {columns}: {:?}", last.text());
        assert!(
            ASKING.starts_with(&last.text()),
            "at {columns}: {:?}",
            last.text()
        );
    }
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
