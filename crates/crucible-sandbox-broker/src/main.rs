//! Entry point of the platform sandbox broker binary.
//!
//! On Linux the broker is namespace PID 1 for one Bubblewrap-confined command.
//! On macOS it is the narrow pre-Seatbelt launcher. On Windows it owns explicit
//! administrator setup state and, after setup, the restricted-account launch
//! boundary. Every platform refuses modes owned by another platform.

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

#[cfg(target_os = "windows")]
fn main() -> ExitCode {
    match crucible_sandbox_broker::run_windows_broker(std::env::args_os().skip(1)) {
        Ok(message) => {
            use std::io::Write as _;

            let _ = writeln!(std::io::stdout().lock(), "{message}");
            ExitCode::SUCCESS
        }
        Err(source) => refuse(&format!("{source}\n")),
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn main() -> ExitCode {
    refuse("crucible-sandbox-broker has no confinement role on this platform\n")
}

#[cfg(not(target_os = "linux"))]
fn refuse(message: &str) -> ExitCode {
    use std::io::Write as _;

    let _ = std::io::stderr().write_all(message.as_bytes());
    ExitCode::from(BROKER_ERROR_EXIT)
}
