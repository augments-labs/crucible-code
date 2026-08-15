//! Unix process groups and non-blocking pipe reads.

use std::io::{self, Read};
use std::os::fd::AsFd;
use std::process::{Child, Command};

use rustix::fs::OFlags;
use rustix::io::Errno;

use super::ReadState;

/// The process group belonging to one command.
#[derive(Debug)]
pub(crate) struct Scope;

impl Scope {
    /// Puts the shell at the head of a process group of its own.
    pub(crate) fn new(command: &mut Command) -> Self {
        std::os::unix::process::CommandExt::process_group(command, 0);
        Self
    }

    /// Stops the shell and every descendant still in its inherited group.
    pub(crate) fn stop(child: &mut Child) -> io::Result<()> {
        let group = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw);
        let group_result = group.map_or(Ok(()), |group| {
            rustix::process::kill_process_group(group, rustix::process::Signal::KILL)
                .or_else(|problem| (problem == Errno::SRCH).then_some(()).ok_or(problem))
        });
        let child_result = child.kill().or_else(|problem| {
            (problem.kind() == io::ErrorKind::InvalidInput)
                .then_some(())
                .ok_or(problem)
        });

        group_result.map_err(io::Error::from).and(child_result)
    }
}

/// Makes a pipe return `WouldBlock` while its writer is merely quiet.
pub(super) fn prepare(pipe: &impl AsFd) -> io::Result<()> {
    let flags = rustix::fs::fcntl_getfl(pipe)?;
    rustix::fs::fcntl_setfl(pipe, flags | OFlags::NONBLOCK)?;
    Ok(())
}

/// Reads whatever a non-blocking pipe has ready.
pub(super) fn read(pipe: &mut impl Read, buffer: &mut [u8]) -> io::Result<ReadState> {
    match pipe.read(buffer) {
        Ok(0) => Ok(ReadState::End),
        Ok(read) => Ok(ReadState::Bytes(read)),
        Err(problem) if problem.kind() == io::ErrorKind::WouldBlock => Ok(ReadState::Pending),
        Err(problem) => Err(problem),
    }
}
