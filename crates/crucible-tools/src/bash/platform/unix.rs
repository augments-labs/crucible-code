//! Unix process groups and non-blocking pipe reads.

use std::io::{self, Read};
use std::os::fd::AsFd;
use std::process::{Child, Command, ExitStatus};

use rustix::fs::OFlags;
use rustix::io::Errno;

use super::ReadState;

/// The process group belonging to one command.
#[derive(Debug)]
pub(crate) struct Scope;

/// Copyable process-group authority borrowed by a supervisor thread.
///
/// The owning process handle remains unreaped until the supervisor is joined,
/// so the numeric group leader cannot be reused while this value is live.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Terminator(rustix::process::Pid);

impl Scope {
    /// Puts the shell at the head of a process group of its own.
    pub(crate) fn new(command: &mut Command) -> Self {
        std::os::unix::process::CommandExt::process_group(command, 0);
        Self
    }

    /// Captures the process-group identity after spawn.
    #[allow(clippy::unused_self)]
    pub(crate) fn terminator(&self, child: &Child) -> io::Result<Terminator> {
        let raw = i32::try_from(child.id())
            .map_err(|_| io::Error::other("command process id does not fit this platform"))?;
        let group = rustix::process::Pid::from_raw(raw)
            .ok_or_else(|| io::Error::other("command process id cannot name a process group"))?;
        Ok(Terminator(group))
    }

    /// Observes leader exit without first releasing its numeric group identity,
    /// then stops descendants before the standard child handle reaps it.
    #[allow(clippy::unused_self)]
    pub(crate) fn try_wait(
        &self,
        child: &mut Child,
        terminator: Terminator,
    ) -> io::Result<Option<ExitStatus>> {
        use rustix::process::{WaitId, WaitIdOptions};

        let raw = i32::try_from(child.id())
            .map_err(|_| io::Error::other("command process id does not fit this platform"))?;
        let pid = rustix::process::Pid::from_raw(raw)
            .ok_or_else(|| io::Error::other("command process id cannot be observed"))?;
        let options = WaitIdOptions::NOHANG | WaitIdOptions::EXITED | WaitIdOptions::NOWAIT;
        if rustix::process::waitid(WaitId::Pid(pid), options)?.is_none() {
            return Ok(None);
        }
        terminator.stop()?;
        child.try_wait()
    }

    /// Stops the shell and every descendant still in its inherited group.
    pub(crate) fn stop(child: &mut Child) -> io::Result<()> {
        let group_result = i32::try_from(child.id())
            .ok()
            .and_then(rustix::process::Pid::from_raw)
            .map_or(Ok(()), |group| Terminator(group).stop());
        let child_result = child.kill().or_else(|problem| {
            (problem.kind() == io::ErrorKind::InvalidInput)
                .then_some(())
                .ok_or(problem)
        });

        group_result.and(child_result)
    }
}

impl Terminator {
    /// Sends an uncatchable signal to every process still in the command group.
    ///
    /// A group whose members have all exited is already stopped. Linux and the
    /// BSDs report that as `ESRCH`; XNU reports `EPERM` when the only members
    /// left are zombies waiting to be reaped, so macOS accepts that too. It
    /// cannot tell that case from a live member the caller may not signal, and
    /// a command group here holds only the user's own descendants.
    pub(crate) fn stop(self) -> io::Result<()> {
        rustix::process::kill_process_group(self.0, rustix::process::Signal::KILL)
            .or_else(|problem| already_stopped(problem).then_some(()).ok_or(problem))
            .map_err(io::Error::from)
    }
}

/// Whether a group kill failed only because nothing living was left in it.
fn already_stopped(problem: Errno) -> bool {
    problem == Errno::SRCH || (cfg!(target_os = "macos") && problem == Errno::PERM)
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
