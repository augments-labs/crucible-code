//! Why a candidate at the sandbox's private state path is refused, and its
//! two texts: one for the internal error chain, which may name the path and
//! another user's uid, and a path-free, uid-free classification for a caller
//! whose reason a model reads.

use std::fmt;
use std::io;
use std::path::Path;

/// Why a candidate at the state path was refused: not this user's private
/// directory, or a symlink where the `NOFOLLOW` open refused to follow one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateDirectoryProblem {
    /// The `NOFOLLOW` open failed because a symlink sits at the state path.
    Symlink,
    NotADirectory,
    /// Owned by another user, naming their uid.
    WrongOwner(u32),
    /// Owned by another group, naming its gid.
    WrongGroup(u32),
    /// Not mode 0700, naming the mode found.
    WrongMode(u32),
}

impl StateDirectoryProblem {
    /// The refusal text for `path`: short, plain, naming the property that
    /// failed and, for a squat by another user's directory, symlink or plain
    /// file, that it must be removed before the sandbox can be used. Kept for
    /// the internal error chain only — it names the path and, for
    /// [`Self::WrongOwner`], another user's uid, so nothing built from it may
    /// reach the model; [`Self::classification`] is what a model-visible
    /// caller shows instead.
    pub(super) fn message(self, path: &Path) -> String {
        let path = path.display();
        match self {
            Self::Symlink => format!(
                "sandbox state directory {path} is a symlink; remove it (an administrator may \
                 need to) before the sandbox can be used"
            ),
            Self::NotADirectory => format!(
                "sandbox state directory {path} is not a directory; remove it (an administrator \
                 may need to) before the sandbox can be used"
            ),
            Self::WrongOwner(uid) => format!(
                "sandbox state directory {path} is owned by uid {uid}, not this user; remove it \
                 (an administrator may need to) before the sandbox can be used"
            ),
            Self::WrongGroup(gid) => format!(
                "sandbox state directory {path} is owned by group {gid}, not this user's group"
            ),
            Self::WrongMode(mode) => {
                format!("sandbox state directory {path} has mode {mode:04o}, not 0700")
            }
        }
    }

    /// The reason shown through `SandboxError::BackendUnavailable`'s
    /// bounded, model-visible diagnostic: which property failed and its
    /// remedy, with neither the path nor another user's uid or gid, which
    /// [`Self::message`] carries only for the internal error chain. A squat
    /// by another user, a symlink or a plain file says to remove it, which
    /// may need an administrator. [`Self::WrongGroup`] and [`Self::WrongMode`]
    /// are reached only on this user's own directory — `private_directory_problem`
    /// checks the owner first — so they share a different reason that says to
    /// restore its group and mode rather than remove it: deleting this user's
    /// own state directory would discard their own unrecovered transaction
    /// journals and quarantine evidence.
    pub(super) fn classification(self) -> &'static str {
        match self {
            Self::Symlink => {
                "the sandbox state directory is a symlink; it must be removed (an administrator \
                 may need to) before the sandbox can be used"
            }
            Self::NotADirectory => {
                "the sandbox state directory is not a directory; it must be removed (an \
                 administrator may need to) before the sandbox can be used"
            }
            Self::WrongOwner(_) => {
                "the sandbox state directory is owned by another user; it must be removed (an \
                 administrator may need to) before the sandbox can be used"
            }
            Self::WrongGroup(_) | Self::WrongMode(_) => {
                "the sandbox state directory is this user's own, but has the wrong group or \
                 permissions; restoring its group to this user's own and its mode to 0700 (for \
                 example `chmod 700`), or running crucible from this user's own primary group, \
                 fixes it"
            }
        }
    }
}

/// The `io::Error` source [`super::open_state_directory`] attaches when it
/// refuses a candidate, carrying the exact [`StateDirectoryProblem`] so a
/// caller that must not show the path or another user's uid —
/// `RegistryLease::acquire` — can still act on the classified reason through
/// [`get_ref`](io::Error::get_ref) and `downcast_ref`, never by matching on
/// this error's text.
#[derive(Debug)]
struct PrivateStateDirectoryRefusal {
    problem: StateDirectoryProblem,
    text: String,
}

impl fmt::Display for PrivateStateDirectoryRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

impl std::error::Error for PrivateStateDirectoryRefusal {}

/// An `io::Error` refusing `path` for `problem`: [`io::ErrorKind::NotADirectory`]
/// for [`StateDirectoryProblem::Symlink`] and
/// [`StateDirectoryProblem::NotADirectory`], matching what the `NOFOLLOW` open
/// itself would have reported before this problem was named; `Other` for a
/// mismatch found only by inspecting an opened directory's metadata, as
/// before this problem was named individually.
pub(super) fn refuse_state_directory(problem: StateDirectoryProblem, path: &Path) -> io::Error {
    let kind = match problem {
        StateDirectoryProblem::Symlink | StateDirectoryProblem::NotADirectory => {
            io::ErrorKind::NotADirectory
        }
        StateDirectoryProblem::WrongOwner(_)
        | StateDirectoryProblem::WrongGroup(_)
        | StateDirectoryProblem::WrongMode(_) => io::ErrorKind::Other,
    };
    io::Error::new(
        kind,
        PrivateStateDirectoryRefusal {
            problem,
            text: problem.message(path),
        },
    )
}

/// The [`StateDirectoryProblem`] `error` was refused for, if
/// [`refuse_state_directory`] is what built it, found through its typed
/// source rather than its text.
fn state_directory_problem_of(error: &io::Error) -> Option<StateDirectoryProblem> {
    error
        .get_ref()?
        .downcast_ref::<PrivateStateDirectoryRefusal>()
        .map(|refusal| refusal.problem)
}

/// `RegistryLease::acquire`'s reason: the classified, model-visible text for
/// `error`'s [`StateDirectoryProblem`] when [`super::open_state_directory`] is
/// what refused it, or the fixed reason every other admission failure has
/// always had.
pub(super) fn registry_admission_reason(error: &io::Error) -> Box<str> {
    state_directory_problem_of(error)
        .map_or(
            "sandbox lifecycle registry admission is unavailable",
            StateDirectoryProblem::classification,
        )
        .into()
}

/// A candidate state directory's identity, pared down to what a refusal
/// reasons about: whether it is a directory, who owns it and its mode.
#[derive(Debug, Clone, Copy)]
pub(super) struct StateDirectoryFound {
    pub(super) is_dir: bool,
    pub(super) uid: u32,
    pub(super) gid: u32,
    pub(super) mode: u32,
}

/// The uid and gid this user's own private state directory must be owned by.
#[derive(Debug, Clone, Copy)]
pub(super) struct StateDirectoryOwner {
    pub(super) uid: u32,
    pub(super) gid: u32,
}

/// Which property, if any, keeps `found` from being `owner`'s private
/// directory: a directory, owned by that user and group, mode 0700. Pure so
/// the message for each property can be tested with synthetic values, with no
/// second user or root needed.
pub(super) fn private_directory_problem(
    found: StateDirectoryFound,
    owner: StateDirectoryOwner,
) -> Option<StateDirectoryProblem> {
    if !found.is_dir {
        Some(StateDirectoryProblem::NotADirectory)
    } else if found.uid != owner.uid {
        Some(StateDirectoryProblem::WrongOwner(found.uid))
    } else if found.gid != owner.gid {
        Some(StateDirectoryProblem::WrongGroup(found.gid))
    } else if found.mode & 0o7777 != 0o700 {
        Some(StateDirectoryProblem::WrongMode(found.mode & 0o7777))
    } else {
        None
    }
}
