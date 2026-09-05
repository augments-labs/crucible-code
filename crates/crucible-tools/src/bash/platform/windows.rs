//! Windows race-free job containment and pollable anonymous pipes.
//!
//! Completion requires a successful job-accounting query with zero active
//! members after termination is requested. Windows can still be finalizing
//! descendant process objects and pending I/O; the caller separately reaps the
//! leader. This scope supplies process control for compatibility execution.
#![allow(
    unsafe_code,
    reason = "Windows exposes job objects and anonymous-pipe polling only through its system API"
)]

use std::io::{self, Read};
use std::mem::size_of;
use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt as _;
use std::process::{Child, Command, ExitStatus};
use std::thread;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_NO_DATA, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JobObjectBasicAccountingInformation, JobObjectExtendedLimitInformation,
    QueryInformationJobObject, SetInformationJobObject, TerminateJobObject,
};
use windows_sys::Win32::System::Pipes::PeekNamedPipe;
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, OpenThread, ResumeThread, THREAD_SUSPEND_RESUME,
};

use super::ReadState;

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
    pub(crate) fn new(command: &mut Command) -> io::Result<Self> {
        // SAFETY: null attributes and name request an unnamed job with default
        // security. The returned owned handle is closed by `Drop`.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let scope = Self(handle);

        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
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

#[cfg(test)]
mod tests;
