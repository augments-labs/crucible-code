use std::fmt::Write as _;

use crucible_tui::{Glyphs, Key, Pressed, Slot};

use super::super::notes::releases;
use super::*;

/// Twelve releases, `0.12.0` the newest. Release `n` has `n % 4 + 1`
/// entries, so the counts differ from row to row and one release has the
/// single entry that is spelled in the singular.
fn changelog() -> String {
    let mut text = String::from("# Changelog\n\n## [Unreleased]\n\n### Added\n\n- not yet\n");
    for minor in (1..=12).rev() {
        let _ = write!(
            text,
            "\n## [0.{minor}.0] - 2026-03-{minor:02}\n\n### Added\n\n"
        );
        for entry in 0..=(minor % 4) {
            let _ = writeln!(text, "- entry {entry}");
        }
    }
    text
}

/// The listing of that changelog, with `0.12.0` the running version.
fn listing(text: &str) -> Listing<'_> {
    Listing::new(releases(text), "0.12.0")
}

/// What the rows say, one string a row.
fn said(rows: &[Row]) -> Vec<String> {
    rows.iter().map(Row::text).collect()
}

/// What each of `keys` did, pressed one after another.
fn pressed(listing: &mut Listing<'_>, keys: impl IntoIterator<Item = Pressed>) -> Vec<Moved> {
    keys.into_iter().map(|key| listing.against(key)).collect()
}

/// `count` presses of the down arrow.
fn down(count: usize) -> impl Iterator<Item = Pressed> {
    std::iter::repeat_n(Pressed::Down, count)
}

fn enter() -> Pressed {
    Pressed::Key(Key::Enter)
}

#[test]
fn release_notes_list_is_newest_first_with_date_count_and_the_running_tag() {
    let text = changelog();
    let mut listing = listing(&text);
    let rows = said(&listing.rows(80, 24, Glyphs::Unicode));

    assert_eq!(rows.first().map(String::as_str), Some(&"─".repeat(80)[..]));
    assert_eq!(
        rows.get(1..4).expect("a title"),
        ["", "Release notes", ""],
        "{rows:#?}"
    );
    assert_eq!(
        rows.get(4..8).expect("four versions"),
        [
            "› 0.12.0      2026-03-12     1 entry     this version",
            "  0.11.0      2026-03-11     4 entries",
            "  0.10.0      2026-03-10     3 entries",
            "  0.9.0       2026-03-09     2 entries",
        ],
        "{rows:#?}"
    );
}

#[test]
fn release_notes_list_shows_the_eight_newest_and_a_last_row_that_reveals_the_rest() {
    let text = changelog();
    let mut listing = listing(&text);
    let rows = said(&listing.rows(80, 24, Glyphs::Unicode));

    assert_eq!(
        rows.get(11).map(String::as_str),
        Some("  0.5.0       2026-03-05     2 entries")
    );
    assert_eq!(
        rows.get(12).map(String::as_str),
        Some("  all 12 releases ↓")
    );
    assert_eq!(
        rows.get(13..),
        Some(
            &[
                String::new(),
                "↑↓ to walk · enter opens it · esc to close".to_owned()
            ][..]
        )
    );
}

#[test]
fn release_notes_list_names_its_keys_in_the_glyphs_of_the_set_and_wraps_the_foot_at_forty() {
    let text = changelog();
    let mut listing = listing(&text);

    let ascii = said(&listing.rows(80, 24, Glyphs::Ascii));
    assert_eq!(
        ascii.last().map(String::as_str),
        Some("^v to walk - enter opens it - esc to close")
    );
    assert_eq!(
        ascii.get(12).map(String::as_str),
        Some("  all 12 releases v")
    );

    let narrow = said(&listing.rows(40, 24, Glyphs::Unicode));
    assert_eq!(
        narrow.get(narrow.len() - 2..),
        Some(
            &[
                "↑↓ to walk · enter opens it · esc to".to_owned(),
                "close".to_owned()
            ][..]
        )
    );
    assert_eq!(
        narrow.get(4..6).expect("two versions"),
        [
            "› 0.12.0     1 entry     this version",
            "  0.11.0     4 entries",
        ],
        "the date goes first and the count stays: {narrow:#?}"
    );
}

#[test]
fn release_notes_list_down_then_enter_takes_the_second_version_only() {
    let text = changelog();
    let mut listing = listing(&text);

    let moved = pressed(&mut listing, [Pressed::Down, enter()]);

    assert_eq!(moved, [Moved::Redraw, Moved::Took]);
    assert_eq!(
        listing.chosen().map(|release| release.version),
        Some("0.11.0")
    );
}

#[test]
fn release_notes_list_stops_at_each_end_and_the_wheel_walks_as_the_arrows_do() {
    let text = changelog();
    let mut listing = listing(&text);

    assert_eq!(listing.against(Pressed::Up), Moved::Still);
    assert_eq!(
        listing.against(Pressed::Scrolled { back: false }),
        Moved::Redraw
    );
    assert_eq!(
        listing.against(Pressed::Scrolled { back: true }),
        Moved::Redraw
    );
    assert_eq!(listing.against(enter()), Moved::Took);
    assert_eq!(
        listing.chosen().map(|release| release.version),
        Some("0.12.0")
    );

    let mut listing = self::listing(&text);
    let _ = pressed(&mut listing, down(20));
    let rows = said(&listing.rows(80, 24, Glyphs::Unicode));
    assert!(
        rows.iter().any(|row| row.starts_with("› all 12 releases")),
        "{rows:#?}"
    );
}

#[test]
fn release_notes_list_enter_on_the_reveal_row_opens_every_release_in_place() {
    let text = changelog();
    let mut listing = listing(&text);

    let moved = pressed(&mut listing, down(8));
    assert!(moved.iter().all(|one| *one == Moved::Redraw), "{moved:?}");
    assert_eq!(
        listing.against(enter()),
        Moved::Redraw,
        "the reveal is not a version"
    );
    assert!(listing.chosen().is_none());

    let rows = said(&listing.rows(80, 24, Glyphs::Unicode));
    let expected = [
        "  ↑ 5 newer",
        "  0.7.0       2026-03-07     4 entries",
        "  0.6.0       2026-03-06     3 entries",
        "  0.5.0       2026-03-05     2 entries",
        "› 0.4.0       2026-03-04     1 entry",
        "  0.3.0       2026-03-03     4 entries",
        "  0.2.0       2026-03-02     3 entries",
        "  0.1.0       2026-03-01     2 entries",
        "",
    ];
    assert_eq!(rows.get(4..13).expect("nine rows"), expected, "{rows:#?}");
}

#[test]
fn release_notes_list_escape_takes_nothing() {
    let text = changelog();
    let mut listing = listing(&text);

    let moved = pressed(&mut listing, [Pressed::Down, Pressed::Escape]);

    assert_eq!(moved, [Moved::Redraw, Moved::Left]);
    assert!(listing.chosen().is_none());
}

#[test]
fn release_notes_list_colours_the_selected_version_strong_and_what_is_about_it_quiet() {
    let text = changelog();
    let mut listing = listing(&text);
    let rows = listing.rows(80, 24, Glyphs::Unicode);

    let slots = |at: usize| -> Vec<Slot> { rows.get(at).expect("a row").kinds().collect() };
    assert_eq!(
        slots(4),
        [Slot::Accent, Slot::Plain, Slot::Strong, Slot::Quiet]
    );
    assert_eq!(
        slots(5),
        [Slot::Accent, Slot::Plain, Slot::Plain, Slot::Quiet]
    );
}

#[test]
fn release_notes_list_fits_every_width_and_gives_up_rather_than_overflow_a_short_window() {
    let text = changelog();
    for columns in 1..=100 {
        for room in [4, 9, 12, 16, 24, 60] {
            let mut listing = listing(&text);
            for _ in 0..2 {
                let rows = listing.rows(columns, room, Glyphs::Unicode);
                assert!(rows.len() <= room || rows.is_empty(), "{columns}x{room}");
                for row in &rows {
                    assert!(
                        row.columns() <= columns,
                        "{columns}x{room}: {:?}",
                        row.text()
                    );
                }
                let _ = pressed(&mut listing, down(9));
                let _ = listing.against(enter());
            }
        }
    }
}

#[test]
fn release_notes_list_with_no_room_for_it_draws_nothing() {
    let text = changelog();
    let mut listing = listing(&text);

    assert!(listing.rows(80, 5, Glyphs::Unicode).is_empty());
}

#[test]
fn release_notes_list_of_eight_or_fewer_has_no_reveal_row() {
    let mut text = String::from("# Changelog\n");
    for minor in (1..=3).rev() {
        let _ = write!(text, "\n## [0.{minor}.0] - 2026-03-0{minor}\n\n- one\n");
    }
    let mut listing = Listing::new(releases(&text), "0.3.0");

    let rows = said(&listing.rows(80, 24, Glyphs::Unicode));
    assert!(
        !rows.iter().any(|row| row.contains("releases")),
        "{rows:#?}"
    );
    let _ = pressed(&mut listing, down(5));
    assert_eq!(listing.against(enter()), Moved::Took);
    assert_eq!(
        listing.chosen().map(|release| release.version),
        Some("0.1.0")
    );
}
