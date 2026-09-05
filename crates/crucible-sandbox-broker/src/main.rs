//! Entry point of the sandbox broker binary.
//!
//! The broker is Linux-only: it is namespace PID 1 for one Bubblewrap-confined
//! command. On macOS the same separately packaged, host-owned executable is the
//! narrow pre-Seatbelt launcher: it closes inherited descriptors, installs
//! hard limits and replaces itself with `/usr/bin/sandbox-exec`. Other uses
//! refuse before starting a workload.

use std::process::ExitCode;

#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "the broker takes ownership of host-supplied descriptors and closes every undeclared one before GO"
)]
mod broker;

#[cfg(target_os = "macos")]
#[allow(
    unsafe_code,
    reason = "the single-threaded launcher closes raw inherited descriptors before exec"
)]
mod macos;

/// Exit status when the broker could not even reach its status channel.
const BROKER_ERROR_EXIT: u8 = 125;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    broker::supervise()
}

#[cfg(target_os = "macos")]
fn main() -> ExitCode {
    if std::env::args_os().nth(1).as_deref()
        == Some(std::ffi::OsStr::new(
            crucible_sandbox_broker::MACOS_LAUNCH_MODE,
        ))
    {
        macos::launch()
    } else {
        refuse("crucible-sandbox-broker requires its macOS launcher protocol\n")
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn main() -> ExitCode {
    refuse("crucible-sandbox-broker has no confinement role on this platform\n")
}

#[cfg(not(target_os = "linux"))]
fn refuse(message: &str) -> ExitCode {
    use std::io::Write as _;

    let _ = std::io::stderr().write_all(message.as_bytes());
    ExitCode::from(BROKER_ERROR_EXIT)
}
