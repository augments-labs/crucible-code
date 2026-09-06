use crate::dump::dump;

use super::*;

/// The screen the design was approved at, with nothing in the box.
const EMPTY_AT_80: &str = "\
────────────────────────────────────────────────────────────────────────────────

Log in

› Anthropic — provide your own API key

Paste or type the key. It goes to crucible's protected store and is never shown.

╭── Anthropic API key ─────────────────────────────────────────────────────────╮
│ ›                                                                            │
╰──────────────────────────────────────────────────────────────────────────────╯
paste or type your API key · esc to cancel";

/// The same screen with sixty-two characters held.
const HELD_AT_80: &str = "\
────────────────────────────────────────────────────────────────────────────────

Log in

› Anthropic — provide your own API key

Paste or type the key. It goes to crucible's protected store and is never shown.

╭── Anthropic API key ─────────────────────────────────────────────────────────╮
│ › ••••••••••••••••••••••••••••••••••••••••••••••••••••••••••••••             │
╰──────────────────────────────────────────────────────────────────────────────╯
enter to save · esc to cancel";

/// The narrow case: the sentence folds, and the dots stop at the frame.
const NARROW_AT_48: &str = "\
────────────────────────────────────────────────

Log in

› Anthropic — provide your own API key

Paste or type the key. It goes to crucible's
protected store and is never shown.

╭── Anthropic API key ─────────────────────────╮
│ › •••••••••••••••••••••••••••••••••••••••••••│
╰──────────────────────────────────────────────╯
enter to save · esc to cancel";

fn anthropic(held: usize) -> KeyPanel<'static> {
    KeyPanel {
        provider: "Anthropic",
        held,
    }
}

/// What the component says, with no colour in it.
fn art(panel: &KeyPanel<'_>, columns: usize, glyphs: Glyphs) -> Vec<String> {
    panel.rows(columns, glyphs).iter().map(Row::text).collect()
}

/// The rows as one block, each ended where its last mark is.
fn block(rows: &[Row]) -> String {
    rows.iter()
        .map(|row| row.text().trim_end().to_owned())
        .collect::<Vec<_>>()
        .join("\n")
}

/// A snapshot drawing without the row-end cells `dump` pads to the width.
fn picture(rows: &[Row], columns: usize) -> String {
    dump(rows, columns)
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

/// Which slots one row opened, in order.
fn slots(row: &Row) -> Vec<Slot> {
    row.spans().map(|(slot, _)| slot).collect()
}

#[test]
fn the_empty_screen_is_drawn_the_way_it_was_designed() {
    assert_eq!(block(&anthropic(0).rows(80, Glyphs::Unicode)), EMPTY_AT_80);
}

#[test]
fn a_held_key_is_drawn_as_that_many_dots_and_the_footer_says_enter_saves() {
    assert_eq!(block(&anthropic(62).rows(80, Glyphs::Unicode)), HELD_AT_80);
}

#[test]
fn a_narrow_window_folds_the_sentence_and_stops_the_dots_at_the_frame() {
    assert_eq!(
        block(&anthropic(43).rows(48, Glyphs::Unicode)),
        NARROW_AT_48
    );
    // Forty-three is what fits. One more is not drawn, and the frame holds.
    assert_eq!(
        block(&anthropic(44).rows(48, Glyphs::Unicode)),
        NARROW_AT_48
    );
    assert_eq!(
        block(&anthropic(400).rows(48, Glyphs::Unicode)),
        NARROW_AT_48
    );
}

#[test]
fn the_screens_are_pictured_in_both_glyph_sets() {
    insta::with_settings!({snapshot_suffix => "80"}, {
        insta::assert_snapshot!("empty", picture(&anthropic(0).rows(80, Glyphs::Unicode), 80));
        insta::assert_snapshot!("held", picture(&anthropic(62).rows(80, Glyphs::Unicode), 80));
        insta::assert_snapshot!("ascii", picture(&anthropic(62).rows(80, Glyphs::Ascii), 80));
    });
    insta::with_settings!({snapshot_suffix => "48"}, {
        insta::assert_snapshot!("narrow", picture(&anthropic(43).rows(48, Glyphs::Unicode), 48));
    });
}

#[test]
fn nothing_of_the_prompt_reaches_the_key_screen() {
    // Not a turn: no window reading, no transcript hint, no model, no map.
    for columns in [24, 48, 80, 120] {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            for row in art(&anthropic(9), columns, glyphs) {
                for banned in ["%", "window", "transcript", "/", "claude", "model"] {
                    assert!(!row.contains(banned), "{banned:?} in {row:?} at {columns}");
                }
            }
        }
    }
}

#[test]
fn the_caret_stands_after_the_last_dot_and_stops_at_the_frame() {
    let (rows, caret) = anthropic(0).within(80, 24, Glyphs::Unicode);
    let boxed = rows
        .iter()
        .position(|row| row.text().starts_with("│ ›"))
        .expect("the row inside the frame");
    assert_eq!(
        caret,
        Some(Caret {
            row: boxed,
            column: 4
        })
    );

    let (_, caret) = anthropic(62).within(80, 24, Glyphs::Unicode);
    assert_eq!(caret.map(|caret| caret.column), Some(66));

    // Past what fits, the caret stays on the frame's right edge.
    let (_, caret) = anthropic(400).within(48, 24, Glyphs::Unicode);
    assert_eq!(caret.map(|caret| caret.column), Some(47));
}

#[test]
fn the_screen_gives_up_its_parts_in_order_and_the_frame_stands_last() {
    let panel = anthropic(5);
    let whole = panel.rows(80, Glyphs::Unicode).len();
    let said = |rows: &[Row]| rows.iter().any(|row| row.text().starts_with("Paste"));
    let crumb = |rows: &[Row]| {
        rows.iter()
            .any(|row| row.text().contains("provide your own"))
    };
    let rule = |rows: &[Row]| rows.first().is_some_and(|row| row.text().starts_with('─'));
    let footer = |rows: &[Row]| rows.last().is_some_and(|row| row.text().contains("esc"));
    let title = |rows: &[Row]| rows.iter().any(|row| row.text() == "Log in");
    let framed = |rows: &[Row]| rows.iter().any(|row| row.text().contains("API key ─"));

    let (rows, _) = panel.within(80, whole, Glyphs::Unicode);
    assert_eq!(rows.len(), whole);

    let (rows, _) = panel.within(80, whole - 1, Glyphs::Unicode);
    assert!(
        !said(&rows) && crumb(&rows) && rule(&rows) && footer(&rows),
        "{rows:?}"
    );

    let (rows, _) = panel.within(80, 8, Glyphs::Unicode);
    assert!(!crumb(&rows) && rule(&rows) && footer(&rows), "{rows:?}");

    let (rows, _) = panel.within(80, 6, Glyphs::Unicode);
    assert!(!rule(&rows) && title(&rows) && footer(&rows), "{rows:?}");

    let (rows, _) = panel.within(80, 5, Glyphs::Unicode);
    assert!(!footer(&rows) && title(&rows) && framed(&rows), "{rows:?}");

    let (rows, caret) = panel.within(80, 3, Glyphs::Unicode);
    assert!(!title(&rows) && framed(&rows), "{rows:?}");
    assert_eq!(rows.len(), 3);
    assert_eq!(caret, Some(Caret { row: 1, column: 9 }));

    let (rows, caret) = panel.within(80, 2, Glyphs::Unicode);
    assert!(rows.is_empty(), "{rows:?}");
    assert_eq!(caret, None);
}

#[test]
fn a_label_that_does_not_fit_beside_the_rule_leaves_the_border_plain() {
    let panel = KeyPanel {
        provider: "a vendor with a name nothing can shorten",
        held: 0,
    };
    let rows = art(&panel, 40, Glyphs::Unicode);
    let top = rows
        .iter()
        .find(|row| row.starts_with('╭'))
        .expect("the top border");

    assert!(!top.contains("API key"), "{top:?}");
    assert_eq!(top.chars().count(), 40);
}

#[test]
fn the_footers_are_joined_with_the_dot_of_the_set_in_force() {
    let unicode = art(&anthropic(0), 80, Glyphs::Unicode);
    let ascii = art(&anthropic(1), 80, Glyphs::Ascii);

    assert_eq!(
        unicode.last().map(String::as_str),
        Some("paste or type your API key · esc to cancel")
    );
    assert_eq!(
        ascii.last().map(String::as_str),
        Some("enter to save - esc to cancel")
    );
}

#[test]
fn a_narrow_key_panel_keeps_the_cancel_hint_whole() {
    for columns in [25, 28, 40] {
        for held in [0, 1] {
            let rows = art(&anthropic(held), columns, Glyphs::Unicode);
            let footer = rows.last().unwrap();
            assert!(footer.contains("esc to cancel"), "{columns}: {footer}");
        }
    }
}

#[test]
fn the_colour_goes_where_the_panels_put_it() {
    let rows = anthropic(3).rows(80, Glyphs::Unicode);
    let at = |at: usize| rows.get(at).map(slots).unwrap_or_default();

    assert_eq!(at(0), [Slot::Accent], "the rule");
    assert_eq!(at(2), [Slot::Strong], "the title");
    assert_eq!(at(4), [Slot::Accent, Slot::Plain], "the breadcrumb");
    assert_eq!(at(6), [Slot::Plain], "the sentence");
    assert_eq!(at(8), [Slot::Quiet], "the top border, label and all");
    assert_eq!(
        at(9).first(),
        Some(&Slot::Quiet),
        "the left edge: {:?}",
        rows.get(9)
    );
    assert_eq!(
        at(9).last(),
        Some(&Slot::Quiet),
        "the right edge: {:?}",
        rows.get(9)
    );
    assert!(at(9).contains(&Slot::Accent), "the mark: {:?}", rows.get(9));
    assert_eq!(at(10), [Slot::Quiet], "the bottom border");
    assert_eq!(at(11), [Slot::Quiet], "the footer");
}
