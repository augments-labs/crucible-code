//! What a component's tests and a drawer crate's tests check a drawing with.
//!
//! The widths and rooms of the fit sweep, the sweep itself, [`dump`] for
//! pictures checked in beside a component, and [`fits`], the one rule every
//! drawer keeps, asserted across the sweep. [`ANSWERS`] is the corpus of
//! answers a model writes, and [`read`] and [`resized`] read one through the
//! reader with a drawer registered, so a drawer's test sees its fence where an
//! answer puts it. This crate's own `fits` sweep reads
//! these, and a drawer crate reaches them by taking this crate with the
//! `testing` feature as a dev-dependency, never as a normal one: a release
//! build carries none of this, and `scripts/sh/repo-checks.sh` refuses a
//! manifest that would.

use crate::color::Theme;
use crate::drawing::{Drawers, Draws, Room};
use crate::escape::Escapes;
use crate::glyphs::Glyphs;
use crate::markdown::{Markdown, Said};
use crate::record::Record;
use crate::row::Row;
use crate::width;

pub use crate::dump::dump;

/// Every answer of the corpus this crate's own pictures are of, under the
/// name a case asks for it by: what a model writes, kept whole.
pub const ANSWERS: &[(&str, &str)] = crate::markdown::corpus::ANSWERS;

/// The rows an answer comes to in `room`, read by the reader the transcript
/// reads with and kept as the transcript keeps it, its fences drawn by
/// `drawers`.
///
/// `deltas` are the answer in the pieces it arrived in, one after another:
/// one piece is the answer whole.
#[must_use]
pub fn read(deltas: &[&str], drawers: &Drawers, room: Room) -> Vec<Row> {
    rows(&kept(deltas, drawers, room))
}

/// The rows [`read`] gives once the window has come to `columns` wide: the
/// answer read in `room`, then laid out again as a window that changes width
/// lays it out.
#[must_use]
pub fn resized(deltas: &[&str], drawers: &Drawers, room: Room, columns: usize) -> Vec<Row> {
    let mut record = kept(deltas, drawers, room);
    record.relay(columns, room.glyphs, Theme::Dark);
    rows(&record)
}

/// The record of an answer read in `room`, taking the steps the renderer
/// takes: what a terminal would obey goes, a run lands in the record, and a
/// block is kept with its source for the record to lay out.
fn kept(deltas: &[&str], drawers: &Drawers, room: Room) -> Record {
    /// `text` without what a terminal would obey or would not show.
    fn shown(escapes: &mut Escapes, text: &str) -> String {
        text.chars()
            .filter(|character| !escapes.holds(*character) && !width::unshown(*character))
            .collect()
    }

    let Room { columns, glyphs } = room;
    let mut record = Record::new(columns);
    record.draws_with(drawers.clone());
    let mut markdown = Markdown::new(glyphs);
    let mut escapes = Escapes::default();
    let mut write = |said: Said<'_>| match said {
        Said::Run { slot, text, link } => record.write(slot, &shown(&mut escapes, text), link),
        // The theme is any: rows are glyphs and slots, and no colour.
        Said::Block(mut block) => {
            block.source = shown(&mut escapes, &block.source);
            record.drawn(block, glyphs, Theme::Dark);
        }
        Said::Retract => record.retract(),
        Said::Aside(text) => record.aside(&shown(&mut escapes, text)),
        Said::Shortened(address) => record.shortened(address),
    };
    for delta in deltas {
        markdown.read(delta, columns, &mut write);
    }
    markdown.finish(columns, &mut write);
    record.end();
    record
}

/// Every row `record` holds, top to bottom.
fn rows(record: &Record) -> Vec<Row> {
    (0..record.lines())
        .flat_map(|line| record.folded(line))
        .collect()
}

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
/// A block the drawer declines in a room is not drawn there, so it has
/// nothing to fit; one it declines in every room proves nothing, and fails.
///
/// # Panics
///
/// When a row is wider than the room it was laid out for, or when the drawer
/// laid `source` out in no room at all.
#[track_caller]
pub fn fits(what: &str, drawer: &dyn Draws, source: &str) {
    let mut laid_out = 0_usize;
    for room in rooms() {
        let Ok(laid) = drawer.block(source, &room) else {
            continue;
        };
        laid_out += 1;
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
    assert!(laid_out > 0, "{what} laid nothing out in any room");
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

    /// Declines every block, in every room.
    struct Declines;

    impl Draws for Declines {
        fn names(&self) -> &'static [&'static str] {
            &["declines"]
        }

        fn block(&self, _: &str, _: &Room) -> Result<Laid, Undrawn> {
            Err(Undrawn::NotHere)
        }
    }

    #[test]
    #[should_panic(expected = "declines laid nothing out in any room")]
    fn a_drawer_that_lays_nothing_out_in_any_room_fails_the_fit_check() {
        fits("declines", &Declines, "a block it will not draw");
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

    /// Draws each line of a `diff` fence under a mark, cut to the room.
    struct Marks;

    impl Draws for Marks {
        fn names(&self) -> &'static [&'static str] {
            &["diff"]
        }

        fn block(&self, source: &str, room: &Room) -> Result<Laid, Undrawn> {
            let rows: Vec<Row> = source
                .lines()
                .map(|line| Row::plain(crate::width::clip(&format!("= {line}"), room.columns)))
                .collect();
            Ok(Laid {
                natural: (room.columns, rows.len()),
                rows,
                narrow: None,
            })
        }
    }

    fn marking() -> Drawers {
        Drawers::default().with(std::sync::Arc::new(Marks))
    }

    fn room(columns: usize) -> Room {
        Room {
            columns,
            glyphs: Glyphs::Unicode,
        }
    }

    fn texts(rows: &[Row]) -> Vec<String> {
        rows.iter().map(Row::text).collect()
    }

    const FENCED: &str = "Said.\n\n```diff\n-a line that was\n+a line that is\n```\n";

    #[test]
    fn an_answer_is_read_by_the_reader_and_drawn_by_the_drawers_it_is_given() {
        assert_eq!(
            texts(&read(&[FENCED], &marking(), room(30))),
            ["Said.", "", "= -a line that was", "= +a line that is"]
        );
        // With none registered the reader draws the fence itself.
        assert_eq!(
            texts(&read(&[FENCED], &Drawers::default(), room(30))),
            [
                "Said.",
                "",
                "┌─ diff",
                "│ -a line that was",
                "│ +a line that is",
                "└─"
            ]
        );
    }

    #[test]
    fn an_answer_cut_into_deltas_anywhere_reads_as_the_answer_whole() {
        let whole = read(&[FENCED], &marking(), room(30));
        assert_eq!(whole.len(), 4);
        let cuts: Vec<usize> = FENCED.char_indices().map(|(at, _)| at).skip(1).collect();
        assert!(cuts.len() > 40);
        for cut in cuts {
            let (before, after) = FENCED.split_at(cut);
            // As pictures: a run that arrived in two pieces is two spans.
            assert_eq!(
                dump(&read(&[before, after], &marking(), room(30)), 30),
                dump(&whole, 30),
                "cut at {cut}"
            );
        }
    }

    #[test]
    fn an_answer_whose_window_changed_width_is_the_answer_read_at_that_width() {
        let narrow = read(&[FENCED], &marking(), room(12));
        assert_eq!(
            texts(&narrow),
            ["Said.", "", "= -a line th", "= +a line th"]
        );
        assert_eq!(resized(&[FENCED], &marking(), room(30), 12), narrow);
        assert_eq!(
            resized(&[FENCED], &marking(), room(12), 30),
            read(&[FENCED], &marking(), room(30))
        );
    }

    #[test]
    fn the_answers_a_drawer_crate_reads_hold_the_corpus_diff_fences() {
        let named: Vec<&str> = ANSWERS
            .iter()
            .filter(|(_, text)| text.contains("```diff\n") && text.contains("```patch\n"))
            .map(|(name, _)| *name)
            .collect();
        assert_eq!(named, ["diff"]);
    }

    #[test]
    fn a_picture_of_rows_names_each_slot_by_its_class() {
        let rows = [Row::new().then(crate::Slot::Added, "x")];
        assert_eq!(dump(&rows, 3), "<Added>x</>  \n");
    }
}
