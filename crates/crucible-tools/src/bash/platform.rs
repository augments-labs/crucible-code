//! Operating-system boundaries for command lifetime and pipe polling.

use std::io::{self, Read};
use std::process::{ChildStderr, ChildStdout};

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::Scope;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::Scope;

/// What one non-blocking attempt found on an output pipe.
pub(crate) enum ReadState {
    /// Bytes were copied into the caller's buffer.
    Bytes(usize),
    /// The writer remains open but has nothing available now.
    Pending,
    /// Every writer has closed its end.
    End,
}

/// A child output stream that can be polled without trapping its reader thread.
pub(crate) trait Output: Read + Send + 'static {
    /// Prepares the stream for non-blocking reads where the platform needs it.
    fn prepare(&self) -> io::Result<()>;

    /// Reads bytes that are available now, or says why there are none.
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState>;
}

macro_rules! output {
    ($pipe:ty) => {
        impl Output for $pipe {
            fn prepare(&self) -> io::Result<()> {
                system::prepare(self)
            }

            fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState> {
                system::read(self, buffer)
            }
        }
    };
}

#[cfg(unix)]
use unix as system;
#[cfg(windows)]
use windows as system;

output!(ChildStdout);
output!(ChildStderr);
