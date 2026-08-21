use super::*;

/// The directory a session in this repository is bound to, spelled the way the
/// caller hands it over — long enough that most windows cannot hold all of it.
const ROOT: &str = "/home/somebody/Projects/augments-labs/crucible-code";

/// A directory short enough to stand whole in an ordinary window.
const NEAR: &str = "/src/crucible";

/// The head row of an ordinary session.
const fn head() -> Head<'static> {
    Head { root: ROOT }
}

/// What the row says at `columns`, as one string.
fn said(head: &Head<'_>, columns: usize) -> String {
    head.row(columns, Glyphs::Unicode).text()
}

#[test]
fn the_row_says_where_the_session_is() {
    // The whole path and nothing else. What the session is talking to is said
    // under the box, beside the keys that change it.
    let row = said(&Head { root: NEAR }, 80);

    assert!(row.starts_with(NEAR), "{row:?}");
    assert!(row.ends_with("transcript map →"), "{row:?}");
    assert_eq!(width::columns(&row), 80);
}

#[test]
fn a_path_longer_than_the_row_keeps_the_end() {
    // The leaf is what tells two checkouts apart. Everything before it is the
    // same for every project somebody keeps in one place, so it is what goes —
    // and it goes a whole segment at a time, which is what leaves something
    // that still reads as a path.
    let row = said(&head(), 46);
    assert!(row.starts_with("…/crucible-code"), "{row:?}");
    assert!(row.ends_with("transcript map →"), "{row:?}");
}

#[test]
fn the_mark_costs_what_the_set_in_force_charges_for_it() {
    // Three columns in the ascii set against one in the other. A path shortened
    // as though the mark were one column is a row three columns past the edge,
    // which is the row the terminal wraps.
    let ascii = head().row(31, Glyphs::Ascii).text();

    assert_eq!(ascii, ".../augments-labs/crucible-code");
}

#[test]
fn a_path_of_one_long_segment_keeps_what_fits_of_the_front() {
    // Nothing to drop a segment at, so there is no end worth keeping over the
    // front: what is left is the only thing left to keep.
    let unbroken = Head {
        root: "a-directory-nobody-should-have-named-this-way",
    };

    assert_eq!(said(&unbroken, 20), "a-directory-nobody-s");
}

#[test]
fn every_width_gives_way_in_the_order_this_row_gives_way_in() {
    // The sweep in `fits.rs` says the row fits. This one says the row is right
    // while it does: there is a path on it at every width there is one, and it
    // never says more columns than it was given.
    for columns in 1..=200 {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let row = head().row(columns, glyphs).text();

            assert!(width::columns(&row) <= columns, "{columns} {row:?}");
            assert!(!row.is_empty(), "{columns} {row:?}");
        }
    }
}

#[test]
fn the_transcript_map_label_is_the_columns_that_open_it() {
    let row = said(&head(), 80);
    let control = Head::transcript(80).expect("room for the control");

    assert!(row.ends_with("transcript map →"), "{row:?}");
    assert_eq!(control.len(), width::columns("transcript map →"));
}

#[test]
fn pointing_at_the_transcript_map_changes_only_its_slot() {
    let quiet = head().pointed(80, Glyphs::Unicode, false);
    let pointed = head().pointed(80, Glyphs::Unicode, true);

    assert_eq!(quiet.text(), pointed.text());
    assert_eq!(quiet.kinds().last(), Some(Slot::Quiet));
    assert_eq!(pointed.kinds().last(), Some(Slot::Accent));
    assert!(
        head()
            .row(80, Glyphs::Ascii)
            .text()
            .ends_with("transcript map >")
    );
}

#[test]
fn a_window_too_narrow_for_a_map_keeps_the_path_instead() {
    let row = said(&Head { root: NEAR }, 10);

    assert_eq!(row, "…/crucible");
    assert_eq!(Head::transcript(10), None);
}
