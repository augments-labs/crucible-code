//! Rendering into a terminal.
//!
//! The counterpart to [`super::plain`]. Here there is a cursor to move and a
//! region to erase, so the live tail is redrawn in full every frame and only
//! the rows that have left it are permanent. Everything a frame is allowed to
//! do to the screen is in these two functions.

use super::frame::Frame;
use super::region::Caret;
use super::tail::Tail;

/// Builds a frame: back over what was drawn, then the rows that have left the
/// tail for good, then the rows still live, then whatever stands under them.
///
/// Returns how many rows ended up below the cursor, which is what the next
/// rewind has to stop short of.
///
/// Where the cursor comes back to depends on who is expected to type next.
/// With no caret it goes to the end of the tail, because that is where the next
/// delta lands. With one it goes into the rows standing under the tail — those
/// rows are a box somebody is writing in while the answer above them arrives,
/// and a cursor left at the end of the answer would put every keystroke on the
/// wrong row of the screen.
pub(crate) fn draw(
    frame: &mut Frame,
    drawn: usize,
    overflow: &mut Vec<String>,
    tail: &Tail,
    under: (&[String], Option<Caret>),
) -> usize {
    let (under, caret) = under;

    open(frame, drawn, overflow);
    frame.live(tail.rows().chain(under.iter().map(String::as_str)));

    if under.is_empty() {
        return 0;
    }

    // The cursor is on the last row written, so both of these count up from
    // there. A caret past the rows it names is clamped rather than refused:
    // the frame is already written by now, and a cursor one row out is better
    // than a frame that failed.
    let (up, column) = match caret {
        Some(caret) => (
            under.len() - 1 - caret.row.min(under.len() - 1),
            caret.column,
        ),
        None => (under.len(), tail.column()),
    };

    frame.park(up, column);
    up
}

/// Builds the end of a turn, and empties the tail into it.
///
/// Three things differ from a frame that stays on screen: the row the cursor
/// sits on is not content, the cursor steps past everything, so nothing will
/// ever move back over these rows again, and whatever stood under the tail is
/// erased rather than written down. The rewind clears from the top of the
/// region downwards, which reaches it without being told about it — and it is
/// chrome about the session rather than something that was said.
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

        draw(&mut frame, 3, &mut overflow, &tail, (&[], None));

        assert_eq!(frame.as_str(), "\r\x1b[2A\x1b[Jone\r\ntwo\r\nthree");
    }

    #[test]
    fn rows_that_left_the_tail_are_written_above_the_live_ones() {
        // Order matters: a row that overflowed is older than every row still
        // live, and it is being written for the only time.
        let (tail, mut overflow) = streamed(80, 2, "alpha\nbeta\ngamma\ndelta");
        let mut frame = Frame::new();

        draw(&mut frame, 2, &mut overflow, &tail, (&[], None));

        assert_eq!(
            frame.as_str(),
            "\r\x1b[1A\x1b[Jalpha\r\nbeta\r\ngamma\r\ndelta"
        );
        assert!(overflow.is_empty(), "overflow was not drained");
    }

    #[test]
    fn the_cursor_comes_back_to_the_end_of_the_tail_over_what_stands_under_it() {
        // The row below is drawn and stays drawn, so the next rewind has to stop
        // one row short of it — and the next delta has to land where the answer
        // stopped rather than on the row standing underneath.
        let (tail, mut overflow) = streamed(80, 24, "the answer");
        let mut frame = Frame::new();
        let under = ["ask mode on".to_owned()];

        let parked = draw(&mut frame, 1, &mut overflow, &tail, (&under, None));

        assert_eq!(parked, 1);
        assert_eq!(
            frame.as_str(),
            "\r\x1b[Jthe answer\r\nask mode on\x1b[1A\x1b[11G"
        );
    }

    #[test]
    fn a_caret_brings_the_cursor_into_the_rows_under_the_tail() {
        // A box being typed into while the answer above it arrives. Left at
        // the end of the answer, the cursor would put every keystroke on the
        // wrong row of the screen.
        let (tail, mut overflow) = streamed(80, 24, "the answer");
        let mut frame = Frame::new();
        let under = [
            "\u{256d}\u{2500}\u{256e}".to_owned(),
            "\u{2502} \u{203a} hi".to_owned(),
            "\u{2570}\u{2500}\u{256f}".to_owned(),
            "ask mode on".to_owned(),
        ];

        let parked = draw(
            &mut frame,
            1,
            &mut overflow,
            &tail,
            (&under, Some(Caret { row: 1, column: 9 })),
        );

        // Two rows below the cursor, so the next rewind stops short of them.
        assert_eq!(parked, 2);
        assert!(
            frame.as_str().ends_with("\x1b[2A\x1b[10G"),
            "{:?}",
            frame.as_str()
        );
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
