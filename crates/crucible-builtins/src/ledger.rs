//! What the agent has already looked at.
//!
//! `write` puts down a whole file, so a call naming one that is already there
//! discards everything in it. That is fine when the agent has seen what it is
//! discarding and is the one destructive thing in this crate when it has not —
//! every other way of changing a file either matches text already in it or
//! creates something new.
//!
//! Deciding that needs tools to agree, and none owns the answer: `read` learns
//! it, and `write` and `edit` act on it. So the answer lives here, in one value
//! each is handed when it is built, and the binary is the only place that knows
//! they share it.
//!
//! Knowing a file was looked at is not knowing what is in it now: another
//! program can change it in place or put a new file at its name after the
//! read. So each file is remembered with the digest of the whole of its
//! content as this session last saw it — read, written, or edited — and a
//! change of content since then is refused as surely as a file never read.
//! The digest is taken by streaming the file and keeps none of it. A partial
//! read still counts as having looked, and its digest is still of the whole
//! file, so that what changed in the part the agent was not shown is caught
//! too.
//!
//! It is a session's memory, so it starts again when the session does. Nothing
//! bounds how long one runs, though, so it is bounded like every other thing
//! this program holds. Forgetting costs a read: a file
//! remembered long enough ago to have been evicted is refused and read again,
//! which is a wasted call rather than a wrong answer. Forgetting in the other
//! direction — claiming a file was seen when it was not — is the failure this
//! module exists to prevent, and no bound can cause it.

use std::collections::VecDeque;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crucible_tools::ToolOutput;
use sha2::{Digest as _, Sha256};

/// How many files are remembered at once.
///
/// A path is a hundred-odd bytes and its digest thirty-two, so this is a tenth of a megabyte against the
/// thirty-five this program is allowed — and, unlike the count of files a
/// session reads, it does not move with how long the session runs. Generous
/// against a real session: a turn that touches a thousand distinct files has
/// spent its context long before it spends this.
const REMEMBERED: usize = 1_024;

/// The files this session has looked at, each with what was in it.
///
/// Cloning shares the record rather than copying it, which is the whole point:
/// the `read` that learns and the `write` that asks are two tools holding two
/// clones of one answer.
#[derive(Clone, Debug, Default)]
pub struct Ledger {
    seen: Arc<Mutex<VecDeque<(PathBuf, Fingerprint)>>>,
}

impl Ledger {
    /// A record with nothing in it yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Remembers that `path` was seen holding what `fingerprint` is the
    /// digest of.
    ///
    /// Oldest out when the bound is reached. A path already remembered moves to
    /// the newest end rather than being stored twice, so a file the agent keeps
    /// coming back to is the last one forgotten.
    pub(crate) fn record(&self, path: &Path, fingerprint: Fingerprint) {
        // A poisoned lock means a thread ended while holding this, which cannot
        // happen from anything in this module. Doing nothing leaves the record
        // saying less than the truth, and this file is written so that saying
        // less costs a read and saying more costs a file.
        let Ok(mut seen) = self.seen.lock() else {
            return;
        };

        seen.retain(|(remembered, _)| remembered != path);
        seen.push_back((path.to_path_buf(), fingerprint));
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
    /// A poisoned lock is the same case as `Ledger::record`'s and is left the
    /// same way. What it costs here is a read, and it cannot cost a file: a
    /// record that failed to empty says a file was seen by a session that did
    /// see it.
    pub fn forget(&self) {
        if let Ok(mut seen) = self.seen.lock() {
            seen.clear();
        }
    }

    /// What `path` held when this session last saw it, or `None` when it has
    /// not seen it.
    pub(crate) fn fingerprint(&self, path: &Path) -> Option<Fingerprint> {
        let seen = self.seen.lock().ok()?;
        seen.iter()
            .find(|(remembered, _)| remembered == path)
            .map(|(_, fingerprint)| *fingerprint)
    }

    /// Whether `path` was seen at all.
    #[cfg(test)]
    pub(crate) fn holds(&self, path: &Path) -> bool {
        self.fingerprint(path).is_some()
    }

    /// The answer a call came to, remembering the file it showed the agent.
    ///
    /// Asked by the call once its work has answered, and never by the work.
    /// Work handed to a worker runs on after its call is dropped, and what it
    /// comes to then reaches nobody: remembering the file it read would let
    /// `write` replace a file the agent was never shown.
    pub(crate) fn shown(&self, shown: Shown) -> ToolOutput {
        if let Some((file, fingerprint)) = &shown.file {
            self.record(file, *fingerprint);
        }
        shown.output
    }

    /// The record itself, held, so that a test can stop a call at the moment
    /// it asks.
    #[cfg(test)]
    pub(crate) fn held(&self) -> std::sync::MutexGuard<'_, VecDeque<(PathBuf, Fingerprint)>> {
        self.seen.lock().expect("a record no test has poisoned")
    }
}

/// What a call's work came to: the answer, and the file it showed the agent
/// where it showed one, with the digest of what the agent was shown.
///
/// The file travels beside the answer rather than into the record, for the
/// reason [`Ledger::shown`] gives.
#[derive(Debug)]
pub(crate) struct Shown {
    /// What the model is answered with.
    pub(crate) output: ToolOutput,
    /// The resolved path of the file the answer showed, and what was in it.
    pub(crate) file: Option<(PathBuf, Fingerprint)>,
}

impl From<ToolOutput> for Shown {
    /// An answer that showed the agent no file: a refusal, or a failure.
    fn from(output: ToolOutput) -> Self {
        Self { output, file: None }
    }
}

/// The SHA-256 of a file's whole content.
///
/// Compared, never shown: it says whether a file is still what it was, and
/// nothing about what that was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Fingerprint([u8; 32]);

impl Fingerprint {
    /// The digest of content already in memory.
    pub(crate) fn of(content: &[u8]) -> Self {
        Self(Sha256::digest(content).into())
    }

    /// The digest of the rest of `file`, read to its end and kept nowhere.
    ///
    /// `None` where `stopped` said to stop between two reads.
    pub(crate) fn read(file: impl Read, stopped: impl Fn() -> bool) -> io::Result<Option<Self>> {
        Fingerprinting::new(file).finish(stopped)
    }
}

impl From<[u8; 32]> for Fingerprint {
    /// A SHA-256 somebody else already took of the same whole content.
    fn from(digest: [u8; 32]) -> Self {
        Self(digest)
    }
}

/// A reader that takes the digest of every byte read through it.
///
/// For a reader that stops early, such as a page of a file: [`Self::finish`]
/// reads on to the end, so the digest is of the whole file while the bytes
/// shown were only the ones the reader above wanted.
pub(crate) struct Fingerprinting<R> {
    inner: R,
    digest: Sha256,
}

impl<R: Read> Fingerprinting<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            digest: Sha256::new(),
        }
    }

    /// Reads what is left to the end and answers with the digest of all of it.
    ///
    /// `None` where `stopped` said to stop between two reads: a file read
    /// part of the way has no digest worth remembering.
    pub(crate) fn finish(mut self, stopped: impl Fn() -> bool) -> io::Result<Option<Fingerprint>> {
        let mut block = [0_u8; 8 * 1024];
        loop {
            if stopped() {
                return Ok(None);
            }
            if self.read(&mut block)? == 0 {
                return Ok(Some(Fingerprint(self.digest.finalize().into())));
            }
        }
    }
}

impl<R: Read> Read for Fingerprinting<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let read = self.inner.read(buffer)?;
        if let Some(arrived) = buffer.get(..read) {
            self.digest.update(arrived);
        }
        Ok(read)
    }
}

#[cfg(test)]
mod tests {
    use super::{Fingerprint, Fingerprinting, Ledger, REMEMBERED};

    fn at(name: usize) -> std::path::PathBuf {
        std::path::PathBuf::from(format!("/w/file-{name:05}.txt"))
    }

    fn any() -> Fingerprint {
        Fingerprint::of(b"")
    }

    #[test]
    fn a_file_that_was_read_is_held_and_one_that_was_not_is_not() {
        let ledger = Ledger::new();

        ledger.record(&at(1), any());

        assert!(ledger.holds(&at(1)));
        assert!(!ledger.holds(&at(2)));
    }

    #[test]
    fn two_clones_are_one_record_rather_than_two() {
        // The whole reason this type exists: the tool that learns and the tool
        // that asks are different objects.
        let learns = Ledger::new();
        let asks = learns.clone();

        learns.record(&at(1), any());

        assert!(asks.holds(&at(1)));
    }

    #[test]
    fn what_it_remembers_never_grows_past_the_bound() {
        let ledger = Ledger::new();

        for number in 0..REMEMBERED * 2 {
            ledger.record(&at(number), any());
        }

        assert!(!ledger.holds(&at(0)), "it kept the oldest path");
        assert!(ledger.holds(&at(REMEMBERED * 2 - 1)));
        assert_eq!(ledger.seen.lock().unwrap().len(), REMEMBERED);
    }

    #[test]
    fn forgetting_leaves_a_file_that_was_read_needing_to_be_read_again() {
        // What `/clear` and `/resume` run. The session the file was read in is
        // over, so the record of having read it is over with it — and `write`
        // asks for the read again rather than replacing a file this session
        // never saw.
        let ledger = Ledger::new();
        ledger.record(&at(1), any());

        ledger.forget();

        assert!(!ledger.holds(&at(1)));
    }

    #[test]
    fn forgetting_is_forgotten_by_every_clone() {
        // The same reason the record is shared at all: the tool that asks is
        // not the object the command reached.
        let asks = Ledger::new();
        let learns = asks.clone();
        learns.record(&at(1), any());

        asks.forget();

        assert!(!learns.holds(&at(1)));
    }

    #[test]
    fn reading_a_file_again_moves_it_away_from_being_forgotten() {
        // Otherwise the file an agent works on all session is evicted by the
        // thousand it glanced at, which is exactly backwards.
        let ledger = Ledger::new();
        ledger.record(&at(0), any());

        for number in 1..REMEMBERED {
            ledger.record(&at(number), any());
        }
        ledger.record(&at(0), any());
        ledger.record(&at(REMEMBERED), any());

        assert!(ledger.holds(&at(0)));
        assert!(!ledger.holds(&at(1)), "it forgot the wrong one");
    }

    #[test]
    fn a_file_is_remembered_with_what_it_held_when_it_was_last_seen() {
        let ledger = Ledger::new();

        ledger.record(&at(1), Fingerprint::of(b"first\n"));
        ledger.record(&at(1), Fingerprint::of(b"second\n"));

        assert_eq!(
            ledger.fingerprint(&at(1)),
            Some(Fingerprint::of(b"second\n"))
        );
        assert_eq!(ledger.fingerprint(&at(2)), None);
    }

    #[test]
    fn a_read_that_stops_part_way_is_still_the_digest_of_the_whole_file() {
        use std::io::{BufRead as _, BufReader};

        let content = b"shown\nnot shown\n";
        let mut hashed = Fingerprinting::new(&content[..]);
        let mut first = String::new();
        BufReader::new(&mut hashed).read_line(&mut first).unwrap();

        assert_eq!(first, "shown\n");
        assert_eq!(
            hashed.finish(|| false).unwrap(),
            Some(Fingerprint::of(content))
        );
    }

    #[test]
    fn a_digest_stopped_before_its_end_is_none() {
        assert_eq!(Fingerprint::read(&b"anything"[..], || true).unwrap(), None);
    }
}
