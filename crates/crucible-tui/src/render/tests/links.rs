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
