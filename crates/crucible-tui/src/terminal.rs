//! The terminal, as four operations.
//!
//! Everything this process needs from a terminal goes through [`Terminal`].
//! That keeps the one crate that talks to the operating system's terminal
//! behind a seam narrow enough to reimplement in an afternoon, which matters
//! because a terminal crate is the kind of dependency that goes quiet for a
//! year while a platform moves underneath it.
//!
//! It is also what makes the renderer testable: [`Recording`] is a terminal
//! that keeps the bytes, and [`Picture`] replays them back into the picture they
//! would have drawn — which is what almost every test asks about, since a frame
//! names the row it writes and the sequences that carry it are asserted once,
//! beside the type that writes them.
//!
//! This module is where `crossterm` is named, and the files below are the whole
//! of it: [`system`] for the size and the handle, [`raw`] for the mode,
//! [`keys`] for what arrives once the mode is raw. Beside them sit the modes
//! that are asked for in plain ANSI and so name nothing — [`mouse`] for the
//! pointer, [`keyboard`] for how a modified key is spelled — and [`ground`],
//! which is the one question this process asks a terminal. Everything the
//! renderer writes is plain ANSI, so what a swap would cost is the first
//! three.

pub(crate) mod ground;
pub(crate) mod keyboard;
pub(crate) mod keys;
pub(crate) mod mouse;
pub(crate) mod raw;
pub(crate) mod screen;
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
/// Tests use this to assert on the bytes a frame is made of, and — through
/// [`Recording::picture`] — on the picture those bytes would have left.
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

    /// The picture everything written so far would have left on this size.
    ///
    /// What a test wants to know almost always. Bytes say how a frame said
    /// something; this says what it said, and goes on being readable when the
    /// sequences underneath it change.
    #[must_use]
    pub fn picture(&self) -> Picture {
        Picture::of(&self.written, self.size.columns, self.size.rows)
    }
}

/// The window as the bytes written to it would leave it.
///
/// Every frame names the row it writes and erases only that row, so a screen is
/// rebuilt by replaying the writes in order: a row nothing named keeps what the
/// frame before left on it. Everything that moves no character — colour, the
/// brackets that hold a frame, the cursor being hidden — is read past, and what
/// is left is where each character ended up.
#[derive(Debug)]
pub struct Picture {
    /// One string a row, in window order.
    rows: Vec<String>,
    /// Where the cursor was left, in rows and columns from the top left.
    caret: (usize, usize),
}

impl Picture {
    /// Replays `written` onto a window `columns` by `rows`.
    ///
    /// Public so a test holding one frame's bytes rather than a whole session's
    /// can ask the same question of them.
    #[must_use]
    pub fn of(written: &str, columns: usize, rows: usize) -> Self {
        let mut picture = Self {
            rows: vec![String::new(); rows],
            caret: (0, 0),
        };
        let mut left = written.chars().peekable();
        let mut text = String::new();

        while let Some(character) = left.next() {
            if character != '\x1b' {
                text.push(character);
                continue;
            }

            picture.put(&text, columns);
            text.clear();

            match left.next() {
                // A control sequence: parameters, then one byte saying what it
                // does. Only two of them move a character.
                Some('[') => {
                    let mut parameters = String::new();
                    let ending = loop {
                        match left.next() {
                            Some(byte @ '@'..='~') => break byte,
                            Some(byte) => parameters.push(byte),
                            None => return picture,
                        }
                    };
                    picture.does(ending, &parameters);
                }
                // An operating-system command — the clipboard request is one —
                // runs until a bell or a string terminator and draws nothing.
                Some(']') => {
                    while let Some(byte) = left.next() {
                        if byte == '\x07' {
                            break;
                        }
                        if byte == '\x1b' && left.peek() == Some(&'\\') {
                            left.next();
                            break;
                        }
                    }
                }
                _ => {}
            }
        }

        picture.put(&text, columns);
        picture
    }

    /// Acts on one control sequence, where it is one of the two that matter.
    fn does(&mut self, ending: char, parameters: &str) {
        match ending {
            // Park: row and column, both counted from one on the wire.
            'H' => {
                let mut at = parameters.split(';');
                let row = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
                let column = at.next().and_then(|one| one.parse().ok()).unwrap_or(1);
                self.caret = (usize::max(row, 1) - 1, usize::max(column, 1) - 1);
            }
            // Erase from the cursor to the end of its row.
            'K' => {
                let (row, column) = self.caret;
                if let Some(line) = self.rows.get_mut(row) {
                    line.truncate(
                        line.char_indices()
                            .nth(column)
                            .map_or(line.len(), |(at, _)| at),
                    );
                }
            }
            _ => {}
        }
    }

    /// Writes `text` where the cursor is, advancing it.
    fn put(&mut self, text: &str, columns: usize) {
        if text.is_empty() {
            return;
        }

        let (row, column) = self.caret;
        if let Some(line) = self.rows.get_mut(row) {
            while line.chars().count() < column {
                line.push(' ');
            }
            let head: String = line.chars().take(column).collect();
            let tail: String = line.chars().skip(column + text.chars().count()).collect();
            *line = format!("{head}{text}{tail}");
        }

        self.caret = (row, usize::min(column + text.chars().count(), columns));
    }

    /// One row, without the padding a frame may have left on it.
    #[must_use]
    pub fn row(&self, at: usize) -> &str {
        self.rows.get(at).map_or("", |row| row.trim_end())
    }

    /// Every row that has anything on it, in window order.
    #[must_use]
    pub fn said(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row.trim_end().to_owned())
            .filter(|row| !row.is_empty())
            .collect()
    }

    /// Every row, blank ones included, in window order.
    #[must_use]
    pub fn rows(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| row.trim_end().to_owned())
            .collect()
    }

    /// Where the cursor was left, in rows and columns from the top left.
    #[must_use]
    pub fn caret(&self) -> (usize, usize) {
        self.caret
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
