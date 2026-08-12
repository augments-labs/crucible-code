//! An access control list says it.
//!
//! Windows has no file mode. The nearest thing is a list with one entry — full
//! control for the account crucible is running as — and the inheritance from the
//! user profile switched off, so that what the profile hands Administrators and
//! SYSTEM does not reach the transcript merely by being above it. That is
//! stricter than the Unix side, where root reads everything regardless. An
//! administrator can still take ownership and read it, which is the same escape
//! by a longer route, and is the honest limit of what a file permission
//! promises on either system.
//!
//! Inheritance downwards is the other half, and it is what the directory's
//! entry is for. A file created below it is born admitting this account and
//! nobody else, so there is no moment at which a log exists carrying the list
//! the profile would have given it.
//!
//! Written rather than read first. The Unix side compares the mode and calls
//! `chmod` only when it is wrong, because `chmod` fails outright on a filesystem
//! carrying no modes. Reading a list back to compare it costs more than writing
//! one, and this runs once per session, so it simply writes.
#![allow(
    unsafe_code,
    reason = "Setting an access control list is FFI, and the safe wrapper on \
              crates.io has not been published since 2021. This is the one \
              module in the tree that opts in; the workspace lint in Cargo.toml \
              is `deny` rather than `forbid` so that it can, and so that it \
              shows up in a diff when another tries."
)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{SE_FILE_OBJECT, SetNamedSecurityInfoW};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_FLAGS, ACL, ACL_REVISION, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE,
    DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, InitializeAcl,
    OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// Makes the sessions directory, out of every other account's reach.
///
/// Its entry is handed down, which is what makes a log born correct rather than
/// corrected a moment after. See [`log`].
pub(in crate::session) fn directory(path: &Path) -> Result<(), io::Error> {
    std::fs::create_dir_all(path)?;

    narrow(path, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
}

/// Opens a log for appending, making it if it is not there, out of reach.
pub(in crate::session) fn log(path: &Path) -> Result<File, io::Error> {
    // The list this account is admitted by is already on the file when the call
    // above returns, because it came down from the directory. That matters for
    // the gap between these two lines: a process killed inside it leaves a log
    // whose list is right, rather than one carrying whatever the user profile
    // hands Administrators and SYSTEM — kept forever, since a session nothing
    // continues is never opened again to be repaired.
    let file = OpenOptions::new().create(true).append(true).open(path)?;

    // What is written here is that same entry, said outright rather than
    // handed down. A list that is inherited is recomputed whenever the
    // directory's is written again; one that is protected is the file's own and
    // is left alone.
    narrow(path, 0)?;

    Ok(file)
}

/// Opens the mark beside a log, making it if it is not there, out of reach.
///
/// Given the same list as the log beside it: it is named for a session and sits
/// in the same directory, so what it gives away by existing is what the log
/// gives away.
///
/// Opened to read and write rather than to append, which is the one way it
/// differs from [`log`]. Nothing is ever written through the handle, and the
/// access is not for writing — `LockFileEx` takes a lock only on a handle
/// carrying `GENERIC_READ` or `GENERIC_WRITE`, and a handle opened to append
/// carries neither. Asked for a lock, it fails with something that is not the
/// refusal a held file gives, so the caller reads it as a filesystem that
/// cannot lock at all and continues over a session that is still open.
pub(in crate::session) fn mark(path: &Path) -> Result<File, io::Error> {
    // Never truncated. A mark holds nothing, so there is nothing in one to
    // lose — but the mark another crucible is holding this instant is a file
    // this call opens, and a call that opens it is not a call that should be
    // able to write to it at all.
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;

    narrow(path, 0)?;

    Ok(file)
}

/// Puts one entry on `path`, for this account: inheriting nothing from above,
/// and handed down to whatever `handed_down` names.
fn narrow(path: &Path, handed_down: ACE_FLAGS) -> Result<(), io::Error> {
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    name.push(0);

    let user = Sid::current()?;
    let mut list = Acl::granting(user.as_psid(), handed_down)?;

    // SAFETY: `name` is nul-terminated and outlives the call, and `list` is an
    // initialized list whose header records the length actually allocated for
    // it. The first two nulls are the owner and the group, which this does not
    // set, and the last is the audit list, which it does not set either.
    let status = unsafe {
        SetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            list.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };

    if status == ERROR_SUCCESS {
        return Ok(());
    }

    Err(io::Error::from_raw_os_error(
        i32::try_from(status).unwrap_or(-1),
    ))
}

/// This process's own user.
struct Sid(Vec<u64>);

impl Sid {
    /// Reads it off the process token.
    ///
    /// `GetTokenInformation` will not say how much room it needs until it has
    /// refused once, so it is called twice: the first call is the question and
    /// the second is the answer. The buffer is `u64` rather than `u8` because
    /// what lands in it is a struct holding a pointer, and a vector of bytes is
    /// not aligned to hold one.
    fn current() -> Result<Self, io::Error> {
        let token = Token::open()?;
        let mut wanted = 0_u32;

        // SAFETY: a null buffer of length zero is how this call is asked for a
        // size. It writes `wanted` and touches nothing else, and it is meant to
        // fail — the size is the answer, not the return value.
        unsafe {
            GetTokenInformation(token.0, TokenUser, std::ptr::null_mut(), 0, &raw mut wanted);
        }

        let words = usize::try_from(wanted)
            .unwrap_or(0)
            .div_ceil(size_of::<u64>());
        if words == 0 {
            return Err(io::Error::last_os_error());
        }

        let mut held = vec![0_u64; words];

        // SAFETY: `held` is at least `wanted` bytes, which is the size the call
        // above asked for, and is aligned for what is written into it.
        let answered = unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                held.as_mut_ptr().cast::<c_void>(),
                wanted,
                &raw mut wanted,
            )
        };

        if answered == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self(held))
    }

    /// The identifier inside it.
    fn as_psid(&self) -> PSID {
        // SAFETY: the buffer holds a `TOKEN_USER` written by the call above,
        // whose first field carries this pointer. What it points at sits in the
        // same buffer, so it lives exactly as long as `self` does.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

/// This process's access token, closed when it goes out of scope.
struct Token(HANDLE);

impl Token {
    /// Opens it for reading.
    fn open() -> Result<Self, io::Error> {
        let mut handle: HANDLE = std::ptr::null_mut();

        // SAFETY: `GetCurrentProcess` hands back a pseudo-handle that needs no
        // closing, and `handle` is written only when this succeeds.
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut handle) };

        if opened == 0 {
            return Err(io::Error::last_os_error());
        }

        Ok(Self(handle))
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: opened by `open`, closed once here, and not used after.
        unsafe { CloseHandle(self.0) };
    }
}

/// An access control list holding one entry and nothing else.
struct Acl(Vec<u32>);

impl Acl {
    /// Full control for `sid`, and no other entry, handed down as
    /// `handed_down` says.
    fn granting(sid: PSID, handed_down: ACE_FLAGS) -> Result<Self, io::Error> {
        // SAFETY: `sid` points into a token buffer this process owns and which
        // outlives this call.
        let identifier = unsafe { GetLengthSid(sid) };

        // The header, one entry, and the identifier that entry ends with — less
        // the placeholder word the entry declares where that identifier starts.
        let bytes = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>()
            + usize::try_from(identifier).unwrap_or(0);
        let length = u32::try_from(bytes)
            .map_err(|_| io::Error::other("an access control list larger than a word"))?;

        // `u32` rather than `u8`, for the same reason the token buffer is not
        // bytes: a list is read as words and has to be aligned as one.
        let mut held = vec![0_u32; bytes.div_ceil(size_of::<u32>())];
        let list = held.as_mut_ptr().cast::<ACL>();

        // SAFETY: `list` points at `length` zeroed bytes, aligned for a list.
        if unsafe { InitializeAcl(list, length, ACL_REVISION) } == 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: `list` was initialized above with room for exactly this one
        // entry and the identifier it carries.
        if unsafe { AddAccessAllowedAceEx(list, ACL_REVISION, handed_down, FILE_ALL_ACCESS, sid) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }

        Ok(Self(held))
    }

    /// The list itself.
    fn as_mut_ptr(&mut self) -> *mut ACL {
        self.0.as_mut_ptr().cast::<ACL>()
    }
}
