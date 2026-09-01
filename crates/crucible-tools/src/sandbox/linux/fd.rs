//! Race-free descriptor isolation and Bubblewrap mount-source inheritance.
#![allow(
    unsafe_code,
    reason = "Linux descriptor allowlisting requires child-only close_range and Command::pre_exec"
)]

use std::io;
use std::os::fd::{BorrowedFd, RawFd};
use std::os::unix::process::CommandExt as _;
use std::process::Command;

use crucible_core::{SandboxError, SandboxFeature};
use rustix::io::FdFlags;

/// Linux `CLOSE_RANGE_CLOEXEC`; stable since Linux 5.11.
const CLOSE_RANGE_CLOEXEC: i32 = 1 << 2;

unsafe extern "C" {
    /// glibc 2.34 is Crucible's Linux floor. The wrapper returns `-1` and sets
    /// `errno` when the running kernel lacks this operation.
    fn close_range(first: u32, last: u32, flags: i32) -> i32;
}

/// Closes every ambient descriptor on exec except explicit mount sources.
///
/// Changing flags in the parent would create a process-wide inheritance race.
/// The pre-exec hook runs after `fork` in the child, first applying
/// `CLOEXEC` to the complete descriptor range atomically and then clearing it
/// only on the bounded allowlist. Both operations are async-signal-safe system
/// calls. An older kernel fails spawn instead of silently weakening isolation.
pub(super) fn inherit(command: &mut Command, descriptors: &[RawFd]) -> Result<(), SandboxError> {
    if descriptors.iter().any(|descriptor| *descriptor <= 2) {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::DescriptorIsolation,
        });
    }
    let descriptors = descriptors.to_vec().into_boxed_slice();
    // SAFETY: the closure performs no allocation, locking, or non-signal-safe
    // library work after `fork`; `close_range` and `fcntl` are direct system
    // boundaries. Each allowlisted raw descriptor is backed by an `OwnedFd`
    // retained by `LinuxSession` until `Command::spawn` returns.
    unsafe {
        command.pre_exec(move || {
            // SAFETY: this changes descriptor flags in the forked child only;
            // the range excludes stdin/stdout/stderr and includes no memory.
            if close_range(3, u32::MAX, CLOSE_RANGE_CLOEXEC) == -1 {
                return Err(io::Error::last_os_error());
            }
            for descriptor in &descriptors {
                // SAFETY: the owning descriptor remains live through spawn and
                // every value was checked to exclude the standard descriptors.
                let descriptor = BorrowedFd::borrow_raw(*descriptor);
                rustix::io::fcntl_setfd(descriptor, FdFlags::empty()).map_err(io::Error::from)?;
            }
            Ok(())
        });
    }
    Ok(())
}
