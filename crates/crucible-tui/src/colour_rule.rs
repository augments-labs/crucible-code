//! The colour rule in [`crate::color`], checked against what a screen drew.
//!
//! Tests only. A screen hands over its rows and says which of them are
//! selected; this answers whether the rule holds: no more than one accent span
//! on a row that is not selected, and the same words in every theme, at every
//! rung, as with no colour at all.

use crate::color::{Palette, Slot, Theme};
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

/// How many accent spans the colour rule counts on `row`.
///
/// A span is a run of [`Slot::Accent`] that shows something, ended by text in
/// any other slot, blank or not. Blank text in the accent counts for nothing,
/// because it puts nothing in front of the eye.
pub(crate) fn accents(row: &Row) -> usize {
    counted(row.spans())
}

/// How many accent spans the colour rule counts on `row`, standing inside a
/// frame.
///
/// A frame's edges are the frame, not a span of the row they hold, so a row
/// that opens and closes in the accent is counted between the two. A row that
/// does not, such as the frame's top or bottom, is counted whole.
pub(crate) fn framed_accents(row: &Row) -> usize {
    let spans: Vec<(Slot, &str)> = row.spans().collect();
    match spans.as_slice() {
        [(Slot::Accent, _), inside @ .., (Slot::Accent, _)] => counted(inside.iter().copied()),
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
/// `selected` answers for a row's place among `rows`.
pub(crate) fn holds(screen: &str, rows: &[Row], selected: impl Fn(usize) -> bool) {
    ruled(screen, rows, selected, accents);
}

/// [`holds`], for a screen drawn inside a frame whose edges run down both
/// sides: each row is counted between its edges, as [`framed_accents`] says.
pub(crate) fn holds_framed(screen: &str, rows: &[Row], selected: impl Fn(usize) -> bool) {
    ruled(screen, rows, selected, framed_accents);
}

/// The rule, with each row's accents counted by `counting`.
fn ruled(
    screen: &str,
    rows: &[Row],
    selected: impl Fn(usize) -> bool,
    counting: fn(&Row) -> usize,
) {
    let plain = Palette::plain();

    for (at, row) in rows.iter().enumerate() {
        let said = row.text();

        assert!(
            selected(at) || counting(row) <= 1,
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
        .then(Slot::Accent, "+q");
    // Any other slot between two runs parts them, a blank one too: a
    // border and a key with a space between are two things lit.
    let parted = Row::new()
        .then(Slot::Accent, "╰────")
        .then(Slot::Plain, " ")
        .then(Slot::Accent, "ctrl+q");
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
fn a_frame_is_counted_apart_from_the_row_it_holds() {
    // An edge each side and the caret between: the caret is the one thing
    // lit. Two things lit between the edges are still two.
    let edged = |inside: Row| {
        Row::new()
            .then(Slot::Accent, "\u{2502}")
            .join(inside)
            .then(Slot::Accent, "\u{2502}")
    };
    let caret = edged(
        Row::new()
            .then(Slot::Accent, "\u{203a}")
            .then(Slot::Strong, " 1. Rust"),
    );
    let quiet = edged(Row::new().then(Slot::Quiet, " a description"));
    let twice = edged(
        Row::new()
            .then(Slot::Accent, "\u{203a}")
            .then(Slot::Plain, " 1. Rust ")
            .then(Slot::Accent, "ctrl+e"),
    );
    let top = Row::new().then(Slot::Accent, "\u{256d}\u{2500}\u{2500}\u{256e}");

    assert_eq!(framed_accents(&caret), 1);
    assert_eq!(framed_accents(&quiet), 0);
    assert_eq!(framed_accents(&twice), 2);
    assert_eq!(framed_accents(&top), 1);
    assert_eq!(accents(&quiet), 2, "counted whole, the edges are two");
}
