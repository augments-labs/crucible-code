//! Reaching a proven path one component at a time, through descriptors.
//!
//! Nothing here hands a whole path to the operating system. The directory
//! containment was settled against is opened by name — it is the anchor, and
//! replacing it takes write access to the directory above the workspace, which
//! is further than anything writing *into* the workspace can reach — and every
//! component below it is opened against the descriptor for the one before.

use std::ffi::{OsStr, OsString};
use std::fs::{File, Permissions};
use std::io;
use std::os::unix::fs::MetadataExt as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rustix::fs::{AtFlags, Mode, OFlags};
use rustix::io::Errno;

use super::{Access, PathError, WalkFiles, WorkspacePath};

/// How every directory on the way down is opened.
///
/// `NOFOLLOW` is the check and `DIRECTORY` is the same check from the other
/// side: the resolved path had a directory here and no link anywhere, so a link
/// or a plain file at this step is something that changed since. `CLOEXEC`
/// because `bash` runs from this process — a descriptor left open across a
/// spawn is a directory handed to a command that was never asked about it.
const DOWN: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC);

/// What a file crucible creates is asked for as, before the umask takes its
/// share: the 0666 [`File::create_new`] asks for, so a file written here is not
/// held at some other mode than one written by anything else on the machine.
const MADE: Mode = Mode::RUSR
    .union(Mode::WUSR)
    .union(Mode::RGRP)
    .union(Mode::WGRP)
    .union(Mode::ROTH)
    .union(Mode::WOTH);

/// What a directory crucible creates is asked for as, before the umask.
const MADE_DIRECTORY: Mode = Mode::RUSR
    .union(Mode::WUSR)
    .union(Mode::XUSR)
    .union(Mode::RGRP)
    .union(Mode::WGRP)
    .union(Mode::XGRP)
    .union(Mode::ROTH)
    .union(Mode::WOTH)
    .union(Mode::XOTH);

/// Distinguishes temporary names made by concurrent replacements.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// Opens the file at the end of the walk.
pub(super) fn opened(path: &WorkspacePath, access: Access) -> Result<File, PathError> {
    let wanted = match access {
        Access::Read => OFlags::RDONLY,
        Access::ReadFile => OFlags::RDONLY | OFlags::NONBLOCK,
        Access::Change => OFlags::RDWR,
        Access::ChangeFile => OFlags::RDWR | OFlags::NONBLOCK,
    };

    // `NOFOLLOW` on the last component too, and here it is the whole of the
    // question the old check on names had to ask twice: this is one lookup, so
    // a link arriving is refused by the same call that would have followed it.
    let file = reached(
        path,
        wanted | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )?;
    if matches!(access, Access::ReadFile | Access::ChangeFile)
        && !file
            .metadata()
            .map_err(|source| path.unopened(source))?
            .is_file()
    {
        return Err(path.not_file());
    }
    Ok(file)
}

/// Opens a regular file relative to the last directory this walk worker used.
pub(super) fn walked_regular(
    files: &mut WalkFiles,
    path: &WorkspacePath,
) -> Result<File, PathError> {
    let parent = path.as_path().parent().unwrap_or(path.as_path());
    let cached = files
        .parent
        .as_ref()
        .is_some_and(|(cached, _)| cached == parent);
    if !cached {
        let Some(parent) = files.from.walked(parent) else {
            return Err(path.swapped());
        };
        let directory = reached(&parent, DOWN, Mode::empty())?;
        files.parent = Some((parent.as_path().to_owned(), directory));
    }

    let (_, directory) = files.parent.as_ref().ok_or_else(|| path.swapped())?;
    let leaf = path.as_path().file_name().unwrap_or(OsStr::new("."));
    let file = rustix::fs::openat(
        directory,
        leaf,
        OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map(File::from)
    .map_err(|errno| refused(path, errno))?;
    if !file
        .metadata()
        .map_err(|source| path.unopened(source))?
        .is_file()
    {
        return Err(path.not_file());
    }
    Ok(file)
}

/// Creates the file at the end of the walk, which nothing may be occupying.
pub(super) fn created(path: &WorkspacePath) -> Result<File, PathError> {
    // `O_CREAT | O_EXCL`, which the operating system refuses to satisfy through
    // a symbolic link at the last component, dangling or not — so the last step
    // needs no `NOFOLLOW` to say what the other two do.
    reached(
        path,
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
        MADE,
    )
}

/// Creates a directory against its already-opened proven parent.
pub(super) fn created_directory(path: &WorkspacePath) -> Result<(), PathError> {
    let (at, leaf) = parent(path)?;
    rustix::fs::mkdirat(&at, &leaf, MADE_DIRECTORY).map_err(|problem| match problem {
        Errno::EXIST | Errno::LOOP | Errno::NOTDIR => path.swapped(),
        other => path.uncreated(other.into()),
    })
}

/// Writes a replacement beside the destination and commits it with one rename.
pub(super) fn replaced(
    path: &WorkspacePath,
    permissions: Option<Permissions>,
    expected: Option<&File>,
    write: impl FnOnce(&mut File) -> io::Result<()>,
) -> Result<(), PathError> {
    let (at, leaf) = parent(path)?;
    let (temporary, mut file) = temporary(path, &at)?;
    let existed = permissions.is_some();

    let prepared = write(&mut file)
        .and_then(|()| match permissions {
            Some(permissions) => file.set_permissions(permissions),
            None => Ok(()),
        })
        .and_then(|()| file.sync_all());

    if let Err(source) = prepared {
        let _ = rustix::fs::unlinkat(&at, &temporary, AtFlags::empty());
        return Err(path.unreplaced(source));
    }

    if let Some(expected) = expected {
        // This is deliberately the last operation before commit. POSIX has no
        // identity compare-and-rename primitive, so a different file can still
        // arrive in the interval after this check. The rename remains relative
        // to `at` and replaces the leaf itself rather than following it, so a
        // leaf link cannot redirect the replacement to another file.
        let current = rustix::fs::openat(
            &at,
            &leaf,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
            Mode::empty(),
        )
        .map(File::from);
        let unchanged = current
            .ok()
            .and_then(|current| current.metadata().ok())
            .zip(expected.metadata().ok())
            .is_some_and(|(current, expected)| {
                current.dev() == expected.dev() && current.ino() == expected.ino()
            });
        if !unchanged {
            let _ = rustix::fs::unlinkat(&at, &temporary, AtFlags::empty());
            return Err(path.changed());
        }
    }

    let committed = if existed {
        rustix::fs::renameat(&at, &temporary, &at, &leaf)
    } else {
        // A creation must not turn into an overwrite if another writer puts a
        // file down after the absence check. A same-directory hard link makes
        // the destination appear atomically and fails when the name is taken;
        // unlinking the private name afterwards leaves the one new file.
        rustix::fs::linkat(&at, &temporary, &at, &leaf, AtFlags::empty())
    };
    if let Err(problem) = committed {
        let _ = rustix::fs::unlinkat(&at, &temporary, AtFlags::empty());
        return Err(path.unreplaced(problem.into()));
    }

    let uncleaned = if existed {
        None
    } else {
        rustix::fs::unlinkat(&at, &temporary, AtFlags::empty()).err()
    };

    // The rename or link is the commit point: a failure after it cannot
    // honestly be called an uncommitted replacement. Report durability and
    // cleanup uncertainty separately so nobody assumes the old state remains.
    rustix::fs::fsync(&at).map_err(|problem| path.unsynced(problem.into()))?;
    match uncleaned {
        Some(problem) => Err(path.uncleaned(problem.into())),
        None => Ok(()),
    }
}

/// A fresh file in the destination directory, under a name nothing reads.
fn temporary(path: &WorkspacePath, at: &File) -> Result<(OsString, File), PathError> {
    for _ in 0..128 {
        let number = NEXT.fetch_add(1, Ordering::Relaxed);
        let name = OsString::from(format!(".crucible-writing-{}-{number}", std::process::id()));
        match rustix::fs::openat(
            at,
            &name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            MADE,
        ) {
            Ok(file) => return Ok((name, File::from(file))),
            Err(Errno::EXIST) => {}
            Err(problem) => return Err(path.unreplaced(problem.into())),
        }
    }

    Err(path.unreplaced(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "no unused temporary name was available",
    )))
}

/// Walks down to the directory holding the last component and opens that
/// component against it.
///
/// Each descriptor lives exactly as long as the step that used it: assigning
/// the next one over it closes the last, and the one that reaches the bottom is
/// dropped when this returns. A walk of any depth therefore holds one open
/// directory at a time.
fn reached(path: &WorkspacePath, flags: OFlags, mode: Mode) -> Result<File, PathError> {
    let (at, leaf) = parent(path)?;

    rustix::fs::openat(&at, &leaf, flags, mode)
        .map(File::from)
        .map_err(|errno| refused(path, errno))
}

/// The proven parent directory and the last name below it.
fn parent(path: &WorkspacePath) -> Result<(File, OsString), PathError> {
    let mut at = rustix::fs::open(path.root(), DOWN, Mode::empty())
        .map(File::from)
        .map_err(|errno| refused(path, errno))?;
    let below = path.below_root();

    // Every component here is a plain name: resolving leaves no `.` or `..`,
    // and each open is relative to the descriptor from the last step.
    for step in below.parent().unwrap_or(Path::new("")).components() {
        at = rustix::fs::openat(&at, step.as_os_str(), DOWN, Mode::empty())
            .map(File::from)
            .map_err(|errno| refused(path, errno))?;
    }

    let leaf = below.file_name().unwrap_or(OsStr::new(".")).to_owned();
    Ok((at, leaf))
}

/// What a refusal at any step of the walk means.
///
/// Three of them are the swap this walk exists to catch, and they are the three
/// a path that resolved a moment ago cannot otherwise produce: a component that
/// is a symbolic link now (`ELOOP`, which is what `O_NOFOLLOW` reports), one
/// that has stopped being a directory (`ENOTDIR`), and something occupying a
/// name that was free when it was checked (`EEXIST`, reachable only from the
/// create above). The rest are the operating system's own business — a file
/// deleted since, a mode that forbids this, a device that is gone — and are
/// reported as what they are rather than as an attack.
fn refused(path: &WorkspacePath, errno: Errno) -> PathError {
    match errno {
        Errno::LOOP | Errno::NOTDIR | Errno::EXIST => path.swapped(),
        other => path.unopened(other.into()),
    }
}
