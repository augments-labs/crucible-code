//! Inline rendering: draw the live tail, commit everything above it.
//!
//! There is no alternate screen. Output goes into the terminal's own scrollback,
//! so the scroll buffer, the find bar and the scrollbar are the terminal's --
//! already written, already fast, already familiar. What this process keeps is
//! only the tail it is still changing.
//!
//! The redraw is the whole design in three sequences: return to the start of the
//! tail, erase from the cursor *down*, and write the tail again. Erasing
//! downward cannot reach scrollback, so committed output is unreachable by
//! construction rather than by care. The sequences themselves live in
//! [`crate::frame`]; this module decides when a frame happens.

use crate::frame::Frame;
use crate::tail::Tail;
use crate::terminal::{Size, Terminal, TerminalError};
use crate::{plain, screen};

/// Draws the transcript into the terminal's scrollback.
#[derive(Debug)]
pub struct Renderer<T: Terminal> {
    terminal: T,
    tail: Tail,
    /// Wraps committed lines by the same rules as streamed output without
    /// duplicating the wrap. Its bound is one, so every row it is given
    /// overflows immediately instead of staying live.
    finished: Tail,
    /// Rows of tail currently on screen. What the next frame must move back
    /// over, and the only piece of screen state this process tracks.
    drawn: usize,
    /// Columns the tail is currently wrapped for.
    columns: usize,
    /// Reused across frames: a frame is one string, so it is one write.
    frame: Frame,
    /// Reused across frames: rows leaving the tail on their way to scrollback.
    overflow: Vec<String>,
}

impl<T: Terminal> Renderer<T> {
    /// A renderer drawing on `terminal`.
    ///
    /// The tail is bounded by the height of the visible area: holding more than
    /// one screen would keep rows nobody can see.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be prepared.
    pub fn new(terminal: T) -> Result<Self, TerminalError> {
        // A terminal that will not report a size is still a terminal worth
        // drawing on, so a guess beats refusing to start.
        let size = terminal.size().unwrap_or(Size::FALLBACK);

        Ok(Self {
            tail: Tail::new(size.columns, size.rows),
            finished: Tail::new(size.columns, 1),
            terminal,
            drawn: 0,
            columns: size.columns,
            frame: Frame::new(),
            overflow: Vec::new(),
        })
    }

    /// Appends streamed output and puts a frame on screen.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn stream(&mut self, delta: &str) -> Result<(), TerminalError> {
        self.tail.push(delta, &mut self.overflow);
        self.draw()
    }

    /// Writes a line that is finished and will never be redrawn.
    ///
    /// Used for a completed message, a tool result, or anything else that
    /// belongs to the record rather than to the live region.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn commit(&mut self, line: &str) -> Result<(), TerminalError> {
        self.finished.push(line, &mut self.overflow);
        // The newline is what makes the last row complete, and only complete
        // rows leave a tail.
        self.finished.push("\n", &mut self.overflow);
        self.draw()
    }

    /// Ends the live region, leaving what it held in scrollback.
    ///
    /// Called between turns. After this the cursor sits on a fresh row and the
    /// next frame starts a new tail.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn settle(&mut self) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return self.settle_plain();
        }

        if self.tail.is_empty() {
            // Nothing live is worth keeping, but the region may still be on
            // screen from a frame that only committed rows.
            if self.drawn > 0 {
                self.erase()?;
                self.terminal.flush()?;
                self.drawn = 0;
            }
            return Ok(());
        }

        screen::settle(
            &mut self.frame,
            self.drawn,
            &mut self.overflow,
            &mut self.tail,
        );

        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()?;

        self.drawn = 0;
        Ok(())
    }

    /// Re-wraps for a terminal the user resized.
    ///
    /// The rows already on screen were wrapped for the old width, so what is
    /// live is dropped rather than redrawn wrongly: it is at most one screen,
    /// and the model is still streaming into it.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal could not be written to.
    pub fn resized(&mut self) -> Result<(), TerminalError> {
        let size = self.terminal.size().unwrap_or(Size::FALLBACK);
        if size.columns == self.columns {
            return Ok(());
        }

        self.erase()?;
        self.terminal.flush()?;

        self.columns = size.columns;
        self.tail = Tail::new(size.columns, size.rows);
        self.finished = Tail::new(size.columns, 1);
        self.drawn = 0;
        Ok(())
    }

    /// The terminal underneath, for the wiring that owns it.
    pub fn terminal(&mut self) -> &mut T {
        &mut self.terminal
    }

    /// One frame, assembled by [`screen`].
    fn draw(&mut self) -> Result<(), TerminalError> {
        if !self.terminal.is_terminal() {
            return self.draw_plain();
        }

        screen::draw(&mut self.frame, self.drawn, &mut self.overflow, &self.tail);

        self.drawn = self.tail.len();
        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()
    }

    /// A frame for a pipe, assembled by [`plain`].
    fn draw_plain(&mut self) -> Result<(), TerminalError> {
        if !plain::draw(&mut self.frame, &mut self.overflow) {
            return Ok(());
        }

        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()
    }

    /// Ending a turn into a pipe, assembled by [`plain`].
    fn settle_plain(&mut self) -> Result<(), TerminalError> {
        if !plain::settle(&mut self.frame, &mut self.overflow, &mut self.tail) {
            return Ok(());
        }

        self.terminal.write(self.frame.as_str())?;
        self.terminal.flush()
    }

    /// Wipes the live region without drawing anything back.
    fn erase(&mut self) -> Result<(), TerminalError> {
        self.frame.rewind(self.drawn);
        self.terminal.write(self.frame.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::Recording;

    /// The escape a frame starts with when `rows` rows are already on screen.
    /// Written out literally rather than borrowed from `frame`, so a change
    /// there has to be asserted here too.
    fn rewind(rows: usize) -> String {
        match rows {
            0 | 1 => "\r\x1b[J".to_owned(),
            n => format!("\r\x1b[{}A\x1b[J", n - 1),
        }
    }

    #[test]
    fn the_first_frame_does_not_move_the_cursor_up() {
        // There is nothing above it yet, and moving up would eat a line the
        // shell printed.
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.stream("hello").unwrap();

        assert_eq!(render.terminal().written(), format!("{}hello", rewind(0)));
    }

    #[test]
    fn a_second_frame_erases_the_first() {
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.stream("hel").unwrap();
        render.terminal().take();

        render.stream("lo").unwrap();

        assert_eq!(render.terminal().written(), format!("{}hello", rewind(1)));
    }

    #[test]
    fn a_frame_is_one_write_and_one_flush() {
        // The burst budget is about frames, not bytes: a redraw that wrote row
        // by row would tear on a slow terminal and cost a syscall each.
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.stream("one\ntwo\nthree").unwrap();

        assert_eq!(render.terminal().flushes(), 1);
    }

    #[test]
    fn nothing_ever_erases_upward() {
        // The property that makes scrollback safe. If one of these appears,
        // committed output is reachable and the design is gone.
        let mut render = Renderer::new(Recording::new(20, 3)).unwrap();
        for turn in 0..200 {
            render.stream(&format!("line {turn}\n")).unwrap();
        }

        let written = render.terminal().written();
        for upward in ["\x1b[2J", "\x1b[1J", "\x1b[3J", "\x1b[H"] {
            assert!(
                !written.contains(upward),
                "the renderer wrote {upward:?}, which can reach scrollback"
            );
        }
    }

    #[test]
    fn rows_pushed_out_of_the_tail_are_written_once() {
        // Committed rows must appear exactly once in the byte stream. Twice
        // means the tail redrew something it had already let go of.
        let mut render = Renderer::new(Recording::new(80, 2)).unwrap();
        render.stream("alpha\nbeta\ngamma\ndelta").unwrap();

        let written = render.terminal().written();
        assert_eq!(written.matches("alpha").count(), 1, "{written:?}");
        assert_eq!(written.matches("beta").count(), 1, "{written:?}");
    }

    #[test]
    fn memory_does_not_grow_with_the_length_of_the_session() {
        // The reason there is no alternate screen. What this process holds is
        // one screen of rows no matter how long the model talks.
        let mut render = Renderer::new(Recording::new(40, 4)).unwrap();
        for turn in 0..5_000 {
            render.stream(&format!("line {turn}\n")).unwrap();
            render.terminal().take();
        }

        assert!(render.drawn <= 4, "drew {} rows", render.drawn);
        assert!(render.tail.len() <= 4, "held {} rows", render.tail.len());
        assert!(render.overflow.is_empty(), "overflow was not drained");
    }

    #[test]
    fn settling_leaves_the_tail_in_scrollback_and_starts_fresh() {
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.stream("answer").unwrap();
        render.settle().unwrap();
        render.terminal().take();

        render.stream("next").unwrap();

        assert_eq!(render.terminal().written(), format!("{}next", rewind(0)));
    }

    #[test]
    fn settling_twice_writes_nothing_the_second_time() {
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.stream("answer").unwrap();
        render.settle().unwrap();
        render.terminal().take();

        render.settle().unwrap();

        assert_eq!(render.terminal().written(), "");
    }

    #[test]
    fn a_committed_line_is_never_drawn_a_second_time() {
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.commit("$ cargo build").unwrap();
        render.stream("done").unwrap();
        render.stream(" now").unwrap();

        let written = render.terminal().written();
        assert_eq!(written.matches("$ cargo build").count(), 1, "{written:?}");
    }

    #[test]
    fn a_committed_line_wider_than_the_terminal_still_wraps() {
        let mut render = Renderer::new(Recording::new(4, 24)).unwrap();
        render.commit("abcdefghij").unwrap();

        let written = render.terminal().written();
        assert!(
            written.contains("abcd\r\nefgh\r\nij\r\n"),
            "expected wrapped rows, got {written:?}"
        );
    }

    #[test]
    fn a_redirected_run_takes_the_pipe_path() {
        // The wiring: that a terminal reporting itself redirected reaches
        // `plain` at all. What that path produces is its own module's tests.
        let mut render = Renderer::new(Recording::redirected(80, 2)).unwrap();
        render.stream("alpha\nbeta\ngamma\ndelta").unwrap();
        render.settle().unwrap();

        assert_eq!(
            render.terminal().written(),
            "alpha\nbeta\ngamma\ndelta\n",
            "every row must reach the pipe, in order, once, and with no escapes"
        );
    }

    #[test]
    fn a_resize_rewraps_instead_of_redrawing_wrongly() {
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.stream("hello").unwrap();

        render.terminal().resize(10, 24);
        render.resized().unwrap();
        render.terminal().take();

        render.stream("abcdefghijkl").unwrap();

        let written = render.terminal().written();
        assert!(
            written.contains("abcdefghij\r\nkl"),
            "expected a wrap at the new width, got {written:?}"
        );
    }

    #[test]
    fn a_resize_that_changes_nothing_writes_nothing() {
        let mut render = Renderer::new(Recording::new(80, 24)).unwrap();
        render.stream("hello").unwrap();
        render.terminal().take();

        render.resized().unwrap();

        assert_eq!(render.terminal().written(), "");
    }
}
