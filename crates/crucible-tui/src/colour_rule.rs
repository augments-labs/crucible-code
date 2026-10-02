//! The colour rule in [`crate::color`], checked against what a screen drew.
//!
//! Tests only. A screen hands over its rows and says which of them are
//! selected; this answers whether the rule holds: no more than one accent span
//! on a row that is not selected, and the same words in every theme, at every
//! rung, as with no colour at all.

use crate::color::{Palette, Theme};
use crate::escape::Escapes;
use crate::row::Row;

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

/// `painted` with every sequence a terminal would read as an instruction
/// taken out.
fn unpainted(painted: &str) -> String {
    let mut escapes = Escapes::default();
    painted
        .chars()
        .filter(|character| !escapes.holds(*character))
        .collect()
}

/// Panics, naming `screen` and the row, where `rows` breaks the rule.
///
/// `selected` answers for a row's place among `rows`.
pub(crate) fn holds(screen: &str, rows: &[Row], selected: impl Fn(usize) -> bool) {
    let plain = Palette::plain();

    for (at, row) in rows.iter().enumerate() {
        let said = row.text();

        assert!(
            selected(at) || row.accents() <= 1,
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
