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
//! Two answers come off the same walk over those bytes. [`lit`] is what the
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

/// A drag over the window, from where the button went down to where the
/// pointer has reached.
///
/// Screen rows and columns, absolute, because a drag crosses bands and the
/// bands are laid out per frame. What it covers is worked out against the
/// window every frame rather than stored, so a selection made before a turn
/// grew still covers the same screen it was drawn on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Taken {
    /// Where the button went down.
    from: (usize, usize),
    /// Where the pointer has reached.
    to: (usize, usize),
}

impl Taken {
    /// A button going down, which covers nothing until it moves.
    pub(crate) fn opened(row: usize, column: usize) -> Self {
        let at = (row, column);
        Self { from: at, to: at }
    }

    /// The pointer has reached `row`, `column`.
    pub(crate) fn reaches(&mut self, row: usize, column: usize) {
        self.to = (row, column);
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
    fn ends(self) -> ((usize, usize), (usize, usize)) {
        if self.from <= self.to {
            (self.from, self.to)
        } else {
            (self.to, self.from)
        }
    }

    /// Every screen row the drag reaches.
    pub(crate) fn rows(self) -> RangeInclusive<usize> {
        let ((top, _), (foot, _)) = self.ends();
        top..=foot
    }

    /// Which columns of screen row `at` the drag covers, in a window `columns`
    /// wide.
    ///
    /// Flowing rather than rectangular: the first row is covered from where the
    /// button went down to the end of the window, the last from the start of it
    /// to the pointer, and everything between them whole. That is what a
    /// selection over prose has to be — a rectangle over a transcript takes the
    /// left half of every line and none of the sentences.
    ///
    /// The column under the pointer is covered, at both ends. A reader dragging
    /// over a word expects the letter they stopped on to come with it.
    pub(crate) fn covers(self, at: usize, columns: usize) -> Option<Range<usize>> {
        if self.empty() {
            return None;
        }

        let ((top, opened), (foot, reached)) = self.ends();
        if at < top || at > foot {
            return None;
        }

        let start = if at == top { opened } else { 0 };
        let end = if at == foot { reached + 1 } else { columns };
        let end = end.min(columns);

        (start < end).then_some(start..end)
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
        let mut taken = Taken::opened(from.0, from.1);
        taken.reaches(to.0, to.1);
        taken
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
        let taken = Taken::opened(3, 10);

        assert!(taken.empty());
        assert_eq!(taken.covers(3, 80), None);
    }

    #[test]
    fn a_drag_across_one_row_covers_its_two_ends_and_what_is_between_them() {
        let taken = dragged((2, 4), (2, 8));

        assert_eq!(taken.covers(2, 80), Some(4..9));
        assert_eq!(taken.covers(1, 80), None);
        assert_eq!(taken.covers(3, 80), None);
    }

    #[test]
    fn a_drag_upwards_covers_what_the_same_drag_downwards_would() {
        // The ends arrive in the order the pointer moved and the text still
        // runs down the screen.
        let down = dragged((1, 3), (4, 9));
        let up = dragged((4, 9), (1, 3));

        for at in 0..6 {
            assert_eq!(up.covers(at, 80), down.covers(at, 80), "row {at}");
        }
    }

    #[test]
    fn a_drag_over_several_rows_takes_the_ends_ragged_and_the_middle_whole() {
        let taken = dragged((1, 12), (3, 5));

        assert_eq!(taken.covers(1, 40), Some(12..40));
        assert_eq!(taken.covers(2, 40), Some(0..40));
        assert_eq!(taken.covers(3, 40), Some(0..6));
    }

    #[test]
    fn a_drag_never_reaches_past_the_window_it_was_made_in() {
        // The pointer is reported in the terminal's columns and the row was
        // painted in this one's. A covered range past the last column is a row
        // padded past the edge, which the terminal wraps itself.
        let taken = dragged((0, 2), (0, 500));

        assert_eq!(taken.covers(0, 40), Some(2..40));
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
