//! Machine-wide serialization for Windows sandbox maintenance and launches.
//!
//! The configured owner retains access when running without administrator
//! membership; maintenance for another owner uses that explicit owner SID.

use std::io;

use windows_sys::Win32::Foundation::{WAIT_ABANDONED, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::System::Threading::{CreateMutexW, ReleaseMutex, WaitForSingleObject};

use super::identity::SetupIdentity;
use super::winutil::{OwnedHandle, SecurityDescriptor, error, sid_string, wide};

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
        let owner = sid_string(&identity.owner_sid)?;
        let descriptor =
            SecurityDescriptor::from_sddl(format!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GA;;;{owner})"))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Security::{
        CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, ImpersonateLoggedOnUser,
        RevertToSelf, SID_AND_ATTRIBUTES, TOKEN_DUPLICATE, TOKEN_QUERY,
        WinBuiltinAdministratorsSid,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct Revert;

    impl Drop for Revert {
        fn drop(&mut self) {
            // SAFETY: this guard is created only after successful thread
            // impersonation and restores that same thread before returning.
            assert_ne!(unsafe { RevertToSelf() }, 0);
        }
    }

    /// Administrative setup and normal launches share the mutex. The host
    /// owner's SID must work even when the Administrators SID is deny-only.
    #[test]
    fn the_owner_keeps_mutex_access_without_administrator_membership() {
        let owner = super::super::winutil::current_user_sid().expect("owner SID");
        let identity = SetupIdentity::for_owner(&owner);
        // Retain the object: creating a fresh mutex under impersonation would
        // hand its creator a handle without checking an existing object's DACL.
        let _original = SetupLock::acquire(&identity).expect("administrative owner lock");
        let mut token = null_mut();
        assert_ne!(
            // SAFETY: the current-process pseudo-handle is valid and token
            // points to live output storage; the result is owned below.
            unsafe {
                OpenProcessToken(
                    GetCurrentProcess(),
                    TOKEN_DUPLICATE | TOKEN_QUERY,
                    &raw mut token,
                )
            },
            0
        );
        let token = OwnedHandle::new(token, "test token").expect("token");
        let mut sid = [0_u8; 256];
        let mut length = u32::try_from(sid.len()).expect("SID bound");
        assert_ne!(
            // SAFETY: sid has the supplied capacity and length is writable;
            // this built-in SID requires no domain SID.
            unsafe {
                CreateWellKnownSid(
                    WinBuiltinAdministratorsSid,
                    null_mut(),
                    sid.as_mut_ptr().cast(),
                    &raw mut length,
                )
            },
            0
        );
        let disabled = SID_AND_ATTRIBUTES {
            Sid: sid.as_mut_ptr().cast(),
            Attributes: 0,
        };
        let mut restricted = null_mut();
        assert_ne!(
            // SAFETY: token, disabled and its SID remain live for this call;
            // the zero-count lists are null and restricted is writable output.
            unsafe {
                CreateRestrictedToken(
                    token.get(),
                    DISABLE_MAX_PRIVILEGE,
                    1,
                    &raw const disabled,
                    0,
                    null(),
                    0,
                    null(),
                    &raw mut restricted,
                )
            },
            0
        );
        let restricted = OwnedHandle::new(restricted, "test limited token").expect("limited token");
        // SAFETY: restricted owns a live token; the guard immediately below
        // restores this thread's identity before the token is closed.
        assert_ne!(unsafe { ImpersonateLoggedOnUser(restricted.get()) }, 0);
        let _impersonation = Revert;
        let limited = SetupLock::acquire(&identity);
        assert!(
            limited.is_ok(),
            "ordinary owner could not open the setup mutex: {}",
            limited
                .err()
                .map_or_else(String::new, |error| error.to_string())
        );
    }
}
