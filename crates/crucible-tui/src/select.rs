//! What a drag over the window covers, drawn on it and read back off it.
//!
//! A terminal forwarding buttons is not using them itself, so the drag that
//! selects text in every other program stops working the moment this one asks
//! to hear about the wheel. What is offered in exchange is this: crucible owns
//! the screen, so crucible answers the drag.
//!
//! The unit is painted bytes rather than a [`Row`](crate::Row), because that is
//! what every band of the window has in common. The transcript reaches the
//! screen as rows and a palette; the turn and the box reach it as strings
//! something else has already coloured. Working on the bytes is what lets one
//! drag cross all three — which is the point of a selection the reader can
//! start in a tool result and finish in the prompt box.
//!
//! The two ends of a drag are kept as a [`Place`] rather than a window row,
//! because the transcript moves under a drag: the pointer resting at the top
//! of the band scrolls it back, and a wheel turned with the button still down
//! scrolls it either way. An end in the transcript names the display row of
//! the record it went down on, so the highlight stays on those words wherever
//! the band carries them — off the window included, since what a reader
//! dragged past on the way down is still part of what they took. An end in
//! the bands below names the window row, which is all there is to name: the
//! box and the turn do not scroll.
//!
//! Two answers come off the same walk over painted bytes. [`lit`] is what the
//! reader sees: the same row with reverse video turned on for the columns the
//! drag covers. [`said`] is what they get when the button comes up: the
//! characters under those columns, with the sequences dropped, because a
//! clipboard holds text and not instructions.

use std::ops::{Range, RangeInclusive};

use crate::escape::Escapes;
use crate::width::along;

/// Reverse video on: what a terminal draws where the reader has selected, and
/// what lights the row under the pointer.
pub(crate) const LIT: &str = "\x1b[7m";

/// Reverse video off. Only that attribute, so a row's own colour survives the
/// end of a selection that crossed part of it.
const DIM: &str = "\x1b[27m";

/// The byte that ends a sequence which set attributes, and so may have turned
/// [`LIT`] off in passing.
const ATTRIBUTES: char = 'm';

/// Where the transcript band is on the window, and what it is showing.
///
/// What turns a window row into a [`Place`] and back. Worked out per frame by
/// whoever lays the bands out, because both halves of it move: the band's
/// rows as the box grows, the record row at its top as the reader scrolls.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct View {
    /// The window rows the transcript band covers.
    pub(crate) band: Range<usize>,
    /// The absolute display row of the record shown on the band's first row.
    pub(crate) top: usize,
}

impl View {
    /// What window row `row` is a place for.
    pub(crate) fn place(&self, row: usize) -> Place {
        if self.band.contains(&row) {
            Place::Said(self.top + (row - self.band.start))
        } else {
            Place::Stood(row)
        }
    }

    /// The window row showing `place`, if the window is showing it.
    pub(crate) fn row(&self, place: Place) -> Option<usize> {
        match place {
            Place::Said(at) => at
                .checked_sub(self.top)
                .filter(|into| *into < self.band.len())
                .map(|into| self.band.start + into),
            Place::Stood(row) => Some(row),
        }
    }

    /// The record rows the band is showing.
    fn showing(&self) -> Range<usize> {
        self.top..self.top + self.band.len()
    }
}

/// A row a drag can have an end on, in the order the text runs.
///
/// Derived order is the order on the window: the transcript band stands over
/// every other band, so a record row comes before any row that stood below it,
/// and within a kind the row number decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Place {
    /// A display row of the record, absolute.
    ///
    /// Stable through scrolling and through the record spilling its oldest
    /// lines, which is what lets a drag keep the words it began on.
    Said(usize),
    /// A window row outside the transcript band, which does not scroll.
    Stood(usize),
}

/// A drag over the window, from where the button went down to where the
/// pointer has reached.
///
/// Two places and the column at each. What it covers is worked out against
/// the window every frame rather than stored, so the same drag lights the
/// right rows after the band it began in has scrolled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Taken {
    /// Where the button went down.
    from: (Place, usize),
    /// Where the pointer has reached.
    to: (Place, usize),
}

impl Taken {
    /// A button going down on window row `row`, which covers nothing until it
    /// moves.
    pub(crate) fn opened(row: usize, column: usize, view: &View) -> Self {
        let at = (view.place(row), column);
        Self { from: at, to: at }
    }

    /// The pointer has reached window row `row`, `column`.
    pub(crate) fn reaches(&mut self, row: usize, column: usize, view: &View) {
        self.to = (view.place(row), column);
    }

    /// Whether the drag covers anything at all.
    ///
    /// A press that never moved is a click, and a click is not a selection —
    /// it is whatever the loop underneath makes of it. One cell of reverse
    /// video under the pointer would be a selection nobody asked for.
    pub(crate) fn empty(&self) -> bool {
        self.from == self.to
    }

    /// The two ends, in the order the reader reads them.
    ///
    /// Which is not the order they arrived in: a drag upwards ends where it
    /// began, and the text it covers still runs down the screen.
    fn ends(self) -> ((Place, usize), (Place, usize)) {
        if self.from <= self.to {
            (self.from, self.to)
        } else {
            (self.to, self.from)
        }
    }

    /// Which columns of window row `at` the drag covers, in a window `columns`
    /// wide showing `view`.
    pub(crate) fn covers(self, at: usize, columns: usize, view: &View) -> Option<Range<usize>> {
        self.covering(view.place(at), columns)
    }

    /// Which columns of `place` the drag covers, in a window `columns` wide.
    ///
    /// Flowing rather than rectangular: the first row is covered from where the
    /// button went down to the end of the window, the last from the start of it
    /// to the pointer, and everything between them whole. That is what a
    /// selection over prose has to be — a rectangle over a transcript takes the
    /// left half of every line and none of the sentences.
    ///
    /// The column under the pointer is covered, at both ends. A reader dragging
    /// over a word expects the letter they stopped on to come with it.
    fn covering(self, place: Place, columns: usize) -> Option<Range<usize>> {
        if self.empty() {
            return None;
        }

        let ((top, opened), (foot, reached)) = self.ends();
        if place < top || place > foot {
            return None;
        }

        let start = if place == top { opened } else { 0 };
        let end = if place == foot { reached + 1 } else { columns };
        let end = end.min(columns);

        (start < end).then_some(start..end)
    }

    /// Every place the drag covers, top to bottom, with the columns covered at
    /// each, for a window `columns` wide showing `view` of a record whose last
    /// display row is `last`.
    ///
    /// What a release reads: the rows on the window and, where the band has
    /// carried an end off it, the record rows between. A drag that ended below
    /// the band takes the record down to the last row the band shows — what
    /// the reader could see between the two ends — and then the rows that
    /// stood under it.
    pub(crate) fn places(
        self,
        columns: usize,
        view: &View,
        last: Option<usize>,
    ) -> Vec<(Place, Range<usize>)> {
        if self.empty() {
            return Vec::new();
        }

        let ((top, _), (foot, _)) = self.ends();
        // Empty where the drag has no end of that kind: `first..=until` with
        // `until` under `first` yields nothing, which is the answer wanted.
        let said: RangeInclusive<usize> = match (top, foot) {
            (Place::Said(first), Place::Said(until)) => first..=until,
            (Place::Said(first), Place::Stood(_)) => {
                let shown = view.showing().end.saturating_sub(1);
                first..=last.map_or(shown, |last| last.min(shown))
            }
            (Place::Stood(_), _) => RangeInclusive::new(1, 0),
        };
        let stood: RangeInclusive<usize> = match (top, foot) {
            (Place::Stood(first), Place::Stood(until)) => first..=until,
            (Place::Said(_), Place::Stood(until)) => view.band.end..=until,
            (_, Place::Said(_)) => RangeInclusive::new(1, 0),
        };

        said.map(Place::Said)
            .chain(stood.map(Place::Stood))
            .filter_map(|place| Some((place, self.covering(place, columns)?)))
            .collect()
    }
}

/// `painted` with the columns in `covered` drawn as selected, into `into`.
///
/// Written into a buffer the caller owns because this runs for every covered
/// row of every frame a selection stands through, and a selection stands until
/// the reader clicks somewhere else.
///
/// The row is otherwise untouched, sequences included. What a selection changes
/// is one attribute, and it changes it back: a row that leaves here has the
/// same colours it arrived with, which is what lets the frame underneath go on
/// comparing bytes to decide what to write.
pub(crate) fn lit(
    painted: &str,
    covered: &Range<usize>,
    structural: &[Range<usize>],
    into: &mut String,
) {
    into.clear();

    if covered.is_empty() {
        into.push_str(painted);
        return;
    }

    let mut escapes = Escapes::default();
    let mut column = 0;
    let mut last: Option<char> = None;
    let mut inside = false;
    let mut selected = false;

    for character in painted.chars() {
        let held = escapes;
        if escapes.holds(character) {
            into.push(character);

            // A sequence that set attributes has just ended, and one of the
            // things it may have set is the absence of this one: a palette
            // closes a span by resetting, and a reset takes reverse video with
            // it. Turned back on rather than left off, because the row under a
            // selection is still coloured.
            if inside && held == Escapes::Control && character == ATTRIBUTES {
                into.push_str(LIT);
            }
            continue;
        }

        selected |= covered.contains(&column);
        let selectable =
            covered.contains(&column) && !structural.iter().any(|range| range.contains(&column));
        if !inside && selectable {
            inside = true;
            into.push_str(LIT);
        } else if inside && !selectable {
            inside = false;
            into.push_str(DIM);
        }

        into.push(character);

        // A character the terminal does not draw is kept and not counted, which
        // is what the walk behind every width here does with one. Dropping it
        // would make this the one place a row loses bytes on its way to the
        // screen.
        if let Some(step) = along(column, character, last) {
            column += step;
            last = Some(character);
        }
    }

    // The row ran out before the drag did. What the reader dragged over past
    // the end of the text is blank screen, and blank screen is part of the
    // block they took — without this a selection over a transcript is a ragged
    // right edge rather than a shape.
    if column < covered.end {
        if inside || selected {
            for _ in column..covered.end {
                into.push(' ');
            }
        } else {
            for _ in column..covered.start {
                into.push(' ');
            }
            column = column.max(covered.start);
            inside = true;
            into.push_str(LIT);
            for _ in column..covered.end {
                into.push(' ');
            }
        }
    }

    if inside {
        into.push_str(DIM);
    }
}

/// The characters of `painted` under the columns in `covered`.
///
/// Sequences dropped, because this is on its way to a clipboard: what the
/// reader selected is the words, and an attribute pasted into a shell is bytes
/// in the middle of a command.
pub(crate) fn said(painted: &str, covered: &Range<usize>, structural: &[Range<usize>]) -> String {
    let mut escapes = Escapes::default();
    let mut column = 0;
    let mut last: Option<char> = None;
    let mut taken = String::new();

    for character in painted.chars() {
        if escapes.holds(character) {
            continue;
        }

        let Some(step) = along(column, character, last) else {
            continue;
        };

        if covered.contains(&column) && !structural.iter().any(|range| range.contains(&column)) {
            taken.push(character);
        }

        column += step;
        last = Some(character);
    }

    taken
}

#[cfg(test)]
mod tests {
    use crate::width::columns;

    use super::*;

    /// A drag, spelled as the two ends a reader would describe it by.
    fn dragged(from: (usize, usize), to: (usize, usize)) -> Taken {
        let mut taken = Taken::opened(from.0, from.1, &View::default());
        taken.reaches(to.0, to.1, &View::default());
        taken
    }

    /// A window whose first `band` rows show the record from display row `top`.
    fn showing(band: usize, top: usize) -> View {
        View { band: 0..band, top }
    }

    /// What `lit` makes of a row, for a test that only wants to read it.
    fn drawn(painted: &str, covered: &Range<usize>) -> String {
        let mut into = String::new();
        lit(painted, covered, &[], &mut into);
        into
    }

    #[test]
    fn a_press_that_never_moved_covers_nothing() {
        // A click is not a selection. One cell of reverse video under the
        // pointer is a highlight nobody asked for, and it would arrive on every
        // click a loop underneath was answering for its own reasons.
        let taken = Taken::opened(3, 10, &View::default());

        assert!(taken.empty());
        assert_eq!(taken.covers(3, 80, &View::default()), None);
    }

    #[test]
    fn a_drag_across_one_row_covers_its_two_ends_and_what_is_between_them() {
        let taken = dragged((2, 4), (2, 8));

        assert_eq!(taken.covers(2, 80, &View::default()), Some(4..9));
        assert_eq!(taken.covers(1, 80, &View::default()), None);
        assert_eq!(taken.covers(3, 80, &View::default()), None);
    }

    #[test]
    fn a_drag_upwards_covers_what_the_same_drag_downwards_would() {
        // The ends arrive in the order the pointer moved and the text still
        // runs down the screen.
        let down = dragged((1, 3), (4, 9));
        let up = dragged((4, 9), (1, 3));

        for at in 0..6 {
            assert_eq!(
                up.covers(at, 80, &View::default()),
                down.covers(at, 80, &View::default()),
                "row {at}"
            );
        }
    }

    #[test]
    fn a_drag_over_several_rows_takes_the_ends_ragged_and_the_middle_whole() {
        let taken = dragged((1, 12), (3, 5));

        assert_eq!(taken.covers(1, 40, &View::default()), Some(12..40));
        assert_eq!(taken.covers(2, 40, &View::default()), Some(0..40));
        assert_eq!(taken.covers(3, 40, &View::default()), Some(0..6));
    }

    #[test]
    fn a_drag_never_reaches_past_the_window_it_was_made_in() {
        // The pointer is reported in the terminal's columns and the row was
        // painted in this one's. A covered range past the last column is a row
        // padded past the edge, which the terminal wraps itself.
        let taken = dragged((0, 2), (0, 500));

        assert_eq!(taken.covers(0, 40, &View::default()), Some(2..40));
    }

    #[test]
    fn a_drag_in_the_transcript_stays_on_its_words_when_the_band_scrolls() {
        // The end went down on record row 12, shown on window row 2. Three rows
        // of scrolling back carry that row to window row 5, and the highlight
        // goes with it: the rows it covers are the rows showing those words.
        let mut taken = Taken::opened(2, 4, &showing(8, 10));
        taken.reaches(3, 1, &showing(8, 10));

        let scrolled = showing(8, 7);
        assert_eq!(taken.covers(2, 40, &scrolled), None);
        assert_eq!(taken.covers(5, 40, &scrolled), Some(4..40));
        assert_eq!(taken.covers(6, 40, &scrolled), Some(0..2));
    }

    #[test]
    fn an_end_carried_off_the_window_still_covers_every_row_down_to_the_pointer() {
        // Opened on record row 10 at the top of the band, then the band
        // scrolled on by five while the pointer stayed on window row 3. The
        // words the button went down on are above the window now, and every
        // row from the top of the band to the pointer is between them.
        let mut taken = Taken::opened(0, 6, &showing(8, 10));
        taken.reaches(3, 2, &showing(8, 15));

        assert_eq!(taken.covers(0, 40, &showing(8, 15)), Some(0..40));
        assert_eq!(taken.covers(3, 40, &showing(8, 15)), Some(0..3));
        assert_eq!(taken.covers(4, 40, &showing(8, 15)), None);

        let places = taken.places(40, &showing(8, 15), Some(99));
        let expected: Vec<Place> = (10..=18).map(Place::Said).collect();
        assert_eq!(
            places.iter().map(|(place, _)| *place).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            places.first().map(|(_, covered)| covered.clone()),
            Some(6..40)
        );
        assert_eq!(
            places.last().map(|(_, covered)| covered.clone()),
            Some(0..3)
        );
    }

    #[test]
    fn an_end_carried_below_the_band_covers_the_band_and_none_of_the_box() {
        // Opened on record row 17, shown on the band's last row, then the band
        // scrolled back by four with the pointer on window row 2. The words
        // are under the box now; the box is not between the two ends and is
        // not lit.
        let mut taken = Taken::opened(7, 3, &showing(8, 10));
        taken.reaches(2, 5, &showing(8, 6));

        assert_eq!(taken.covers(2, 40, &showing(8, 6)), Some(5..40));
        assert_eq!(taken.covers(7, 40, &showing(8, 6)), Some(0..40));
        assert_eq!(taken.covers(8, 40, &showing(8, 6)), None);
        assert_eq!(taken.covers(9, 40, &showing(8, 6)), None);

        let places = taken.places(40, &showing(8, 6), Some(99));
        assert_eq!(places.len(), 10);
        assert_eq!(places.last(), Some(&(Place::Said(17), 0..4)));
    }

    #[test]
    fn a_drag_from_the_transcript_into_the_box_takes_what_stood_between() {
        // The band shows record rows 10..18 on window rows 0..8, and the box
        // stands on rows 8..11. What is between row 12 and the box is the rest
        // of the band, and then the rows of the box down to the pointer.
        let view = showing(8, 10);
        let mut taken = Taken::opened(2, 0, &view);
        taken.reaches(9, 3, &view);

        let places = taken.places(40, &view, Some(99));
        let expected: Vec<Place> = (12..18)
            .map(Place::Said)
            .chain((8..=9).map(Place::Stood))
            .collect();
        assert_eq!(
            places.iter().map(|(place, _)| *place).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            places.last().map(|(_, covered)| covered.clone()),
            Some(0..4)
        );
    }

    #[test]
    fn a_drag_below_the_band_ends_where_the_record_does() {
        // A record two rows long on an eight-row band: the rows under it are
        // blank window, and a drag into the box takes no record row that
        // does not exist.
        let view = showing(8, 0);
        let mut taken = Taken::opened(0, 0, &view);
        taken.reaches(8, 1, &view);

        let places = taken.places(40, &view, Some(1));
        assert_eq!(
            places.iter().map(|(place, _)| *place).collect::<Vec<_>>(),
            vec![Place::Said(0), Place::Said(1), Place::Stood(8)]
        );
    }

    #[test]
    fn a_lit_row_draws_the_same_number_of_columns_as_the_row_it_was_made_from() {
        // The one thing a highlight may not change. A row a column wider than
        // its window wraps, which puts a second row where one was allowed for
        // and moves every band below it down the screen.
        let painted = "\x1b[36mfn main\x1b[0m() { 日本 }";
        let wide = columns(painted);

        for window in 1..=wide {
            for start in 0..window {
                let covered = start..window;
                assert_eq!(
                    columns(&drawn(painted, &covered)),
                    wide.max(window),
                    "{covered:?} in a window {window} wide"
                );
            }
        }
    }

    #[test]
    fn a_span_closing_inside_a_selection_does_not_end_it() {
        // A palette closes a span by resetting, and a reset turns reverse video
        // off with everything else. Without turning it back on, a selection
        // over a coloured row is lit as far as the first colour and plain
        // after it.
        let drawn = drawn("\x1b[36mcoloured\x1b[0m plain", &(0..14));

        let ended = drawn.rfind(DIM).expect("the selection was never closed");
        let reset = drawn.find("\x1b[0m").expect("the span was never closed");
        assert!(
            drawn[reset..ended].contains(LIT),
            "the reset ended the selection: {drawn:?}"
        );
    }

    #[test]
    fn a_row_that_runs_out_inside_a_selection_is_padded_to_the_end_of_it() {
        // Otherwise a drag over a transcript is a ragged right edge rather than
        // a block, and the reader cannot see where their selection stops.
        let drawn = drawn("short", &(0..20));

        assert_eq!(columns(&drawn), 20);
        assert!(drawn.ends_with(DIM), "{drawn:?}");
    }

    #[test]
    fn a_row_the_selection_starts_past_the_end_of_is_padded_up_to_it_unlit() {
        let drawn = drawn("short", &(10..20));

        assert_eq!(columns(&drawn), 20);
        let opened = drawn.find(LIT).expect("nothing was lit");
        assert_eq!(
            columns(&drawn[..opened]),
            10,
            "the highlight did not start where the drag did: {drawn:?}"
        );
    }

    #[test]
    fn a_row_no_column_of_which_is_covered_is_left_exactly_as_it_arrived() {
        let painted = "\x1b[36muntouched\x1b[0m";

        assert_eq!(drawn(painted, &(0..0)), painted);
    }

    #[test]
    fn structural_art_is_neither_lit_nor_read_back() {
        let structural = std::slice::from_ref(&(2..3));
        let drawn = {
            let mut into = String::new();
            lit("  ⎿ result", &(2..3), structural, &mut into);
            into
        };

        assert!(!drawn.contains(LIT), "{drawn:?}");
        assert_eq!(said("  ⎿ result", &(0..10), structural), "   result");
    }

    #[test]
    fn the_same_character_in_literal_text_remains_selectable() {
        let drawn = drawn("literal ⎿ text", &(8..9));

        assert!(drawn.contains("\x1b[7m⎿"), "{drawn:?}");
        assert_eq!(said("literal ⎿ text", &(8..9), &[]), "⎿");
    }

    #[test]
    fn what_is_read_back_is_the_words_and_not_the_sequences() {
        assert_eq!(said("\x1b[36mfn main\x1b[0m()", &(0..7), &[]), "fn main");
    }

    #[test]
    fn what_is_read_back_stops_where_the_drag_did() {
        assert_eq!(said("one two three", &(4..7), &[]), "two");
    }

    #[test]
    fn a_wide_character_is_read_back_whole_from_the_column_it_starts_in() {
        // Two columns and one character. Selected by its first column it comes
        // back; selected by its second it does not come back twice.
        assert_eq!(said("日本", &(0..2), &[]), "日");
        assert_eq!(said("日本", &(0..4), &[]), "日本");
    }
}
