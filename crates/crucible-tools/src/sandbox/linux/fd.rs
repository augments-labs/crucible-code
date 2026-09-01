//! Race-free descriptor inheritance for Bubblewrap mount sources.
#![allow(
    unsafe_code,
    reason = "Unix exposes child-only close-on-exec changes through Command::pre_exec"
)]

use std::io;
use std::os::fd::{BorrowedFd, RawFd};
use std::os::unix::process::CommandExt as _;
use std::process::Command;

use crucible_core::{SandboxError, SandboxFeature};
use rustix::io::FdFlags;

/// Makes descriptor-backed mount sources survive only the Bubblewrap exec.
///
/// Clearing `CLOEXEC` in the parent would create a process-wide inheritance
/// race. The pre-exec hook runs after `fork` in the child and performs only the
/// async-signal-safe `fcntl` syscall.
pub(super) fn inherit(command: &mut Command, descriptors: &[RawFd]) -> Result<(), SandboxError> {
    if descriptors.iter().any(|descriptor| *descriptor <= 2) {
        return Err(SandboxError::Unsupported {
            feature: SandboxFeature::DescriptorIsolation,
        });
    }
    let descriptors = descriptors.to_vec().into_boxed_slice();
    // SAFETY: the closure performs no allocation, locking, or non-signal-safe
    // library work after `fork`; each raw descriptor is backed by an `OwnedFd`
    // retained by `LinuxSession` until `Command::spawn` returns.
    unsafe {
        command.pre_exec(move || {
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
