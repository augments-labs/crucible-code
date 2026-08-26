//! Windows descriptor-bound file replacement.
#![allow(
    unsafe_code,
    reason = "Windows exposes final-path validation, handle-relative rename and handle deletion only through its system API"
)]

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions, Permissions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_core::{PathError, WorkspacePath};
use windows_sys::Wdk::Storage::FileSystem::{
    FILE_RENAME_INFORMATION, FILE_RENAME_INFORMATION_0, FILE_RENAME_POSIX_SEMANTICS,
    FILE_RENAME_REPLACE_IF_EXISTS, FileRenameInformation, FileRenameInformationEx,
    NtSetInformationFile,
};
use windows_sys::Win32::Foundation::{GENERIC_WRITE, HANDLE, RtlNtStatusToDosError};
use windows_sys::Win32::Storage::FileSystem::{
    BY_HANDLE_FILE_INFORMATION, DELETE, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_FLAG_WRITE_THROUGH, FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FILE_WRITE_ATTRIBUTES, FileDispositionInfo,
    GetFileInformationByHandle, GetFinalPathNameByHandleW, SYNCHRONIZE, SetFileInformationByHandle,
};
use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

/// Distinguishes temporary names made by concurrent replacements.
static NEXT: AtomicU64 = AtomicU64::new(0);

pub(super) fn replace_with(
    path: &WorkspacePath,
    permissions: Option<Permissions>,
    expected: Option<&File>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), PathError> {
    let expected_parent = path
        .as_path()
        .parent()
        .ok_or_else(|| failure(path, io::Error::other("the destination has no parent")))?;
    let leaf = path
        .as_path()
        .file_name()
        .ok_or_else(|| failure(path, io::Error::other("the destination has no file name")))?;

    // Holding the validated directory is the security boundary. Every name
    // used at commit is relative to this handle, so replacing any ancestor with
    // a directory reparse point after this point cannot redirect the destination.
    let parent = parent(expected_parent).map_err(|source| failure(path, source))?;
    validate(&parent, expected_parent).map_err(|source| swapped(path, source))?;
    let existed = permissions.is_some();
    let mut temporary = temporary(path, expected_parent)?;

    let prepared = write(&mut temporary.file)
        .and_then(|()| match permissions {
            Some(permissions) => temporary.file.set_permissions(permissions),
            None => Ok(()),
        })
        .and_then(|()| temporary.file.sync_all());
    prepared.map_err(|source| failure(path, source))?;

    if let Some(expected) = expected {
        // Adjacent to the commit because Windows has no identity
        // compare-and-rename operation. A later leaf swap can still win this
        // narrow interval, but the rename remains relative to the held parent
        // handle and replaces that leaf rather than following a reparse point.
        let current = path.open_regular().map_err(|_| changed(path))?;
        if !same(&current, expected).unwrap_or(false) {
            return Err(changed(path));
        }
    }

    temporary
        .commit(&parent, leaf, existed)
        .map_err(|source| failure(path, source))?;

    // The handle names the committed file after the rename. A second file
    // flush is the strongest handle-relative guarantee available here; unlike
    // MoveFileExW's write-through flag it does not resolve the destination by
    // a mutable full path, and it does not prove the directory namespace was
    // flushed.
    temporary
        .file
        .sync_all()
        .map_err(|source| PathError::Unsynced {
            at: path.to_string().into(),
            source,
        })?;
    Ok(())
}

/// Whether two handles identify the same file on the same volume.
fn same(left: &File, right: &File) -> io::Result<bool> {
    Ok(identity(left)? == identity(right)?)
}

/// A file identity assigned by its volume and stable for the life of the file.
fn identity(file: &File) -> io::Result<(u32, u64)> {
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: the handle remains live and `information` points to writable
    // storage of exactly the structure the system call initializes.
    let read =
        unsafe { GetFileInformationByHandle(file.as_raw_handle() as HANDLE, &raw mut information) };
    if read == 0 {
        return Err(io::Error::last_os_error());
    }

    let index = u64::from(information.nFileIndexHigh) << 32 | u64::from(information.nFileIndexLow);
    if index == 0 {
        return Err(io::Error::other(
            "the filesystem did not supply a stable file identity",
        ));
    }
    Ok((information.dwVolumeSerialNumber, index))
}

/// Opens the destination directory with only the rights relative rename needs.
fn parent(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .access_mode(FILE_TRAVERSE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    options.open(path)
}

/// A fresh file beside the destination, validated by its opened handle.
fn temporary(path: &WorkspacePath, parent: &Path) -> Result<Temporary, PathError> {
    for _ in 0..128 {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let expected = parent.join(format!(".crucible-writing-{}-{number}", std::process::id()));
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .access_mode(
                GENERIC_WRITE
                    | DELETE
                    | FILE_READ_ATTRIBUTES
                    | FILE_WRITE_ATTRIBUTES
                    | SYNCHRONIZE,
            )
            // No sharing while private: another process cannot read prepared
            // workspace content or hold cleanup open through the temporary
            // name if its ancestor is concurrently moved.
            .share_mode(0)
            .custom_flags(FILE_FLAG_WRITE_THROUGH);

        match options.open(&expected) {
            Ok(file) => {
                let temporary = Temporary {
                    file,
                    landed: false,
                };
                validate(&temporary.file, &expected).map_err(|source| swapped(path, source))?;
                return Ok(temporary);
            }
            Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {}
            Err(problem) => return Err(failure(path, problem)),
        }
    }

    Err(failure(
        path,
        io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no unused temporary name was available",
        ),
    ))
}

/// The final path of one open handle, compared before any bytes can escape it.
fn validate(file: &File, expected: &Path) -> io::Result<()> {
    let mut buffer = Vec::<u16>::new();
    loop {
        let capacity = u32::try_from(buffer.capacity()).unwrap_or(u32::MAX);
        // SAFETY: the handle is live. A null buffer with zero capacity asks for
        // the size; otherwise the allocation has `capacity` writable units and
        // Rust reads them only after the call reports what it initialized.
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
        if written >= capacity {
            let wanted = (written as usize)
                .checked_add(1)
                .ok_or_else(|| io::Error::other("the final path is too long to hold"))?;
            buffer.try_reserve_exact(wanted).map_err(io::Error::other)?;
            continue;
        }

        // SAFETY: a successful call initialized `written` UTF-16 units.
        unsafe { buffer.set_len(written as usize) };
        break;
    }

    let reached = PathBuf::from(OsString::from_wide(&buffer));
    if reached == expected {
        Ok(())
    } else {
        Err(io::Error::other("the opened handle reached another path"))
    }
}

/// A private preparation file, deleted by handle unless its rename commits.
struct Temporary {
    file: File,
    landed: bool,
}

impl Temporary {
    /// Atomically renames this handle under the held destination directory.
    fn commit(&mut self, parent: &File, leaf: &OsStr, replace: bool) -> io::Result<()> {
        let name: Vec<u16> = leaf.encode_wide().collect();
        let name_bytes = name
            .len()
            .checked_mul(size_of::<u16>())
            .ok_or_else(|| io::Error::other("the destination name is too long"))?;
        let bytes = size_of::<FILE_RENAME_INFORMATION>()
            .checked_add(name_bytes)
            .ok_or_else(|| io::Error::other("the rename request is too large"))?;
        let words = bytes.div_ceil(size_of::<usize>());
        let mut storage = vec![0_usize; words];
        let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();

        // SAFETY: `storage` is aligned for every pointer-sized field and holds
        // the fixed prefix plus every UTF-16 unit copied below. The parent and
        // source handles remain live through NtSetInformationFile.
        unsafe {
            let action = if replace {
                FILE_RENAME_INFORMATION_0 {
                    // The identity handle deliberately remains open across
                    // commit. POSIX replacement leaves that handle on the old
                    // file while subsequent opens of the name reach the new
                    // one, even when another Windows opener omitted delete
                    // sharing.
                    Flags: FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS,
                }
            } else {
                FILE_RENAME_INFORMATION_0 {
                    ReplaceIfExists: false,
                }
            };
            std::ptr::addr_of_mut!((*info).Anonymous).write(action);
            std::ptr::addr_of_mut!((*info).RootDirectory).write(parent.as_raw_handle() as HANDLE);
            std::ptr::addr_of_mut!((*info).FileNameLength).write(
                u32::try_from(name_bytes)
                    .map_err(|_| io::Error::other("the destination name is too long"))?,
            );
            name.as_ptr().copy_to_nonoverlapping(
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                name.len(),
            );
        }

        let size = u32::try_from(bytes)
            .map_err(|_| io::Error::other("the rename request is too large"))?;
        let mut status = IO_STATUS_BLOCK::default();
        let class = if replace {
            FileRenameInformationEx
        } else {
            FileRenameInformation
        };
        // SAFETY: `info` describes the initialized buffer above, `status` is
        // writable, and the source handle has DELETE access. The ntdll boundary
        // honors the held RootDirectory; the Win32 wrapper rejects it.
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
            // SAFETY: this conversion has no preconditions and preserves the
            // operating system error category used by the typed tool boundary.
            let code = unsafe { RtlNtStatusToDosError(renamed) };
            return Err(io::Error::from_raw_os_error(
                i32::try_from(code).unwrap_or(i32::MAX),
            ));
        }
        self.landed = true;
        Ok(())
    }
}

impl Drop for Temporary {
    fn drop(&mut self) {
        if self.landed {
            return;
        }

        let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: the file is still live and was opened with DELETE access.
        // Deletion is tied to this handle, so a renamed ancestor cannot make
        // cleanup delete some other writer's file at the old full path.
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

fn failure(path: &WorkspacePath, source: io::Error) -> PathError {
    PathError::Unreplaced {
        at: path.to_string().into(),
        source,
    }
}

fn swapped(path: &WorkspacePath, _source: io::Error) -> PathError {
    PathError::Swapped {
        at: path.to_string().into(),
    }
}

fn changed(path: &WorkspacePath) -> PathError {
    PathError::Changed {
        at: path.to_string().into(),
    }
}
