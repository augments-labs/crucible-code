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
use std::process::{Command, ExitCode};

use crucible_sandbox_broker::{
    BROKER_FAILURE_STATUS, GO_FRAME, READY_FRAME, REFUSED_DESCRIPTOR_CLOSURE, REFUSED_FRAME,
    REFUSED_SCAN, encode_wait_status,
};
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

    let status = match Command::new(program)
        .args(workload_arguments)
        .process_group(0)
        .spawn()
    {
        Ok(mut child) => match child.wait() {
            Ok(status) => status,
            Err(_) => return write_failure(&mut status_channel),
        },
        Err(_) => return write_failure(&mut status_channel),
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
