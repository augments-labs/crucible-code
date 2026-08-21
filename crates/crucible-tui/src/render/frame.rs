//! What a frame is made of.
//!
//! One frame is one string and one write. Building it here keeps the escape
//! sequences in a single place and keeps [`crate::render`] about *when* to draw
//! rather than what to emit -- and it means the buffer is reused across frames,
//! so a redraw allocates nothing.
//!
//! Every row this frame writes says where it goes. That is the whole design: on
//! a screen this process owns, the position of each of its rows is known before
//! a byte is written, so nothing here counts how far to move or how much to
//! erase. A frame cannot wipe a row it did not name, and a frame that drew the
//! wrong number of rows cannot leave the next one erasing somebody else's.
//!
//! A frame also says where it begins and ends. A terminal is free to paint
//! whenever bytes arrive, so between the row a frame clears and the row it
//! writes in its place there is a picture with a hole in it -- and at a delta a
//! second, that hole is what the reader sees instead of the text. The frame is
//! bracketed as a synchronized update, which asks the terminal to hold what it
//! has until the closing sequence arrives and then swap the two pictures at
//! once. A terminal that does not know the mode ignores both sequences and is
//! exactly where it was; one that does applies a timeout of its own, so a
//! process dying mid-frame cannot leave a screen frozen.

use std::fmt::Write as _;

/// Erase from the cursor to the end of the row.
const ERASE_ROW: &str = "\x1b[K";

/// Forget any colour still set.
///
/// Written before the erase and not after it: erasing with a background still
/// in force paints that background across the rest of the row on every terminal
/// that honours it, so a row whose colour ran to the edge would lend it to the
/// row replacing it.
const PLAIN: &str = "\x1b[m";

/// Hold the screen: what follows is one picture, not a sequence of them.
const BEGIN_SYNC: &str = "\x1b[?2026h";

/// Show it. Paired with [`BEGIN_SYNC`] by [`Frame::sealed`], which is the only
/// way bytes leave this type.
const END_SYNC: &str = "\x1b[?2026l";

/// Take the cursor off the screen while the frame is assembled.
///
/// Not decoration. A terminal that does not know [`BEGIN_SYNC`] paints as the
/// bytes arrive, and a cursor stepping through every row of a redraw is visible
/// on exactly those terminals.
const HIDE: &str = "\x1b[?25l";

/// Put it back. Paired with [`HIDE`] by [`Frame::sealed`], for the same reason
/// the sequences above it are.
const SHOW: &str = "\x1b[?25h";

/// The bytes of one frame, assembled into a buffer that outlives it.
#[derive(Debug, Default)]
pub(crate) struct Frame {
    buffer: String,
    /// Whether this frame took the screen, and so owes the sequences that give
    /// it back.
    held: bool,
}

impl Frame {
    /// Starts a frame.
    ///
    /// Nothing is rewound and nothing is erased ahead of time, because nothing
    /// here is relative: the rows this frame writes name where they go, and the
    /// rows it does not write are the rows it means to leave alone.
    pub(crate) fn open(&mut self) -> &mut Self {
        self.buffer.clear();
        self.held = true;
        self.buffer.push_str(BEGIN_SYNC);
        self.buffer.push_str(HIDE);
        self
    }

    /// Draws `painted` on screen row `row`, replacing whatever was there.
    pub(crate) fn row(&mut self, row: usize, painted: &str) -> &mut Self {
        self.place(row, 0);
        self.buffer.push_str(PLAIN);
        self.buffer.push_str(ERASE_ROW);
        self.buffer.push_str(painted);
        self
    }

    /// Leaves the cursor where the reader is typing.
    ///
    /// Absolute, and the last thing a frame writes, so it does not matter in
    /// what order the rows above it were drawn — which is what lets a frame
    /// write only the rows whose picture changed.
    pub(crate) fn park(&mut self, row: usize, column: usize) -> &mut Self {
        self.place(row, column);
        self
    }

    /// Both counted from one, as the terminal counts them.
    fn place(&mut self, row: usize, column: usize) {
        // Into the buffer that is already allocated. Writing to a `String`
        // cannot fail; the `Result` is there for writers that can.
        let _ = write!(self.buffer, "\x1b[{};{}H", row + 1, column + 1);
    }

    /// The assembled bytes, closed.
    ///
    /// The one way out of this type, so a frame that hid the cursor and opened a
    /// synchronized update cannot be written without the sequences that undo
    /// both — and what a test reads here is what the terminal receives. Sealing
    /// twice seals once: the second call is the same bytes rather than a stray
    /// closing sequence.
    pub(crate) fn sealed(&mut self) -> &str {
        if self.held {
            self.held = false;
            self.buffer.push_str(SHOW);
            self.buffer.push_str(END_SYNC);
        }
        &self.buffer
    }

    /// What the frame put between the sequences that hold the screen.
    ///
    /// For the tests about placement, which is the thing underneath them: those
    /// sequences are the same on every frame, and the pairing is tested here
    /// rather than restated in each of them.
    #[cfg(test)]
    pub(crate) fn unheld(&mut self) -> &str {
        let sealed = self.sealed();

        sealed
            .strip_prefix(BEGIN_SYNC)
            .and_then(|inside| inside.strip_prefix(HIDE))
            .and_then(|inside| inside.strip_suffix(END_SYNC))
            .and_then(|inside| inside.strip_suffix(SHOW))
            .unwrap_or(sealed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_names_where_it_goes() {
        // The whole of what replaced the rewind. Both counted from one, as the
        // terminal counts them, so the first row of the screen is row zero here
        // and row one on the wire.
        let mut frame = Frame::default();

        assert_eq!(
            frame.open().row(0, "head").unheld(),
            "\x1b[1;1H\x1b[m\x1b[Khead"
        );
    }

    #[test]
    fn a_row_far_down_the_screen_is_placed_rather_than_stepped_to() {
        let mut frame = Frame::default();

        assert_eq!(
            frame.open().row(23, "foot").unheld(),
            "\x1b[24;1H\x1b[m\x1b[Kfoot"
        );
    }

    #[test]
    fn a_row_is_cleared_before_it_is_written() {
        // A shorter row drawn over a longer one would otherwise keep the tail
        // of the one it replaced.
        let mut frame = Frame::default();
        let written = frame.open().row(4, "short").unheld();

        let erased = written.find(ERASE_ROW).expect("the row is erased");
        let wrote = written.find("short").expect("the row is written");
        assert!(erased < wrote, "{written:?}");
    }

    #[test]
    fn a_row_forgets_the_colour_above_it_before_it_erases_and_not_after() {
        // Erasing with a background still set paints that background across the
        // rest of the row, so the reset has to come first. The order is the
        // whole of the test: both sequences are present either way.
        let mut frame = Frame::default();
        let written = frame.open().row(1, "text").unheld();

        let forgot = written.find(PLAIN).expect("the colour is forgotten");
        let erased = written.find(ERASE_ROW).expect("the row is erased");
        assert!(forgot < erased, "{written:?}");
    }

    #[test]
    fn the_caret_is_parked_after_every_row_so_their_order_does_not_matter() {
        // What lets a frame write only the rows that changed: wherever the last
        // of them left the cursor, the park puts it where the reader is typing.
        let mut frame = Frame::default();

        assert_eq!(
            frame.open().row(9, "prompt").park(9, 4).unheld(),
            "\x1b[10;1H\x1b[m\x1b[Kprompt\x1b[10;5H"
        );
    }

    #[test]
    fn a_frame_takes_the_cursor_off_the_screen_and_gives_it_back() {
        // On a terminal that does not know the synchronized update, a cursor
        // stepping through every row of a redraw is the thing the reader sees.
        let mut frame = Frame::default();
        let written = frame.open().row(0, "one").row(1, "two").sealed();

        assert!(written.starts_with("\x1b[?2026h\x1b[?25l"), "{written:?}");
        assert!(written.ends_with("\x1b[?25h\x1b[?2026l"), "{written:?}");
    }

    #[test]
    fn sealing_a_frame_twice_closes_it_once() {
        // Every write goes through the seal, and a renderer that wrote a frame
        // and then looked at it again would otherwise send a closing sequence
        // for an update nothing had opened.
        let mut frame = Frame::default();
        frame.open();

        let once = frame.sealed().to_owned();
        assert_eq!(frame.sealed(), once);
    }

    #[test]
    fn a_frame_never_paints_a_ground_of_its_own() {
        // The ground behind every row belongs to the theme the rows were
        // painted with, and it stays that way by nothing here ever setting one.
        // A fill added to make a row line up would be the edit this test is
        // here to stop.
        let mut frame = Frame::default();
        frame.open().row(0, "one").row(1, "two").park(1, 0);

        let written = frame.sealed();
        for painted in ["\x1b[48;", "\x1b[40m", "\x1b[47m", "\x1b[107m"] {
            assert!(
                !written.contains(painted),
                "a frame contained {painted:?}: {written:?}"
            );
        }
    }

    #[test]
    fn a_frame_erases_by_the_row_and_never_by_the_screen() {
        // What keeps a redraw proportional to what changed. An erase of the
        // whole screen would be correct and would also repaint every row of the
        // window on every delta, which is the budget gone.
        let mut frame = Frame::default();
        frame.open().row(3, "one").row(4, "two").park(4, 2);

        let written = frame.sealed();
        for wholesale in ["\x1b[2J", "\x1b[J", "\x1b[0J", "\x1b[1J", "\x1b[3J"] {
            assert!(
                !written.contains(wholesale),
                "a frame contained {wholesale:?}: {written:?}"
            );
        }
    }

    #[test]
    fn starting_a_frame_forgets_the_last_one() {
        let mut frame = Frame::default();
        frame.open().row(0, "old");
        frame.sealed();
        frame.open();

        assert_eq!(frame.sealed(), "\x1b[?2026h\x1b[?25l\x1b[?25h\x1b[?2026l");
    }
}
