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

use crate::color::Palette;
use crate::row::Row;

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

    /// Starts a frame for a window `rows` tall.
    pub(crate) fn open(&mut self, rows: usize) {
        self.was.resize(rows, None);
        self.changed = false;
        self.frame.open();
    }

    /// Forgets what is on screen, so the next frame writes every row of it.
    pub(crate) fn forget(&mut self) {
        self.was.clear();
        self.parked = None;
    }

    /// Draws `row` on screen row `at`, in `palette`, if it is not there already.
    pub(crate) fn paint(&mut self, at: usize, row: &Row, palette: &Palette) {
        self.row.clear();
        row.paint_into(palette, &mut self.row);
        Self::set(
            &mut self.was,
            &mut self.frame,
            &mut self.changed,
            at,
            &self.row,
        );
    }

    /// Draws bytes something else painted on screen row `at`.
    pub(crate) fn put(&mut self, at: usize, painted: &str) {
        Self::set(
            &mut self.was,
            &mut self.frame,
            &mut self.changed,
            at,
            painted,
        );
    }

    /// Leaves screen row `at` empty.
    pub(crate) fn blank(&mut self, at: usize) {
        Self::set(&mut self.was, &mut self.frame, &mut self.changed, at, "");
    }

    /// Whichever of the two above ends up writing a row.
    ///
    /// Not a method, so that a caller may hand it bytes borrowed from a field
    /// beside the ones it changes.
    fn set(
        was: &mut [Option<String>],
        frame: &mut Frame,
        changed: &mut bool,
        at: usize,
        painted: &str,
    ) {
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
        painted.open(4);
        painted.paint(0, &Row::plain("hello"), &plain());
        assert!(painted.moved());

        painted.open(4);
        painted.paint(0, &Row::plain("hello"), &plain());
        assert!(!painted.moved(), "a row nothing changed was written again");
    }

    #[test]
    fn a_row_whose_words_changed_is_written_and_the_others_are_not() {
        let mut painted = Painted::new();
        painted.open(3);
        for (at, text) in ["one", "two", "three"].iter().enumerate() {
            painted.paint(at, &Row::plain(*text), &plain());
        }
        painted.sealed();

        painted.open(3);
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
        painted.open(1);
        painted.paint(0, &Row::plain("same"), &plain());
        painted.sealed();

        let mut row = Row::new();
        row.push(Slot::Accent, "same");
        painted.open(1);
        painted.paint(0, &row, &worn());

        assert!(painted.moved(), "a recoloured row was left as it was");
    }

    #[test]
    fn forgetting_writes_every_row_again() {
        let mut painted = Painted::new();
        painted.open(2);
        painted.paint(0, &Row::plain("kept"), &plain());
        painted.sealed();

        painted.forget();
        painted.open(2);
        painted.paint(0, &Row::plain("kept"), &plain());

        assert!(painted.moved(), "the screen was assumed to be unchanged");
    }

    #[test]
    fn a_frame_that_changed_nothing_says_so() {
        let mut painted = Painted::new();
        painted.open(2);
        painted.paint(0, &Row::plain("still"), &plain());
        painted.park(1, 0);
        painted.sealed();

        painted.open(2);
        painted.paint(0, &Row::plain("still"), &plain());
        painted.park(1, 0);

        assert!(!painted.moved());
    }

    #[test]
    fn a_cursor_that_moved_is_a_frame_worth_writing() {
        // Nothing on screen changed and the reader still has to see the caret
        // where they put it.
        let mut painted = Painted::new();
        painted.open(2);
        painted.park(1, 0);
        painted.sealed();

        painted.open(2);
        painted.park(1, 4);

        assert!(painted.moved());
    }

    #[test]
    fn a_row_past_the_foot_of_the_window_is_dropped_rather_than_drawn() {
        let mut painted = Painted::new();
        painted.open(2);
        painted.paint(5, &Row::plain("beyond"), &plain());

        assert!(!painted.moved());
    }

    #[test]
    fn a_window_that_grew_has_rows_nothing_is_known_about() {
        let mut painted = Painted::new();
        painted.open(1);
        painted.paint(0, &Row::plain("top"), &plain());
        painted.sealed();

        painted.open(3);
        painted.blank(2);

        assert!(painted.moved(), "a row never drawn on was assumed empty");
    }
}
