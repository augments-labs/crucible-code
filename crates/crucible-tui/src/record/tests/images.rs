//! An image a turn named: listed by its path, made once from its PNG, and
//! kept as its edge alone where it is refused or let go of.

use super::super::*;
use super::{fake, picturing};

/// An image named by `path`, read by `by`.
fn named<'a>(path: &'a str, by: Option<&'a str>) -> Named<'a> {
    Named { path, by }
}

#[test]
fn an_image_is_listed_by_its_path_under_the_word_image() {
    let (fake, _) = fake(100, 3, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.image(
        named("out/plot.png", None),
        Ok(vec![1]),
        Glyphs::Unicode,
        Theme::Dark,
    );
    record.image(
        named("gone.png", Some("read")),
        Err(Undrawn::ImageNotShown),
        Glyphs::Unicode,
        Theme::Dark,
    );

    let walked: Vec<(usize, &str, &str)> = record
        .sources()
        .map(|source| (source.line, source.word, source.text))
        .collect();

    assert_eq!(
        walked,
        [(1, "image", "gone.png"), (0, "image", "out/plot.png")]
    );
}

#[test]
fn an_image_is_made_once_and_encoded_again_from_its_png_for_a_new_theme_or_size() {
    let (fake, asked) = fake(100, 20, vec![]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.windowed(8, false);
    record.image(
        named("out/plot.png", None),
        Ok(vec![1]),
        Glyphs::Unicode,
        Theme::Dark,
    );
    assert!(picturing::shown(&record, 0));
    let folded = record.folded(0);
    assert_eq!(
        folded.first().map(Row::text).as_deref(),
        Some("┌─ image · out/plot.png")
    );
    assert!(folded.len() > 1, "{folded:?}");

    let light = picturing::coloured(Theme::Light);
    record.pictured_as(
        Capability::Graphics {
            protocol: Protocol::Kitty,
            cell: picturing::CELL,
        },
        light.identity(),
    );
    record.relay(30, Glyphs::Unicode, Theme::Light);
    assert_eq!(picturing::asked(&asked), ["make out/plot.png", "reencode"]);
    assert!(picturing::shown(&record, 0));

    record.pictures_reshaped();
    assert!(record.cycles(0), "an image has no larger size");
    assert!(record.pictures_reshaped(), "shown larger and not told");
}

#[test]
fn an_image_refused_or_let_go_keeps_its_edge_and_is_never_made_again() {
    let (mut fake, asked) = fake(100, 3, vec![]);
    fake.refuses = Some(Undrawn::TooWide);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.image(
        named("big.png", None),
        Ok(vec![1]),
        Glyphs::Unicode,
        Theme::Dark,
    );

    assert!(!picturing::shown(&record, 0));
    assert_eq!(
        record.folded(0).iter().map(Row::text).collect::<Vec<_>>(),
        ["┌─ image · big.png · too large"]
    );
    record.relay(60, Glyphs::Ascii, Theme::Dark);
    assert_eq!(picturing::asked(&asked), ["make big.png"]);
    assert_eq!(
        record.folded(0).iter().map(Row::text).collect::<Vec<_>>(),
        ["+- image - big.png - too large"]
    );
}
