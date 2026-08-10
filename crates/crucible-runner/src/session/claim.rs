//! Telling a session that is open from one that ended.
//!
//! The newest log for a workspace is the newest whether or not something is
//! still writing to it, so two crucibles started in one directory both pick the
//! same one to continue. The second replays it, cuts it back to what it read
//! and appends — which deletes lines the first has already written and still
//! believes are there, and leaves two processes appending to one file. Neither
//! notices, and what the next `--continue` reads back is one conversation's
//! prompts interleaved with another's.
//!
//! A claim is how a session that is open says so. The operating system holds it
//! and releases it when the process ends, however it ends, so a crash leaves
//! nothing to clean up and no way to be wrong about whether a session is still
//! running.

use std::fs::{File, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use super::privacy;

/// What the mark beside a log is called.
const MARK: &str = "lock";

/// A log this process has open.
///
/// Held for as long as the session is. Nothing is written through the handle;
/// keeping it open is the whole of what it does, and dropping it is what hands
/// the session back.
#[derive(Debug)]
pub(super) struct Claim {
    held: File,
}

impl Drop for Claim {
    /// Releases the claim.
    ///
    /// Closing the handle does this on every platform, and this is here to say
    /// that the field is what the type is for rather than something unused.
    fn drop(&mut self) {
        drop(self.held.unlock());
    }
}

/// Claims `log` for this process.
///
/// `Ok(None)` is a log another crucible has open.
///
/// # Errors
///
/// When the mark cannot be made, or when the filesystem holding it has no locks
/// to take — some network filesystems have none. That is not the same answer as
/// "another crucible has it", and the caller decides what to do with it,
/// because a `--continue` that refuses where it worked yesterday costs more
/// than the collision it would be guarding against.
pub(super) fn claim(log: &Path) -> Result<Option<Claim>, io::Error> {
    let held = privacy::log(&beside(log))?;

    match held.try_lock() {
        Ok(()) => Ok(Some(Claim { held })),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(problem)) => Err(problem),
    }
}

/// Where the mark for `log` lives.
///
/// Beside the log rather than on it, because continuing a session opens the log
/// three more times — to read it back, to cut it, and to append to it — and on
/// Windows a lock on a file bars every one of those, including the ones this
/// process makes itself.
///
/// The mark is never deleted. One left behind by a crashed process holds no
/// lock, so it costs a file and nothing else, and it is passed over when the
/// newest log is looked for: what is looked for is named for a session and ends
/// in the log suffix, and this ends in neither. Deleting one is what would make
/// two processes able to hold two different files of the same name.
fn beside(log: &Path) -> PathBuf {
    let mut mark = log.as_os_str().to_owned();
    mark.push(".");
    mark.push(MARK);

    PathBuf::from(mark)
}
