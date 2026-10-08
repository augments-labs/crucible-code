//! The colour rule written in `crucible_tui`'s colour module, checked against
//! the screens this crate draws.
//!
//! Tests only. A screen hands over its rows and says which of them are
//! selected; this answers whether the rule holds: no more than one accent span
//! on a row that is not selected, and the same words in every theme, at every
//! rung, as with no colour at all. The welcome card's rows are counted
//! between its frame's edges, as the colour module says a frame's are.

use crucible_tui::{Palette, Row, Slot, Theme};

/// Every theme there is.
const THEMES: [Theme; 5] = [
    Theme::Dark,
    Theme::Light,
    Theme::ColourblindDark,
    Theme::ColourblindLight,
    Theme::Ansi,
];

/// What a terminal says about itself, one entry per rung of colour it can take.
const RUNGS: [(&str, &str); 3] = [
    ("COLORTERM", "truecolor"),
    ("TERM", "xterm-256color"),
    ("TERM", "xterm"),
];

/// Whether `row` is the one a list's mark is on: its first glyph is the caret
/// in either glyph set.
pub(crate) fn marked(row: &Row) -> bool {
    let said = row.text();
    let said = said.trim_start();
    said.starts_with('\u{203a}') || said.starts_with("> ")
}

/// `painted` with every sequence a terminal would read as an instruction taken
/// out: a control sequence up to its final byte, and an operating-system
/// command up to the bell or the string terminator that ends it.
fn unpainted(painted: &str) -> String {
    let mut said = String::new();
    let mut chars = painted.chars().peekable();

    while let Some(one) = chars.next() {
        if one != '\u{1b}' {
            said.push(one);
            continue;
        }
        match chars.next() {
            Some('[') => {
                for inside in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&inside) {
                        break;
                    }
                }
            }
            Some(']') => {
                while let Some(inside) = chars.next() {
                    if inside == '\u{7}' {
                        break;
                    }
                    if inside == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    said
}

/// How many accent spans the colour rule counts on `row`.
///
/// A span is a run of [`Slot::Accent`] that shows something, ended by text in
/// any other slot, blank or not. Blank text in the accent counts for nothing,
/// because it puts nothing in front of the eye.
pub(crate) fn accents(row: &Row) -> usize {
    counted(row.spans())
}

/// How many accent spans the colour rule counts on `row`, a row of the
/// welcome card.
///
/// The card's frame is the frame, not a span of the row it holds. A row that
/// is the accent alone, such as the bottom, is all frame and counts nothing.
/// A row that opens and closes in the accent is counted between the two: on
/// the top that leaves the name and the version, and on a row of two columns
/// the edge between them, drawn as the row's first edge is, is frame too.
/// Anything else lit is counted, a second such edge included.
pub(crate) fn card_accents(row: &Row) -> usize {
    let spans: Vec<(Slot, &str)> = row.spans().collect();
    if spans.iter().all(|(slot, _)| *slot == Slot::Accent) {
        return 0;
    }

    match spans.as_slice() {
        [(Slot::Accent, edge), inside @ .., (Slot::Accent, _)] => {
            let parting = inside
                .iter()
                .position(|span| *span == (Slot::Accent, *edge));
            // Still a part between what stands either side of it, so the
            // columns' runs are not read as one.
            counted(inside.iter().enumerate().map(|(at, &(slot, text))| {
                if Some(at) == parting {
                    (Slot::Plain, text)
                } else {
                    (slot, text)
                }
            }))
        }
        whole => counted(whole.iter().copied()),
    }
}

/// The accent spans among `spans`, as [`accents`] counts them.
fn counted<'a>(spans: impl Iterator<Item = (Slot, &'a str)>) -> usize {
    let mut counted = 0;
    let mut inside = false;

    for (slot, text) in spans {
        if slot != Slot::Accent {
            inside = false;
        } else if !text.trim().is_empty() {
            counted += usize::from(!inside);
            inside = true;
        }
    }

    counted
}

/// Panics, naming `screen` and the row, where `rows` breaks the rule.
///
/// `selected` answers whether a row is the selected one, where the limit on
/// accents does not apply.
pub(crate) fn holds(screen: &str, rows: &[Row], selected: impl Fn(&Row) -> bool) {
    ruled(screen, rows, selected, accents);
}

/// [`holds`], for the welcome card: each row is counted as [`card_accents`]
/// says.
pub(crate) fn holds_card(screen: &str, rows: &[Row]) {
    ruled(screen, rows, |_| false, card_accents);
}

/// The rule, with each row's accents counted by `counting`.
fn ruled(screen: &str, rows: &[Row], selected: impl Fn(&Row) -> bool, counting: fn(&Row) -> usize) {
    assert!(!rows.is_empty(), "{screen}: drew nothing to check");
    let plain = Palette::plain();

    for (at, row) in rows.iter().enumerate() {
        let said = row.text();

        assert!(
            selected(row) || counting(row) <= 1,
            "{screen}: row {at} has {} accent spans: {said:?}",
            counting(row)
        );

        assert_eq!(row.paint(&plain), said, "{screen}: row {at} with no colour");

        for theme in THEMES {
            for (name, value) in RUNGS {
                let palette = Palette::resolve(true, theme, None, &|asked| {
                    (asked == name).then(|| value.to_owned())
                });

                assert_eq!(
                    unpainted(&row.paint(&palette)),
                    said,
                    "{screen}: row {at} in {theme:?} on {value}"
                );
            }
        }
    }
}

#[test]
fn what_a_terminal_reads_as_an_instruction_is_taken_out_and_nothing_else() {
    let painted = "\u{1b}[1;36mRead\u{1b}[0m(\u{1b}]8;;https://a\u{1b}\\a.rs\u{1b}]8;;\u{7}) ok";

    assert_eq!(unpainted(painted), "Read(a.rs) ok");
}

#[test]
fn the_colour_rule_counts_runs_of_the_accent_slot_and_nothing_else() {
    // Strong, code and the pointer's slot share the accent's ink or ground
    // and are not counted: the rule is about the accent slot.
    let none = Row::new()
        .then(Slot::Strong, "4")
        .then(Slot::Quiet, " lines, removed ")
        .then(Slot::Strong, "2")
        .then(Slot::Code, "wait")
        .then(Slot::Pointed, "here");
    let one = Row::new()
        .then(Slot::Accent, "ctrl")
        .then(Slot::Accent, "+x");
    // Any other slot between two runs parts them, a blank one too: a
    // border and a key with a space between are two things lit.
    let parted = Row::new()
        .then(Slot::Accent, "╰────")
        .then(Slot::Plain, " ")
        .then(Slot::Accent, "ctrl+x");
    let two = Row::new()
        .then(Slot::Accent, "●")
        .then(Slot::Plain, " Read ")
        .then(Slot::Accent, "opens");

    assert_eq!(accents(&none), 0);
    assert_eq!(accents(&one), 1);
    assert_eq!(accents(&parted), 2);
    assert_eq!(accents(&two), 2);

    // The caret's place on a row that is not marked is blank, and blank
    // in any colour puts nothing in front of the eye.
    let unmarked = Row::new()
        .then(Slot::Accent, " ")
        .then(Slot::Plain, " /plugin ")
        .then(Slot::Accent, "●");
    assert_eq!(accents(&unmarked), 1);
}

#[test]
fn the_cards_frame_is_counted_apart_from_the_rows_it_holds() {
    let edge = || Row::new().then(Slot::Accent, "\u{2502}");
    let top = Row::new()
        .then(Slot::Accent, "\u{256d}\u{2500} ")
        .then(Slot::Strong, "crucible")
        .then(Slot::Plain, " ")
        .then(Slot::Quiet, "v0.46.0")
        .then(Slot::Accent, " \u{2500}\u{2500}\u{256e}");
    let bottom = Row::new().then(Slot::Accent, "\u{2570}\u{2500}\u{2500}\u{256f}");
    // Two columns, the identity's empty, and a tip beside it whose key is
    // the one thing lit.
    let tip = |key: &str| {
        edge()
            .then(Slot::Plain, "   ")
            .join(edge())
            .then(Slot::Plain, " ")
            .then(Slot::Accent, key)
            .then(Slot::Plain, " to see the rest ")
            .join(edge())
    };
    let keyed = tip("ctrl+o");
    let two_keys = edge()
        .then(Slot::Plain, " ")
        .then(Slot::Accent, "/resume")
        .then(Slot::Plain, " ")
        .join(edge())
        .then(Slot::Plain, " ")
        .then(Slot::Accent, "ctrl+o")
        .then(Slot::Plain, " ")
        .join(edge());
    // A second edge between the columns is not the one that parts them.
    let two_edges = tip("ctrl+o").join(edge()).join(edge());

    assert_eq!(card_accents(&top), 0);
    assert_eq!(card_accents(&bottom), 0);
    assert_eq!(card_accents(&tip("")), 0);
    assert_eq!(card_accents(&keyed), 1);
    assert_eq!(card_accents(&two_keys), 2);
    assert_eq!(card_accents(&two_edges), 2);

    // The rule the frame is counted apart under is the same rule.
    let held = std::panic::catch_unwind(|| holds_card("a card", &[top, keyed, two_keys, bottom]));
    assert!(held.is_err(), "a row with two keys lit broke no rule");
}
