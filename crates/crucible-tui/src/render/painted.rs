//! What is on the screen, and the frame that changes it.
//!
//! A frame writes only the rows whose picture is not already there. That is
//! both what a screen this process owns costs and what it saves: every row is
//! compared against what the last frame left, and only the rows that differ
//! are written. A keystroke writes the row it changed; a delta arriving into a
//! full transcript writes the rows the record moved and nothing else.
//!
//! What is on screen is held as painted bytes rather than as rows, because that
//! is the comparison worth making: two rows that paint the same are the same
//! picture however they were built, and the string is what would be written
//! either way.
//!
//! A row nobody has drawn on is not the same as a row drawn blank, so the
//! answer for each is [`Option`] rather than an empty string. [`Painted::forget`]
//! is how a frame says it no longer knows — after taking the screen, and after
//! a window changed size, which are the two things that make every row wrong at
//! once.
//!
//! What the reader has selected is applied here rather than by whoever built
//! the row, for two reasons. It is the one place every band's bytes pass
//! through, so a drag crosses the transcript, the turn and the box without any
//! of them knowing about it. And it goes on *before* the comparison, so a
//! selection that grew by a row writes the two rows it changed and no others —
//! a highlight is part of the picture, and a picture that changed is a row
//! worth writing.

use crate::color::Palette;
use std::ops::Range;

use crate::row::Row;
use crate::select::{self, Taken};

use super::frame::Frame;

/// The screen as the last frame left it, and the frame replacing it.
#[derive(Debug, Default)]
pub(crate) struct Painted {
    /// What each screen row was painted as, or `None` where that is unknown.
    was: Vec<Option<String>>,
    /// Reused across frames: a frame is one string, so it is one write.
    frame: Frame,
    /// Reused across frames: one row on its way into the frame.
    row: String,
    /// Reused across frames: the same row with the selection drawn on it.
    lit: String,
    /// What the reader has selected, if anything.
    taken: Option<Taken>,
    /// Structural columns on each row, absent from highlights and copied text.
    structural: Vec<Vec<Range<usize>>>,
    /// How wide the window is, which is how far a covered row reaches.
    columns: usize,
    /// Whether this frame has written a row.
    ///
    /// What lets a frame that changed nothing cost nothing: the sequences that
    /// bracket a frame are bytes too, and a redraw asked for by something that
    /// turned out not to have moved would otherwise write them at whatever rate
    /// it was asked at.
    changed: bool,
    /// Where the last frame left the cursor.
    parked: Option<(usize, usize)>,
}

impl Painted {
    /// A screen nothing has been drawn on.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Starts a frame for a window `rows` tall and `columns` wide.
    pub(crate) fn open(&mut self, rows: usize, columns: usize) {
        self.was.resize(rows, None);
        self.structural.clear();
        self.structural.resize_with(rows, Vec::new);
        self.columns = columns;
        self.changed = false;
        self.frame.open();
    }

    /// Draws the rows `taken` covers as selected, until told otherwise.
    pub(crate) fn selects(&mut self, taken: Option<Taken>) {
        self.taken = taken;
    }

    /// The text of what is selected, read back off the screen.
    ///
    /// Off the screen rather than out of the record, so that what reaches a
    /// clipboard is what the reader was looking at — the same clipping, the
    /// same folding, and the box and the turn band included, none of which the
    /// record holds. Each row is trimmed of the blank it was padded out with,
    /// because that padding is screen and not text.
    pub(crate) fn read(&self) -> String {
        let Some(taken) = self.taken else {
            return String::new();
        };

        let mut lines: Vec<&str> = Vec::new();
        let said: Vec<String> = taken
            .rows()
            .filter_map(|at| {
                let covered = taken.covers(at, self.columns)?;
                let was = self.was.get(at)?.as_deref().unwrap_or_default();
                let structural = self.structural.get(at).map_or(&[][..], Vec::as_slice);
                Some(select::said(was, &covered, structural))
            })
            .collect();

        lines.extend(said.iter().map(|line| line.trim_end()));
        lines.join("\n")
    }

    /// Forgets what is on screen, so the next frame writes every row of it.
    pub(crate) fn forget(&mut self) {
        self.was.clear();
        self.parked = None;
    }

    /// Draws `row` on screen row `at`, in `palette`, if it is not there already.
    pub(crate) fn paint(&mut self, at: usize, row: &Row, palette: &Palette) {
        // Taken out and put back, so the buffer the row is painted into is not
        // borrowed from the same value the frame is written into.
        let mut into = std::mem::take(&mut self.row);
        into.clear();
        row.paint_into(palette, &mut into);
        if let Some(structural) = self.structural.get_mut(at) {
            *structural = row.structural();
        }
        self.show(at, &into);
        self.row = into;
    }

    /// Draws bytes something else painted on screen row `at`.
    pub(crate) fn put(&mut self, at: usize, painted: &str) {
        if let Some(structural) = self.structural.get_mut(at) {
            structural.clear();
        }
        self.show(at, painted);
    }

    /// Leaves screen row `at` empty.
    pub(crate) fn blank(&mut self, at: usize) {
        if let Some(structural) = self.structural.get_mut(at) {
            structural.clear();
        }
        self.show(at, "");
    }

    /// Whichever of the three above ends up writing a row, selection and all.
    fn show(&mut self, at: usize, painted: &str) {
        let Some(covered) = self.taken.and_then(|taken| taken.covers(at, self.columns)) else {
            self.set(at, painted);
            return;
        };

        let structural = self.structural.get(at).map_or(&[][..], Vec::as_slice);
        let mut into = std::mem::take(&mut self.lit);
        select::lit(painted, &covered, structural, &mut into);
        self.set(at, &into);
        self.lit = into;
    }

    /// The comparison, and the write it decides.
    fn set(&mut self, at: usize, painted: &str) {
        let Self {
            was,
            frame,
            changed,
            ..
        } = self;

        // A row past the foot of the window is one the caller worked out from a
        // size this has not been told about yet. Dropped rather than drawn: the
        // frame that follows the resize draws every row anyway.
        let Some(was) = was.get_mut(at) else {
            return;
        };

        if was.as_deref() == Some(painted) {
            return;
        }

        let was = was.get_or_insert_with(String::new);
        was.clear();
        was.push_str(painted);
        frame.row(at, painted);
        *changed = true;
    }

    /// Leaves the cursor at `row`, `column`.
    pub(crate) fn park(&mut self, row: usize, column: usize) {
        if self.parked != Some((row, column)) {
            self.parked = Some((row, column));
            self.changed = true;
        }
        self.frame.park(row, column);
    }

    /// Whether this frame is worth writing.
    pub(crate) fn moved(&self) -> bool {
        self.changed
    }

    /// The assembled frame, closed.
    pub(crate) fn sealed(&mut self) -> &str {
        self.frame.sealed()
    }
}

#[cfg(test)]
mod tests {
    use crate::color::Slot;

    use super::*;

    /// A window wide enough that no test here is about its edge.
    const WIDE: usize = 40;

    fn plain() -> Palette {
        Palette::plain()
    }

    /// A palette that writes colour, for the one test about the difference.
    fn worn() -> Palette {
        Palette::resolve(true, crate::color::Theme::Dark, None, &|name| {
            (name == "COLORTERM").then(|| "truecolor".to_owned())
        })
    }

    #[test]
    fn a_row_already_showing_what_it_would_be_given_is_not_written_again() {
        let mut painted = Painted::new();
        painted.open(4, WIDE);
        painted.paint(0, &Row::plain("hello"), &plain());
        assert!(painted.moved());

        painted.open(4, WIDE);
        painted.paint(0, &Row::plain("hello"), &plain());
        assert!(!painted.moved(), "a row nothing changed was written again");
    }

    #[test]
    fn a_row_whose_words_changed_is_written_and_the_others_are_not() {
        let mut painted = Painted::new();
        painted.open(3, WIDE);
        for (at, text) in ["one", "two", "three"].iter().enumerate() {
            painted.paint(at, &Row::plain(*text), &plain());
        }
        painted.sealed();

        painted.open(3, WIDE);
        painted.paint(0, &Row::plain("one"), &plain());
        painted.paint(1, &Row::plain("TWO"), &plain());
        painted.paint(2, &Row::plain("three"), &plain());

        let written = painted.sealed();
        assert!(written.contains("TWO"), "{written:?}");
        assert!(!written.contains("one"), "{written:?}");
        assert!(!written.contains("three"), "{written:?}");
    }

    #[test]
    fn a_row_that_only_changed_colour_is_written_again() {
        // What is compared is the picture rather than the words, because the
        // picture is what the terminal was sent.
        let mut painted = Painted::new();
        painted.open(1, WIDE);
        painted.paint(0, &Row::plain("same"), &plain());
        painted.sealed();

        let mut row = Row::new();
        row.push(Slot::Accent, "same");
        painted.open(1, WIDE);
        painted.paint(0, &row, &worn());

        assert!(painted.moved(), "a recoloured row was left as it was");
    }

    #[test]
    fn forgetting_writes_every_row_again() {
        let mut painted = Painted::new();
        painted.open(2, WIDE);
        painted.paint(0, &Row::plain("kept"), &plain());
        painted.sealed();

        painted.forget();
        painted.open(2, WIDE);
        painted.paint(0, &Row::plain("kept"), &plain());

        assert!(painted.moved(), "the screen was assumed to be unchanged");
    }

    #[test]
    fn a_frame_that_changed_nothing_says_so() {
        let mut painted = Painted::new();
        painted.open(2, WIDE);
        painted.paint(0, &Row::plain("still"), &plain());
        painted.park(1, 0);
        painted.sealed();

        painted.open(2, WIDE);
        painted.paint(0, &Row::plain("still"), &plain());
        painted.park(1, 0);

        assert!(!painted.moved());
    }

    #[test]
    fn a_cursor_that_moved_is_a_frame_worth_writing() {
        // Nothing on screen changed and the reader still has to see the caret
        // where they put it.
        let mut painted = Painted::new();
        painted.open(2, WIDE);
        painted.park(1, 0);
        painted.sealed();

        painted.open(2, WIDE);
        painted.park(1, 4);

        assert!(painted.moved());
    }

    #[test]
    fn a_row_past_the_foot_of_the_window_is_dropped_rather_than_drawn() {
        let mut painted = Painted::new();
        painted.open(2, WIDE);
        painted.paint(5, &Row::plain("beyond"), &plain());

        assert!(!painted.moved());
    }

    #[test]
    fn a_window_that_grew_has_rows_nothing_is_known_about() {
        let mut painted = Painted::new();
        painted.open(1, WIDE);
        painted.paint(0, &Row::plain("top"), &plain());
        painted.sealed();

        painted.open(3, WIDE);
        painted.blank(2);

        assert!(painted.moved(), "a row never drawn on was assumed empty");
    }

    /// A drag, spelled as the two ends a reader would describe it by.
    fn dragged(from: (usize, usize), to: (usize, usize)) -> Taken {
        let mut taken = Taken::opened(from.0, from.1);
        taken.reaches(to.0, to.1);
        taken
    }

    #[test]
    fn a_drag_reaching_a_row_writes_that_row_and_leaves_the_others_alone() {
        // The reason the highlight is applied here rather than by a component:
        // it goes on before the comparison, so a drag that grew by one row
        // costs one row of writing.
        let mut painted = Painted::new();
        painted.open(3, WIDE);
        for (at, text) in ["one", "two", "three"].iter().enumerate() {
            painted.paint(at, &Row::plain(*text), &plain());
        }
        painted.sealed();

        painted.selects(Some(dragged((0, 0), (0, 2))));
        painted.open(3, WIDE);
        for (at, text) in ["one", "two", "three"].iter().enumerate() {
            painted.paint(at, &Row::plain(*text), &plain());
        }

        let written = painted.sealed();
        assert!(
            written.contains("one"),
            "the covered row was not lit: {written:?}"
        );
        assert!(
            !written.contains("two"),
            "an untouched row was written: {written:?}"
        );
    }

    #[test]
    fn what_is_read_back_is_what_was_drawn_under_the_drag() {
        let mut painted = Painted::new();
        painted.open(2, WIDE);
        painted.paint(0, &Row::plain("first row"), &worn());
        painted.paint(1, &Row::plain("second row"), &worn());

        painted.selects(Some(dragged((0, 6), (1, 5))));

        assert_eq!(painted.read(), "row\nsecond");
    }

    #[test]
    fn nothing_is_read_back_from_a_screen_nobody_dragged_over() {
        let mut painted = Painted::new();
        painted.open(1, WIDE);
        painted.paint(0, &Row::plain("untouched"), &plain());

        assert_eq!(painted.read(), String::new());
    }
}
