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

/// What claiming a log came back with.
///
/// Three answers rather than two, because the two that hold no claim are not
/// the same answer and call for opposite decisions: one stops the caller and
/// the other carries on without a guard.
///
/// What stopping means depends on which caller asked, and the two are worth
/// keeping apart. Continuing a session names the log it wants, so a busy one is
/// the answer to the question and the run says so. A session starting named
/// nothing — it minted a name and this is the news that the name is somebody
/// else's — so it mints another. Both refuse to write; only one of them has a
/// user to tell.
#[derive(Debug)]
pub(super) enum Claimed {
    /// This process holds the log.
    Taken(Claim),
    /// Another crucible holds it.
    Busy,
    /// There was no lock to take. Some network filesystems have none.
    Lockless,
}

/// Claims `log` for this process.
///
/// # Errors
///
/// When the mark beside the log cannot be made. That is not one of the three
/// answers above and must not be read as one: the lock was never reached, so
/// nothing was asked about the log and nothing was learned about it. Read as
/// [`Claimed::Lockless`] — the answer it most resembles, being the other one
/// with no claim in it — a directory that had gone read-only would take the
/// guard away with nothing said, which is the failure the guard exists for.
pub(super) fn claim(log: &Path) -> Result<Claimed, io::Error> {
    let held = privacy::mark(&beside(log))?;

    Ok(match held.try_lock() {
        Ok(()) => Claimed::Taken(Claim { held }),
        Err(TryLockError::WouldBlock) => Claimed::Busy,
        // Every other way the attempt itself can fail is read as the filesystem
        // having none to take. Which numbers mean exactly that differs by
        // platform and by mount, and the alternative is a list of them that is
        // wrong on the first filesystem nobody tested on — where being wrong
        // means refusing every `--continue` there for good.
        Err(TryLockError::Error(_)) => Claimed::Lockless,
    })
}

/// Waits until this process exclusively holds `log`'s mark.
///
/// The recent-session index needs serialization rather than a non-blocking
/// claim: its replacement is brief, and carrying on without the lock would
/// let two starts lose one another.
///
/// # Errors
///
/// When the mark beside the log cannot be made. A lock the filesystem cannot
/// take is not an error, for the reason [`claim`] gives: this sits on the path
/// every start and every resume walks, so refusing there would refuse all of
/// them for good, and what the missing guard costs is two simultaneous starts
/// racing one bounded index replacement rather than anything in a log.
pub(super) fn exclusive(log: &Path) -> Result<Claim, io::Error> {
    let held = privacy::mark(&beside(log))?;
    drop(held.lock());
    Ok(Claim { held })
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

#[cfg(test)]
mod tests {
    use super::{Claimed, claim};
    use crate::sample::Sample;

    /// Taken, busy, and taken again once it was handed back.
    ///
    /// Asserted as the exact answer each time rather than as "some claim came
    /// back", because the third answer carries on regardless: a run where every
    /// claim came back [`Claimed::Lockless`] would have no guard at all, and a
    /// test that only asked whether one was held would pass through it. A
    /// temporary directory on the machine running the tests has locks.
    #[test]
    fn a_log_this_process_is_already_holding_is_reported_held_rather_than_claimed_again() {
        let sample = Sample::new("claim-twice");
        let log = sample.logs().join("1786713045000-3f9c2a.jsonl");

        let held = match claim(&log) {
            Ok(Claimed::Taken(held)) => held,
            other => panic!("a log nothing is holding was not claimed: {other:?}"),
        };

        assert!(
            matches!(claim(&log), Ok(Claimed::Busy)),
            "one log was claimed by two"
        );

        // Handed back when the session ends, which is what lets the log be
        // continued afterwards rather than being busy for as long as the file
        // is there.
        drop(held);

        assert!(
            matches!(claim(&log), Ok(Claimed::Taken(_))),
            "a log that was given back is still held"
        );
    }

    #[test]
    fn a_claim_that_could_not_be_attempted_is_not_a_filesystem_with_no_locks() {
        // The mark sits beside the log, so a directory that is not there is a
        // mark that cannot be made — and the lock is never reached. Answered as
        // one of the three, this would be answered as the one that carries on,
        // and `--continue` would go past a guard that had never run.
        let sample = Sample::new("claim-nowhere");
        let log = sample
            .logs()
            .join("gone")
            .join("1786713045000-3f9c2a.jsonl");

        assert!(claim(&log).is_err(), "a mark was made where nothing exists");
    }
}
