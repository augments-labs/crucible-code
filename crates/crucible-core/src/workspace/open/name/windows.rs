//! Verifying what a by-name Windows open actually reached.
#![allow(
    unsafe_code,
    reason = "Windows exposes a file handle's final path only through GetFinalPathNameByHandleW"
)]

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io;
use std::mem::size_of;
use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::windows::fs::OpenOptionsExt as _;
use std::os::windows::io::AsRawHandle as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use windows_sys::Win32::Foundation::{GENERIC_WRITE, HANDLE};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_ADD_FILE, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS, FILE_LIST_DIRECTORY,
    FILE_NAME_NORMALIZED, FILE_READ_ATTRIBUTES, FILE_RENAME_INFO, FILE_RENAME_INFO_0,
    FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, FileDispositionInfo,
    FileRenameInfo, GetFinalPathNameByHandleW, SYNCHRONIZE, SetFileInformationByHandle,
};

use super::WorkspacePath;

/// Distinguishes private names made by concurrent creations.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Refuses a handle whose resolved destination is not the proven path.
pub(super) fn validate(file: &File, expected: &WorkspacePath) -> Result<(), super::PathError> {
    validate_path(file, expected.as_path()).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            expected.swapped()
        } else {
            expected.unopened(source)
        }
    })
}

/// Refuses a handle whose final path differs from the name that was proven.
fn validate_path(file: &File, expected: &Path) -> io::Result<()> {
    let mut buffer = Vec::<u16>::new();
    let handle = file.as_raw_handle() as HANDLE;

    loop {
        let capacity = u32::try_from(buffer.capacity()).unwrap_or(u32::MAX);
        // SAFETY: `handle` belongs to the live `File`. When capacity is zero a
        // null buffer asks Windows for the required size; otherwise spare
        // capacity is writable for exactly the count passed and is not read by
        // Rust until `set_len` after the call reports how much it initialized.
        let written = unsafe {
            GetFinalPathNameByHandleW(
                handle,
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

        // SAFETY: a successful call initialized `written` UTF-16 code units in
        // the allocation, excluding its terminator.
        unsafe { buffer.set_len(written as usize) };
        break;
    }

    let reached = PathBuf::from(OsString::from_wide(&buffer));
    if reached != expected {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "the opened handle reached another path",
        ));
    }
    Ok(())
}

/// Creates privately, then claims the destination under its proven parent.
pub(super) fn created(path: &WorkspacePath) -> Result<File, super::PathError> {
    let parent_path = path
        .as_path()
        .parent()
        .ok_or_else(|| path.unopened(io::Error::other("the destination has no parent")))?;
    let leaf = path
        .as_path()
        .file_name()
        .ok_or_else(|| path.unopened(io::Error::other("the destination has no file name")))?;

    let parent = opened_parent(parent_path).map_err(|source| path.unopened(source))?;
    validate_path(&parent, parent_path).map_err(|_| path.swapped())?;
    let (temporary_path, file) = temporary(parent_path).map_err(|source| path.unopened(source))?;

    if validate_path(&file, &temporary_path).is_err() {
        delete(&file);
        return Err(path.swapped());
    }
    if let Err(source) = rename(&file, &parent, leaf) {
        delete(&file);
        return Err(if source.kind() == io::ErrorKind::AlreadyExists {
            path.swapped()
        } else {
            path.unopened(source)
        });
    }

    // The relative rename is the commit. Validate the returned handle too,
    // before a caller can write a byte, and remove the empty file by that same
    // handle if the platform reports an unexpected final name.
    if validate(&file, path).is_err() {
        delete(&file);
        return Err(path.swapped());
    }
    Ok(file)
}

/// Opens the destination directory with the rights relative rename needs.
fn opened_parent(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .access_mode(
            FILE_LIST_DIRECTORY
                | FILE_ADD_FILE
                | FILE_TRAVERSE
                | FILE_READ_ATTRIBUTES
                | SYNCHRONIZE,
        )
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    options.open(path)
}

/// Creates one unshared empty file beside the intended destination.
fn temporary(parent: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..128 {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".crucible-creating-{}-{number}",
            std::process::id()
        ));
        let mut options = OpenOptions::new();
        options
            .write(true)
            .create_new(true)
            .access_mode(GENERIC_WRITE | DELETE | FILE_READ_ATTRIBUTES | SYNCHRONIZE)
            // Exclusive while private, so marking it for deletion cannot be
            // delayed by a handle another process opened after this one.
            .share_mode(0);
        match options.open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {}
            Err(problem) => return Err(problem),
        }
    }

    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused private creation name was available",
    ))
}

/// Renames the private handle below `parent` without replacing an existing name.
fn rename(file: &File, parent: &File, leaf: &OsStr) -> io::Result<()> {
    let name: Vec<u16> = leaf.encode_wide().chain(Some(0)).collect();
    let name_bytes = name
        .len()
        .saturating_sub(1)
        .checked_mul(size_of::<u16>())
        .ok_or_else(|| io::Error::other("the destination name is too long"))?;
    // Windows validates this buffer against the padded C structure size, not
    // merely the offset of its trailing array. Add the bytes beyond the one
    // UTF-16 unit already represented by `FILE_RENAME_INFO`; using the smaller
    // offset-based size is rejected with `ERROR_INVALID_PARAMETER` on Win32.
    let bytes = size_of::<FILE_RENAME_INFO>()
        .checked_add(name_bytes)
        .ok_or_else(|| io::Error::other("the rename request is too large"))?;
    let mut storage = vec![0_usize; bytes.div_ceil(size_of::<usize>())];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();

    // SAFETY: `storage` is pointer-aligned and large enough for the fixed
    // fields plus all copied UTF-16 units. Both handles remain live.
    unsafe {
        std::ptr::addr_of_mut!((*info).Anonymous).write(FILE_RENAME_INFO_0 {
            ReplaceIfExists: false,
        });
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

    let size = u32::try_from(bytes).map_err(|_| io::Error::other("rename request too large"))?;
    // SAFETY: `info` describes the initialized buffer above, and `file` was
    // opened with DELETE access for this handle-relative rename.
    let renamed = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileRenameInfo,
            info.cast(),
            size,
        )
    };
    if renamed == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Marks one private or committed empty file for deletion by its live handle.
fn delete(file: &File) {
    let disposition = FILE_DISPOSITION_INFO { DeleteFile: true };
    // SAFETY: `file` is live and was opened with DELETE access.
    unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle() as HANDLE,
            FileDispositionInfo,
            std::ptr::from_ref(&disposition).cast(),
            u32::try_from(size_of::<FILE_DISPOSITION_INFO>()).unwrap_or(u32::MAX),
        )
    };
}
