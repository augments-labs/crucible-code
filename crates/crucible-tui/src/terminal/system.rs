//! The real terminal, and the only file that names `crossterm`.
//!
//! Everything a terminal crate is needed for lives behind [`Terminal`]: one
//! ioctl for the size, and the standard-output handle. The escape sequences the
//! renderer writes are ANSI, not `crossterm`, so replacing this file is a day's
//! work rather than a rewrite -- which is the point of keeping it this thin.

use std::io::{self, IsTerminal, StdoutLock, Write};

use super::{Size, Terminal, TerminalError};

/// Standard output, as a terminal.
///
/// Holds the lock for the life of the renderer. This process has exactly one
/// thread that writes to the terminal, so the lock is taken once rather than
/// re-acquired on every frame.
#[derive(Debug)]
pub struct SystemTerminal {
    out: StdoutLock<'static>,
    is_terminal: bool,
}

impl SystemTerminal {
    /// Takes standard output.
    #[must_use]
    pub fn stdout() -> Self {
        let out = io::stdout().lock();
        Self {
            is_terminal: out.is_terminal(),
            out,
        }
    }
}

impl Terminal for SystemTerminal {
    fn size(&self) -> Result<Size, TerminalError> {
        let (columns, rows) = crossterm::terminal::size()?;

        Ok(Size {
            columns: columns as usize,
            rows: rows as usize,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        self.out.write_all(text.as_bytes())?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.out.flush()?;
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
    fn a_redirected_stdout_is_not_a_terminal() {
        // The test harness captures stdout, so this is the redirected case --
        // which is exactly the one where escape sequences must not be written.
        assert!(!SystemTerminal::stdout().is_terminal());
    }

    #[test]
    fn a_size_that_cannot_be_read_is_an_error_and_not_a_zero() {
        // Under the harness there is no terminal to measure, so this asks the
        // real thing and only requires that a failure is reported rather than
        // silently reported as a zero-column screen -- which would wrap forever.
        if let Ok(size) = SystemTerminal::stdout().size() {
            assert!(size.columns > 0, "a size of zero columns is not a size");
        }
    }
}
