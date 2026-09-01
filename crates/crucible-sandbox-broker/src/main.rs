//! Minimal namespace PID 1 for exactly one confined workload.
#![allow(
    unsafe_code,
    reason = "the broker takes ownership of one host-supplied status descriptor"
)]

use std::ffi::OsStr;
use std::fs::File;
use std::io::Write as _;
use std::os::fd::{FromRawFd as _, RawFd};
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Command, ExitCode};

use crucible_sandbox_broker::{BROKER_FAILURE_STATUS, encode_wait_status};
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
    if arguments.next()?.to_str()? != "--" {
        return None;
    }
    let program = arguments.next()?;
    let workload_arguments = arguments.collect::<Vec<_>>();

    // SAFETY: the host passes one live, uniquely owned descriptor above stderr.
    // Ownership is transferred to this process, and File closes it exactly once.
    let mut status_channel = unsafe { File::from_raw_fd(descriptor) };
    if rustix::io::fcntl_setfd(&status_channel, FdFlags::CLOEXEC).is_err() {
        return write_failure(&mut status_channel);
    }

    let status = match Command::new(program).args(workload_arguments).spawn() {
        Ok(mut child) => match child.wait() {
            Ok(status) => status,
            Err(_) => return write_failure(&mut status_channel),
        },
        Err(_) => return write_failure(&mut status_channel),
    };
    if status_channel
        .write_all(&encode_wait_status(status.into_raw()))
        .and_then(|()| status_channel.flush())
        .is_err()
    {
        return None;
    }
    Some(exit_code(status.code(), status.signal()))
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

fn exit_code(code: Option<i32>, signal: Option<i32>) -> u8 {
    if let Some(code) = code.and_then(|code| u8::try_from(code).ok()) {
        return code;
    }
    signal
        .and_then(|signal| u8::try_from(signal).ok())
        .map_or(BROKER_ERROR_EXIT, |signal| 128_u8.saturating_add(signal))
}
