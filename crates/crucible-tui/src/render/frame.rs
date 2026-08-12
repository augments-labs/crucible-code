//! What a frame is made of.
//!
//! One frame is one string and one write. Building it here keeps the escape
//! sequences in a single place and keeps [`crate::render`] about *when* to draw
//! rather than what to emit -- and it means the buffer is reused across frames,
//! so a redraw allocates nothing.
//!
//! Only three sequences are ever written, and the choice of the third is the
//! whole inline design: erasing from the cursor *down* cannot touch a line that
//! has scrolled off, so committed output is unreachable by construction.

use std::fmt::Write as _;

/// Return to column one. Written explicitly because raw mode stops a newline
/// from doing it, and the renderer must behave the same either way.
const COLUMN_ONE: &str = "\r";

/// Erase from the cursor to the end of the screen.
const ERASE_DOWN: &str = "\x1b[J";

/// End of a row, on a terminal that may be in raw mode.
const NEW_ROW: &str = "\r\n";

/// The bytes of one frame, assembled into a buffer that outlives it.
#[derive(Debug, Default)]
pub(crate) struct Frame {
    buffer: String,
}

impl Frame {
    /// An empty frame.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Starts a frame by putting the cursor back at the top of the live region
    /// and clearing from there down.
    ///
    /// `drawn` is how many rows the last frame left on screen.
    pub(crate) fn rewind(&mut self, drawn: usize) -> &mut Self {
        self.buffer.clear();
        self.buffer.push_str(COLUMN_ONE);

        // The cursor sits at the end of the last row drawn, so getting back to
        // the first is one fewer move than there are rows. `CUU 0` moves by one
        // in some terminals, so zero has to mean writing nothing at all.
        if let Some(up) = drawn.checked_sub(1).filter(|up| *up > 0) {
            // Into the buffer that is already allocated. Writing to a `String`
            // cannot fail; the `Result` is there for writers that can.
            let _ = write!(self.buffer, "\x1b[{up}A");
        }

        self.buffer.push_str(ERASE_DOWN);
        self
    }

    /// Starts a frame with no cursor movement, for output going to a pipe.
    ///
    /// Escape bytes written to a pipe end up in whatever consumed the output,
    /// so the redirected path emits none at all.
    pub(crate) fn plain(&mut self) -> &mut Self {
        self.buffer.clear();
        self
    }

    /// Adds a row that is finished, followed by the line ending that makes the
    /// cursor step past it. Nothing ever moves back above this point, which is
    /// what makes the row permanent.
    pub(crate) fn settled(&mut self, row: &str, terminal: bool) -> &mut Self {
        self.buffer.push_str(row);
        self.buffer.push_str(if terminal { NEW_ROW } else { "\n" });
        self
    }

    /// Adds the rows that are still live, in order. These are redrawn by the
    /// next frame, so they end without a line ending: the cursor stays where
    /// the next rewind expects it.
    pub(crate) fn live<'a>(&mut self, rows: impl Iterator<Item = &'a str>) -> &mut Self {
        for (index, row) in rows.enumerate() {
            if index > 0 {
                self.buffer.push_str(NEW_ROW);
            }
            self.buffer.push_str(row);
        }
        self
    }

    /// Puts the cursor `up` rows above the last row written, at `column`.
    ///
    /// For a live region the reader is typing into: the rows below the one
    /// being typed on are still drawn, and the cursor has to come back up over
    /// them. The column is set absolutely rather than stepped to, because what
    /// the caller knows is where the cursor belongs and not where it is.
    ///
    /// Only ever within the region this frame just wrote, so it cannot reach a
    /// row that has already gone to scrollback.
    pub(crate) fn park(&mut self, up: usize, column: usize) -> &mut Self {
        if up > 0 {
            let _ = write!(self.buffer, "\x1b[{up}A");
        }

        // One-based, as every column in the terminal's own counting is.
        let _ = write!(self.buffer, "\x1b[{}G", column + 1);
        self
    }

    /// Ends the live region, leaving what it held above the cursor.
    pub(crate) fn break_row(&mut self) -> &mut Self {
        self.buffer.push_str(NEW_ROW);
        self
    }

    /// The assembled bytes.
    pub(crate) fn as_str(&self) -> &str {
        &self.buffer
    }

    /// Whether there is anything to write.
    pub(crate) fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_frame_does_not_move_the_cursor_up() {
        // There is nothing above it yet, and moving up would eat a line the
        // shell printed.
        let mut frame = Frame::new();
        assert_eq!(frame.rewind(0).as_str(), "\r\x1b[J");
    }

    #[test]
    fn one_drawn_row_needs_no_move_either() {
        // The cursor is already on the only row there is.
        let mut frame = Frame::new();
        assert_eq!(frame.rewind(1).as_str(), "\r\x1b[J");
    }

    #[test]
    fn moving_back_is_one_fewer_than_the_rows_drawn() {
        let mut frame = Frame::new();
        assert_eq!(frame.rewind(4).as_str(), "\r\x1b[3A\x1b[J");
    }

    #[test]
    fn a_frame_never_emits_a_sequence_that_reaches_scrollback() {
        // The property the inline design rests on. If one of these can appear,
        // committed output is reachable and the design is gone.
        let mut frame = Frame::new();
        frame
            .rewind(9)
            .settled("done", true)
            .live(["one", "two"].into_iter())
            .break_row();

        for upward in ["\x1b[2J", "\x1b[1J", "\x1b[3J", "\x1b[H", "\x1b[0J"] {
            assert!(
                !frame.as_str().contains(upward),
                "a frame contained {upward:?}: {:?}",
                frame.as_str()
            );
        }
    }

    #[test]
    fn a_frame_never_paints_a_ground_of_its_own() {
        // The ground behind every row belongs to the terminal, and it stays
        // that way by nothing here ever setting one -- not by this process
        // working out what theirs is. A fill added to make a row line up would
        // be the edit this test is here to stop.
        let mut frame = Frame::new();
        frame
            .rewind(3)
            .settled("done", true)
            .live(["one", "two"].into_iter())
            .break_row();

        for painted in ["\x1b[48;", "\x1b[40m", "\x1b[47m", "\x1b[107m"] {
            assert!(
                !frame.as_str().contains(painted),
                "a frame contained {painted:?}: {:?}",
                frame.as_str()
            );
        }
    }

    #[test]
    fn live_rows_end_without_a_line_ending() {
        // A trailing newline would leave the cursor one row below the tail, and
        // the next rewind would erase the wrong lines.
        let mut frame = Frame::new();
        let written = frame.plain().live(["one", "two"].into_iter()).as_str();

        assert_eq!(written, "one\r\ntwo");
    }

    #[test]
    fn a_settled_row_ends_with_a_carriage_return_on_a_terminal() {
        // Raw mode does not return the carriage on a bare newline, so a row
        // written without one would stair-step across the screen.
        let mut frame = Frame::new();
        assert_eq!(frame.plain().settled("done", true).as_str(), "done\r\n");
    }

    #[test]
    fn a_settled_row_is_a_plain_newline_for_a_pipe() {
        let mut frame = Frame::new();
        assert_eq!(frame.plain().settled("done", false).as_str(), "done\n");
    }

    #[test]
    fn starting_a_frame_forgets_the_last_one() {
        let mut frame = Frame::new();
        frame.plain().settled("old", true);
        frame.rewind(0);

        assert_eq!(frame.as_str(), "\r\x1b[J");
    }

    #[test]
    fn a_plain_frame_carries_no_escape_bytes() {
        let mut frame = Frame::new();
        frame.plain().settled("alpha", false).settled("beta", false);

        assert!(!frame.as_str().contains('\x1b'));
        assert_eq!(frame.as_str(), "alpha\nbeta\n");
    }
}
