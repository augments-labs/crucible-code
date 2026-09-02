//! Entry point of the sandbox broker binary.
//!
//! The broker is Linux-only: it is namespace PID 1 for one Bubblewrap-confined
//! command. The workspace still builds this binary on every platform, so other
//! operating systems get an entry point that refuses to run rather than a
//! crate that does not compile.

use std::process::ExitCode;

#[cfg(target_os = "linux")]
#[allow(
    unsafe_code,
    reason = "the broker takes ownership of host-supplied descriptors and closes every undeclared one before GO"
)]
mod broker;

/// Exit status when the broker could not even reach its status channel.
const BROKER_ERROR_EXIT: u8 = 125;

#[cfg(target_os = "linux")]
fn main() -> ExitCode {
    broker::supervise()
}

#[cfg(not(target_os = "linux"))]
fn main() -> ExitCode {
    use std::io::Write as _;

    let _ =
        std::io::stderr().write_all(b"crucible-sandbox-broker confines commands only on Linux\n");
    ExitCode::from(BROKER_ERROR_EXIT)
}
