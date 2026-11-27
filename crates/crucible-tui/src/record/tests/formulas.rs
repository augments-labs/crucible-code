//! The display formulas a live record tells out, and the pictures handed
//! back for them.

use super::super::*;
use super::{fake, picturing};

#[test]
fn a_display_formula_that_converts_is_its_text_told_out_once_until_its_picture_comes() {
    let (fake, asked) = fake(100, 3, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

    record.drawn(picturing::formula("x^2"), Glyphs::Unicode, Theme::Dark);

    assert!(!picturing::shown(&record, 0), "drawn as its text meanwhile");
    assert_eq!(
        record.folded(0).first().map(Row::text),
        Some("x^2".to_owned())
    );
    let told = record.formulas();
    assert_eq!(
        told.iter()
            .map(|formula| formula.source().trim())
            .collect::<Vec<_>>(),
        ["x^2"]
    );
    assert!(picturing::told(&mut record).is_empty(), "told once");
    // The cache is asked, as for any block; nothing is drawn here.
    assert_eq!(picturing::asked(&asked), ["cached x^2"]);

    // A new palette asks nothing more and tells nothing again.
    record.pictured_as(
        Capability::Graphics {
            protocol: Protocol::Kitty,
            cell: picturing::CELL,
        },
        picturing::coloured(Theme::Light).identity(),
    );
    record.relay(40, Glyphs::Unicode, Theme::Light);
    assert!(picturing::told(&mut record).is_empty());

    let formula = told.first().expect("one formula");
    assert!(record.formula_made(formula, picturing::svg("x^2")));
    assert!(
        picturing::shown(&record, 0),
        "the picture replaces the text"
    );
    assert_eq!(
        picturing::asked(&asked).last().map(String::as_str),
        Some("make x^2")
    );
}

#[test]
fn a_display_formula_that_does_not_convert_is_never_told_out_nor_asked_for() {
    let (fake, asked) = fake(100, 3, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

    record.drawn(picturing::formula("bad"), Glyphs::Unicode, Theme::Dark);

    assert!(picturing::told(&mut record).is_empty());
    // Never made: what the cache is asked costs it nothing.
    assert_eq!(picturing::asked(&asked), ["cached bad"]);
}

#[test]
fn a_display_formula_too_wide_as_text_is_told_out_and_replaced_by_its_picture() {
    let (fake, _) = fake(100, 3, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    let refusal = "┌─ math · too wide to draw at this width";
    let heading = |record: &Record, line: usize| record.folded(line).first().map(Row::text);

    record.drawn(picturing::formula("wide"), Glyphs::Unicode, Theme::Dark);
    record.drawn(
        picturing::formula("also wide"),
        Glyphs::Unicode,
        Theme::Dark,
    );

    let told = record.formulas();
    assert_eq!(told.len(), 2);
    // While each waits, it is its source under the refusal.
    for line in [0, 1] {
        assert!(!picturing::shown(&record, line), "{line}");
        assert_eq!(heading(&record, line).as_deref(), Some(refusal), "{line}");
    }

    let formula = told.first().expect("two formulas");
    assert!(record.formula_made(formula, picturing::svg("wide")));
    assert!(picturing::shown(&record, 0));

    // An answer with no picture hands the record nothing, and the frames
    // after it leave the formula as it was while it waited.
    record.relay(40, Glyphs::Unicode, Theme::Dark);
    assert!(!picturing::shown(&record, 1));
    assert_eq!(heading(&record, 1).as_deref(), Some(refusal));
    assert!(picturing::shown(&record, 0));
}

#[test]
fn display_math_is_told_out_only_as_a_live_picture_capable_record_draws_it() {
    // Drawn as text, or as written: no picture is shown, so none is made.
    for math in [Mode::Text, Mode::Written] {
        let (fake, _) = fake(100, 3, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.draws_in(Modes {
            math,
            ..Modes::default()
        });
        record.drawn(picturing::formula("x^2"), Glyphs::Unicode, Theme::Dark);
        assert!(picturing::told(&mut record).is_empty(), "{math:?}");
    }

    // Where colour is not written.
    let (uncoloured, _) = fake(100, 3, vec![]);
    let plain = crate::color::Palette::resolve(false, Theme::Dark, None, &|_: &str| None);
    let mut record = picturing::record(40, uncoloured, &plain);
    record.drawn(picturing::formula("x^2"), Glyphs::Unicode, Theme::Dark);
    assert!(picturing::told(&mut record).is_empty(), "no colour");

    // While a session is put back in the terminal's own buffer, a miss
    // stays text and nothing is made.
    let screen = ScreenMode::Native;
    let (fake, asked) = fake(100, 3, vec!["kept"]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.resuming(true, screen);
    record.drawn(picturing::formula("kept"), Glyphs::Unicode, Theme::Dark);
    record.drawn(picturing::formula("missed"), Glyphs::Unicode, Theme::Dark);
    record.resuming(false, screen);
    assert!(picturing::told(&mut record).is_empty(), "{screen:?}");
    assert!(
        picturing::shown(&record, 0),
        "{screen:?}: the cache's picture"
    );
    assert!(!picturing::shown(&record, 1), "{screen:?}");
    assert!(
        picturing::asked(&asked)
            .iter()
            .all(|asked| !asked.starts_with("make")),
        "{screen:?}: {:?}",
        picturing::asked(&asked)
    );
}

#[test]
fn a_formula_put_back_on_a_full_screen_is_told_out_to_be_read_and_nothing_is_read_or_made_here() {
    let (fake, asked) = fake(100, 3, vec!["kept", "missed"]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.resuming(true, ScreenMode::Fullscreen);
    record.drawn(picturing::formula("kept"), Glyphs::Unicode, Theme::Dark);
    record.drawn(picturing::formula("missed"), Glyphs::Unicode, Theme::Dark);
    record.resuming(false, ScreenMode::Fullscreen);

    let told = record.formulas();
    let sources: Vec<&str> = told.iter().map(|told| told.source().trim()).collect();
    assert_eq!(sources, ["kept", "missed"]);
    assert!(
        told.iter()
            .all(|told| told.kept().is_some_and(|key| key.kind == Kind::DisplayMath)),
        "{told:?}"
    );
    assert!(!picturing::shown(&record, 0) && !picturing::shown(&record, 1));
    let asked = picturing::asked(&asked);
    assert!(
        asked
            .iter()
            .all(|asked| !asked.starts_with("make") && !asked.starts_with("cached")),
        "read or made on the drawing thread: {asked:?}"
    );

    // What was read for it is shown as a picture made of it is.
    let kept = told.first().expect("the kept formula");
    assert!(record.formula_made(kept, picturing::svg("kept")));
    assert!(picturing::shown(&record, 0));
    assert!(!picturing::shown(&record, 1));
}

#[test]
fn no_more_formulas_put_back_are_told_out_to_be_read_than_the_record_keeps_pictures_of() {
    let (fake, _) = fake(100, 3, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.resuming(true, ScreenMode::Fullscreen);
    for at in 0..=MOST_KEPT_IMAGES {
        record.drawn(
            picturing::formula(&format!("f{at}")),
            Glyphs::Unicode,
            Theme::Dark,
        );
    }
    record.resuming(false, ScreenMode::Fullscreen);

    let told = picturing::told(&mut record);
    let newest: Vec<String> = (1..=MOST_KEPT_IMAGES).map(|at| format!("f{at}")).collect();
    assert_eq!(told, newest, "the newest, oldest first");
}

#[test]
fn a_math_picture_one_row_taller_than_the_window_is_text_and_one_its_height_is_shown() {
    for (rows, shown) in [(9, false), (8, true)] {
        let (fake, _) = fake(100, rows, vec![]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.windowed(8, false);
        record.drawn(picturing::formula("x^2"), Glyphs::Unicode, Theme::Dark);
        let told = record.formulas();
        let formula = told.first().expect("one formula");

        assert_eq!(record.formula_made(formula, picturing::svg("x^2")), shown);
        assert_eq!(picturing::shown(&record, 0), shown, "{rows} rows");
        assert_eq!(
            record.folded(0).len(),
            if shown { 8 } else { 1 },
            "{rows} rows"
        );
    }
}

#[test]
fn a_math_picture_arriving_after_its_block_left_changes_nothing() {
    let (fake, asked) = fake(100, 3, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.drawn(picturing::formula("x^2"), Glyphs::Unicode, Theme::Dark);
    let told = record.formulas();
    let formula = told.first().expect("one formula");

    record.empties();
    record.drawn(picturing::formula("y^2"), Glyphs::Unicode, Theme::Dark);

    assert!(!record.formula_made(formula, picturing::svg("x^2")));
    assert!(!picturing::shown(&record, 0));
    assert!(
        picturing::asked(&asked)
            .iter()
            .all(|asked| !asked.starts_with("make")),
        "{:?}",
        picturing::asked(&asked)
    );
}

#[test]
fn a_formula_put_back_natively_is_shown_large_where_the_window_holds_it_and_text_where_not() {
    // Six rows is more than half an eight-row window, which a diagram's Fit
    // would shrink it to; nine is more than the window holds.
    for (rows, shown, folded) in [(6, true, 6), (9, false, 1)] {
        let (fake, _) = fake(100, rows, vec!["kept"]);
        let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
        record.windowed(8, false);
        record.resuming(true, ScreenMode::Native);
        record.drawn(picturing::formula("kept"), Glyphs::Unicode, Theme::Dark);
        record.resuming(false, ScreenMode::Native);

        assert_eq!(picturing::shown(&record, 0), shown, "{rows} rows");
        assert_eq!(record.folded(0).len(), folded, "{rows} rows");
    }
}

#[test]
fn a_math_picture_encoded_again_in_a_shorter_window_is_kept() {
    let (fake, asked) = fake(100, 6, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.windowed(8, false);
    record.drawn(picturing::formula("x^2"), Glyphs::Unicode, Theme::Dark);
    let told = record.formulas();
    let formula = told.first().expect("one formula");
    assert!(record.formula_made(formula, picturing::svg("x^2")));
    assert_eq!(record.folded(0).len(), 6, "shown large");

    // A new palette encodes it again, in a window it no longer fits: the
    // window judged its first size, and it keeps that size here.
    record.windowed(5, false);
    record.pictured_as(
        Capability::Graphics {
            protocol: Protocol::Kitty,
            cell: picturing::CELL,
        },
        picturing::coloured(Theme::Light).identity(),
    );
    record.relay(40, Glyphs::Unicode, Theme::Light);

    assert!(
        picturing::asked(&asked)
            .iter()
            .any(|asked| asked.starts_with("reencode")),
        "{:?}",
        picturing::asked(&asked)
    );
    assert!(picturing::shown(&record, 0));
    assert_eq!(picturing::refused(&record, 0), None);
    assert_eq!(record.folded(0).len(), 5, "large, at the window's height");
}

/// Whether a formula is shown as its picture, the rows it is in, and why it
/// is text where it is.
type Step = (bool, usize, Option<Undrawn>);

/// Shows a formula picture six rows tall in a window of eight, then shrinks
/// the window to five and grows it back to eight, with a new palette at
/// each, then a new palette once more; answers, after each step, whether it
/// is shown, the rows it is in, and why it is text where it is, then how
/// many times it was encoded again and how many made.
fn shrunk_and_grown(keeps: bool) -> (Vec<Step>, usize, usize) {
    let (fake, asked) = fake(100, 6, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.windowed(8, keeps);
    record.drawn(picturing::formula("x^2"), Glyphs::Unicode, Theme::Dark);
    let told = record.formulas();
    let formula = told.first().expect("one formula");
    assert!(record.formula_made(formula, picturing::svg("x^2")));
    let mut steps = vec![(
        picturing::shown(&record, 0),
        record.folded(0).len(),
        picturing::refused(&record, 0),
    )];
    for (window, theme) in [(5, Theme::Light), (8, Theme::Dark), (8, Theme::Light)] {
        record.windowed(window, keeps);
        record.pictured_as(
            Capability::Graphics {
                protocol: Protocol::Kitty,
                cell: picturing::CELL,
            },
            picturing::coloured(theme).identity(),
        );
        record.relay(40, Glyphs::Unicode, theme);
        steps.push((
            picturing::shown(&record, 0),
            record.folded(0).len(),
            picturing::refused(&record, 0),
        ));
    }
    // Each new palette encodes the picture kept again, and makes nothing.
    let asked = picturing::asked(&asked);
    let reencoded = asked.iter().filter(|asked| *asked == "reencode").count();
    let made = asked
        .iter()
        .filter(|asked| asked.starts_with("make"))
        .count();
    (steps, reencoded, made)
}

#[test]
fn a_formula_shown_on_a_full_screen_is_shown_again_once_a_shorter_window_grows_back() {
    let (steps, reencoded, made) = shrunk_and_grown(false);

    assert!(
        steps
            .iter()
            .all(|&(_, _, why)| why != Some(Undrawn::TooWide)),
        "never refused for the window: {steps:?}"
    );
    assert_eq!(
        steps.last(),
        Some(&(true, 6, None)),
        "shown at its own size: {steps:?}"
    );
    assert_eq!((reencoded, made), (3, 1), "{steps:?}");
}

#[test]
fn a_formula_shown_natively_keeps_its_first_rows_through_a_shorter_window_and_back() {
    let (steps, reencoded, made) = shrunk_and_grown(true);

    assert_eq!(steps, [(true, 6, None); 4], "{steps:?}");
    assert_eq!((reencoded, made), (3, 1), "{steps:?}");
}
