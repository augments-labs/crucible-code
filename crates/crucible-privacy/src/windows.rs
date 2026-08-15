//! Windows protected access control lists.
#![allow(
    unsafe_code,
    reason = "Windows exposes protected DACLs and bounded file locks through FFI; this module is their single audited unsafe boundary"
)]

use std::ffi::c_void;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::windows::ffi::OsStrExt as _;
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::windows::io::AsRawHandle as _;
use std::path::Path;

use windows_sys::Win32::Foundation::{CloseHandle, ERROR_LOCK_VIOLATION, ERROR_SUCCESS, HANDLE};
use windows_sys::Win32::Security::Authorization::{
    SE_FILE_OBJECT, SetNamedSecurityInfoW, SetSecurityInfo,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_FLAGS, ACL, ACL_REVISION, AddAccessAllowedAceEx, CONTAINER_INHERIT_ACE,
    DACL_SECURITY_INFORMATION, GetLengthSid, GetTokenInformation, InitializeAcl,
    OBJECT_INHERIT_ACE, PROTECTED_DACL_SECURITY_INFORMATION, PSID, TOKEN_QUERY, TOKEN_USER,
    TokenUser,
};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
    LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx, MOVEFILE_REPLACE_EXISTING,
    MOVEFILE_WRITE_THROUGH, MoveFileExW, UnlockFileEx,
};
use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// One byte at 4 EiB, outside the range of a replayable private file.
const CLAIM_OFFSET_HIGH: u32 = 0x4000_0000;
const CLAIM_OFFSET_LOW: u32 = 0;

pub(super) fn directory(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    reject_reparse(path)?;
    narrow(path, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)
}

pub(super) fn append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    reject_reparse(path)?;
    narrow(path, 0)?;
    Ok(file)
}

pub(super) fn open_read(path: &Path) -> io::Result<File> {
    existing(path, false)
}

pub(super) fn open_read_append(path: &Path) -> io::Result<File> {
    existing(path, true)
}

pub(super) fn create_append(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        // Keep this newly assigned session name stable while its handle is
        // live, matching the existing-file open below.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .open(path)?;
    reject_reparse(path)?;
    narrow(path, 0)?;
    Ok(file)
}

pub(super) fn single_name(file: &File) -> io::Result<()> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: `file` owns a live handle and `information` is initialized
    // storage whose address remains valid for the duration of the call.
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &raw mut information) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if information.nNumberOfLinks != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state has another filesystem name",
        ));
    }
    Ok(())
}

pub(super) fn tighten_open(file: &File) -> io::Result<bool> {
    let user = Sid::current()?;
    let mut list = AccessList::granting(user.as_psid(), 0)?;

    // SAFETY: `file` is a live handle and `list` is initialized for its
    // allocation. The call changes that handle's object, not a mutable name.
    let status = unsafe {
        SetSecurityInfo(
            file.as_raw_handle() as HANDLE,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            list.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if status == ERROR_SUCCESS {
        Ok(false)
    } else {
        Err(io::Error::from_raw_os_error(
            i32::try_from(status).unwrap_or(-1),
        ))
    }
}

pub(super) fn try_lock_identity(file: &File) -> io::Result<bool> {
    let mut position = claim_position();
    // SAFETY: `file` owns a live synchronous handle and `position` is a valid
    // OVERLAPPED value whose offset names the one-byte sentinel range. The
    // fail-immediately flag makes this call non-blocking.
    let locked = unsafe {
        LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &raw mut position,
        )
    };
    if locked != 0 {
        return Ok(true);
    }

    let problem = io::Error::last_os_error();
    if problem.raw_os_error() == i32::try_from(ERROR_LOCK_VIOLATION).ok() {
        Ok(false)
    } else {
        Err(problem)
    }
}

pub(super) fn unlock_identity(file: &File) -> io::Result<()> {
    let mut position = claim_position();
    // SAFETY: this uses the live handle and exact offset/length pair passed to
    // `LockFileEx`; `position` remains valid for the duration of the call.
    if unsafe { UnlockFileEx(file.as_raw_handle() as HANDLE, 0, 1, 0, &raw mut position) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn claim_position() -> OVERLAPPED {
    OVERLAPPED {
        Anonymous: OVERLAPPED_0 {
            Anonymous: OVERLAPPED_0_0 {
                Offset: CLAIM_OFFSET_LOW,
                OffsetHigh: CLAIM_OFFSET_HIGH,
            },
        },
        ..OVERLAPPED::default()
    }
}

pub(super) fn create_write(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new().create_new(true).write(true).open(path)?;
    reject_reparse(path)?;
    narrow(path, 0)?;
    Ok(file)
}

pub(super) fn lock(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    reject_reparse(path)?;
    narrow(path, 0)?;
    Ok(file)
}

pub(super) fn tighten(path: &Path) -> io::Result<bool> {
    reject_reparse(path)?;
    narrow(path, 0)?;
    Ok(false)
}

#[allow(
    clippy::unnecessary_wraps,
    reason = "the shared platform boundary reports a fallible parent sync where one exists"
)]
pub(super) fn sync_parent(_path: &Path) -> io::Result<()> {
    // Windows does not expose the Unix directory-fsync contract through
    // `std`; the file itself has already been flushed before its rename.
    Ok(())
}

pub(super) fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    let source = wide(source);
    let destination = wide(destination);

    // SAFETY: both buffers are nul-terminated and live for the duration of the
    // call. The flags request one replace-over-existing operation and ask the
    // system to complete the move before returning. Windows exposes no
    // separate parent-directory sync here, so this is deliberately narrower
    // than Unix's rename followed by directory fsync.
    let moved = unsafe {
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn existing(path: &Path, writable: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        // Keep the file's name stable while this handle is live. Other readers
        // and writers can still open it and meet the lock on this file.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        // Open a final reparse point itself instead of following it. Validation
        // below is therefore about this handle, not a name checked beforehand.
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if writable {
        // `File::set_len` requires the ordinary write right on Windows.
        // Session recovery positions this handle at the new end immediately
        // after shortening, then one writer owns it for the rest of the run.
        options.write(true);
    }

    let file = options.open(path)?;
    ordinary(&file)?;
    Ok(file)
}

fn ordinary(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state is not one ordinary file",
        ));
    }
    single_name(file)
}

fn reject_reparse(path: &Path) -> io::Result<()> {
    let attributes = std::fs::symlink_metadata(path)?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state cannot be a reparse point",
        ));
    }
    Ok(())
}

fn narrow(path: &Path, inherited: ACE_FLAGS) -> io::Result<()> {
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    name.push(0);
    let user = Sid::current()?;
    let mut list = AccessList::granting(user.as_psid(), inherited)?;

    // SAFETY: `name` is nul-terminated and `list` is initialized for its
    // allocated length. All buffers outlive this call.
    let status = unsafe {
        SetNamedSecurityInfoW(
            name.as_mut_ptr(),
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

struct Sid(Vec<u64>);

impl Sid {
    fn current() -> io::Result<Self> {
        let token = Token::open()?;
        let mut wanted = 0_u32;

        // SAFETY: a null zero-length buffer is the documented size query.
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

        // SAFETY: `held` has the queried size and alignment for `TOKEN_USER`.
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

    fn as_psid(&self) -> PSID {
        // SAFETY: the buffer contains the `TOKEN_USER` written above.
        unsafe { (*self.0.as_ptr().cast::<TOKEN_USER>()).User.Sid }
    }
}

struct Token(HANDLE);

impl Token {
    fn open() -> io::Result<Self> {
        let mut handle: HANDLE = std::ptr::null_mut();
        // SAFETY: `handle` is an out pointer and the pseudo process handle
        // requires no close of its own.
        let opened = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut handle) };
        if opened == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(handle))
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: this handle was opened once by `Token::open`.
        unsafe { CloseHandle(self.0) };
    }
}

struct AccessList(Vec<u32>);

impl AccessList {
    fn granting(sid: PSID, inherited: ACE_FLAGS) -> io::Result<Self> {
        // SAFETY: `sid` points into the live token buffer.
        let identifier = unsafe { GetLengthSid(sid) };
        let bytes = size_of::<ACL>() + size_of::<ACCESS_ALLOWED_ACE>() - size_of::<u32>()
            + usize::try_from(identifier).unwrap_or(0);
        let length = u32::try_from(bytes)
            .map_err(|_| io::Error::other("an access control list larger than a word"))?;
        let mut held = vec![0_u32; bytes.div_ceil(size_of::<u32>())];
        let list = held.as_mut_ptr().cast::<ACL>();

        // SAFETY: `list` points to `length` aligned and zeroed bytes.
        if unsafe { InitializeAcl(list, length, ACL_REVISION) } == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: the initialized list has room for this entry and SID.
        if unsafe { AddAccessAllowedAceEx(list, ACL_REVISION, inherited, FILE_ALL_ACCESS, sid) }
            == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(held))
    }

    fn as_mut_ptr(&mut self) -> *mut ACL {
        self.0.as_mut_ptr().cast::<ACL>()
    }
}
