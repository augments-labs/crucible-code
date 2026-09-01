//! Descriptor-relative terminal mutations beneath one retained writable root.
//!
//! Every intermediate directory is opened relative to the descriptor that
//! proved the component before it, with symbolic links refused. Mutations then
//! name only one leaf against that opened parent. A concurrent rename can make
//! an operation fail, but cannot redirect it outside the retained authority.

use std::ffi::{OsStr, OsString};
use std::fs::{self, File, FileTimes};
use std::io;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path};
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{AtFlags, Mode, OFlags, RenameFlags};
use rustix::io::Errno;

use super::{Root, copy_file_into};

const DOWN: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const ENTRY: OFlags = OFlags::RDONLY
    .union(OFlags::NONBLOCK)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);
const PRIVATE_FILE: Mode = Mode::RUSR.union(Mode::WUSR);
const PRIVATE_DIRECTORY: Mode = Mode::RUSR.union(Mode::WUSR).union(Mode::XUSR);

static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(0);

pub(super) fn remove(root: &Root, relative: &Path, directory: bool) -> io::Result<()> {
    let (parent, leaf) = parent(root, relative)?;
    let flags = if directory {
        AtFlags::REMOVEDIR
    } else {
        AtFlags::empty()
    };
    rustix::fs::unlinkat(&parent, &leaf, flags)?;
    parent.sync_all()
}

pub(super) fn create_directory(root: &Root, relative: &Path) -> io::Result<()> {
    let (parent, leaf) = parent(root, relative)?;
    rustix::fs::mkdirat(&parent, &leaf, PRIVATE_DIRECTORY)?;
    parent.sync_all()
}

pub(super) fn replace_file(
    root: &Root,
    relative: &Path,
    source: &Path,
    metadata: (u32, Option<std::time::SystemTime>),
    replace: bool,
) -> io::Result<()> {
    let (parent, leaf) = parent(root, relative)?;
    let (temporary, mut output) = temporary_file(&parent)?;
    let prepared = copy_file_into(source, &mut output)
        .and_then(|()| set_metadata(&output, metadata.0, metadata.1))
        .and_then(|()| output.sync_all());
    if let Err(problem) = prepared {
        let _ = rustix::fs::unlinkat(&parent, &temporary, AtFlags::empty());
        return Err(problem);
    }
    commit(&parent, &temporary, &leaf, replace)
}

pub(super) fn replace_root_file(
    root: &Root,
    source: &Path,
    metadata: (u32, Option<std::time::SystemTime>),
) -> io::Result<()> {
    if root.directory {
        return Err(invalid("file publication authority is a directory"));
    }
    let mut output = duplicate_any(root)?;
    if !output.metadata()?.is_file() {
        return Err(invalid("file publication authority changed type"));
    }
    copy_file_into(source, &mut output)?;
    set_metadata(&output, metadata.0, metadata.1)?;
    output.sync_all()
}

pub(super) fn replace_symlink(
    root: &Root,
    relative: &Path,
    target: &OsStr,
    replace: bool,
) -> io::Result<()> {
    let (parent, leaf) = parent(root, relative)?;
    let temporary = temporary_symlink(&parent, target)?;
    commit(&parent, &temporary, &leaf, replace)
}

pub(super) fn hard_link(
    root: &Root,
    anchor: &Path,
    relative: &Path,
    replace: bool,
) -> io::Result<()> {
    let (anchor_parent, anchor_leaf) = parent(root, anchor)?;
    let (target_parent, target_leaf) = parent(root, relative)?;
    if replace {
        rustix::fs::unlinkat(&target_parent, &target_leaf, AtFlags::empty())?;
    }
    rustix::fs::linkat(
        &anchor_parent,
        &anchor_leaf,
        &target_parent,
        &target_leaf,
        AtFlags::empty(),
    )?;
    target_parent.sync_all()
}

pub(super) fn apply_metadata(
    root: &Root,
    relative: &Path,
    mode: u32,
    modified: Option<std::time::SystemTime>,
    directory: bool,
) -> io::Result<()> {
    let file = open(root, relative, directory)?;
    set_metadata(&file, mode, modified)?;
    file.sync_all()
}

pub(super) fn sync(root: &Root) -> io::Result<()> {
    rustix::fs::fsync(&root.authority).map_err(Into::into)
}

fn open(root: &Root, relative: &Path, directory: bool) -> io::Result<File> {
    if relative.as_os_str().is_empty() {
        let file = duplicate_any(root)?;
        let metadata = file.metadata()?;
        if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
            return Err(invalid("publication root changed type"));
        }
        return Ok(file);
    }
    let (parent, leaf) = parent(root, relative)?;
    let flags = if directory {
        ENTRY | OFlags::DIRECTORY
    } else {
        ENTRY
    };
    let file = File::from(rustix::fs::openat(&parent, &leaf, flags, Mode::empty())?);
    let metadata = file.metadata()?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err(invalid(
            "publication entry changed type during descriptor descent",
        ));
    }
    Ok(file)
}

fn parent(root: &Root, relative: &Path) -> io::Result<(File, OsString)> {
    if !root.directory {
        return Err(invalid(
            "descriptor-relative descent requires a directory authority",
        ));
    }
    if relative.is_absolute() || relative.as_os_str().is_empty() {
        return Err(invalid("publication path is not a nonempty relative path"));
    }
    let mut directory = duplicate(root)?;
    let parent = relative.parent().unwrap_or_else(|| Path::new(""));
    for component in parent.components() {
        let Component::Normal(name) = component else {
            return Err(invalid("publication path contains a non-normal component"));
        };
        directory = File::from(rustix::fs::openat(&directory, name, DOWN, Mode::empty())?);
    }
    let leaf = relative
        .file_name()
        .filter(|leaf| !leaf.is_empty())
        .ok_or_else(|| invalid("publication path has no leaf component"))?
        .to_owned();
    Ok((directory, leaf))
}

fn duplicate(root: &Root) -> io::Result<File> {
    if !root.directory {
        return Err(invalid("publication authority is not a directory"));
    }
    duplicate_any(root)
}

fn duplicate_any(root: &Root) -> io::Result<File> {
    rustix::io::fcntl_dupfd_cloexec(&root.authority, 3)
        .map(File::from)
        .map_err(Into::into)
}

fn temporary_file(parent: &File) -> io::Result<(OsString, File)> {
    for _ in 0..128 {
        let name = temporary_name("file");
        match rustix::fs::openat(
            parent,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            PRIVATE_FILE,
        ) {
            Ok(file) => return Ok((name, File::from(file))),
            Err(Errno::EXIST) => {}
            Err(problem) => return Err(problem.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused publication file name is available",
    ))
}

fn temporary_symlink(parent: &File, target: &OsStr) -> io::Result<OsString> {
    for _ in 0..128 {
        let name = temporary_name("link");
        match rustix::fs::symlinkat(target, parent, &name) {
            Ok(()) => return Ok(name),
            Err(Errno::EXIST) => {}
            Err(problem) => return Err(problem.into()),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused publication link name is available",
    ))
}

fn temporary_name(kind: &str) -> OsString {
    let sequence = NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed);
    OsString::from(format!(
        ".crucible-sandbox-{kind}-{}-{sequence}",
        std::process::id()
    ))
}

fn commit(parent: &File, temporary: &OsStr, leaf: &OsStr, replace: bool) -> io::Result<()> {
    let committed = if replace {
        rustix::fs::renameat(parent, temporary, parent, leaf)
    } else {
        rustix::fs::renameat_with(parent, temporary, parent, leaf, RenameFlags::NOREPLACE)
    };
    if let Err(problem) = committed {
        let _ = rustix::fs::unlinkat(parent, temporary, AtFlags::empty());
        return Err(problem.into());
    }
    parent.sync_all()
}

fn set_metadata(file: &File, mode: u32, modified: Option<std::time::SystemTime>) -> io::Result<()> {
    file.set_permissions(fs::Permissions::from_mode(mode))?;
    let mut times = FileTimes::new();
    if let Some(modified) = modified {
        times = times.set_modified(modified);
    }
    file.set_times(times)
}

fn invalid(problem: &'static str) -> io::Error {
    io::Error::other(problem)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::sample::{Sample, symlink};

    use super::*;
    use crate::sandbox::linux::projection::Snapshot;

    #[test]
    fn descriptor_descent_refuses_an_intermediate_symlink() {
        let sample = Sample::new("sandbox-publication-symlink-swap");
        let outside = PathBuf::from(sample.beside("publication-outside"));
        symlink(&outside, sample.root().join("swapped"));
        let authority = File::open(sample.root()).expect("root authority").into();
        let root = Root {
            authority,
            destination: sample.root().clone(),
            source: None,
            directory: true,
            exclusions: Vec::new(),
            baseline: Snapshot {
                entries: BTreeMap::new(),
            },
        };

        let problem = create_directory(&root, Path::new("swapped/escaped"))
            .expect_err("symlink descent must be refused");

        assert!(
            matches!(
                problem.raw_os_error(),
                Some(code) if code == Errno::LOOP.raw_os_error()
                    || code == Errno::NOTDIR.raw_os_error()
            ),
            "unexpected refusal: {problem}"
        );
        assert!(!outside.join("escaped").exists());
    }
}
