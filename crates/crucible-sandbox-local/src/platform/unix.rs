//! Unix process groups, non-blocking pipe reads, and pipes the runtime waits
//! on.

use std::io::{self, Read, Write};
use std::os::fd::{AsFd, AsRawFd};
use std::process::{Child, ChildStdin, Command, ExitStatus};

use crucible_runtime::BoxFuture;
use crucible_sandbox::SandboxInput;
use rustix::fs::OFlags;
use rustix::io::Errno;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

use super::ReadState;

/// The process group belonging to one command.
#[derive(Debug)]
pub(crate) struct Scope;

/// Copyable process-group authority borrowed by a command's status task and
/// the thread a violation's cancel runs on.
///
/// Each of them signals with it only under the lock the leader is reaped
/// under, and only while the leader is unreaped, so the numeric group leader
/// cannot have been reused by the time a signal is sent.
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

/// An output pipe read without waiting until it is first waited on, when it
/// moves into the reactor of the runtime polling that read, and stays there:
/// a later waiting read is polled on that runtime, which needs its I/O driver
/// (Tokio panics on one without it).
pub(super) struct Waited<P: AsRawFd> {
    plain: Option<P>,
    registered: Option<AsyncFd<P>>,
}

impl<P: super::Pipe> Waited<P> {
    pub(super) const fn new(pipe: P) -> Self {
        Self {
            plain: Some(pipe),
            registered: None,
        }
    }
}

impl<P: super::Pipe> super::Stream for Waited<P> {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState> {
        if let Some(pipe) = &mut self.registered {
            return pipe.get_mut().read_ready(buffer);
        }
        self.plain.as_mut().ok_or_else(lost)?.read_ready(buffer)
    }

    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<ReadState>> {
        Box::pin(async move {
            if buffer.is_empty() {
                return Ok(ReadState::Bytes(0));
            }
            let pipe = registered(&mut self.plain, &mut self.registered, Interest::READABLE)?;
            loop {
                let mut ready = pipe.readable_mut().await?;
                // A read that would block clears the readiness the reactor
                // reported, and the loop waits for the next.
                if let Ok(read) = ready.try_io(|pipe| pipe.get_mut().read(buffer)) {
                    return Ok(match read? {
                        0 => ReadState::End,
                        count => ReadState::Bytes(count),
                    });
                }
            }
        })
    }
}

/// The writing end of a command's standard input, made non-blocking and
/// moved into the reactor of the runtime polling its first write.
struct Input {
    plain: Option<ChildStdin>,
    registered: Option<AsyncFd<ChildStdin>>,
}

/// What a command holds for its asynchronous input: nothing, because the
/// reactor waits on the pipe and no thread is started for it. Its end is the
/// same call a platform with such a thread makes.
#[derive(Debug, Clone, Default)]
pub(crate) struct InputThread {
    _none: (),
}

impl InputThread {
    /// Ends nothing: no thread writes a command's input here.
    #[allow(
        clippy::unnecessary_wraps,
        clippy::unused_self,
        reason = "the same call as the platform whose input is written on a thread"
    )]
    pub(crate) fn end(&self) -> io::Result<()> {
        Ok(())
    }
}

/// Hands `pipe` back as something written asynchronously.
///
/// Nothing changes about the pipe until the first write, which is what finds
/// the runtime that waits on it. No thread is started, so `_thread` is left
/// as it is.
pub(crate) fn input(pipe: ChildStdin, _thread: &InputThread) -> Box<dyn SandboxInput> {
    Box::new(Input {
        plain: Some(pipe),
        registered: None,
    })
}

/// A write takes what the pipe has room for when the reactor says it has
/// some, so a write dropped before it answers has delivered nothing.
impl SandboxInput for Input {
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async move {
            if bytes.is_empty() {
                return Ok(0);
            }
            if let Some(pipe) = &self.plain {
                prepare(pipe)?;
            }
            let pipe = registered(&mut self.plain, &mut self.registered, Interest::WRITABLE)?;
            loop {
                let mut ready = pipe.writable_mut().await?;
                if let Ok(written) = ready.try_io(|pipe| pipe.get_mut().write(bytes)) {
                    return written;
                }
            }
        })
    }
}

/// The pipe in `registered`, moving it there from `plain` first if it has
/// not been waited on before.
///
/// # Errors
///
/// No Tokio runtime is polling this, or the reactor refused the descriptor;
/// the pipe then stays as it was. A runtime without its I/O driver enabled
/// panics inside Tokio instead, which is why every caller documents needing
/// one.
fn registered<'a, P: AsRawFd>(
    plain: &mut Option<P>,
    registered: &'a mut Option<AsyncFd<P>>,
    interest: Interest,
) -> io::Result<&'a mut AsyncFd<P>> {
    if registered.is_none() {
        tokio::runtime::Handle::try_current().map_err(io::Error::other)?;
        let pipe = plain.take().ok_or_else(lost)?;
        match AsyncFd::try_with_interest(pipe, interest) {
            Ok(pipe) => *registered = Some(pipe),
            Err(refused) => {
                let (pipe, problem) = refused.into_parts();
                *plain = Some(pipe);
                return Err(problem);
            }
        }
    }
    registered.as_mut().ok_or_else(lost)
}

fn lost() -> io::Error {
    io::Error::other("the command's pipe is no longer held here")
}
