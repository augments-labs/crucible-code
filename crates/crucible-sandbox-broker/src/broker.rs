//! Minimal namespace PID 1 for exactly one confined workload.

mod scan;
mod scope;

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{Read as _, Write as _};
use std::os::fd::{FromRawFd as _, RawFd};
use std::os::unix::process::CommandExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::path::PathBuf;
use std::process::{Child, Command, ExitCode, ExitStatus};
use std::thread;
use std::time::Duration;

use crucible_sandbox_broker::{
    BROKER_FAILURE_STATUS, CANCEL_FRAME, GO_FRAME, READY_FRAME, REFUSED_DESCRIPTOR_CLOSURE,
    REFUSED_FRAME, REFUSED_SCAN, encode_wait_status,
};
use rustix::fs::OFlags;
use rustix::io::Errno;
use rustix::io::FdFlags;
use rustix::process::{Resource, Rlimit};

use super::BROKER_ERROR_EXIT;

/// Supervises the one workload named on the command line until it is gone.
pub(super) fn supervise() -> ExitCode {
    ExitCode::from(run().unwrap_or(BROKER_ERROR_EXIT))
}

fn run() -> Option<u8> {
    let mut arguments = std::env::args_os();
    let _program = arguments.next()?;
    if arguments.next()?.to_str()? != "--status-fd" {
        return None;
    }
    let descriptor_argument = arguments.next()?;
    let descriptor = parse_descriptor(&descriptor_argument)?;
    let (roots, exclusions, limits) = parse_plan(&mut arguments)?;
    let program = arguments.next()?;
    let workload_arguments = arguments.collect::<Vec<_>>();

    // SAFETY: the host passes one live, uniquely owned descriptor above stderr.
    // Ownership is transferred to this process, and File closes it exactly once.
    let mut status_channel = unsafe { File::from_raw_fd(descriptor) };
    if rustix::io::fcntl_setfd(&status_channel, FdFlags::CLOEXEC).is_err() {
        return write_failure(&mut status_channel);
    }
    let Ok(baselines) = scan::prepare(&roots, &exclusions) else {
        return write_refusal(&mut status_channel, REFUSED_SCAN);
    };
    if close_inherited_except(descriptor).is_err() {
        return write_refusal(&mut status_channel, REFUSED_DESCRIPTOR_CLOSURE);
    }
    if status_channel
        .write_all(&READY_FRAME)
        .and_then(|()| status_channel.flush())
        .is_err()
    {
        return None;
    }
    let mut release = [0_u8; GO_FRAME.len()];
    if status_channel.read_exact(&mut release).is_err() || release != GO_FRAME {
        return None;
    }

    let mut workload = Command::new(program);
    workload.args(workload_arguments).process_group(0);
    // SAFETY: only async-signal-safe `setrlimit` syscalls run between fork and
    // exec; the copied limit record owns no allocator-backed state.
    unsafe {
        workload.pre_exec(move || limits.apply());
    }
    let Ok(status) = workload
        .spawn()
        .and_then(|mut child| wait_workload(&mut child, &mut status_channel))
    else {
        return write_failure(&mut status_channel);
    };
    if scope::empty().is_err() {
        return write_failure(&mut status_channel);
    }
    if status_channel
        .write_all(&encode_wait_status(status.into_raw()))
        .and_then(|()| scan::write_terminal(&mut status_channel, &roots, &exclusions, &baselines))
        .is_err()
    {
        return None;
    }
    Some(exit_code(status.code(), status.signal()))
}

fn wait_workload(child: &mut Child, channel: &mut File) -> std::io::Result<ExitStatus> {
    let flags = rustix::fs::fcntl_getfl(&*channel)?;
    rustix::fs::fcntl_setfl(&*channel, flags | OFlags::NONBLOCK)?;
    let waited = wait_or_cancel(child, channel);
    rustix::fs::fcntl_setfl(&*channel, flags)?;
    waited?.ok_or_else(|| std::io::Error::other("sandbox workload status is unavailable"))
}

fn wait_or_cancel(child: &mut Child, channel: &mut File) -> std::io::Result<Option<ExitStatus>> {
    let mut cancellation = [0_u8; CANCEL_FRAME.len()];
    let mut received = 0_usize;
    loop {
        if let Some(status) = scope::reap_until_workload(child.id())? {
            return Ok(Some(status));
        }
        match channel.read(cancellation.get_mut(received..).ok_or_else(|| {
            std::io::Error::other("sandbox cancellation frame exceeded its bound")
        })?) {
            Ok(0) => return stop_workload(child).map(Some),
            Ok(read) => {
                received = received.saturating_add(read);
                if received == cancellation.len() {
                    if cancellation != CANCEL_FRAME {
                        return Err(std::io::Error::other(
                            "sandbox cancellation frame is invalid",
                        ));
                    }
                    return stop_workload(child).map(Some);
                }
            }
            Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(problem) if problem.kind() == std::io::ErrorKind::Interrupted => {}
            Err(problem) => return Err(problem),
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn stop_workload(child: &mut Child) -> std::io::Result<ExitStatus> {
    let group = i32::try_from(child.id())
        .ok()
        .and_then(rustix::process::Pid::from_raw)
        .ok_or_else(|| std::io::Error::other("sandbox workload process group is invalid"))?;
    let group_result = rustix::process::kill_process_group(group, rustix::process::Signal::KILL)
        .or_else(|problem| (problem == Errno::SRCH).then_some(()).ok_or(problem))
        .map_err(std::io::Error::from);
    let child_result = child.kill().or_else(|problem| {
        (problem.kind() == std::io::ErrorKind::InvalidInput)
            .then_some(())
            .ok_or(problem)
    });
    group_result.and(child_result)?;
    child.wait()
}

fn close_inherited_except(status: RawFd) -> std::io::Result<()> {
    let descriptor_directory = std::fs::canonicalize("/proc/self/fd")?;
    let directory = std::fs::read_dir("/proc/self/fd")?;
    let mut descriptors = Vec::new();
    for entry in directory {
        let entry = entry?;
        if std::fs::read_link(entry.path()).ok().as_ref() == Some(&descriptor_directory) {
            continue;
        }
        let Some(descriptor) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<RawFd>().ok())
        else {
            continue;
        };
        if descriptor > 2 && descriptor != status {
            descriptors.push(descriptor);
        }
    }
    for descriptor in descriptors {
        // SAFETY: `/proc/self/fd` named this live descriptor after the directory
        // iterator was dropped. This single-threaded broker owns every inherited
        // descriptor except `status`, and each number occurs at most once.
        drop(unsafe { File::from_raw_fd(descriptor) });
    }
    Ok(())
}

fn parse_plan(
    arguments: &mut impl Iterator<Item = OsString>,
) -> Option<(Vec<PathBuf>, Vec<PathBuf>, Limits)> {
    let mut roots = Vec::new();
    let mut exclusions = Vec::new();
    let mut limits = Limits::default();
    loop {
        let argument = arguments.next()?;
        match argument.to_str()? {
            "--project-root" => {
                let root = PathBuf::from(arguments.next()?);
                if !root.is_absolute() || roots.len() >= crucible_sandbox_broker::MAX_SCAN_ROOTS {
                    return None;
                }
                roots.push(root);
            }
            "--project-exclude" => {
                let exclusion = PathBuf::from(arguments.next()?);
                if !exclusion.is_absolute()
                    || exclusions.len() >= crucible_sandbox_broker::MAX_SCAN_EXCLUSIONS
                {
                    return None;
                }
                exclusions.push(exclusion);
            }
            "--limit-cpu-seconds" => {
                limits.cpu_seconds = Some(parse_limit(arguments, limits.cpu_seconds)?);
            }
            "--limit-memory-bytes" => {
                limits.memory_bytes = Some(parse_limit(arguments, limits.memory_bytes)?);
            }
            "--limit-open-files" => {
                limits.open_files = Some(parse_limit(arguments, limits.open_files)?);
            }
            "--limit-processes" => {
                limits.processes = Some(parse_limit(arguments, limits.processes)?);
            }
            "--" => return Some((roots, exclusions, limits)),
            _ => return None,
        }
    }
}

fn parse_limit(
    arguments: &mut impl Iterator<Item = OsString>,
    current: Option<u64>,
) -> Option<u64> {
    if current.is_some() {
        return None;
    }
    arguments
        .next()?
        .to_str()?
        .parse::<u64>()
        .ok()
        .filter(|value| *value > 0)
}

/// Processes the broker's scope may hold at once when nothing states fewer.
///
/// The broker owns this one whether or not a policy mentions it, the way it
/// owns the core-dump ceiling: it is not a budget a caller negotiated, it is
/// what PID 1 does to the scope beneath it. Without it a workload that forks in
/// a loop is bounded by nothing this process controls, and the processor and
/// descriptor ceilings do not help — each new process gets its own.
///
/// Far above what a parallel build reaches and far below what costs the host
/// its process table. A policy may state fewer; it may not state more, because
/// this is the ceiling and not a default.
const PROCESSES: u64 = 1024;

#[derive(Clone, Copy, Default)]
struct Limits {
    cpu_seconds: Option<u64>,
    memory_bytes: Option<u64>,
    open_files: Option<u64>,
    processes: Option<u64>,
}

impl Limits {
    fn apply(self) -> std::io::Result<()> {
        set_limit(Resource::Core, 0)?;
        set_limit(Resource::Nproc, process_ceiling(self.processes))?;
        if let Some(value) = self.cpu_seconds {
            set_limit(Resource::Cpu, value)?;
        }
        if let Some(value) = self.memory_bytes {
            set_limit(Resource::As, value)?;
        }
        if let Some(value) = self.open_files {
            set_limit(Resource::Nofile, value)?;
        }
        Ok(())
    }
}

/// The process ceiling this scope runs under, given whatever a policy stated.
///
/// A stated ceiling narrows; it never widens, so a plan the host assembled
/// wrongly, or one an argument list was edited into, cannot buy more of the
/// machine than the broker was willing to give away in the first place.
const fn process_ceiling(stated: Option<u64>) -> u64 {
    match stated {
        Some(value) if value < PROCESSES => value,
        _ => PROCESSES,
    }
}

fn set_limit(resource: Resource, requested: u64) -> std::io::Result<()> {
    let inherited = rustix::process::getrlimit(resource);
    let effective = inherited
        .maximum
        .map_or(requested, |maximum| requested.min(maximum));
    rustix::process::setrlimit(
        resource,
        Rlimit {
            current: Some(effective),
            maximum: Some(effective),
        },
    )
    .map_err(Into::into)
}

fn parse_descriptor(value: &OsStr) -> Option<RawFd> {
    let descriptor = value.to_str()?.parse::<RawFd>().ok()?;
    (descriptor > 2).then_some(descriptor)
}

fn write_failure(channel: &mut File) -> Option<u8> {
    channel
        .write_all(&encode_wait_status(BROKER_FAILURE_STATUS))
        .and_then(|()| channel.flush())
        .ok()?;
    Some(BROKER_ERROR_EXIT)
}

fn write_refusal(channel: &mut File, reason: u8) -> Option<u8> {
    channel
        .write_all(&REFUSED_FRAME)
        .and_then(|()| channel.write_all(&[reason]))
        .and_then(|()| channel.flush())
        .ok()?;
    Some(BROKER_ERROR_EXIT)
}

fn exit_code(code: Option<i32>, signal: Option<i32>) -> u8 {
    if let Some(code) = code.and_then(|code| u8::try_from(code).ok()) {
        return code;
    }
    signal
        .and_then(|signal| u8::try_from(signal).ok())
        .map_or(BROKER_ERROR_EXIT, |signal| 128_u8.saturating_add(signal))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stated_process_ceiling_may_only_lower_the_one_the_broker_already_owns() {
        assert_eq!(process_ceiling(None), PROCESSES);
        assert_eq!(process_ceiling(Some(1)), 1);
        assert_eq!(process_ceiling(Some(PROCESSES - 1)), PROCESSES - 1);
        assert_eq!(process_ceiling(Some(PROCESSES)), PROCESSES);
        assert_eq!(process_ceiling(Some(PROCESSES + 1)), PROCESSES);
        assert_eq!(process_ceiling(Some(u64::MAX)), PROCESSES);
    }

    #[test]
    fn a_plan_names_each_ceiling_once_and_the_broker_reads_every_one_of_them() {
        let plan = [
            "--limit-cpu-seconds",
            "3600",
            "--limit-open-files",
            "4096",
            "--limit-processes",
            "64",
            "--",
        ];
        let mut arguments = plan.iter().map(OsString::from);
        let (roots, exclusions, limits) = parse_plan(&mut arguments).expect("a readable plan");
        assert!(roots.is_empty() && exclusions.is_empty());
        assert_eq!(limits.cpu_seconds, Some(3600));
        assert_eq!(limits.open_files, Some(4096));
        assert_eq!(limits.processes, Some(64));
        assert_eq!(limits.memory_bytes, None);

        // Twice is not narrower, it is ambiguous, and the broker refuses rather
        // than deciding which of the two the host meant.
        let repeated = ["--limit-processes", "64", "--limit-processes", "8", "--"];
        assert!(parse_plan(&mut repeated.iter().map(OsString::from)).is_none());

        // Zero would be a ceiling nothing can run under, and an unread flag
        // would be one the host believed it had set.
        for refused in [
            ["--limit-processes", "0", "--"].as_slice(),
            ["--limit-processes", "--"].as_slice(),
            ["--limit-threads", "8", "--"].as_slice(),
        ] {
            assert!(
                parse_plan(&mut refused.iter().map(OsString::from)).is_none(),
                "{refused:?}"
            );
        }
    }
}
