//! The terminal, as four operations.
//!
//! Everything this process needs from a terminal goes through [`Terminal`].
//! That keeps the one crate that talks to the operating system's terminal
//! behind a seam narrow enough to reimplement in an afternoon, which matters
//! because a terminal crate is the kind of dependency that goes quiet for a
//! year while a platform moves underneath it.
//!
//! It is also what makes the renderer testable: [`Recording`] is a terminal
//! that keeps the bytes, so a test can assert on the exact escape sequences
//! written rather than on a screenshot.
//!
//! This module is where `crossterm` is named, and the two files below are the
//! whole of it: [`system`] for the size and the handle, [`raw`] for the mode.
//! Everything the renderer writes is plain ANSI, so what a swap would cost is
//! those two files.

pub(crate) mod raw;
pub(crate) mod system;

use std::io;

/// What can go wrong while talking to a terminal.
#[derive(Debug, thiserror::Error)]
pub enum TerminalError {
    /// The terminal could not be written to, or would not report its size.
    #[error("terminal: {0}")]
    Io(#[from] io::Error),
}

/// The size of the visible area, in cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    /// Columns.
    pub columns: usize,
    /// Rows.
    pub rows: usize,
}

impl Size {
    /// The size assumed when a terminal will not say.
    ///
    /// Eighty by twenty-four is the size of the terminal every other default
    /// was chosen for, and a wrong guess here costs a reflow, not a crash.
    pub const FALLBACK: Self = Self {
        columns: 80,
        rows: 24,
    };
}

/// A terminal this process can draw on.
pub trait Terminal {
    /// The visible area right now.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the terminal will not report a size.
    fn size(&self) -> Result<Size, TerminalError>;

    /// Queues text. Nothing is guaranteed to appear until [`Terminal::flush`].
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the write fails.
    fn write(&mut self, text: &str) -> Result<(), TerminalError>;

    /// Puts everything queued on the screen.
    ///
    /// A frame is one `write` and one `flush`, so a partially drawn frame is
    /// never on screen.
    ///
    /// # Errors
    ///
    /// [`TerminalError::Io`] if the flush fails.
    fn flush(&mut self) -> Result<(), TerminalError>;

    /// Whether output is going to a terminal rather than a pipe or a file.
    ///
    /// A redirected run must not emit cursor movement: the escape bytes would
    /// end up in whatever consumed the output.
    fn is_terminal(&self) -> bool;
}

/// A terminal that keeps what was written instead of showing it.
///
/// Tests use this to assert on the bytes a frame is made of.
#[derive(Debug)]
pub struct Recording {
    written: String,
    size: Size,
    is_terminal: bool,
    /// How many times [`Terminal::flush`] was called — one per frame.
    flushes: usize,
}

impl Recording {
    /// A recording terminal of the given size, which claims to be a terminal.
    #[must_use]
    pub fn new(columns: usize, rows: usize) -> Self {
        Self {
            written: String::new(),
            size: Size { columns, rows },
            is_terminal: true,
            flushes: 0,
        }
    }

    /// The same, but standing in for a pipe.
    #[must_use]
    pub fn redirected(columns: usize, rows: usize) -> Self {
        Self {
            is_terminal: false,
            ..Self::new(columns, rows)
        }
    }

    /// Everything written so far.
    #[must_use]
    pub fn written(&self) -> &str {
        &self.written
    }

    /// How many frames were put on screen.
    #[must_use]
    pub fn flushes(&self) -> usize {
        self.flushes
    }

    /// Forgets what was written, keeping the size and the flush count.
    pub fn take(&mut self) -> String {
        std::mem::take(&mut self.written)
    }

    /// Changes the reported size, standing in for a window the user resized.
    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.size = Size { columns, rows };
    }
}

impl Terminal for Recording {
    fn size(&self) -> Result<Size, TerminalError> {
        Ok(self.size)
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        self.written.push_str(text);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.flushes += 1;
        Ok(())
    }

    fn is_terminal(&self) -> bool {
        self.is_terminal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_recording_terminal_keeps_what_was_written() {
        let mut term = Recording::new(80, 24);
        term.write("one").unwrap();
        term.write("two").unwrap();

        assert_eq!(term.written(), "onetwo");
    }

    #[test]
    fn a_recording_terminal_counts_frames() {
        let mut term = Recording::new(80, 24);
        assert_eq!(term.flushes(), 0);

        term.write("x").unwrap();
        term.flush().unwrap();

        assert_eq!(term.flushes(), 1);
    }

    #[test]
    fn taking_clears_the_bytes_but_not_the_frame_count() {
        let mut term = Recording::new(80, 24);
        term.write("x").unwrap();
        term.flush().unwrap();

        assert_eq!(term.take(), "x");
        assert_eq!(term.written(), "");
        assert_eq!(term.flushes(), 1, "a frame still happened");
    }

    #[test]
    fn a_redirected_terminal_says_so() {
        assert!(Recording::new(80, 24).is_terminal());
        assert!(!Recording::redirected(80, 24).is_terminal());
    }
}
