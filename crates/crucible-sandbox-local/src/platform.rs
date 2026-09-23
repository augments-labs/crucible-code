//! Operating-system boundaries for command lifetime and pipe polling.
//!
//! A command's pipes are read and written two ways. Without waiting, which is
//! how the synchronous callers poll them. And waiting, where the platform lets
//! a runtime wait on a pipe: Unix hands the pipe to the reactor of the runtime
//! polling the first waiting read or write asked of it, and waits on it
//! through that runtime from then on, so that runtime needs its I/O driver.
//! Windows cannot, because an anonymous pipe tells nobody when it becomes
//! ready, so the waiting is done by a thread each pipe owns (the `owned`
//! module), and that thread is the final shape of the adapter there, not a
//! stand-in for one.

use std::io::{self, Read};
use std::process::{ChildStderr, ChildStdout};

use crucible_runtime::BoxFuture;

#[cfg(unix)]
mod unix;
#[cfg(unix)]
pub(crate) use unix::{InputThread, Scope, Terminator, input};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub(crate) use windows::{InputThread, Scope, Terminator, input};

#[cfg(any(windows, test))]
mod owned;

#[cfg(test)]
mod tests;

/// What one non-blocking attempt found on an output pipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

/// A child output stream that can also be waited on.
pub(crate) trait Stream: Send {
    /// Reads bytes that are available now, or says why there are none.
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState>;

    /// Reads as [`Self::read_ready`] does, once there is something to answer:
    /// never [`ReadState::Pending`]. Dropping the future before it answers
    /// takes nothing out of the stream.
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<ReadState>>;
}

/// What an output pipe must be to be waited on: read without waiting, and on
/// Unix named by a descriptor the reactor can watch.
#[cfg(unix)]
pub(crate) trait Pipe: Output + std::os::fd::AsRawFd {}
#[cfg(unix)]
impl<P: Output + std::os::fd::AsRawFd> Pipe for P {}

/// What an output pipe must be to be waited on: read without waiting.
#[cfg(windows)]
pub(crate) trait Pipe: Output {}
#[cfg(windows)]
impl<P: Output> Pipe for P {}

/// Prepares `pipe` for reads without waiting, and hands it back as a stream
/// that can also be waited on.
pub(crate) fn stream(pipe: impl Pipe) -> io::Result<Box<dyn Stream>> {
    pipe.prepare()?;
    Ok(Box::new(system::Waited::new(pipe)))
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
// A pipe with no process behind it, which is what the tests of the owned
// reader hold and close.
#[cfg(test)]
output!(io::PipeReader);
