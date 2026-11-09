//! What a component's tests and a drawer crate's tests check a drawing with.
//!
//! The widths and rooms of the fit sweep, the sweep itself, [`dump`] for
//! pictures checked in beside a component, and [`fits`], the one rule every
//! drawer keeps, asserted across the sweep. This crate's own `fits` sweep reads
//! these, and a drawer crate reaches them by taking this crate with the
//! `testing` feature as a dev-dependency, never as a normal one: a release
//! build carries none of this, and `scripts/sh/repo-checks.sh` refuses a
//! manifest that would.

use crate::drawing::{Draws, Room};
use crate::glyphs::Glyphs;
use crate::row::Row;

pub use crate::dump::dump;

/// Every width worth walking: one column, and on past the widest terminal
/// anybody works in.
///
/// From one rather than from nothing, because a window of no columns is not a
/// window. A terminal that will not say its size is given
/// [`Size::FALLBACK`](crate::Size::FALLBACK) instead of a zero, and the one
/// component that draws into a nought-column window does it on purpose — the
/// prompt mark, so that a record says a prompt was asked — with its own test
/// beside it saying so.
pub const WIDTHS: std::ops::RangeInclusive<usize> = 1..=200;

/// Every room worth walking, for the components that are given one.
///
/// From no rows at all — which is what a window with something already standing
/// in it has left — through every height a component decides something at, and
/// on past an ordinary terminal.
///
/// A list rather than the range the widths are, because the two are not the
/// same kind of axis. Width is where the wrapping arithmetic lives, and it is
/// wrong at one column and right at the next. Height is where a component picks
/// which rung of its ladder to draw, and those are counted in single rows near
/// the bottom and nowhere near the top.
pub const ROOMS: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 8, 12, 24, 40];

/// Both fonts, because a component picks its glyphs by which it was handed and
/// the two are not the same width.
pub const FONTS: [Glyphs; 2] = [Glyphs::Unicode, Glyphs::Ascii];

/// Sweeps `laid` across every width in both fonts.
///
/// # Panics
///
/// When a row is wider than the window it was laid out for.
#[track_caller]
pub fn across(what: &str, laid: impl Fn(usize, Glyphs) -> Vec<Row>) {
    for glyphs in FONTS {
        for columns in WIDTHS {
            within(&laid(columns, glyphs), columns, what, glyphs);
        }
    }
}

/// The same, across every room as well, checking the height too.
///
/// # Panics
///
/// When a row is wider than its window, or there are more rows than room.
#[track_caller]
pub fn down(what: &str, laid: impl Fn(usize, usize, Glyphs) -> Vec<Row>) {
    for glyphs in FONTS {
        for columns in WIDTHS {
            for room in ROOMS {
                let rows = laid(columns, room, glyphs);
                within(&rows, columns, what, glyphs);
                assert!(
                    rows.len() <= room,
                    "{what} drew {} rows into a window with room for {room}, at {columns} \
                     columns with {glyphs:?}",
                    rows.len()
                );
            }
        }
    }
}

/// Asserts that nothing in `rows` reaches past column `columns`.
///
/// # Panics
///
/// When a row does.
#[track_caller]
pub fn within(rows: &[Row], columns: usize, what: &str, glyphs: Glyphs) {
    for row in rows {
        assert!(
            row.columns() <= columns,
            "{what} drew {} columns into a window {columns} wide with {glyphs:?}: {:?}",
            row.columns(),
            row.text()
        );
    }
}

/// Every room the sweep walks: each width in both fonts.
pub fn rooms() -> impl Iterator<Item = Room> {
    FONTS
        .into_iter()
        .flat_map(|glyphs| WIDTHS.map(move |columns| Room { columns, glyphs }))
}

/// Asserts that `drawer` lays `source` out within every room of the sweep:
/// its rows, and its narrow layout where it offers one.
///
/// A block the drawer declines is not drawn, so it has nothing to fit.
///
/// # Panics
///
/// When a row is wider than the room it was laid out for.
#[track_caller]
pub fn fits(what: &str, drawer: &dyn Draws, source: &str) {
    for room in rooms() {
        let Ok(laid) = drawer.block(source, &room) else {
            continue;
        };
        within(&laid.rows, room.columns, what, room.glyphs);
        if let Some(narrow) = &laid.narrow {
            within(
                narrow,
                room.columns,
                &format!("{what}'s narrow layout"),
                room.glyphs,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chrome::Undrawn;
    use crate::drawing::{Draws, Laid, Room};

    /// Draws every row the width of the block's first line, whatever the room.
    struct Wide;

    impl Draws for Wide {
        fn names(&self) -> &'static [&'static str] {
            &["wide"]
        }

        fn block(&self, source: &str, _: &Room) -> Result<Laid, Undrawn> {
            Ok(Laid {
                rows: vec![Row::plain(source)],
                natural: (crate::width::columns(source), 1),
                narrow: None,
            })
        }
    }

    /// Fits its rows, and offers a narrow layout that does not.
    struct NarrowTooWide;

    impl Draws for NarrowTooWide {
        fn names(&self) -> &'static [&'static str] {
            &["narrow"]
        }

        fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
            Ok(Laid {
                rows: vec![Row::plain(crate::width::clip(source, room.columns))],
                natural: (crate::width::columns(source), 1),
                narrow: Some(vec![Row::plain(source)]),
            })
        }
    }

    /// Fits, by clipping.
    struct Clipped;

    impl Draws for Clipped {
        fn names(&self) -> &'static [&'static str] {
            &["clipped"]
        }

        fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
            Ok(Laid {
                rows: vec![Row::plain(crate::width::clip(source, room.columns))],
                natural: (crate::width::columns(source), 1),
                narrow: None,
            })
        }
    }

    #[test]
    fn the_sweep_walks_every_width_in_both_fonts() {
        let rooms: Vec<Room> = rooms().collect();
        assert_eq!(rooms.len(), WIDTHS.count() * FONTS.len());
        assert!(rooms.iter().any(|room| room.columns == 1));
        assert!(rooms.iter().any(|room| room.columns == 200));
    }

    #[test]
    #[should_panic(expected = "drew 11 columns into a window 1 wide")]
    fn a_drawer_whose_rows_outgrow_the_room_fails_the_fit_check() {
        fits("wide", &Wide, "0123456789a");
    }

    #[test]
    #[should_panic(expected = "narrow layout")]
    fn a_narrow_layout_that_outgrows_the_room_fails_the_fit_check() {
        fits("narrow", &NarrowTooWide, "0123456789a");
    }

    #[test]
    fn a_drawer_that_clips_to_its_room_passes_the_fit_check() {
        fits(
            "clipped",
            &Clipped,
            "a line far longer than the narrow end of the sweep",
        );
    }

    #[test]
    fn a_picture_of_rows_names_each_slot_by_its_class() {
        let rows = [Row::new().then(crate::Slot::Added, "x")];
        assert_eq!(dump(&rows, 3), "<Added>x</>  \n");
    }
}
