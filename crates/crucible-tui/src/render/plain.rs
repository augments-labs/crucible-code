//! Rendering into something that is not a terminal.
//!
//! A pipe has no cursor to move and no screen to erase, so the live tail can
//! never be redrawn: every byte written is final. That makes this a different
//! contract from the terminal path rather than a stripped-down version of it. A
//! row is written once, at the moment it can no longer change, and the rows
//! still live when a turn ends are written then instead of being lost.

use super::frame::Frame;
use super::tail::Tail;

/// Builds a frame from the rows that have left the tail for good.
///
/// Returns whether there is anything to write: mid-turn, a delta that pushed
/// nothing out of the tail produces no output at all.
pub(crate) fn draw(frame: &mut Frame, overflow: &mut Vec<String>) -> bool {
    if overflow.is_empty() {
        return false;
    }

    frame.plain();
    for row in overflow.drain(..) {
        frame.settled(&row, false);
    }

    true
}

/// Builds the end of a turn: what overflowed, then what is still live.
///
/// Without the second part, `crucible > out.txt` would be missing however much
/// of the answer had not yet scrolled out of the tail.
pub(crate) fn settle(frame: &mut Frame, overflow: &mut Vec<String>, tail: &mut Tail) -> bool {
    frame.plain();

    for row in overflow.drain(..) {
        frame.settled(&row, false);
    }

    for row in tail.content() {
        frame.settled(row, false);
    }

    tail.clear();
    !frame.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overflowing(width: usize, bound: usize, text: &str) -> (Tail, Vec<String>) {
        let mut tail = Tail::new(width, bound);
        let mut overflow = Vec::new();
        tail.push(text, &mut overflow);
        (tail, overflow)
    }

    #[test]
    fn a_pipe_never_receives_an_escape_byte() {
        // Cursor movement written to a pipe ends up in whatever consumed the
        // output — a file, a diff, another program's stdin.
        let (mut tail, mut overflow) = overflowing(80, 2, "alpha\nbeta\ngamma\ndelta");
        let mut frame = Frame::new();

        settle(&mut frame, &mut overflow, &mut tail);

        assert!(
            !frame.as_str().contains('\x1b'),
            "a pipe received escape bytes: {:?}",
            frame.as_str()
        );
    }

    #[test]
    fn nothing_is_written_until_a_row_is_final() {
        // A row still being appended to would be written again next frame, and
        // a pipe cannot take anything back.
        let (_, mut overflow) = overflowing(80, 5, "still writing");
        let mut frame = Frame::new();

        assert!(!draw(&mut frame, &mut overflow));
    }

    #[test]
    fn a_row_that_left_the_tail_is_written_once() {
        let (_, mut overflow) = overflowing(80, 2, "alpha\nbeta\ngamma\ndelta");
        let mut frame = Frame::new();

        assert!(draw(&mut frame, &mut overflow));
        assert_eq!(frame.as_str(), "alpha\nbeta\n");
        assert!(overflow.is_empty(), "overflow was not drained");
    }

    #[test]
    fn settling_writes_the_rows_that_never_overflowed() {
        let (mut tail, mut overflow) = overflowing(80, 2, "alpha\nbeta\ngamma\ndelta");
        let mut frame = Frame::new();

        assert!(settle(&mut frame, &mut overflow, &mut tail));
        assert_eq!(frame.as_str(), "alpha\nbeta\ngamma\ndelta\n");
    }

    #[test]
    fn settling_an_empty_turn_writes_nothing() {
        let mut tail = Tail::new(80, 2);
        let mut overflow = Vec::new();
        let mut frame = Frame::new();

        assert!(!settle(&mut frame, &mut overflow, &mut tail));
    }
}
