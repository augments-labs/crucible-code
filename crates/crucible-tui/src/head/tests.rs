use super::*;

/// The directory a session in this repository is bound to, spelled the way the
/// caller hands it over — long enough that most windows cannot hold all of it.
const ROOT: &str = "/home/somebody/Projects/augments-labs/crucible-code";

/// A directory short enough to stand beside the model in an ordinary window.
const NEAR: &str = "/src/crucible";

/// The model the row says the next turn goes to.
const NAMED: &str = "claude-sonnet-5";

/// Whose model it is.
const VENDOR: &str = "anthropic";

/// How hard it is being asked to think.
const RUNG: &str = "high";

/// The three of them as the row joins them.
const MODEL: &str = "anthropic/claude-sonnet-5 · high";

/// The head row of an ordinary session.
const fn head() -> Head<'static> {
    Head {
        provider: VENDOR,
        model: NAMED,
        effort: Some(RUNG),
        root: ROOT,
    }
}

/// What the row says at `columns`, as one string.
fn said(head: &Head<'_>, columns: usize) -> String {
    head.row(columns, Glyphs::Unicode).text()
}

#[test]
fn the_row_says_where_the_session_is_and_what_it_is_talking_to() {
    // Both ends, and nothing joining them: the path starts the row where the
    // eye starts, and the model ends it in the column it ended the status row
    // in before it came up here.
    let row = said(
        &Head {
            root: NEAR,
            ..head()
        },
        80,
    );

    assert!(row.starts_with(NEAR), "{row:?}");
    assert!(row.ends_with(MODEL), "{row:?}");
    assert_eq!(width::columns(&row), 80, "{row:?}");
}

#[test]
fn a_path_longer_than_the_room_left_for_it_keeps_the_end() {
    // The leaf is what tells two checkouts apart. Everything before it is the
    // same for every project somebody keeps in one place, so it is what goes —
    // and it goes a whole segment at a time, which is what leaves something
    // that still reads as a path.
    let row = said(&head(), 80);

    assert!(
        row.starts_with("…/Projects/augments-labs/crucible-code"),
        "{row:?}"
    );
    assert!(row.ends_with(MODEL), "{row:?}");
    assert_eq!(width::columns(&row), 80, "{row:?}");
}

#[test]
fn a_row_that_cannot_hold_the_leaf_beside_the_model_drops_the_model() {
    // The one place this row gives way in the opposite order to every other:
    // the model is a fact the reader chose and can ask for again, and the path
    // is the fact this row was added to carry.
    let row = said(&head(), 46);

    assert!(!row.contains(NAMED), "{row:?}");
    assert_eq!(row, "…/Projects/augments-labs/crucible-code");
}

#[test]
fn a_session_with_no_model_chosen_says_nothing_in_its_place() {
    // Not a vendor over an empty name, and not a gap where a name goes. A
    // session nobody has chosen a model for has a path and the whole row.
    let unchosen = Head {
        model: "",
        root: NEAR,
        ..head()
    };

    assert_eq!(said(&unchosen, 80), NEAR);
}

#[test]
fn a_model_nobody_named_a_vendor_for_is_drawn_on_its_own() {
    // `--model` takes a bare name back, so a bare name is a state the row has
    // to be able to say. A slash with nothing before it would read as a vendor
    // whose name failed to arrive.
    let bare = Head {
        provider: "",
        root: NEAR,
        ..head()
    };

    assert!(
        said(&bare, 80).ends_with("claude-sonnet-5 · high"),
        "{:?}",
        said(&bare, 80)
    );
}

#[test]
fn the_rung_is_drawn_only_where_one_is_in_force() {
    // The vendor's own default is not this program's to name. A rung on this
    // row that was never sent is the one thing it must never be.
    let unasked = Head {
        effort: None,
        root: NEAR,
        ..head()
    };

    assert!(
        said(&unasked, 80).ends_with("anthropic/claude-sonnet-5"),
        "{:?}",
        said(&unasked, 80)
    );
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
        ..head()
    };

    assert_eq!(said(&unbroken, 20), "a-directory-nobody-s");
}

#[test]
fn every_width_gives_way_in_the_order_this_row_gives_way_in() {
    // The sweep in `fits.rs` says the row fits. This one says the row is right
    // while it does: the model never stands beside a path that was cut in the
    // middle of a segment, and the path is on the row at every width there is
    // one.
    for columns in 1..=200 {
        for glyphs in [Glyphs::Unicode, Glyphs::Ascii] {
            let row = head().row(columns, glyphs).text();

            assert!(width::columns(&row) <= columns, "{columns} {row:?}");
            assert!(!row.is_empty(), "{columns} {row:?}");

            let mark = glyphs.ellipsis();
            let cut = !row.starts_with(mark) && !row.starts_with('/');
            assert!(!(cut && row.contains(NAMED)), "{columns} {row:?}");
        }
    }
}
