//! Rendering a region the reader is still changing.
//!
//! The counterpart to [`super::screen`], which draws output arriving from
//! somewhere else. Two things differ. The rows are composed here rather than
//! streamed, so they are painted from a palette instead of being wrapped and
//! stripped of escapes. And the cursor does not end up at the end of what was
//! written: it is put back on the row being typed on, which is what makes the
//! rows below it — a closing edge, a status row — part of the region rather
//! than something that would have to be drawn again after every keystroke.

use crate::color::Palette;
use crate::row::Row;

use super::frame::Frame;

/// Where the cursor rests inside a live region.
///
/// Counted from the top left of the rows the region was drawn from rather than
/// from the terminal's own origin, because that origin is a thing an inline
/// renderer never learns: the region starts wherever the last committed row
/// left off, and the component that built the rows knows only which of them the
/// cursor belongs on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Caret {
    /// Which row of the region, from the top.
    pub row: usize,
    /// How many columns from the left of it.
    pub column: usize,
}

/// Paints `rows` into buffers the caller keeps between frames.
///
/// Kept rather than built per frame because a live region is redrawn on every
/// keystroke, and a `String` per row per key is an allocation the render path
/// does not have to make.
pub(crate) fn paint(rows: &[Row], palette: &Palette, into: &mut Vec<String>) {
    into.resize_with(rows.len(), String::new);

    for (row, painted) in rows.iter().zip(into.iter_mut()) {
        painted.clear();
        row.paint_into(palette, painted);
    }
}

/// Builds a frame that puts `painted` back where it stood, with the cursor
/// where `caret` says it goes.
///
/// Returns how many of those rows end up below the cursor, which is what the
/// next rewind has to stop short of.
pub(crate) fn draw(frame: &mut Frame, region: usize, painted: &[String], caret: Caret) -> usize {
    frame.rewind(region);
    frame.live(painted.iter().map(String::as_str));

    let parked = painted.len().saturating_sub(caret.row + 1);
    frame.park(parked, caret.column);
    parked
}

#[cfg(test)]
mod tests {
    use crate::color::Slot;

    use super::*;

    /// A prompt-shaped region: an edge, the row being typed on, an edge, and
    /// the status row under it.
    fn boxed() -> Vec<String> {
        ["╭──╮", "│ a│", "╰──╯", "ask"]
            .iter()
            .map(|row| (*row).to_owned())
            .collect()
    }

    #[test]
    fn the_cursor_comes_back_up_over_the_rows_below_the_one_it_is_on() {
        // Two of the four rows sit under the row being typed on, so the frame
        // ends by moving two rows back up and setting the column outright.
        let mut frame = Frame::new();
        let parked = draw(&mut frame, 0, &boxed(), Caret { row: 1, column: 3 });

        assert_eq!(parked, 2);
        let written = frame.unheld();
        assert!(written.ends_with("\x1b[2A\x1b[4G"), "{written:?}");
    }

    #[test]
    fn a_cursor_on_the_last_row_is_not_moved_up_at_all() {
        // `CUU 0` moves by one in some terminals, so nothing at all is written
        // where there is nothing to move over.
        let mut frame = Frame::new();
        let parked = draw(&mut frame, 0, &boxed(), Caret { row: 3, column: 0 });

        assert_eq!(parked, 0);
        let written = frame.unheld();
        assert!(written.ends_with("ask\x1b[1G"), "{written:?}");
    }

    #[test]
    fn the_next_frame_rewinds_only_as_far_as_the_cursor_came_down() {
        // The region is four rows tall but the cursor was left on the second,
        // so the way back to the top is one row and not three. Rewinding by
        // the height of the region would reach over two rows that are already
        // in scrollback.
        let mut frame = Frame::new();
        draw(&mut frame, 4 - 2, &boxed(), Caret { row: 1, column: 3 });

        let written = frame.unheld();
        assert!(written.starts_with("\r\x1b[1A\x1b[J"), "{written:?}");
    }

    #[test]
    fn painting_reuses_the_buffers_it_was_given() {
        // One allocation per row for the whole session rather than one per row
        // per keystroke.
        let mut into = vec![String::with_capacity(64), String::with_capacity(64)];

        paint(
            &[Row::plain("one"), Row::plain("two")],
            &Palette::plain(),
            &mut into,
        );

        assert_eq!(into, ["one", "two"]);
        assert!(
            into.iter().all(|row| row.capacity() == 64),
            "a row was painted into a buffer that had to be grown for it"
        );
    }

    #[test]
    fn a_region_that_lost_a_row_keeps_no_buffer_for_it() {
        let mut into = Vec::new();
        paint(
            &[Row::plain("one"), Row::plain("two")],
            &Palette::plain(),
            &mut into,
        );
        paint(&[Row::plain("one")], &Palette::plain(), &mut into);

        assert_eq!(into, ["one"]);
    }

    #[test]
    fn a_row_is_painted_from_the_palette_and_not_from_its_own_bytes() {
        // What separates this from a committed line: colour arrives here, at
        // the last moment, rather than being in the text that was handed over.
        let mut into = Vec::new();
        let row = Row::new().then(Slot::Quiet, "ask");

        paint(&[row], &Palette::plain(), &mut into);
        assert_eq!(into, ["ask"]);
    }
}
