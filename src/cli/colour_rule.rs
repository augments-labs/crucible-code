//! The colour rule written in `crucible_tui`'s colour module, checked against
//! the screens this crate draws.
//!
//! Tests only. A screen hands over its rows and says which of them are
//! selected; this answers whether the rule holds: no more than one accent span
//! on a row that is not selected, and the same words in every theme, at every
//! rung, as with no colour at all.

use crucible_tui::{Palette, Row, Theme};

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

/// Panics, naming `screen` and the row, where `rows` breaks the rule.
///
/// `selected` answers whether a row is the selected one, where the limit on
/// accents does not apply.
pub(crate) fn holds(screen: &str, rows: &[Row], selected: impl Fn(&Row) -> bool) {
    assert!(!rows.is_empty(), "{screen}: drew nothing to check");
    let plain = Palette::plain();

    for (at, row) in rows.iter().enumerate() {
        let said = row.text();

        assert!(
            selected(row) || row.accents() <= 1,
            "{screen}: row {at} has {} accent spans: {said:?}",
            row.accents()
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
