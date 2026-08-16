//! What the agent has already looked at.
//!
//! `write` puts down a whole file, so a call naming one that is already there
//! discards everything in it. That is fine when the agent has seen what it is
//! discarding and is the one destructive thing in this crate when it has not —
//! every other way of changing a file either matches text already in it or
//! creates something new.
//!
//! Deciding that needs two tools to agree, and neither owns the answer: `read`
//! learns it and `write` acts on it. So the answer lives here, in one value
//! both are handed when they are built, and the binary is the only place that
//! knows they share it.
//!
//! It is a session's memory, so it starts again when the session does. Nothing
//! bounds how long one runs, though, so it is bounded like every other thing
//! this program holds. Forgetting costs a read: a file
//! remembered long enough ago to have been evicted is refused and read again,
//! which is a wasted call rather than a wrong answer. Forgetting in the other
//! direction — claiming a file was seen when it was not — is the failure this
//! module exists to prevent, and no bound can cause it.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// How many files are remembered at once.
///
/// A path is a hundred-odd bytes, so this is a tenth of a megabyte against the
/// thirty-five this program is allowed — and, unlike the count of files a
/// session reads, it does not move with how long the session runs. Generous
/// against a real session: a turn that touches a thousand distinct files has
/// spent its context long before it spends this.
const REMEMBERED: usize = 1_024;

/// The files this session has looked at.
///
/// Cloning shares the record rather than copying it, which is the whole point:
/// the `read` that learns and the `write` that asks are two tools holding two
/// clones of one answer.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    seen: Arc<Mutex<VecDeque<PathBuf>>>,
}

impl Ledger {
    /// A record with nothing in it yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Remembers that `path` was read.
    ///
    /// Oldest out when the bound is reached. A path already remembered moves to
    /// the newest end rather than being stored twice, so a file the agent keeps
    /// coming back to is the last one forgotten.
    pub(crate) fn record(&self, path: &Path) {
        // A poisoned lock means a thread ended while holding this, which cannot
        // happen from anything in this module. Doing nothing leaves the record
        // saying less than the truth, and this file is written so that saying
        // less costs a read and saying more costs a file.
        let Ok(mut seen) = self.seen.lock() else {
            return;
        };

        seen.retain(|remembered| remembered != path);
        seen.push_back(path.to_path_buf());
        if seen.len() > REMEMBERED {
            seen.pop_front();
        }
    }

    /// Forgets every file in it.
    ///
    /// What `/clear` and `/resume` run: the session those files were read in is
    /// not the session the next `write` is in, and a record that outlived its
    /// session would answer for one the agent has left.
    ///
    /// A poisoned lock is the same case as [`Ledger::record`]'s and is left the
    /// same way. What it costs here is a read, and it cannot cost a file: a
    /// record that failed to empty says a file was seen by a session that did
    /// see it.
    pub fn forget(&self) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.clear();
        }
    }

    /// Whether `path` was read.
    pub(crate) fn holds(&self, path: &Path) -> bool {
        self.seen
            .lock()
            .is_ok_and(|seen| seen.iter().any(|remembered| remembered == path))
    }
}

#[cfg(test)]
mod tests {
    use super::{Ledger, REMEMBERED};

    fn at(name: usize) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("/w/file-{name:05}.txt"))
    }

    #[test]
    fn a_file_that_was_read_is_held_and_one_that_was_not_is_not() {
        let ledger = Ledger::new();

        ledger.record(&at(1));

        assert!(ledger.holds(&at(1)));
        assert!(!ledger.holds(&at(2)));
    }

    #[test]
    fn two_clones_are_one_record_rather_than_two() {
        // The whole reason this type exists: the tool that learns and the tool
        // that asks are different objects.
        let learns = Ledger::new();
        let asks = learns.clone();

        learns.record(&at(1));

        assert!(asks.holds(&at(1)));
    }

    #[test]
    fn what_it_remembers_never_grows_past_the_bound() {
        let ledger = Ledger::new();

        for number in 0..REMEMBERED * 2 {
            ledger.record(&at(number));
        }

        assert!(!ledger.holds(&at(0)), "it kept the oldest path");
        assert!(ledger.holds(&at(REMEMBERED * 2 - 1)));
        assert_eq!(ledger.seen.lock().unwrap().len(), REMEMBERED);
    }

    #[test]
    fn forgetting_leaves_a_file_that_was_read_needing_to_be_read_again() {
        // What `/clear` runs. The session the file was read in is over, so the
        // record of having read it is over with it — and `write` asks for the
        // read again rather than replacing a file this session never saw.
        let ledger = Ledger::new();
        ledger.record(&at(1));

        ledger.forget();

        assert!(!ledger.holds(&at(1)));
    }

    #[test]
    fn forgetting_is_forgotten_by_every_clone() {
        // The same reason the record is shared at all: the tool that asks is
        // not the object the command reached.
        let asks = Ledger::new();
        let learns = asks.clone();
        learns.record(&at(1));

        asks.forget();

        assert!(!learns.holds(&at(1)));
    }

    #[test]
    fn reading_a_file_again_moves_it_away_from_being_forgotten() {
        // Otherwise the file an agent works on all session is evicted by the
        // thousand it glanced at, which is exactly backwards.
        let ledger = Ledger::new();
        ledger.record(&at(0));

        for number in 1..REMEMBERED {
            ledger.record(&at(number));
        }
        ledger.record(&at(0));
        ledger.record(&at(REMEMBERED));

        assert!(ledger.holds(&at(0)));
        assert!(!ledger.holds(&at(1)), "it forgot the wrong one");
    }
}
