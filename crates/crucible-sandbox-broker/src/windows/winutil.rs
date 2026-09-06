//! Narrow ownership wrappers for Windows handles, SIDs, and errors.

use std::ffi::{OsStr, c_void};
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, GetLastError, HANDLE, HLOCAL, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    GetLengthSid, GetTokenInformation, IsValidSid, LookupAccountNameW, PSECURITY_DESCRIPTOR, PSID,
    SID_NAME_USE, SidTypeUser, TOKEN_ELEVATION, TOKEN_QUERY, TOKEN_USER, TokenElevation, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) fn wide(value: impl AsRef<OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(Some(0)).collect()
}

pub(super) fn error(operation: &str) -> io::Error {
    let source = io::Error::last_os_error();
    io::Error::new(source.kind(), format!("{operation} failed: {source}"))
}

pub(super) fn code_error(operation: &str, code: u32) -> io::Error {
    let source = io::Error::from_raw_os_error(code.cast_signed());
    io::Error::new(source.kind(), format!("{operation} failed: {source}"))
}

pub(super) struct OwnedHandle(HANDLE);

impl OwnedHandle {
    pub(super) fn new(handle: HANDLE, operation: &str) -> io::Result<Self> {
        if handle.is_null() {
            Err(error(operation))
        } else {
            Ok(Self(handle))
        }
    }

    pub(super) fn get(&self) -> HANDLE {
        self.0
    }

    pub(super) fn into_raw(self) -> HANDLE {
        let owned = std::mem::ManuallyDrop::new(self);
        owned.0
    }
}

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper uniquely owns the live kernel handle.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

pub(super) struct LocalMemory(pub(super) *mut c_void);

impl Drop for LocalMemory {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: this wrapper uniquely owns the LocalAlloc allocation.
            unsafe {
                LocalFree(self.0 as HLOCAL);
            }
        }
    }
}

pub(super) struct SecurityDescriptor(LocalMemory);

impl SecurityDescriptor {
    pub(super) fn from_sddl(sddl: impl AsRef<OsStr>) -> io::Result<Self> {
        let sddl = wide(sddl);
        let mut descriptor = null_mut();
        // SAFETY: `sddl` is live and NUL terminated and `descriptor` is
        // initialized out-pointer storage owned by LocalFree on success.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                sddl.as_ptr(),
                SDDL_REVISION_1,
                &raw mut descriptor,
                null_mut(),
            )
        } == 0
        {
            return Err(error(
                "ConvertStringSecurityDescriptorToSecurityDescriptorW",
            ));
        }
        Ok(Self(LocalMemory(descriptor)))
    }

    pub(super) fn as_ptr(&self) -> PSECURITY_DESCRIPTOR {
        self.0.0.cast()
    }
}

pub(super) fn current_process_token() -> io::Result<OwnedHandle> {
    let mut token = null_mut();
    // SAFETY: the current-process pseudo-handle is always valid and `token` is
    // initialized out-handle storage.
    let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) };
    if opened == 0 {
        return Err(error("OpenProcessToken"));
    }
    OwnedHandle::new(token, "OpenProcessToken")
}

pub(super) fn require_elevated() -> io::Result<()> {
    let token = current_process_token()?;
    let mut elevation = TOKEN_ELEVATION::default();
    let mut returned = 0_u32;
    let elevation_size = u32::try_from(size_of::<TOKEN_ELEVATION>())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid token elevation size"))?;
    // SAFETY: the token is live and queryable and both output buffers remain
    // initialized and writable for their advertised sizes.
    let read = unsafe {
        GetTokenInformation(
            token.get(),
            TokenElevation,
            (&raw mut elevation).cast(),
            elevation_size,
            &raw mut returned,
        )
    };
    if read == 0 {
        return Err(error("GetTokenInformation(TokenElevation)"));
    }
    if usize::try_from(returned).ok() != Some(size_of::<TOKEN_ELEVATION>())
        || elevation.TokenIsElevated == 0
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "Windows sandbox setup requires an Administrator PowerShell",
        ));
    }
    Ok(())
}

pub(super) fn current_user_sid() -> io::Result<Vec<u8>> {
    let token = current_process_token()?;
    token_user_sid(token.get())
}

fn token_user_sid(token: HANDLE) -> io::Result<Vec<u8>> {
    let mut needed = 0_u32;
    // SAFETY: the token is live; a null buffer requests the required length.
    unsafe {
        GetTokenInformation(token, TokenUser, null_mut(), 0, &raw mut needed);
    }
    // SAFETY: `GetLastError` reads thread-local state immediately after the
    // failed sizing call above.
    let sizing_error = unsafe { GetLastError() };
    if needed == 0 || sizing_error != ERROR_INSUFFICIENT_BUFFER {
        return Err(error("GetTokenInformation(TokenUser size)"));
    }
    let words = (needed as usize).div_ceil(size_of::<usize>());
    let mut buffer = vec![0_usize; words];
    // SAFETY: `buffer` is aligned and writable for at least `needed` bytes and
    // `needed` is also valid out-length storage.
    let read = unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &raw mut needed,
        )
    };
    if read == 0 {
        return Err(error("GetTokenInformation(TokenUser)"));
    }
    // SAFETY: the successful TokenUser query initialized a TOKEN_USER at the
    // start of this suitably aligned allocation.
    let user = unsafe { &*buffer.as_ptr().cast::<TOKEN_USER>() };
    copy_sid(user.User.Sid)
}

pub(super) fn account_sid(account: &OsStr) -> io::Result<Vec<u8>> {
    let account = wide(account);
    let mut sid_bytes = 0_u32;
    let mut domain_units = 0_u32;
    let mut usage: SID_NAME_USE = 0;
    // SAFETY: the account is NUL terminated; null output buffers request the
    // two required lengths in initialized storage.
    unsafe {
        LookupAccountNameW(
            null(),
            account.as_ptr(),
            null_mut(),
            &raw mut sid_bytes,
            null_mut(),
            &raw mut domain_units,
            &raw mut usage,
        );
    }
    // SAFETY: `GetLastError` reads thread-local state immediately after the
    // sizing call above.
    let sizing_error = unsafe { GetLastError() };
    if sid_bytes == 0 || sizing_error != ERROR_INSUFFICIENT_BUFFER {
        return Err(error("LookupAccountNameW(size)"));
    }
    let mut sid = vec![0_u8; sid_bytes as usize];
    let mut domain = vec![0_u16; domain_units as usize];
    // SAFETY: both buffers are writable for the sizes returned by the sizing
    // call and every in/out counter remains live for this call.
    let found = unsafe {
        LookupAccountNameW(
            null(),
            account.as_ptr(),
            sid.as_mut_ptr().cast(),
            &raw mut sid_bytes,
            domain.as_mut_ptr(),
            &raw mut domain_units,
            &raw mut usage,
        )
    };
    if found == 0 {
        return Err(error("LookupAccountNameW"));
    }
    if usage != SidTypeUser {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "the Windows sandbox owner must be a user account",
        ));
    }
    sid.truncate(sid_bytes as usize);
    // SAFETY: `sid` contains the bytes initialized by LookupAccountNameW.
    if unsafe { IsValidSid(sid.as_mut_ptr().cast()) } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "LookupAccountNameW returned an invalid SID",
        ));
    }
    Ok(sid)
}

pub(super) fn copy_sid(sid: PSID) -> io::Result<Vec<u8>> {
    // SAFETY: the pointer comes from a live TokenUser buffer at the caller.
    if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned an invalid user SID",
        ));
    }
    // SAFETY: the SID was validated immediately above.
    let length = unsafe { GetLengthSid(sid) } as usize;
    if length == 0 || length > 256 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Windows returned an invalid user SID length",
        ));
    }
    // SAFETY: a valid SID is readable for the exact length reported by the OS.
    Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), length) }.to_vec())
}

pub(super) fn sid_string(sid: &[u8]) -> io::Result<String> {
    // SAFETY: the pointer covers the complete caller-owned SID byte slice.
    if sid.is_empty() || unsafe { IsValidSid(sid.as_ptr().cast_mut().cast()) } == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid SID"));
    }
    let mut encoded = null_mut();
    // SAFETY: the SID is valid and `encoded` is initialized out-pointer
    // storage whose LocalAlloc ownership is transferred on success.
    if unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &raw mut encoded) } == 0 {
        return Err(error("ConvertSidToStringSidW"));
    }
    let memory = LocalMemory(encoded.cast());
    let mut length = 0_usize;
    while length < 184 {
        // SAFETY: the conversion contract returns a NUL-terminated LocalAlloc
        // string. A SID string cannot exceed the 184-unit bound.
        if unsafe { *encoded.add(length) } == 0 {
            break;
        }
        length += 1;
    }
    if length == 184 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unbounded SID string",
        ));
    }
    // SAFETY: the preceding scan found the terminator after `length` readable
    // UTF-16 units in the still-live LocalAlloc buffer.
    let result = String::from_utf16(unsafe { std::slice::from_raw_parts(encoded, length) })
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid SID string"));
    drop(memory);
    result
}
