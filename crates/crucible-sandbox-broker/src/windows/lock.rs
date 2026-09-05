//! Machine-wide serialization for Windows sandbox maintenance.

use std::io;

use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

use super::identity::SetupIdentity;
use super::winutil::{OwnedHandle, SecurityDescriptor, error, wide};

const LOCK_TIMEOUT_MILLIS: u32 = 30_000;

pub(super) struct SetupLock {
    handle: OwnedHandle,
}

impl SetupLock {
    pub(super) fn acquire(identity: &SetupIdentity) -> io::Result<Self> {
        let suffix = identity
            .registry_subkey
            .rsplit('\\')
            .next()
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid Windows sandbox setup identity",
                )
            })?;
        let name = wide(format!(
            "Global\\AugmentsLabs.Crucible.Sandbox.Setup.{suffix}"
        ));
        let descriptor = SecurityDescriptor::from_sddl("D:P(A;;GA;;;SY)(A;;GA;;;BA)")?;
        let attributes = SECURITY_ATTRIBUTES {
            nLength: u32::try_from(size_of::<SECURITY_ATTRIBUTES>()).map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "invalid security attributes size",
                )
            })?,
            lpSecurityDescriptor: descriptor.as_ptr(),
            bInheritHandle: 0,
        };
        // SAFETY: the name and security descriptor remain live for this call;
        // the non-inheritable returned handle is uniquely owned below.
        let handle = OwnedHandle::new(
            unsafe { CreateMutexW(&raw const attributes, 0, name.as_ptr()) },
            "CreateMutexW",
        )?;
        // SAFETY: `handle` owns a live mutex handle. A finite wait prevents a
        // stuck maintenance process from blocking future repair indefinitely.
        let wait = unsafe { WaitForSingleObject(handle.get(), LOCK_TIMEOUT_MILLIS) };
        match wait {
            WAIT_OBJECT_0 | WAIT_ABANDONED => Ok(Self { handle }),
            WAIT_TIMEOUT => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "another Windows sandbox maintenance command is still running",
            )),
            WAIT_FAILED => Err(error("WaitForSingleObject")),
            other => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("WaitForSingleObject returned unexpected status {other}"),
            )),
        }
    }
}

impl Drop for SetupLock {
    fn drop(&mut self) {
        // SAFETY: successful acquisition transferred mutex ownership to this
        // guard, which releases it exactly once before closing the handle.
        unsafe {
            ReleaseMutex(self.handle.get());
        }
    }
}
