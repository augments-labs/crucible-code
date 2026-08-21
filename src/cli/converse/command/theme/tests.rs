use crucible_tui::{Glyphs, Recording, Renderer};

use super::*;
use crate::cli::converse::tests::plain;

/// A window tall enough to hold every theme this program offers at once.
const ROOMY: usize = 200;

/// The terms a `/theme` runs under, with nothing named yet.
fn terms() -> Terms {
    plain()
}

#[test]
fn every_answer_the_document_accepts_is_offered_and_no_other() {
    // The panel and the shape are two lists of the same thing, and a theme in
    // one and not the other is either a row nobody can keep or an answer nobody
    // is offered.
    let offered: Vec<&str> = EVERY.iter().map(|(_, name, _)| *name).collect();

    assert_eq!(offered, crucible_config::THEME);
}

#[test]
fn a_theme_named_on_the_line_is_taken_without_a_panel() {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let terms = terms();

    run("light", &mut renderer, &terms, false).expect("the theme to be taken");

    assert_eq!(terms.chosen.get(), Some(ThemeChoice::Light));
    assert!(renderer.terminal().written().contains("light"));
}

#[test]
fn a_word_that_names_no_theme_is_said_so_and_the_list_is_written() {
    // A window with room for the whole list. On a shorter one the reader sees
    // its foot and scrolls back for the rest, which is the transcript doing its
    // job rather than anything this test is about.
    let mut renderer = Renderer::new(Recording::new(80, ROOMY));
    let terms = terms();

    run("chartreuse", &mut renderer, &terms, false).expect("the run to finish");

    let shown = renderer.terminal().picture();
    let said = shown.said().join("\n");
    assert!(said.contains("no such theme: chartreuse"), "{said}");
    assert!(said.contains("colourblind-dark"), "{said}");
    // And nothing was taken on the way past.
    assert_eq!(terms.chosen.get(), None);
}

#[test]
fn the_listing_marks_the_one_in_force_and_only_that_one() {
    let mut renderer = Renderer::new(Recording::new(80, ROOMY));
    let terms = terms();
    terms.chosen.set(Some(ThemeChoice::Ansi));

    listed(&mut renderer, &terms).expect("the list to be written");

    let shown = renderer.terminal().picture();
    let said = shown.said();
    let marked: Vec<&String> = said
        .iter()
        .filter(|row| row.trim_start().starts_with(Glyphs::Unicode.done()))
        .collect();

    assert_eq!(marked.len(), 1, "{said:?}");
    assert!(
        marked.first().is_some_and(|row| row.contains("ansi")),
        "{marked:?}"
    );
}

/// A mark standing at the top of both lists.
fn standing() -> Standing {
    Standing {
        axis: Axis::Interface,
        interface: 0,
        code: 0,
        themes: crucible_tui::syntax::every_theme(),
    }
}

#[test]
fn the_specimen_shows_the_rows_a_theme_is_actually_judged_by() {
    // A diff, because that is where a theme paints a ground and picks a pair to
    // go on it, and the prompt row, because that one takes the reader's own.
    let rows = specimen(&standing(), 60, Glyphs::Unicode);
    let slots: Vec<Slot> = rows
        .iter()
        .filter_map(|row| row.text().is_empty().then_some(Slot::Plain))
        .collect();

    let said: String = rows.iter().map(Row::text).collect::<Vec<_>>().join("\n");

    assert!(said.contains('-'), "no line taken out: {said:?}");
    assert!(said.contains('+'), "no line put in: {said:?}");
    assert!(!slots.is_empty(), "nothing parts the blocks");

    // The rows a change did not touch are read, which is the half a syntax
    // theme decides — and the reason the signature changes colour with it.
    let kinds: Vec<Slot> = rows.iter().flat_map(Row::kinds).collect();
    assert!(
        kinds.contains(&Slot::Keyword),
        "the context rows were not read"
    );
    assert!(kinds.contains(&Slot::Comment), "the comment was not read");

    // And the rows it did touch carry a ground instead, which is the half an
    // interface theme decides. Both, in one picture.
    assert!(
        kinds.contains(&Slot::Removed),
        "no ground on the line taken out"
    );
    assert!(kinds.contains(&Slot::Added), "no ground on the line put in");
}

#[test]
fn every_row_of_a_specimen_ends_at_the_last_column() {
    // A ground that stops where the words stop has a ragged right edge with the
    // reader's own showing through it, and the specimen is the one place a
    // reader is looking straight at the grounds.
    for columns in 30..=100 {
        for row in specimen(&standing(), columns, Glyphs::Unicode) {
            assert!(
                row.columns() <= columns,
                "row past the last column at {columns}: {:?}",
                row.text()
            );
        }
    }
}

#[test]
fn a_preview_is_the_table_the_mark_is_standing_on() {
    let was = Style::plain();

    for (at, (choice, ..)) in EVERY.iter().enumerate() {
        let mut standing = standing();
        standing.interface = at;
        let previewed = previewing(was, None, &standing);

        assert_eq!(
            previewed.palette().theme(),
            Style::theme(Some(*choice), None),
            "row {at}"
        );
    }
}

#[test]
fn taking_a_theme_leaves_the_session_drawing_in_it() {
    let mut renderer = Renderer::new(Recording::new(80, 24));
    let terms = terms();

    taken(
        ThemeChoice::ColourblindDark,
        "colourblind-dark",
        &mut renderer,
        &terms,
    )
    .expect("the theme to be taken");

    assert_eq!(
        terms.style().palette().theme(),
        crucible_tui::Theme::ColourblindDark
    );
    assert_eq!(terms.chosen.get(), Some(ThemeChoice::ColourblindDark));
}
