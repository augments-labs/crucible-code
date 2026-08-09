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
    /// Nothing here can fail, and the signature says so. A terminal that will
    /// not report a size is still a terminal worth drawing on, so the size is
    /// guessed rather than refused — and a width that turns out wrong is
    /// corrected by [`Renderer::resized`] at the next prompt, once the terminal
    /// will say what it is. Somewhere that never answers keeps the guess for the
    /// whole session, which is the right trade for a pipe.
    #[must_use]
    pub fn new(terminal: T) -> Self {
        let size = terminal.size().unwrap_or(Size::FALLBACK);

        Self {
            tail: Tail::new(size.columns, size.rows),
            finished: Tail::new(size.columns, 1),
            terminal,
            drawn: 0,
            columns: size.columns,
            frame: Frame::new(),
            overflow: Vec::new(),
        }
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

        // The size comes from the terminal, not from the stream, so a run whose
        // output is redirected still sees the window change. There is no live
        // region in a pipe to wipe, and an erase sequence written into one ends
        // up in whatever kept the output. The rewrap below still applies.
        if self.terminal.is_terminal() {
            self.erase()?;
            self.terminal.flush()?;
        }

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

    /// How wide the terminal was when this last looked.
    ///
    /// Held rather than asked for, because a caller that decides how much of a
    /// line to show does so once per event and asking the operating system each
    /// time would put a syscall on the render path. [`Renderer::resized`] is
    /// what keeps it true.
    #[must_use]
    pub fn columns(&self) -> usize {
        self.columns
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
mod tests;
