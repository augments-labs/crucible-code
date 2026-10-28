//! Windows protected access control lists.
#![allow(
    unsafe_code,
    reason = "Windows exposes protected DACLs, bounded file locks, final paths and handle-relative renames through FFI; this module is their single audited unsafe boundary"
)]

use std::ffi::{OsStr, OsString, c_void};
use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::{MetadataExt as _, OpenOptionsExt as _};
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};

use windows_sys::Wdk::Storage::FileSystem::{
    FILE_INFORMATION_CLASS, FILE_RENAME_INFORMATION, FILE_RENAME_INFORMATION_0,
    FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS, FileRenameInformation,
    FileRenameInformationEx, NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_INVALID_PARAMETER, ERROR_LOCK_VIOLATION,
    ERROR_NOT_SUPPORTED, ERROR_SUCCESS, GENERIC_WRITE, HANDLE, RtlNtStatusToDosError,
};
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
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_REPARSE_POINT,
    FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
    FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FILE_TRAVERSE, FileDispositionInfo, GetFileInformationByHandle,
    GetFinalPathNameByHandleW, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY, LockFileEx,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, READ_CONTROL, SYNCHRONIZE,
    SetFileInformationByHandle, UnlockFileEx, WRITE_DAC,
};
use windows_sys::Win32::System::IO::{IO_STATUS_BLOCK, OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};
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

pub(super) fn open_read_ordinary(path: &Path) -> io::Result<File> {
    // Opened as `reached` opens, except that the name is not held: a file read
    // and not trusted is one another process may replace whole while it is
    // read, and holding its name would refuse that replace rather than leave
    // this reader the file it opened, as Unix does.
    let file = OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    regular(&file)?;
    Ok(file)
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
    narrow_open(file, 0)?;
    Ok(false)
}

/// Makes the object `file` holds owner-only through the handle, so a name
/// changed meanwhile cannot move the list to another object.
fn narrow_open(file: &File, inherited: ACE_FLAGS) -> io::Result<()> {
    let user = Sid::current()?;
    let mut list = AccessList::granting(user.as_psid(), inherited)?;

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
        Ok(())
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
    // A final reparse point is opened as itself rather than followed, so a
    // link to nothing makes nothing where it leads, and what is proved is the
    // handle that opened. The protected list is then written by name, as
    // everywhere in this module.
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    regular(&file)?;
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
    let wide_source = wide(source);
    let wide_destination = wide(destination);

    // SAFETY: both buffers are nul-terminated and live for the duration of the
    // call. The flags request one replace-over-existing operation and ask the
    // system to complete the move before returning. Windows exposes no
    // separate parent-directory sync here, so this is deliberately narrower
    // than Unix's rename followed by directory fsync.
    let moved = unsafe {
        MoveFileExW(
            wide_source.as_ptr(),
            wide_destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved != 0 {
        return Ok(());
    }
    let refused = io::Error::last_os_error();

    // `MoveFileExW` refuses a destination another handle has open, even one
    // opened to allow exactly this: a second crucible reading the store, or
    // anything else looking at the file as it is replaced. Under POSIX
    // semantics the name moves on to the new file and that reader keeps the
    // one it opened, which is what Unix does; `std::fs::rename` asks for
    // them after the same refusal, where the file system supports it. A
    // handle that did not allow deletion still refuses, and that error is
    // the one returned. Write-through is not asked for again: the flush
    // Windows documents for it is that of a move made by copying, and a rename
    // within one directory is never one.
    if refused.raw_os_error() == Some(ERROR_ACCESS_DENIED.cast_signed()) {
        return std::fs::rename(source, destination);
    }
    Err(refused)
}

pub(super) fn replace_in(
    directory: &Path,
    name: &OsStr,
    draft: &OsStr,
    contents: &[u8],
) -> io::Result<()> {
    // Windows creates a file only by name, so the directory is held instead:
    // its handle withholds the delete sharing that renaming or removing the
    // directory needs, so while it is open the name cannot be moved, removed
    // or put back as a link, and the names below reach what this open did.
    let held = held(directory)?;
    narrow_open(&held, OBJECT_INHERIT_ACE | CONTAINER_INHERIT_ACE)?;
    // A reparse point made at `name` after this is a directory entry the
    // rename below replaces, and is never opened.
    match std::fs::symlink_metadata(directory.join(name)) {
        Ok(found) if found.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 => {
            return Err(reparse());
        }
        Ok(_) => {}
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => {}
        Err(problem) => return Err(problem),
    }
    let mut written = Draft::create(&directory.join(draft))?;

    // Setting a reparse point needs no share right, so an empty directory can
    // still become a junction while held. The draft is proved by its handle to
    // be a child of the held directory before a byte is written; once it is,
    // the directory is not empty and cannot become one.
    if final_path(&written.file)? != final_path(&held)?.join(draft) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state left the directory it was written in",
        ));
    }
    narrow_open(&written.file, 0)?;
    written.file.write_all(contents)?;
    written.file.sync_all()?;
    written.commit(&held, name)
}

/// The directory at `path`, opened as itself and held so its name stays
/// where it is, created first where nothing has the name.
fn held(path: &Path) -> io::Result<File> {
    let access = FILE_TRAVERSE | FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC | SYNCHRONIZE;
    let open = || {
        OpenOptions::new()
            .access_mode(access)
            // No delete sharing: the name cannot be renamed or removed while
            // this handle is open. Write sharing is what the rename into it
            // asks for.
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    };
    let held = match open() {
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?;
            open()?
        }
        opened => opened?,
    };
    let metadata = held.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(reparse());
    }
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state is kept in a directory",
        ));
    }
    Ok(held)
}

/// The path the system reaches `file` by now, read from its handle.
fn final_path(file: &File) -> io::Result<PathBuf> {
    let mut buffer = Vec::<u16>::new();
    loop {
        let capacity = u32::try_from(buffer.capacity()).unwrap_or(u32::MAX);
        // SAFETY: the handle is live. A null buffer with zero capacity asks for
        // the size; otherwise the allocation has `capacity` writable units and
        // they are read only after the call reports what it initialized.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as HANDLE,
                if capacity == 0 {
                    std::ptr::null_mut()
                } else {
                    buffer.as_mut_ptr()
                },
                capacity,
                FILE_NAME_NORMALIZED,
            )
        };
        if written == 0 {
            return Err(io::Error::last_os_error());
        }
        if written < capacity {
            // SAFETY: a successful call initialized `written` UTF-16 units.
            unsafe { buffer.set_len(written as usize) };
            return Ok(PathBuf::from(OsString::from_wide(&buffer)));
        }
        let wanted = (written as usize)
            .checked_add(1)
            .ok_or_else(|| io::Error::other("the final path is too long to hold"))?;
        buffer.try_reserve_exact(wanted).map_err(io::Error::other)?;
    }
}

/// An owner-only file being written, removed through its handle unless it
/// was renamed into place.
struct Draft {
    file: File,
    landed: bool,
}

impl Draft {
    fn create(path: &Path) -> io::Result<Self> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .access_mode(
                GENERIC_WRITE
                    | DELETE
                    | READ_CONTROL
                    | WRITE_DAC
                    | FILE_READ_ATTRIBUTES
                    | SYNCHRONIZE,
            )
            // Unshared: nothing else opens what is being written.
            .share_mode(0)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)?;
        Ok(Self {
            file,
            landed: false,
        })
    }

    /// Renames this file to `name` in the directory `held` holds, replacing
    /// whatever entry has that name rather than opening it.
    fn commit(&mut self, held: &File, name: &OsStr) -> io::Result<()> {
        let posix = FILE_RENAME_INFORMATION_0 {
            // Under POSIX semantics a reader holding the old file keeps it
            // while the name moves on, as on Unix.
            Flags: FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS,
        };
        match self.rename(held, name, posix, FileRenameInformationEx) {
            // A file system without POSIX renames refuses the extended
            // request; `std::fs::rename` falls back on the same two errors.
            Err(problem)
                if problem.raw_os_error() == Some(ERROR_INVALID_PARAMETER.cast_signed())
                    || problem.raw_os_error() == Some(ERROR_NOT_SUPPORTED.cast_signed()) =>
            {
                let classic = FILE_RENAME_INFORMATION_0 {
                    ReplaceIfExists: true,
                };
                self.rename(held, name, classic, FileRenameInformation)
            }
            renamed => renamed,
        }?;
        self.landed = true;
        Ok(())
    }

    fn rename(
        &self,
        held: &File,
        name: &OsStr,
        action: FILE_RENAME_INFORMATION_0,
        class: FILE_INFORMATION_CLASS,
    ) -> io::Result<()> {
        let name: Vec<u16> = name.encode_wide().collect();
        let name_bytes = name
            .len()
            .checked_mul(size_of::<u16>())
            .ok_or_else(|| io::Error::other("the private file's name is too long"))?;
        let bytes = size_of::<FILE_RENAME_INFORMATION>()
            .checked_add(name_bytes)
            .ok_or_else(|| io::Error::other("the rename request is too large"))?;
        let length = u32::try_from(name_bytes)
            .map_err(|_| io::Error::other("the private file's name is too long"))?;
        let size = u32::try_from(bytes)
            .map_err(|_| io::Error::other("the rename request is too large"))?;
        let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();

        // SAFETY: `storage` is aligned for every pointer-sized field and holds
        // the fixed prefix plus every UTF-16 unit copied below.
        unsafe {
            std::ptr::addr_of_mut!((*info).Anonymous).write(action);
            std::ptr::addr_of_mut!((*info).RootDirectory).write(held.as_raw_handle() as HANDLE);
            std::ptr::addr_of_mut!((*info).FileNameLength).write(length);
            name.as_ptr().copy_to_nonoverlapping(
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
        }

        let mut status = IO_STATUS_BLOCK::default();
        // SAFETY: `info` describes the initialized buffer above, `status` is
        // writable, this handle has delete access, and both handles stay live
        // through the call. The ntdll boundary honors the held root directory,
        // which the Win32 wrapper refuses.
        let renamed = unsafe {
            NtSetInformationFile(
                self.file.as_raw_handle() as HANDLE,
                &raw mut status,
                info.cast(),
                size,
                class,
            )
        };
        if renamed < 0 {
            // SAFETY: this conversion has no preconditions.
            let code = unsafe { RtlNtStatusToDosError(renamed) };
            return Err(io::Error::from_raw_os_error(
                i32::try_from(code).unwrap_or(i32::MAX),
            ));
        }
        Ok(())
    }
}

impl Drop for Draft {
    fn drop(&mut self) {
        if self.landed {
            return;
        }
        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: the file is still live and was opened with delete access.
        // Deleting through the handle removes this file and no other, whatever
        // its name reaches by now.
        unsafe {
            SetFileInformationByHandle(
                self.file.as_raw_handle() as HANDLE,
                FileDispositionInfo,
                std::ptr::from_ref(&disposition).cast(),
                u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).unwrap_or(u32::MAX),
            )
        };
    }
}

fn reparse() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "private state cannot be a reparse point",
    )
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

fn existing(path: &Path, writable: bool) -> io::Result<File> {
    let file = reached(path, writable)?;
    ordinary(&file)?;
    Ok(file)
}

/// The existing file at `path`, opened as itself rather than through a final
/// reparse point, and not yet proved to be anything.
fn reached(path: &Path, writable: bool) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        // Keep the file's name stable while this handle is live. Other readers
        // and writers can still open it and meet the lock on this file.
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        // Open a final reparse point itself instead of following it. What the
        // caller then proves is therefore about this handle, not a name checked
        // beforehand.
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    if writable {
        // `File::set_len` requires the ordinary write right on Windows.
        // Session recovery positions this handle at the new end immediately
        // after shortening, then one writer owns it for the rest of the run.
        options.write(true);
    }

    options.open(path)
}

/// One ordinary file under one name: what private state has to be.
fn ordinary(file: &File) -> io::Result<()> {
    regular(file)?;
    single_name(file)
}

/// One ordinary file, not a reparse point, however many names reach it.
fn regular(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 || !metadata.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state is not one ordinary file",
        ));
    }
    Ok(())
}

fn reject_reparse(path: &Path) -> io::Result<()> {
    let attributes = std::fs::symlink_metadata(path)?.file_attributes();
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(reparse());
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
