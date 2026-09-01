//! Minimal namespace PID 1 for exactly one confined workload.
#![allow(
    unsafe_code,
    reason = "the broker takes ownership of host-supplied descriptors and closes every undeclared one before GO"
)]

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

const BROKER_ERROR_EXIT: u8 = 125;

fn main() -> ExitCode {
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
    let (roots, exclusions) = parse_plan(&mut arguments)?;
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

    let Ok(status) = Command::new(program)
        .args(workload_arguments)
        .process_group(0)
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
        if let Some(status) = child.try_wait()? {
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
) -> Option<(Vec<PathBuf>, Vec<PathBuf>)> {
    let mut roots = Vec::new();
    let mut exclusions = Vec::new();
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
            "--" => return Some((roots, exclusions)),
            _ => return None,
        }
    }
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
