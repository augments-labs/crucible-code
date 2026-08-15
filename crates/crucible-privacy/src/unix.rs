//! Unix owner-only modes.

use std::fs::{File, TryLockError};
use std::io;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, PermissionsExt as _};
use std::path::Path;

use rustix::fs::{Mode, OFlags};

const DIRECTORY: u32 = 0o700;
const FILE: u32 = 0o600;

pub(super) fn directory(path: &Path) -> io::Result<()> {
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(DIRECTORY)
        .create(path)?;
    let directory = opened(
        path,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        0,
    )?;
    narrow(&directory, DIRECTORY).map(drop)
}

pub(super) fn append(path: &Path) -> io::Result<File> {
    let file = opened(
        path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::APPEND | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        FILE,
    )?;
    narrow(&file, FILE)?;
    Ok(file)
}

pub(super) fn open_read(path: &Path) -> io::Result<File> {
    let file = opened(path, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, 0)?;
    ordinary(&file)?;
    Ok(file)
}

pub(super) fn open_read_append(path: &Path) -> io::Result<File> {
    let file = opened(
        path,
        OFlags::RDWR | OFlags::APPEND | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        0,
    )?;
    ordinary(&file)?;
    Ok(file)
}

pub(super) fn create_append(path: &Path) -> io::Result<File> {
    let file = opened(
        path,
        OFlags::RDWR
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::APPEND
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC,
        FILE,
    )?;
    narrow(&file, FILE)?;
    Ok(file)
}

pub(super) fn single_name(file: &File) -> io::Result<()> {
    let metadata = file.metadata()?;
    if metadata.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state has another filesystem name",
        ));
    }
    Ok(())
}

pub(super) fn tighten_open(file: &File) -> io::Result<bool> {
    narrow(file, FILE)
}

pub(super) fn try_lock_identity(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(problem)) => Err(problem),
    }
}

pub(super) fn unlock_identity(file: &File) -> io::Result<()> {
    file.unlock()
}

pub(super) fn create_write(path: &Path) -> io::Result<File> {
    let file = opened(
        path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        FILE,
    )?;
    narrow(&file, FILE)?;
    Ok(file)
}

pub(super) fn lock(path: &Path) -> io::Result<File> {
    let file = opened(
        path,
        OFlags::RDWR | OFlags::CREATE | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        FILE,
    )?;
    narrow(&file, FILE)?;
    Ok(file)
}

pub(super) fn tighten(path: &Path) -> io::Result<bool> {
    let file = opened(path, OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC, 0)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state is not a regular file",
        ));
    }
    narrow(&file, FILE)
}

pub(super) fn sync_parent(path: &Path) -> io::Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "a path without a parent"))?;
    File::open(parent)?.sync_all()
}

pub(super) fn replace(source: &Path, destination: &Path) -> io::Result<()> {
    std::fs::rename(source, destination)?;
    sync_parent(destination)
}

fn opened(path: &Path, flags: OFlags, mode: u32) -> io::Result<File> {
    rustix::fs::open(path, flags, Mode::from_raw_mode(mode))
        .map(File::from)
        .map_err(|problem| {
            if problem == rustix::io::Errno::LOOP {
                io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "private state cannot be a symbolic link",
                )
            } else {
                problem.into()
            }
        })
}

fn ordinary(file: &File) -> io::Result<()> {
    if !file.metadata()?.is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "private state is not a regular file",
        ));
    }
    single_name(file)
}

fn narrow(file: &File, wanted: u32) -> io::Result<bool> {
    if file.metadata()?.permissions().mode() & 0o777 == wanted {
        return Ok(false);
    }

    file.set_permissions(std::fs::Permissions::from_mode(wanted))?;
    Ok(true)
}
