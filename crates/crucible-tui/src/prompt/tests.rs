use crate::color::Palette;
use crate::color::Theme;
use crate::dump::dump;

use super::*;

/// The widths worth walking: every one the component changes shape at, and a
/// stretch on either side of each.
const WIDTHS: std::ops::RangeInclusive<usize> = 1..=200;

/// What a set of rows says, one string each, for an assertion about the art
/// rather than about the colour.
fn rows_of(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

/// What the status row says while this release is being typed into.
const MODE: &str = "ask before edits";

/// What is said quietly after it.
const HINT: &str = "(shift+tab to cycle)";

/// The model the other end of the status row says the next turn goes to.
const NAMED: &str = "claude-sonnet-5";

/// Whose model it is, said before it.
const VENDOR: &str = "anthropic";

/// How hard it says that model is being asked to think.
const RUNG: &str = "high";

/// The three of them as the row joins them.
const MODEL: &str = "anthropic/claude-sonnet-5 · high";

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
const ASKING: &str = "press ctrl+c again to leave";

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
        line: 0,
        column,
        mode: MODE,
        tone: Slot::Accent,
        hint: HINT,
        // Empty, so that every test written before this row had a right-hand
        // end is still a test about its left-hand one.
        model: "",
        provider: "",
        effort: None,
        asking: None,
        // Nothing, so that every test written before this row said what was
        // running is still a test about what it said before.
        running: None,
        room: ROOM,
    }
}

/// The same box, with a model to say on the right of its status row.
fn asking_of(said: &str) -> Prompt<'_> {
    Prompt {
        model: NAMED,
        provider: VENDOR,
        effort: Some(RUNG),
        ..typed(said)
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
    Palette::resolve(true, Theme::Dark, None, &|name| {
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
fn the_model_being_asked_stands_against_the_right_edge_of_the_status_row() {
    // The one fact the welcome card cannot keep. `/model` and `/effort` both
    // change it, and by then the card is scrollback -- which this process
    // draws inline and can never go back over. So it lives on the one row that
    // is redrawn every keystroke, at the end away from the mode: the mode is
    // what the next tool call costs and this is what the next turn is asked
    // of, and run together they read as one sentence.
    let status = row(&asking_of(""), 3, 80, Glyphs::Unicode);

    assert!(status.starts_with(MODE), "{status:?}");
    assert!(status.ends_with(MODEL), "{status:?}");
    assert_eq!(width::columns(&status), 80, "{status:?}");
}

#[test]
fn the_status_row_says_whose_model_it_is_before_saying_which() {
    // The row is redrawn every keystroke, so it is the one place this can be
    // said and stay true: `/login` changes the vendor mid-session, and a name
    // on its own never said whose it was in the first place.
    let status = row(&asking_of(""), 3, 80, Glyphs::Unicode);

    assert!(status.contains("anthropic/claude-sonnet-5"), "{status:?}");
}

#[test]
fn a_row_with_no_model_says_nothing_about_a_vendor_either() {
    // A vendor is not a fact about the next turn on its own: nothing is being
    // asked of it. The row this width has is the row it had before there was
    // anything on its right at all.
    let vendorless = Prompt {
        provider: VENDOR,
        ..typed("")
    };

    assert_eq!(
        row(&vendorless, 3, 80, Glyphs::Unicode),
        row(&typed(""), 3, 80, Glyphs::Unicode),
    );
}

#[test]
fn the_status_row_is_the_mode_in_its_own_colour_and_everything_else_quietly() {
    // The mode is the subject of the row: it is the one fact on it that says
    // what a tool call arriving now costs. The keys and the model are both
    // quiet, at opposite ends, so neither competes with it for the eye.
    pictured("status", &asking_of(""), 80, Glyphs::Unicode);
}

#[test]
fn the_keys_go_before_the_model_does() {
    // Both are drawn where both fit. Past that the keys are what gives way:
    // they are a reminder of a key that is also on the welcome card, and the
    // model is the only place this fact is said at all.
    let all = width::columns(MODE) + 1 + width::columns(HINT) + APART + width::columns(MODEL);
    let both = row(&asking_of(""), 3, all, Glyphs::Unicode);

    assert!(both.contains(HINT), "{both:?}");
    assert!(both.ends_with(MODEL), "{both:?}");

    let tight = row(&asking_of(""), 3, all - 1, Glyphs::Unicode);

    assert!(!tight.contains(HINT), "{tight:?}");
    assert!(tight.ends_with(MODEL), "{tight:?}");
}

#[test]
fn a_model_with_no_room_beside_the_mode_is_not_drawn_at_all() {
    // Half a model name says which model and half an effort says nothing, so
    // neither is drawn: what the row owes above everything is the mode, and a
    // clipped fact crowding it is worse than the fact being absent.
    let least = width::columns(MODE) + APART + width::columns(MODEL);

    assert!(row(&asking_of(""), 3, least, Glyphs::Unicode).ends_with(MODEL));

    // A column narrower and none of it is there -- not the name on its own,
    // and not a shortened one. The room it gave up goes back to the keys,
    // which is the row this width had before there was anything on its right.
    let tight = row(&asking_of(""), 3, least - 1, Glyphs::Unicode);

    assert!(!tight.contains(NAMED), "{tight:?}");
    assert_eq!(tight, row(&typed(""), 3, least - 1, Glyphs::Unicode));
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

    let opened = colourful().open(Slot::FullAccess).as_str().to_owned();
    assert!(!opened.is_empty(), "the palette had no hue to test with");

    for row in prompt.rows(80, Glyphs::Unicode).iter().take(3) {
        assert!(
            row.paint(&colourful()).starts_with(&opened),
            "{:?}",
            row.text()
        );
    }
}

/// A palette over a terminal that answered with these channels.
fn over(ground: (u8, u8, u8)) -> Palette {
    Palette::resolve(true, Theme::Dark, Some(ground), &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

#[test]
fn a_committed_line_takes_the_ground_the_terminal_reported() {
    // The band. Not a colour this file chose: the reader's own, moved one step,
    // which is why it can be painted at all without taking the ground away from
    // them.
    let rows = Prompt::committed("fix the resolution", 40, Glyphs::Unicode, true);
    let painted = rows.first().expect("a row").paint(&over((13, 13, 16)));

    assert!(
        painted.contains("\x1b[48;2;"),
        "no ground on the row: {painted:?}"
    );
}

#[test]
fn the_band_reaches_the_last_column_on_every_row_it_wrapped_onto() {
    // A ground that stops where the text stops has a ragged right edge with the
    // reader's own showing through it, and a wrapped prompt would show that on
    // every row but the longest.
    let said = "why does the grep probe walk the whole tree before it reports";

    for columns in 20..=60 {
        let rows = Prompt::committed(said, columns, Glyphs::Unicode, true);
        assert!(rows.len() > 1, "the fixture has to wrap at {columns}");

        for row in &rows {
            assert_eq!(
                row.columns(),
                columns,
                "row short of the last column at {columns}: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn the_mark_on_a_committed_line_is_on_the_band_rather_than_beside_it() {
    // One row, one ground. A mark drawn in the plain accent would sit in a hole
    // the band was painted around.
    let rows = Prompt::committed("fix it", 40, Glyphs::Unicode, true);
    let painted = rows.first().expect("a row").paint(&over((13, 13, 16)));

    let mark = painted.find('›').expect("the mark");
    let opened = painted[..mark]
        .rfind("\x1b[")
        .expect("a sequence before it");

    assert!(
        painted[opened..mark].contains("48;2;"),
        "the mark is not on the band: {painted:?}"
    );
}

#[test]
fn a_committed_line_takes_no_ground_where_the_terminal_said_nothing() {
    // The state a terminal that answered neither question leaves this in, and
    // it is correct rather than merely safe: the mark, the words, and the blank
    // row above, which is what the row looked like before any of this existed.
    let plain = Palette::resolve(true, Theme::Dark, None, &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    });

    for row in Prompt::committed("fix the resolution", 40, Glyphs::Unicode, true) {
        assert!(!row.paint(&plain).contains("\x1b[48;"), "{:?}", row.text());
    }
}

#[test]
fn nothing_is_painted_on_a_committed_line_where_there_is_no_colour_at_all() {
    for row in Prompt::committed("fix the resolution", 40, Glyphs::Unicode, true) {
        assert_eq!(row.paint(&Palette::plain()), row.text());
    }
}

#[test]
fn a_line_that_was_committed_keeps_the_mark() {
    // Trimmed, because the row reaches the last column: it carries a ground,
    // and the columns past the words are the part of it with nothing written on
    // them for a reader to notice the edge by.
    let said = "why does the grep probe walk the whole tree before it reports";
    let wide = said.len() + 8;
    let trimmed = |glyphs| {
        rows_of(&Prompt::committed(said, wide, glyphs, true))
            .into_iter()
            .map(|row| row.trim_end().to_owned())
            .collect::<Vec<_>>()
    };

    assert_eq!(trimmed(Glyphs::Unicode), [format!("› {said}")]);
    assert_eq!(trimmed(Glyphs::Ascii), [format!("> {said}")]);
}

#[test]
fn a_committed_line_wider_than_the_window_is_wrapped_here_rather_than_by_the_terminal() {
    // The renderer counts the rows it drew so it can move back over them, and
    // `present` does not wrap. A row handed over wider than the window is one
    // the terminal breaks itself, leaving the count short by however many rows
    // it took -- which is the whole reason this returns more than one.
    let said = "why does the grep probe walk the whole tree before it reports \
                the first match it found";

    for columns in 12..=60 {
        let rows = Prompt::committed(said, columns, Glyphs::Unicode, true);

        assert!(rows.len() > 1, "not wrapped at {columns}: {rows:?}");
        for row in &rows {
            assert!(
                row.columns() <= columns,
                "row past the last column at {columns}: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn the_rows_under_a_wrapped_line_stand_where_the_mark_left_off() {
    // The same arrangement the box uses while the line is being typed: the mark
    // on the first row and the ones under it indented to match, so a line that
    // wrapped reads as one line rather than as a stack of separate ones.
    let said = "why does the grep probe walk the whole tree before it reports";
    let rows = rows_of(&Prompt::committed(said, 24, Glyphs::Unicode, true));
    assert!(rows.len() > 1, "nothing wrapped, so nothing is under it");

    let (first, rest) = rows.split_first().expect("a row");
    assert!(first.starts_with("› "), "{first:?}");

    for row in rest {
        assert!(
            row.starts_with("  "),
            "not indented under the mark: {row:?}"
        );
        assert!(!row.starts_with("   "), "over-indented: {row:?}");
    }
}

#[test]
fn a_wide_glyph_is_two_of_the_columns_a_committed_line_is_wrapped_to() {
    // Display width, not bytes and not characters. Getting this wrong is what
    // corrupts a screen rather than what merely looks off.
    let said = "日本語".repeat(20);

    for columns in 12..=60 {
        for row in Prompt::committed(&said, columns, Glyphs::Unicode, true) {
            assert!(
                row.columns() <= columns,
                "row past the last column at {columns}: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn a_wide_glyph_never_pushes_a_committed_row_past_the_last_column() {
    // `fold` deliberately never hands back an empty row, so at a width of one
    // it returns a two-column glyph rather than nothing — and a row built from
    // that is wider than the window it was folded for. The terminal wraps it
    // and the record is short by one per row, which is the exact miscount this
    // function exists to prevent.
    for columns in 1..=12 {
        for row in Prompt::committed("\u{65e5}\u{672c}\u{8a9e}", columns, Glyphs::Unicode, true) {
            assert!(
                row.columns() <= columns,
                "row past the last column at {columns}: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn a_committed_line_says_exactly_what_was_typed() {
    // The record is what was asked. A line that arrives at the model with four
    // spaces in front of it and reads back without them is a transcript that
    // disagrees with the request it is the record of — and pasted code is the
    // case that actually happens.
    for said in [
        "    let x = 1;",
        "\thello",
        "  two  spaces  inside  ",
        "plain",
    ] {
        let rows = Prompt::committed(said, 200, Glyphs::Unicode, false);
        let back: String = rows
            .iter()
            .map(Row::text)
            .collect::<String>()
            .strip_prefix("› ")
            .expect("the mark")
            .to_owned();

        assert_eq!(back, said, "{said:?}");
    }
}

#[test]
fn a_window_too_narrow_for_anything_still_leaves_the_mark() {
    // There is no row to draw a line on at this width, and nothing true to say
    // about the line either -- but a record with no mark in it is a record that
    // does not say a prompt was ever there.
    for columns in 0..=2 {
        let rows = Prompt::committed("hello", columns, Glyphs::Unicode, true);
        let said: Vec<String> = rows_of(&rows)
            .into_iter()
            .map(|row| row.trim_end().to_owned())
            .collect();

        assert_eq!(said, ["›"], "at {columns} columns");
    }
}

#[test]
fn a_click_on_the_line_says_how_far_into_it_the_pointer_was() {
    // Counted from the top left of the rows, which is what a caller that knows
    // where it drew the component can work out from where the mouse was.
    let said = "abcdefghijklmnopqrstuvwxyz";
    let prompt = typed(said);

    assert_eq!(prompt.clicked(FRAMED_AT, FRAMED_ROW, FRAMED), Some((0, 0)));
    assert_eq!(
        prompt.clicked(FRAMED_AT, FRAMED_ROW, FRAMED + 5),
        Some((0, 5))
    );

    // The second row of the same line carries on where the first left off.
    assert_eq!(
        prompt.clicked(FRAMED_AT, FRAMED_ROW + 1, FRAMED + 3),
        Some((0, 21))
    );
}

#[test]
fn a_click_past_the_end_of_a_row_lands_where_the_line_ends() {
    // Which is where the eye reads a line as ending, and what every other
    // terminal does with the same click.
    let prompt = typed("hello");

    assert_eq!(prompt.clicked(80, FRAMED_ROW, FRAMED + 40), Some((0, 5)));
}

#[test]
fn a_click_anywhere_but_the_line_moves_nothing() {
    // The border, the status row, and anything drawn above the box. Moving the
    // cursor to the nearest place that is inside would answer a click nobody
    // aimed at the line.
    let prompt = typed("hello");

    assert_eq!(prompt.clicked(80, 0, FRAMED + 1), None);
    assert_eq!(prompt.clicked(80, FRAMED_ROW + 1, FRAMED + 1), None);
    assert_eq!(prompt.clicked(80, FRAMED_ROW + 2, FRAMED + 1), None);
}

#[test]
fn a_click_left_of_the_mark_lands_at_the_start_of_the_row() {
    // On the border or on the mark itself, which is the one part of the row
    // that is chrome rather than line.
    let prompt = typed("hello");

    assert_eq!(prompt.clicked(80, FRAMED_ROW, 0), Some((0, 0)));
    assert_eq!(prompt.clicked(80, FRAMED_ROW, 2), Some((0, 0)));
}

#[test]
fn a_click_reads_the_same_rows_the_caret_was_placed_against() {
    // The two are separate calls and lay the same component out. A click read
    // against a box drawn any other way lands somewhere nobody pointed at.
    for said in SAID {
        for column in 0..=width::columns(said) {
            let prompt = typing(said, column);
            let caret = prompt.caret(FRAMED_AT);
            let back = prompt.clicked(FRAMED_AT, caret.row, caret.column);

            assert_eq!(back, Some((0, column)), "{said:?} at {column}");
        }
    }
}

/// The same box, with commands left running behind it.
fn leaving(said: &'static str, running: usize) -> Prompt<'static> {
    Prompt {
        running: Some(running),
        ..asking_of(said)
    }
}

#[test]
fn what_is_still_running_is_named_on_the_status_row_and_nowhere_else() {
    // One row rather than two. Everything on that row is a fact that stays true
    // until something changes it, and a command still running is one of those —
    // where a notice under it is true until the next keystroke.
    let rows = leaving("", 2).rows(80, Glyphs::Unicode);
    let status = rows.last().map(Row::text).unwrap_or_default();

    assert!(status.contains(MODE), "{status:?}");
    assert!(status.contains("2 commands"), "{status:?}");
    assert!(status.contains(NAMED), "{status:?}");
    assert_eq!(
        rows.len(),
        asking_of("").rows(80, Glyphs::Unicode).len(),
        "naming what is running cost the box a row"
    );
}

#[test]
fn nothing_running_says_nothing_about_it() {
    let status = asking_of("")
        .rows(80, Glyphs::Unicode)
        .last()
        .map(Row::text)
        .unwrap_or_default();

    assert!(!status.contains("command"), "{status:?}");
}

#[test]
fn what_is_running_outlasts_the_keys_when_the_row_narrows() {
    // The order things give way on this row. The keys after the mode are
    // documentation and a second look gets them back; the count is the only way
    // to find a process, so it is the last thing to go before the mode itself.
    let narrow = leaving("", 2).rows(46, Glyphs::Unicode);
    let status = narrow.last().map(Row::text).unwrap_or_default();

    assert!(status.contains("2 commands"), "{status:?}");
    assert!(
        !status.contains(HINT),
        "the keys took room the count needed: {status:?}"
    );
}

#[test]
fn what_is_running_is_the_one_thing_on_that_row_drawn_in_the_accent() {
    // Because it is the one thing on it you can act on. Marked by colour and by
    // being the only colour, which is what a reader picks out at a glance — and
    // it holds in every mode, since the three a mode row is ever drawn in are the
    // quiet one and the two a permission mode owns. The tone here is the one ask
    // mode passes, rather than the helper's, because that is the row this claim is
    // about.
    let rows = Prompt {
        tone: Slot::Quiet,
        ..leaving("", 2)
    }
    .rows(80, Glyphs::Unicode);
    let status = rows.last().expect("a status row");

    let accented: Vec<&str> = status
        .spans()
        .filter(|(slot, _)| *slot == Slot::Accent)
        .map(|(_, text)| text)
        .collect();

    assert_eq!(accented, ["2 commands"], "{:?}", status.text());
}

#[test]
fn a_click_on_the_row_naming_what_is_running_lands_on_it() {
    // The row is the affordance, and it is the component that knows which row it
    // came out on: a caller working that out would be a second copy of this box's
    // own arithmetic.
    // Wide enough that the count is drawn at all: a row that gave it up has no
    // door on it, which is what the ladder above already asserts.
    for columns in [80, 120] {
        let box_of = leaving("what I typed", 2);
        let rows = box_of.rows(columns, Glyphs::Unicode);

        let status = rows
            .iter()
            .position(|row| row.text().contains("2 commands"))
            .expect("a row naming what is running");

        assert!(box_of.counting(columns, status), "at {columns} columns");
        assert!(
            !box_of.counting(columns, status + 1),
            "at {columns} columns"
        );
        assert!(
            !box_of.counting(columns, status.saturating_sub(1)),
            "at {columns} columns"
        );
    }
}

#[test]
fn nothing_running_makes_that_row_no_door() {
    // With nothing to name, the row is the mode and the model — facts rather than
    // offers, and a click on a fact does nothing.
    let box_of = asking_of("");
    let rows = box_of.rows(80, Glyphs::Unicode);

    assert!(!box_of.counting(80, rows.len() - 1));
}
