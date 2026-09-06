//! Entry point of the platform sandbox broker binary.
//!
//! On Linux the broker is namespace PID 1 for one Bubblewrap-confined command.
//! On macOS it is the narrow pre-Seatbelt launcher. On Windows it owns explicit
//! administrator setup state and, after setup, the restricted-account launch
//! boundary. Every platform refuses modes owned by another platform.

#[cfg(not(target_os = "windows"))]
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

#[cfg(target_os = "windows")]
#[allow(
    unsafe_code,
    reason = "the Windows entry point reads its unbuffered inherited input handle and preserves the native 32-bit workload exit status"
)]
mod windows_entry {
    use std::io::Write as _;
    use std::os::windows::io::{FromRawHandle as _, RawHandle};

    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::System::Console::{GetStdHandle, STD_INPUT_HANDLE};
    use windows_sys::Win32::System::Threading::ExitProcess;

    pub(super) fn run() -> ! {
        let code = execute();
        let _ = std::io::stdout().flush();
        let _ = std::io::stderr().flush();
        // SAFETY: every owned launch resource has been dropped and both output
        // streams were flushed. The native API preserves all 32 status bits.
        unsafe { ExitProcess(code) }
    }

    fn execute() -> u32 {
        let arguments: Vec<_> = std::env::args_os().skip(1).collect();
        let launch = match arguments.as_slice() {
            [mode] if mode == crucible_sandbox_broker::WINDOWS_LAUNCH_MODE => {
                Some(input().and_then(|mut source| {
                    crucible_sandbox_broker::launch_windows_sandbox(&mut *source)
                }))
            }
            [mode] if mode == crucible_sandbox_broker::WINDOWS_CHILD_MODE => {
                Some(input().and_then(|mut source| {
                    crucible_sandbox_broker::launch_windows_sandbox_child(&mut *source)
                }))
            }
            _ => None,
        };
        if let Some(result) = launch {
            return match result {
                Ok(code) => code,
                Err(source) => refuse(&format!("{source}\n")),
            };
        }
        match crucible_sandbox_broker::run_windows_broker(arguments.into_iter()) {
            Ok(message) => {
                let _ = writeln!(std::io::stdout().lock(), "{message}");
                0
            }
            Err(source) => refuse(&format!("{source}\n")),
        }
    }

    fn input() -> std::io::Result<std::mem::ManuallyDrop<std::fs::File>> {
        // SAFETY: reads the process-global standard input slot.
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        // The process owns its standard handle. ManuallyDrop prevents File
        // from closing that borrowed slot while providing an unbuffered read,
        // so command bytes cannot be prefetched past the launch frame.
        // SAFETY: the handle remains live through this synchronous operation.
        Ok(std::mem::ManuallyDrop::new(unsafe {
            std::fs::File::from_raw_handle(handle as RawHandle)
        }))
    }

    fn refuse(message: &str) -> u32 {
        let _ = std::io::stderr().write_all(message.as_bytes());
        u32::from(super::BROKER_ERROR_EXIT)
    }
}

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
fn main() {
    windows_entry::run()
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
fn main() -> ExitCode {
    refuse("crucible-sandbox-broker has no confinement role on this platform\n")
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn refuse(message: &str) -> ExitCode {
    use std::io::Write as _;

    let _ = std::io::stderr().write_all(message.as_bytes());
    ExitCode::from(BROKER_ERROR_EXIT)
}
