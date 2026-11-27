//! A session put back: what the picture cache kept is drawn from it, and the
//! rest is text with no drawer asked.

use super::super::*;
use super::{fake, picturing};

#[test]
fn a_session_put_back_draws_what_the_cache_kept_and_the_rest_as_text_with_no_drawer() {
    let (fake, asked) = fake(100, 3, vec!["kept"]);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

    record.resuming(true, ScreenMode::Fullscreen);
    record.drawn(picturing::diagram("kept"), Glyphs::Unicode, Theme::Dark);
    record.drawn(picturing::diagram("missed"), Glyphs::Unicode, Theme::Dark);
    record.resuming(false, ScreenMode::Fullscreen);

    assert!(picturing::shown(&record, 0), "the cache's picture");
    assert!(!picturing::shown(&record, 1), "a miss was drawn");
    // As a picture not drawn here: its text, with nothing said of it.
    assert_eq!(picturing::refused(&record, 1), Some(Undrawn::NotHere));
    assert_eq!(
        picturing::asked(&asked),
        [
            "resuming true",
            "cached kept",
            "cached missed",
            "resuming false"
        ]
    );

    // Once it is put back, a new block is drawn as live ones are.
    record.drawn(picturing::diagram("live"), Glyphs::Unicode, Theme::Dark);
    assert!(picturing::shown(&record, 2));
    assert_eq!(
        picturing::asked(&asked).get(4..),
        Some(&["cached live", "picture live", "make live"].map(str::to_owned)[..])
    );
}

#[test]
fn a_session_put_back_reads_the_cache_only_for_the_pictures_it_keeps() {
    let names: Vec<String> = (0..MOST_KEPT_IMAGES + 4)
        .map(|at| format!("d{at}"))
        .collect();
    let cached: Vec<&'static str> = names
        .iter()
        .map(|name| &*Box::leak(name.clone().into_boxed_str()))
        .collect();
    let (fake, asked) = fake(100, 3, cached);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));

    record.resuming(true, ScreenMode::Fullscreen);
    for name in &names {
        record.drawn(picturing::diagram(name), Glyphs::Unicode, Theme::Dark);
    }
    assert_eq!(
        picturing::asked(&asked),
        ["resuming true"],
        "the walk read the cache"
    );
    record.resuming(false, ScreenMode::Fullscreen);

    let asked = picturing::asked(&asked);
    let read = asked
        .iter()
        .filter(|said| said.starts_with("cached "))
        .count();
    assert_eq!(read, MOST_KEPT_IMAGES, "{asked:?}");
    assert!(
        !asked
            .iter()
            .any(|said| said.starts_with("picture ") || said.starts_with("make ")),
        "{asked:?}"
    );
    assert_eq!(asked.last().map(String::as_str), Some("resuming false"));
    // The newest are the ones shown, and the oldest are their text.
    assert!((0..4).all(|line| !picturing::shown(&record, line)));
    assert!((4..MOST_KEPT_IMAGES + 4).all(|line| picturing::shown(&record, line)));
}

#[test]
fn a_new_palette_after_a_session_put_back_asks_nothing_for_the_diagrams_it_did_not_keep() {
    // The older diagrams the put back did not reach are text for as long
    // as they are held, as those a live session drew past the cap are: a
    // new palette encodes the kept pictures again and asks for no other.
    let older = 10;
    let names: Vec<String> = (0..MOST_KEPT_IMAGES + older)
        .map(|at| format!("d{at}"))
        .collect();
    let cached: Vec<&'static str> = names
        .iter()
        .map(|name| &*Box::leak(name.clone().into_boxed_str()))
        .collect();
    let (fake, asked) = fake(100, 3, cached);
    let mut record = picturing::record(40, fake, &picturing::coloured(Theme::Dark));
    record.resuming(true, ScreenMode::Fullscreen);
    for name in &names {
        record.drawn(picturing::diagram(name), Glyphs::Unicode, Theme::Dark);
    }
    record.resuming(false, ScreenMode::Fullscreen);
    let before = picturing::asked(&asked).len();

    let light = picturing::coloured(Theme::Light);
    record.pictured_as(
        Capability::Graphics {
            protocol: Protocol::Kitty,
            cell: picturing::CELL,
        },
        light.identity(),
    );
    record.relay(40, Glyphs::Unicode, Theme::Light);

    let after = picturing::asked(&asked);
    let since = after.get(before..).unwrap_or_default();
    assert!(
        since.iter().all(|said| said == "reencode"),
        "asked for a diagram not kept: {since:?}"
    );
    assert_eq!(since.len(), MOST_KEPT_IMAGES, "{since:?}");
    assert!((0..older).all(|line| !picturing::shown(&record, line)));
    assert!((older..MOST_KEPT_IMAGES + older).all(|line| picturing::shown(&record, line)));
}
