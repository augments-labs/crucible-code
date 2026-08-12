//! A terminal and a log that do what a real one will not do on request.
//!
//! What the loop writes to is where several of its promises are kept: the
//! window is read again at every prompt, a write that fails must not take the
//! turn's own record with it, and a log has to be finished by whichever thread
//! is holding it. None of that can be driven from a real terminal on a real
//! disk, so each of these is one thing going wrong, on purpose, at a moment a
//! test chose.

use std::cell::Cell;
use std::io;
use std::sync::{Arc, Mutex};

use crucible_tui::{Recording, Size, Terminal, TerminalError};

/// A terminal that narrows to ten columns once the renderer has read the size
/// it starts with.
///
/// The loop owns the renderer for its whole run, so nothing outside can resize
/// between turns the way a user does. This one resizes itself.
pub(super) struct Narrowing {
    inner: Recording,
    asked: Cell<usize>,
}

impl Narrowing {
    pub(super) fn new() -> Self {
        Self {
            inner: Recording::new(80, 24),
            asked: Cell::new(0),
        }
    }

    pub(super) fn written(&self) -> &str {
        self.inner.written()
    }
}

impl Terminal for Narrowing {
    fn size(&self) -> Result<Size, TerminalError> {
        let asked = self.asked.get();
        self.asked.set(asked + 1);

        Ok(Size {
            columns: if asked == 0 { 80 } else { 10 },
            rows: 24,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        self.inner.write(text)
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.inner.flush()
    }

    fn is_terminal(&self) -> bool {
        self.inner.is_terminal()
    }
}

/// A terminal that takes `left` writes and refuses everything after them, the
/// way one whose window has been closed does.
pub(super) struct Breaking {
    pub(super) inner: Recording,
    pub(super) left: usize,
}

impl Terminal for Breaking {
    fn size(&self) -> Result<Size, TerminalError> {
        self.inner.size()
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        if self.left == 0 {
            return Err(TerminalError::Io(io::ErrorKind::BrokenPipe.into()));
        }

        self.left -= 1;
        self.inner.write(text)
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.inner.flush()
    }

    fn is_terminal(&self) -> bool {
        self.inner.is_terminal()
    }
}

/// A log that fails every write, the way a full disk does.
pub(super) struct Failing;

impl io::Write for Failing {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::new(
            io::ErrorKind::StorageFull,
            "no space left on device",
        ))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A log the test can read back once the session has finished writing it.
#[derive(Debug)]
pub(super) struct Kept(pub(super) Arc<Mutex<Vec<u8>>>);

impl io::Write for Kept {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .expect("a lock nothing panicked in")
            .extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
