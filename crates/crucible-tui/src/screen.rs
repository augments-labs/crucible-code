//! Rendering into a terminal.
//!
//! The counterpart to [`crate::plain`]. Here there is a cursor to move and a
//! region to erase, so the live tail is redrawn in full every frame and only
//! the rows that have left it are permanent. Everything a frame is allowed to
//! do to the screen is in these two functions.

use crate::frame::Frame;
use crate::tail::Tail;

/// Builds a frame: back over what was drawn, then the rows that have left the
/// tail for good, then the rows still live.
pub(crate) fn draw(frame: &mut Frame, drawn: usize, overflow: &mut Vec<String>, tail: &Tail) {
    open(frame, drawn, overflow);
    frame.live(tail.rows());
}

/// Builds the end of a turn, and empties the tail into it.
///
/// Two things differ from a frame that stays on screen: the row the cursor sits
/// on is not content, and the cursor steps past everything, so nothing will
/// ever move back over these rows again.
pub(crate) fn settle(frame: &mut Frame, drawn: usize, overflow: &mut Vec<String>, tail: &mut Tail) {
    open(frame, drawn, overflow);
    frame.live(tail.content());
    frame.break_row();
    tail.clear();
}

/// The part both share: reclaim the live region, then write off what left it.
fn open(frame: &mut Frame, drawn: usize, overflow: &mut Vec<String>) {
    frame.rewind(drawn);

    for row in overflow.drain(..) {
        frame.settled(&row, true);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn streamed(width: usize, bound: usize, text: &str) -> (Tail, Vec<String>) {
        let mut tail = Tail::new(width, bound);
        let mut overflow = Vec::new();
        tail.push(text, &mut overflow);
        (tail, overflow)
    }

    #[test]
    fn a_frame_moves_back_over_every_row_it_drew() {
        // One fewer move than there are rows, because the cursor is already on
        // the last of them. Getting this wrong erases a committed line.
        let (tail, mut overflow) = streamed(80, 24, "one\ntwo\nthree");
        let mut frame = Frame::new();

        draw(&mut frame, 3, &mut overflow, &tail);

        assert_eq!(frame.as_str(), "\r\x1b[2A\x1b[Jone\r\ntwo\r\nthree");
    }

    #[test]
    fn rows_that_left_the_tail_are_written_above_the_live_ones() {
        // Order matters: a row that overflowed is older than every row still
        // live, and it is being written for the only time.
        let (tail, mut overflow) = streamed(80, 2, "alpha\nbeta\ngamma\ndelta");
        let mut frame = Frame::new();

        draw(&mut frame, 2, &mut overflow, &tail);

        assert_eq!(
            frame.as_str(),
            "\r\x1b[1A\x1b[Jalpha\r\nbeta\r\ngamma\r\ndelta"
        );
        assert!(overflow.is_empty(), "overflow was not drained");
    }

    #[test]
    fn settling_steps_past_the_tail() {
        // Without the last line ending the next frame would rewind into the
        // answer that was just finished.
        let (mut tail, mut overflow) = streamed(80, 24, "answer");
        let mut frame = Frame::new();

        settle(&mut frame, 1, &mut overflow, &mut tail);

        assert_eq!(frame.as_str(), "\r\x1b[Janswer\r\n");
    }

    #[test]
    fn settling_does_not_leave_a_blank_line_behind() {
        // An answer ending in a newline leaves the tail one empty row, and that
        // row is the cursor's, not output.
        let (mut tail, mut overflow) = streamed(80, 24, "answer\n");
        let mut frame = Frame::new();

        settle(&mut frame, 2, &mut overflow, &mut tail);

        assert_eq!(frame.as_str(), "\r\x1b[1A\x1b[Janswer\r\n");
    }

    #[test]
    fn settling_empties_the_tail() {
        let (mut tail, mut overflow) = streamed(80, 24, "answer");
        let mut frame = Frame::new();

        settle(&mut frame, 1, &mut overflow, &mut tail);

        assert!(tail.is_empty(), "the tail survived a settle");
    }
}
