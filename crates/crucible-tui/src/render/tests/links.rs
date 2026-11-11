//! Links, addresses and forge references on the screen: what a row draws for
//! each, what a drag copies off it, and what reaches the terminal when a
//! model writes one to carry an escape.

use super::*;
use crate::Forge;

/// The repository this checkout was cloned from, as a session finds it.
fn here() -> Forge {
    Forge::new(
        "https://github.com",
        "augments-labs/crucible-code",
        "/issues/",
    )
}

#[test]
fn a_pull_request_address_is_drawn_as_its_reference() {
    let answer = "opened https://github.com/augments-labs/crucible-code/pull/1324 today\n";

    let mut drawn = reading(100, 4);
    drawn.answer(answer);
    let rows = drawn.rows();
    assert!(
        rows.iter()
            .any(|row| row.contains("opened augments-labs/crucible-code#1324 today")),
        "{rows:#?}"
    );
    assert!(
        !rows.iter().any(|row| row.contains("https://github.com")),
        "{rows:#?}"
    );

    let mut drawn = reading(100, 4);
    drawn.render.counts(Some(here()));
    drawn.answer(answer);
    let rows = drawn.rows();
    assert!(
        rows.iter().any(|row| row.contains("opened #1324 today")),
        "{rows:#?}"
    );
}

#[test]
fn an_image_reference_keeps_its_words_and_drops_its_marker() {
    let answer = "look at ![the screenshot](out/shot.png) here\n";
    // Cut anywhere, the bang and its bracket included, the row is the same.
    for cut in (0..=answer.len()).filter(|&cut| answer.is_char_boundary(cut)) {
        let mut drawn = reading(100, 4);
        drawn.answer_in_pieces(answer, &[cut]);
        let rows = drawn.rows();
        assert!(
            rows.iter()
                .any(|row| row.contains("look at [image: the screenshot] (out/shot.png) here")),
            "cut at {cut}: {rows:#?}"
        );
        assert!(
            !rows.iter().any(|row| row.contains('!')),
            "cut at {cut}: {rows:#?}"
        );
    }
}

#[test]
fn a_drag_over_a_worded_link_copies_its_words_and_not_its_quiet_address() {
    let mut drawn = reading(60, 6);
    drawn.answer("see [the guide](https://example.com/docs/guide/intro) now\n");
    let rows = window(&drawn, 6);
    let row = row_containing(&rows, "see the guide (example.com/docs/…) now");
    drawn.take();

    let wrote = drag(&mut drawn, (row, 0), (row, 59));

    assert!(
        wrote.contains(&onto_the_clipboard("see the guide now")),
        "{rows:#?} gave {wrote:?}"
    );
}

#[test]
fn every_address_drawn_shorter_is_listed_whole_newest_first_after_the_blocks() {
    let home = "https://github.com/augments-labs/crucible-code";
    let mut drawn = reading(100, 20);
    drawn.render.counts(Some(here()));
    drawn.answer(&format!(
        "a {home}/pull/1324 b #7 c [d](https://example.com/docs/guide/intro) \
         e [f](https://example.com/x)\n\n{WRITTEN_TABLE}\nafter\n"
    ));

    // A reference the answer wrote short itself, and a quiet address shown
    // whole, were drawn as written: neither is listed.
    let listed: Vec<(&str, &str)> = drawn
        .sources()
        .map(|source| (source.word, source.text))
        .collect();
    let pull = format!("{home}/pull/1324");
    assert_eq!(
        listed,
        [
            ("table", WRITTEN_TABLE),
            ("address", "https://example.com/docs/guide/intro"),
            ("address", pull.as_str()),
        ]
    );

    // The bound a list taken now is held to covers them.
    let bound = drawn.sourced();
    assert!(drawn.sources().all(|source| source.line < bound));
}
