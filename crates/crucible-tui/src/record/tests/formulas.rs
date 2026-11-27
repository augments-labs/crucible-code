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

    record.drawn(picturing::formula("wide"), Glyphs::Unicode, Theme::Dark);

    let told = record.formulas();
    assert_eq!(told.len(), 1);
    let formula = told.first().expect("one formula");
    assert!(record.formula_made(formula, picturing::svg("wide")));
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
