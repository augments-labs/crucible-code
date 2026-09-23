//! Windows race-free job containment, pollable anonymous pipes, and the
//! threads that wait on them.
//!
//! Completion requires a successful job-accounting query with zero active
//! members after termination is requested. Windows can still be finalizing
//! descendant process objects and pending I/O; the caller separately reaps the
//! leader. This scope supplies process control for compatibility execution.
//!
//! A pipe read or written asynchronously is handed to a thread of its own
//! (the `owned` module), because no runtime here is told when an anonymous pipe
//! becomes ready. A write that thread is parked in is abandoned by cancelling
//! the pipe's pending I/O, which is what dropping the writer asks for.
#![allow(
    unsafe_code,
    reason = "Windows exposes job objects and anonymous-pipe polling only through its system API"
)]

use std::io::{self, Read, Write};
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt as _;
use std::process::{Child, ChildStdin, Command, ExitStatus};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crucible_runtime::BoxFuture;
use crucible_sandbox::SandboxInput;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_NO_DATA, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::IO::{CancelIoEx, CancelSynchronousIo};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_JOB_TIME,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicAccountingInformation,
    JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
    TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

use super::ReadState;
use super::owned::{Reader, Writer, WriterOwner};

/// Bounds job-state polling separately from the caller's leader reap.
const STOP_WAIT: Duration = Duration::from_millis(250);
const STOP_POLL: Duration = Duration::from_millis(5);

/// A kill-on-close job containing one command and all its descendants.
pub(crate) struct Scope(HANDLE);

/// Job termination authority borrowed by the bounded supervisor thread.
#[derive(Clone, Copy)]
pub(crate) struct Terminator(HANDLE);

// SAFETY: `LocalProcess` joins the only thread receiving this borrowed raw
// handle before its owning `Scope` can be dropped. `TerminateJobObject` accepts
// a job handle from any thread and does not take ownership of it.
unsafe impl Send for Terminator {}

// SAFETY: a job object is a kernel handle rather than anything owned by the
// thread that made it. Every call this module makes through it —
// `AssignProcessToJobObject`, `TerminateJobObject`, `QueryInformationJobObject`,
// `CloseHandle` — is documented as usable from any thread, and the handle is
// not duplicated: exactly one `Scope`
// owns it and closes it once. What crossing a thread means here is that a command
// left running is owned by the registry the whole process shares, and the thread
// that started it has gone.
//
// `Sync` is deliberately *not* claimed. Nothing needs it: the registry keeps every
// scope behind its own lock, so two threads never hold one at the same time, and a
// claim nobody needs is a claim nobody has checked.
unsafe impl Send for Scope {}

impl std::fmt::Debug for Scope {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Scope(<job>)")
    }
}

impl Scope {
    /// Creates the job before the child so every configuration failure is early.
    pub(crate) fn new(
        command: &mut Command,
        resource_limits: crucible_sandbox::SandboxResourceLimits,
    ) -> io::Result<Self> {
        // SAFETY: null attributes and name request an unnamed job with default
        // security. The returned owned handle is closed by `Drop`.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let scope = Self(handle);

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        if let Some(ticks) = job_time_limit(resource_limits.cpu_seconds)? {
            limits.BasicLimitInformation.PerJobUserTimeLimit = ticks;
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_TIME;
        }
        let length = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(|_| io::Error::other("job limits do not fit the Windows API"))?;
        // SAFETY: `limits` has the layout named by the information class and
        // lives through the call; `scope` holds a valid job handle.
        let set = unsafe {
            SetInformationJobObject(
                scope.0,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                length,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        // The shell must not execute between CreateProcess returning and its
        // job assignment. A fast shell can launch an uncontained descendant in
        // that interval, so it starts suspended and is resumed only by
        // `attach` after the assignment succeeds.
        command.creation_flags(CREATE_SUSPENDED);
        Ok(scope)
    }

    /// Assigns and then starts the shell; descendants inherit membership.
    pub(crate) fn attach(&self, child: &Child) -> io::Result<()> {
        // SAFETY: both handles are live for the duration of the call.
        let assigned = unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        resume(child)
    }

    /// Borrows the job handle for the lifetime of the joined supervisor.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the Unix scope can fail to name its process group; both share one call shape"
    )]
    pub(crate) fn terminator(&self, _child: &Child) -> io::Result<Terminator> {
        Ok(Terminator(self.0))
    }

    /// Reports a finished leader after confirming no active job members.
    pub(crate) fn try_wait(
        &self,
        child: &mut Child,
        terminator: Terminator,
    ) -> io::Result<Option<ExitStatus>> {
        let status = child.try_wait()?;
        if status.is_some() {
            terminator.stop()?;
            if !self.empty()? {
                return Ok(None);
            }
        }
        Ok(status)
    }

    /// Requests job termination, kills the leader, and confirms job emptiness.
    pub(crate) fn stop(&self, child: &mut Child) -> io::Result<()> {
        // SAFETY: the job handle remains owned by `self`.
        let stopped = unsafe { TerminateJobObject(self.0, 1) };
        let job_result = if stopped == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        };
        let child_result = child.kill().or_else(|problem| {
            (problem.kind() == io::ErrorKind::InvalidInput)
                .then_some(())
                .ok_or(problem)
        });
        job_result.and(child_result)?;
        self.wait_empty()
    }

    /// Waits for observed extinction without changing the job's membership.
    fn wait_empty(&self) -> io::Result<()> {
        let deadline = Instant::now() + STOP_WAIT;
        loop {
            if self.empty()? {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the Windows job still has active processes after termination",
                ));
            }
            thread::sleep(remaining.min(STOP_POLL));
        }
    }

    /// Observes job membership without mistaking an unavailable count for zero.
    fn empty(&self) -> io::Result<bool> {
        let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
        let length = u32::try_from(size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>())
            .map_err(|_| io::Error::other("job accounting does not fit the Windows API"))?;
        // SAFETY: `self` owns the live job handle and the writable buffer has
        // exactly the layout and size selected by the information class.
        let queried = unsafe {
            QueryInformationJobObject(
                self.0,
                JobObjectBasicAccountingInformation,
                std::ptr::from_mut(&mut accounting).cast(),
                length,
                std::ptr::null_mut(),
            )
        };
        if queried == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(accounting.ActiveProcesses == 0)
        }
    }
}

fn job_time_limit(seconds: Option<u64>) -> io::Result<Option<i64>> {
    seconds
        .map(|seconds| {
            seconds
                .checked_mul(10_000_000)
                .and_then(|ticks| i64::try_from(ticks).ok())
                .ok_or_else(|| io::Error::other("CPU time limit does not fit a Windows Job"))
        })
        .transpose()
}

impl Terminator {
    /// Requests termination; the owning scope separately observes completion.
    pub(crate) fn stop(self) -> io::Result<()> {
        // SAFETY: the owning `Scope` remains live until this supervisor call
        // returns and the thread is joined.
        if unsafe { TerminateJobObject(self.0, 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

/// Resumes the one thread a newly-created suspended process contains.
fn resume(child: &Child) -> io::Result<()> {
    // SAFETY: the snapshot handle is owned below and closed on every path.
    let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) };
    if snapshot == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let snapshot = Handle(snapshot);
    let mut entry = THREADENTRY32 {
        dwSize: u32::try_from(size_of::<THREADENTRY32>())
            .map_err(|_| io::Error::other("a thread description does not fit the Windows API"))?,
        ..THREADENTRY32::default()
    };

    // SAFETY: `entry` is sized as the API requires and writable throughout the
    // enumeration; `snapshot` remains live.
    let mut found = unsafe { Thread32First(snapshot.0, &raw mut entry) } != 0;
    while found {
        if entry.th32OwnerProcessID == child.id() {
            // SAFETY: the id came from the live snapshot. The owned handle is
            // closed before this function returns.
            let thread = unsafe { OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID) };
            if thread.is_null() {
                return Err(io::Error::last_os_error());
            }
            let thread = Handle(thread);
            // SAFETY: this handle has suspend/resume access and names the sole
            // thread created suspended by `Scope::new`.
            if unsafe { ResumeThread(thread.0) } == u32::MAX {
                return Err(io::Error::last_os_error());
            }
            return Ok(());
        }

        // SAFETY: the arguments remain the same valid snapshot and entry.
        found = unsafe { Thread32Next(snapshot.0, &raw mut entry) } != 0;
    }

    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "the suspended command thread was not found",
    ))
}

/// One Windows handle closed on every return path.
struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this is the one close of the owned snapshot or thread handle.
        unsafe { CloseHandle(self.0) };
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        // SAFETY: this is the one close of the owned handle. Kill-on-close is a
        // second containment attempt if an earlier termination call failed.
        unsafe { CloseHandle(self.0) };
    }
}

/// Windows pipes are polled with `PeekNamedPipe`, so no mode change is needed.
pub(super) fn prepare(pipe: &impl AsRawHandle) -> io::Result<()> {
    if raw(pipe).is_null() {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the command output pipe has no Windows handle",
        ))
    } else {
        Ok(())
    }
}

/// Reads only after Windows says bytes are immediately available.
pub(super) fn read(
    pipe: &mut (impl Read + AsRawHandle),
    buffer: &mut [u8],
) -> io::Result<ReadState> {
    let mut available = 0;
    // SAFETY: the pipe handle is live; no output buffer is requested, and the
    // one non-null pointer names a writable `u32` for the duration of the call.
    let peeked = unsafe {
        PeekNamedPipe(
            raw(pipe),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
            &raw mut available,
            std::ptr::null_mut(),
        )
    };
    if peeked == 0 {
        let problem = io::Error::last_os_error();
        return match problem.raw_os_error().map(i32::cast_unsigned) {
            Some(ERROR_BROKEN_PIPE | ERROR_NO_DATA) => Ok(ReadState::End),
            _ => Err(problem),
        };
    }
    if available == 0 {
        return Ok(ReadState::Pending);
    }

    let limit = usize::try_from(available)
        .unwrap_or(usize::MAX)
        .min(buffer.len());
    let Some(ready) = buffer.get_mut(..limit) else {
        return Err(io::Error::other(
            "Windows reported more pipe bytes than the reader can hold",
        ));
    };
    pipe.read(ready).map(|read| {
        if read == 0 {
            ReadState::End
        } else {
            ReadState::Bytes(read)
        }
    })
}

fn raw(pipe: &impl AsRawHandle) -> HANDLE {
    pipe.as_raw_handle() as HANDLE
}

/// An output pipe read without waiting until it is first waited on, when it
/// moves onto a thread of its own for as long as this lives.
pub(super) struct Waited<P> {
    plain: Option<P>,
    owned: Option<Reader>,
}

impl<P: super::Pipe> Waited<P> {
    pub(super) const fn new(pipe: P) -> Self {
        Self {
            plain: Some(pipe),
            owned: None,
        }
    }
}

impl<P: super::Pipe> super::Stream for Waited<P> {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<ReadState> {
        if let Some(reader) = &mut self.owned {
            return reader.read_ready(buffer);
        }
        self.plain.as_mut().ok_or_else(lost)?.read_ready(buffer)
    }

    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<ReadState>> {
        Box::pin(async move {
            if buffer.is_empty() {
                return Ok(ReadState::Bytes(0));
            }
            if self.owned.is_none() {
                let pipe = self.plain.take().ok_or_else(lost)?;
                self.owned = Some(Reader::start(pipe)?);
            }
            self.owned.as_mut().ok_or_else(lost)?.read(buffer).await
        })
    }
}

/// The writing end of a command's standard input, moved onto a thread of its
/// own at the first write.
struct Input {
    plain: Option<ChildStdin>,
    owned: Option<Writer>,
    /// Where the thread's owner is left for the command to end.
    thread: InputThread,
}

/// The thread a command's asynchronous input is written on, once its first
/// write has started one, held by the command, whose stop ends and joins it.
/// Once ended it starts no thread, so a write first made after the stop is
/// refused rather than start a thread nothing would join.
pub(crate) type InputThread = WriterOwner;

/// Hands `pipe` back as something written asynchronously, leaving the thread
/// its first write starts in `thread`.
///
/// Nothing changes about the pipe until that first write.
pub(crate) fn input(pipe: ChildStdin, thread: &InputThread) -> Box<dyn SandboxInput> {
    Box::new(Input {
        plain: Some(pipe),
        owned: None,
        thread: thread.clone(),
    })
}

impl SandboxInput for Input {
    fn write<'a>(&'a mut self, bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async move {
            if bytes.is_empty() {
                return Ok(0);
            }
            if self.owned.is_none() {
                let pipe = Arc::new(self.plain.take().ok_or_else(lost)?);
                // Weak, so the pipe closes when the thread lets go of it,
                // whatever still holds the interruption.
                let held = Arc::downgrade(&pipe);
                let writer = self.thread.start(
                    Shared(pipe),
                    Box::new(move |thread| {
                        if let Some(pipe) = held.upgrade() {
                            abandon(&pipe, thread);
                        }
                    }),
                )?;
                self.owned = Some(writer);
            }
            self.owned.as_mut().ok_or_else(lost)?.write(bytes).await
        })
    }
}

/// The pipe the writer's thread writes through, while the interruption holds
/// the same pipe open so its handle stays the pipe's.
struct Shared(Arc<ChildStdin>);

impl Write for Shared {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        (&*self.0).write(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        (&*self.0).flush()
    }
}

/// Abandons whatever write `thread` is parked in on `pipe`.
///
/// The interruption calls this with the pipe held for the call, so the handle
/// it names is open throughout.
///
/// The standard library writes a child's input through an overlapped handle,
/// which `CancelIoEx` reaches from any thread; a synchronous write, which the
/// handle could also be given, is reached by `CancelSynchronousIo` on the
/// thread instead. Either answers failure when nothing is pending, which is
/// the case the caller retries.
fn abandon(pipe: &ChildStdin, thread: &JoinHandle<()>) {
    // SAFETY: `pipe` is borrowed from an `Arc` the caller upgraded and holds
    // for the call, so its handle is open throughout, and `thread` is borrowed
    // from the join handle that owns the thread's handle. Neither call writes through a pointer, and a
    // null `OVERLAPPED` asks for every pending request on the handle.
    unsafe {
        CancelIoEx(raw(pipe), std::ptr::null());
        CancelSynchronousIo(thread.as_raw_handle() as HANDLE);
    }
}

fn lost() -> io::Error {
    io::Error::other("the command's pipe is no longer held here")
}

#[cfg(test)]
mod tests;
