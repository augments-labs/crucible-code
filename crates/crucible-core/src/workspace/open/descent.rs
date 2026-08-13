//! Reaching a proven path one component at a time, through descriptors.
//!
//! Nothing here hands a whole path to the operating system. The directory
//! containment was settled against is opened by name — it is the anchor, and
//! replacing it takes write access to the directory above the workspace, which
//! is further than anything writing *into* the workspace can reach — and every
//! component below it is opened against the descriptor for the one before.

use std::ffi::OsStr;
use std::fs::File;
use std::path::Path;

use rustix::fs::{Mode, OFlags};
use rustix::io::Errno;

use super::{Access, PathError, WorkspacePath};

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

/// Opens the file at the end of the walk.
pub(super) fn opened(path: &WorkspacePath, access: Access) -> Result<File, PathError> {
    let wanted = match access {
        Access::Read => OFlags::RDONLY,
        Access::Change => OFlags::RDWR,
    };

    // `NOFOLLOW` on the last component too, and here it is the whole of the
    // question the old check on names had to ask twice: this is one lookup, so
    // a link arriving is refused by the same call that would have followed it.
    reached(
        path,
        wanted | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
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

/// Walks down to the directory holding the last component and opens that
/// component against it.
///
/// Each descriptor lives exactly as long as the step that used it: assigning
/// the next one over it closes the last, and the one that reaches the bottom is
/// dropped when this returns. A walk of any depth therefore holds one open
/// directory at a time.
fn reached(path: &WorkspacePath, flags: OFlags, mode: Mode) -> Result<File, PathError> {
    let mut at =
        rustix::fs::open(path.root(), DOWN, Mode::empty()).map_err(|errno| refused(path, errno))?;

    let below = path.below_root();

    // Every component here is a plain name: what is walked came back from
    // resolving, which leaves no `.` and no `..` in a path, and a last component
    // that was either is refused a step earlier as a path naming no file.
    for step in below.parent().unwrap_or(Path::new("")).components() {
        at = rustix::fs::openat(&at, step.as_os_str(), DOWN, Mode::empty())
            .map_err(|errno| refused(path, errno))?;
    }

    // A path that *is* the directory it was proved against has no last
    // component to ask for, and `.` is how a directory names itself to a
    // descriptor already holding it.
    let leaf = below.file_name().unwrap_or(OsStr::new("."));

    rustix::fs::openat(&at, leaf, flags, mode)
        .map(File::from)
        .map_err(|errno| refused(path, errno))
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
