//! Activating a staged release unit, rolling it back, and recovering what an
//! interrupted install left, all under the install's lock.
//!
//! The lock is the installer's own: a link at `<prefix>/lock` whose text,
//! `<pid>@<host>`, names the process that holds it, made in one call that fails
//! while it exists. An install that finds it held is refused at once; one that
//! finds it left by a process on this host that no longer runs is refused as
//! well, naming the lock, since the process may have stopped part way and
//! nothing can tell a person's copy from one still in use. A lock is never
//! taken from anybody, and only the holder removes it.
//!
//! Under the lock the layout is read again and must still be the one the
//! caller read, so an install that finished meanwhile is not undone. What an
//! interrupted install left is then removed: the staging directories under
//! `releases/` and the activation links beside `current`, whichever of this
//! module or the installer made them, since only an install holding the lock
//! makes either. The installer removes the same names under the same lock.
//!
//! A unit is staged, moved into place and activated only under the lock. It
//! is checked whole again just before it is renamed into `releases/<version>`,
//! from what is on disk, so a unit changed after staging is not activated. A
//! release already in place under that version is used again only when it is
//! the same build, and is otherwise refused and left as it is. The active
//! release is switched by renaming a new link over `current`, which is the
//! one change that makes another release active. The release that was active
//! is kept as it is, and rolling back points `current` at it again once it is
//! checked whole.
//!
//! So every moment leaves one whole release active, the old one or the new:
//! `current` names a unit that was whole before it was named, and nothing is
//! written into a unit once it is in place. Each change is synced before the
//! next one is made, which `fsync` on the file or the directory that changed
//! makes durable across a crash of the process or the system on Linux and the
//! BSDs, and on macOS through the full flush `std` asks for. Whether a file
//! system keeps that promise across a loss of power is its own; the tests kill
//! the process, not the machine. A switch made but not synced is reported
//! apart from one not made, so the caller knows which release is active.

use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs;
use std::io;
use std::os::unix::ffi::{OsStrExt as _, OsStringExt as _};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use super::boundary::crossed;
use super::layout::{CRUCIBLE, CURRENT, RELEASES, unit_receipt};
use super::stage::INCOMING;
use super::{LayoutError, Receipt, ReceiptLayout, StageError, StagedUnit, Version};

/// The lock's name inside the prefix.
const LOCK: &str = "lock";

/// What an activation link's name opens with, beside `current`, which the
/// installer's own share.
const NEXT_CURRENT: &str = ".current.";

/// A managed install held under its lock, where a release is staged and made
/// active, and from which an interrupted install has been recovered.
#[derive(Debug)]
pub struct RecoverableActivation {
    /// The layout, as it was read under the lock.
    layout: ReceiptLayout,
    /// The lock, released with the value.
    lock: Lock,
}

/// A release made active, which may be rolled back to the one it replaced
/// while the lock is still held.
#[derive(Debug)]
pub struct Activated {
    /// The canonical prefix.
    prefix: PathBuf,
    /// The receipt of the release that was active before.
    previous: Receipt,
    /// The receipt of the release now active.
    active: Receipt,
    /// The lock, released with the value.
    lock: Lock,
}

/// Why an install was not locked, activated or rolled back.
///
/// Each variant names what it refuses by its place in the layout rather than
/// by its path, which is somebody's home directory.
#[derive(Debug, thiserror::Error)]
pub enum ActivationError {
    /// Another install or update holds the lock.
    #[error("another install or update holds .crucible-install/lock; try again once it finishes")]
    Held,
    /// The lock was left by a process on this host that no longer runs.
    #[error(
        "an install or update that stopped before it finished left .crucible-install/lock behind; once no install is running, remove it and try again"
    )]
    Stale,
    /// Something other than a link is where the lock goes.
    #[error("refusing to use .crucible-install/lock, which is not a link")]
    NotLock,
    /// The layout under the lock is not the one the caller read.
    #[error("the install changed while its lock was being taken")]
    Changed,
    /// The layout under the lock is not a managed install's.
    #[error("the install is not as an install leaves it")]
    Layout(#[source] LayoutError),
    /// A unit about to be used is not whole.
    #[error("the {unit} is not a whole release")]
    Unit {
        /// The unit.
        unit: UnitRole,
        /// What is wrong with it.
        #[source]
        source: LayoutError,
    },
    /// A unit about to be used is whole but another build than expected.
    #[error("the {unit} is not the build it should be")]
    Altered {
        /// The unit.
        unit: UnitRole,
    },
    /// The staged unit was not staged into this install.
    #[error("the staged release was staged for another install")]
    Foreign,
    /// The active release was switched, but the switch could not be synced,
    /// so a crash of the system may still undo it.
    #[error(
        "switched the active release, but could not make the switch durable; a crash of the system may undo it"
    )]
    Unsynced(#[source] io::Error),
    /// A step could not be taken.
    #[error("could not {step}")]
    Io {
        /// The step.
        step: ActivationStep,
        /// What the operating system said.
        #[source]
        source: io::Error,
    },
}

/// A release unit, by the part it plays in an activation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitRole {
    /// The unit just staged.
    Staged,
    /// The unit already in place under the staged release's version.
    Existing,
    /// The unit that was active before, which a rollback restores.
    Previous,
}

/// A step of an activation, as an error names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationStep {
    /// Taking the lock.
    Lock,
    /// Removing what an interrupted install left.
    Recover,
    /// Moving the staged unit into place.
    Place,
    /// Switching the active release.
    Switch,
}

impl fmt::Display for UnitRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Staged => "staged release",
            Self::Existing => "release already installed under its version",
            Self::Previous => "previous release",
        })
    }
}

impl fmt::Display for ActivationStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Lock => "take the install's lock",
            Self::Recover => "remove what an interrupted install left",
            Self::Place => "move the staged release into place",
            Self::Switch => "switch the active release",
        })
    }
}

impl RecoverableActivation {
    /// Takes the lock of the install `layout` was read from, reads the layout
    /// again under it, and removes what an interrupted install left.
    ///
    /// # Errors
    ///
    /// [`ActivationError`] when the lock is held, was left behind or is not
    /// one, when the layout is no longer the one read, or when a leftover
    /// cannot be removed.
    pub fn begin(layout: &ReceiptLayout) -> Result<Self, ActivationError> {
        let lock = Lock::take(layout.prefix())?;
        let now = layout.again().map_err(ActivationError::Layout)?;
        if now.receipt() != layout.receipt() {
            return Err(ActivationError::Changed);
        }
        recover(now.prefix())?;
        Ok(Self { layout: now, lock })
    }

    /// The layout as it was read under the lock.
    pub fn layout(&self) -> &ReceiptLayout {
        &self.layout
    }

    /// Stages `version` from its archive and `SHA256SUMS` beside the active
    /// release, under the lock.
    ///
    /// # Errors
    ///
    /// [`StageError`] naming the first thing that is not as a release ships
    /// it.
    pub fn stage(
        &self,
        version: &Version,
        archive: &Path,
        checksums: &Path,
    ) -> Result<StagedUnit, StageError> {
        StagedUnit::stage(&self.layout, version, archive, checksums)
    }

    /// Makes `staged` the active release, once it is checked whole again.
    ///
    /// # Errors
    ///
    /// [`ActivationError`] when the unit is not whole, was staged for another
    /// install, or another build of its release is already in place, or when
    /// a step cannot be taken. The active release is then the one that was,
    /// except after [`ActivationError::Unsynced`]: the staged release is then
    /// active, but a crash of the system may still make the one before active
    /// again.
    pub fn activate(self, staged: StagedUnit) -> Result<Activated, ActivationError> {
        let prefix = self.layout.prefix();
        let releases = prefix.join(RELEASES);
        if staged.path().parent() != Some(releases.as_path()) {
            return Err(ActivationError::Foreign);
        }
        let version = staged.receipt().version().clone();
        let on_disk = unit_receipt(prefix, staged.path(), &version).map_err(|source| {
            ActivationError::Unit {
                unit: UnitRole::Staged,
                source,
            }
        })?;
        if on_disk != *staged.receipt() {
            return Err(ActivationError::Altered {
                unit: UnitRole::Staged,
            });
        }

        let unit = releases.join(version.as_str());
        let active = match fs::symlink_metadata(&unit) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let receipt = staged.moved(&unit).map_err(io(ActivationStep::Place))?;
                synced(&unit, ActivationStep::Place)?;
                crossed("moved the release into place");
                receipt
            }
            Err(source) => {
                return Err(ActivationError::Io {
                    step: ActivationStep::Place,
                    source,
                });
            }
            Ok(_) => {
                let existing = unit_receipt(prefix, &unit, &version).map_err(|source| {
                    ActivationError::Unit {
                        unit: UnitRole::Existing,
                        source,
                    }
                })?;
                if existing != *staged.receipt() {
                    return Err(ActivationError::Altered {
                        unit: UnitRole::Existing,
                    });
                }
                drop(staged);
                existing
            }
        };
        switch(prefix, &version)?;
        Ok(Activated {
            prefix: prefix.to_path_buf(),
            previous: self.layout.receipt().clone(),
            active,
            lock: self.lock,
        })
    }
}

impl Activated {
    /// The receipt of the release that was active before.
    pub fn previous(&self) -> &Receipt {
        &self.previous
    }

    /// The receipt of the release now active.
    pub fn active(&self) -> &Receipt {
        &self.active
    }

    /// The `crucible` of the release now active, which an update starts to
    /// see that it is the release it should be.
    pub(crate) fn executable(&self) -> PathBuf {
        self.prefix
            .join(RELEASES)
            .join(self.active.version().as_str())
            .join(CRUCIBLE)
    }

    /// Makes the release that was active before active again, once it is
    /// checked whole and still the build it was.
    ///
    /// # Errors
    ///
    /// [`ActivationError`] when the previous release is no longer whole or
    /// the same build, or when the switch cannot be made. The release just
    /// activated is then still the active one, except after
    /// [`ActivationError::Unsynced`]: the previous release is then active
    /// again, but a crash of the system may still make the one just
    /// activated active.
    pub fn roll_back(self) -> Result<(), ActivationError> {
        let version = self.previous.version();
        let unit = self.prefix.join(RELEASES).join(version.as_str());
        let found =
            unit_receipt(&self.prefix, &unit, version).map_err(|source| ActivationError::Unit {
                unit: UnitRole::Previous,
                source,
            })?;
        if found != self.previous {
            return Err(ActivationError::Altered {
                unit: UnitRole::Previous,
            });
        }
        switch(&self.prefix, version)?;
        drop(self.lock);
        Ok(())
    }
}

/// Makes `version` the active release by renaming a new link over `current`,
/// and syncs the switch, which [`ActivationError::Unsynced`] says was made
/// when it cannot be synced.
fn switch(prefix: &Path, version: &Version) -> Result<(), ActivationError> {
    let step = ActivationStep::Switch;
    let next = prefix.join(format!("{NEXT_CURRENT}{}", std::process::id()));
    symlink(format!("{RELEASES}/{version}"), &next).map_err(io(step))?;
    crossed("made the activation link");
    if let Err(source) = fs::rename(&next, prefix.join(CURRENT)) {
        let _ = fs::remove_file(&next);
        return Err(ActivationError::Io { step, source });
    }
    sync(&next).map_err(ActivationError::Unsynced)?;
    crossed("switched the active release");
    Ok(())
}

/// Removes what an interrupted install left under `prefix`: each staging
/// directory under `releases/` and each activation link beside `current`.
fn recover(prefix: &Path) -> Result<(), ActivationError> {
    for (directory, opening) in [
        (prefix.join(RELEASES), INCOMING),
        (prefix.to_path_buf(), NEXT_CURRENT),
    ] {
        for entry in fs::read_dir(&directory).map_err(io(ActivationStep::Recover))? {
            let entry = entry.map_err(io(ActivationStep::Recover))?;
            if !entry.file_name().as_bytes().starts_with(opening.as_bytes()) {
                continue;
            }
            let left = entry.path();
            let removed = match entry.file_type() {
                Ok(kind) if kind.is_dir() => fs::remove_dir_all(&left),
                Ok(_) => fs::remove_file(&left),
                Err(error) => Err(error),
            };
            match removed {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(source) => {
                    return Err(ActivationError::Io {
                        step: ActivationStep::Recover,
                        source,
                    });
                }
            }
            synced(&left, ActivationStep::Recover)?;
            crossed("removed what an interrupted install left");
        }
    }
    Ok(())
}

/// The install's lock, held by this process and removed with the value.
#[derive(Debug)]
struct Lock {
    /// Where it is.
    at: PathBuf,
    /// What it says: this process, as `<pid>@<host>`.
    holder: OsString,
}

impl Lock {
    /// Takes the lock under `prefix`, or says why it cannot be taken.
    fn take(prefix: &Path) -> Result<Self, ActivationError> {
        let at = prefix.join(LOCK);
        let host = host();
        let mut holder = format!("{}@", std::process::id()).into_bytes();
        holder.extend_from_slice(&host);
        let holder = OsString::from_vec(holder);
        match symlink(&holder, &at) {
            Ok(()) => {
                let lock = Self { at, holder };
                synced(&lock.at, ActivationStep::Lock)?;
                crossed("took the lock");
                Ok(lock)
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Err(refused(&at, &host)),
            Err(source) => Err(ActivationError::Io {
                step: ActivationStep::Lock,
                source,
            }),
        }
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        if fs::read_link(&self.at).is_ok_and(|text| text.as_os_str() == self.holder.as_os_str())
            && fs::remove_file(&self.at).is_ok()
        {
            let _ = crucible_privacy::sync_parent(&self.at);
            crossed("released the lock");
        }
    }
}

/// Why the lock at `at`, which exists, is not taken: left by a process on
/// `host` that no longer runs, not a link, or held.
fn refused(at: &Path, host: &[u8]) -> ActivationError {
    match fs::symlink_metadata(at) {
        Ok(metadata) if !metadata.file_type().is_symlink() => return ActivationError::NotLock,
        Ok(_) => {}
        Err(_) => return ActivationError::Held,
    }
    let Ok(text) = fs::read_link(at) else {
        return ActivationError::Held;
    };
    let text = text.as_os_str().as_bytes();
    let Some(split) = text.iter().position(|byte| *byte == b'@') else {
        return ActivationError::Held;
    };
    let (pid, rest) = text.split_at(split);
    if rest.get(1..) == Some(host) && gone(pid) {
        ActivationError::Stale
    } else {
        ActivationError::Held
    }
}

/// Whether `pid`, all digits, names no process that runs, as the installer
/// decides it: signal 0 finds none. A process of another user's still answers.
fn gone(pid: &[u8]) -> bool {
    if pid.is_empty() || !pid.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let Some(pid) = OsStr::from_bytes(pid)
        .to_str()
        .and_then(|pid| pid.parse::<i32>().ok())
        .and_then(rustix::process::Pid::from_raw)
    else {
        return false;
    };
    rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH)
}

/// This host's name as `uname -n` prints it, which the installer's lock
/// carries too.
fn host() -> Vec<u8> {
    rustix::system::uname().nodename().to_bytes().to_vec()
}

/// Makes the change to the name `at` durable by syncing its directory.
fn synced(at: &Path, step: ActivationStep) -> Result<(), ActivationError> {
    sync(at).map_err(|source| ActivationError::Io { step, source })
}

/// Syncs the directory holding the name `at`.
fn sync(at: &Path) -> io::Result<()> {
    #[cfg(test)]
    super::boundary::refuse_sync(at)?;
    crucible_privacy::sync_parent(at).map_err(crucible_privacy::PrivacyError::into_io)
}

/// The refusal for a failure to take `step`.
fn io(step: ActivationStep) -> impl Fn(io::Error) -> ActivationError {
    move |source| ActivationError::Io { step, source }
}

#[cfg(test)]
mod tests;
