//! The session log: what makes a session survive the process.
//!
//! One file per session, one line per message, appended in order and never
//! rewritten. Append-only is what makes a crash cost the last line rather than
//! the file: there is no offset to seek to, no length to update, and nothing
//! that has already been written can be left half-changed.
//!
//! Writing happens on the session's own thread. The thread that draws must not
//! wait for a disk, and a queue is how it stops having to.
//!
//! Durability here means "survives the process", not "survives the machine":
//! each line reaches the operating system as it is recorded, and nothing calls
//! `fsync`. Paying milliseconds per message to also survive a power cut is not
//! the trade a coding session wants.

use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use crucible_core::{Message, SessionId, Transcript, Workspace};

mod claim;
mod log;
mod privacy;
mod recent;
mod replay;
mod wire;

use claim::{Claim, claim};
use log::{Trouble, open, shorten};
pub use recent::{Recorded, recent};
use replay::{belongs, newest, replay};

/// How many lines may be waiting to be written.
///
/// A local file drains far faster than turns produce messages, so this is
/// never reached in practice. When it is, recording blocks — which is the
/// right answer for a durable log, and a wrong one for a queue that drops.
const QUEUE: usize = 256;

/// What a session log is called.
const SUFFIX: &str = "jsonl";

/// Why a session could not be recorded or continued.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// The directory sessions live in could not be made or read.
    #[error("could not use the session directory {at}: {source}")]
    Directory {
        /// Where.
        at: Box<str>,
        /// What the operating system said.
        source: io::Error,
    },

    /// One session file could not be opened or read.
    #[error("could not read the session log {at}: {source}")]
    Log {
        /// Which file.
        at: Box<str>,
        /// What the operating system said.
        source: io::Error,
    },

    /// `--continue` was asked for where nothing has been recorded.
    #[error("no earlier session for {at}")]
    Nothing {
        /// The workspace that has none.
        at: Box<str>,
    },

    /// A log this build does not understand.
    #[error("{at} was written by a different version of crucible")]
    Foreign {
        /// Which file.
        at: Box<str>,
    },

    /// The session asked for is still being written by another crucible.
    #[error("{at} is open in another crucible")]
    Busy {
        /// Which file.
        at: Box<str>,
    },

    /// A session named outright that this directory has no log of.
    #[error("no session {id} recorded for {at}")]
    Unknown {
        /// Which session was asked for.
        id: Box<str>,
        /// The workspace it was asked for in.
        at: Box<str>,
    },
}

/// One session's durable record.
#[derive(Debug)]
pub struct Session {
    path: PathBuf,
    /// Which session this is, read back from what its log is called. `None`
    /// where there is no log to be named by.
    id: Option<SessionId>,
    /// `None` in a session that records nothing.
    to: Option<SyncSender<Box<str>>>,
    /// Taken by whichever of [`Session::finish`] and `drop` comes first, both
    /// of which wait for the queue.
    writer: Option<JoinHandle<()>>,
    /// What keeps another crucible from continuing a log this one is still
    /// writing. `None` where the filesystem has no locks to take, and in a
    /// session that records nothing — see [`claim`].
    ///
    /// Released after the queue has drained: [`Drop`] runs before a struct's
    /// fields do, and joining the writer is the first thing it does.
    claim: Option<Claim>,
    trouble: Trouble,
}

impl Session {
    /// Begins recording a new session in `directory`.
    ///
    /// # Errors
    ///
    /// [`SessionError`] when the directory or the file cannot be made.
    pub fn start(directory: &Path, workspace: &Workspace) -> Result<Self, SessionError> {
        privacy::directory(directory).map_err(|source| SessionError::Directory {
            at: directory.display().to_string().into(),
            source,
        })?;

        let id = SessionId::new();
        let path = directory.join(format!("{}.{SUFFIX}", id.as_str()));
        let mut file = open(&path)?;

        let header = wire::header(&id, workspace.root());
        writeln!(file, "{header}").map_err(|source| SessionError::Log {
            at: path.display().to_string().into(),
            source,
        })?;

        // Nothing can be holding a log this run has just named, so there is
        // nothing to refuse here. It is claimed so that a crucible started
        // afterwards in the same directory finds this session open for as long
        // as it is being written, rather than continuing it underneath.
        let held = claim(&path).ok().flatten();

        let mut session = Self::writing(path, file);
        session.claim = held;

        Ok(session)
    }

    /// Continues the newest session recorded for `workspace`, and hands back
    /// everything it already holds.
    ///
    /// # Errors
    ///
    /// [`SessionError`] when there is nothing to continue, or when what is
    /// there cannot be read.
    pub fn resume(
        directory: &Path,
        workspace: &Workspace,
    ) -> Result<(Self, Transcript), SessionError> {
        // Narrowed here as well as in `start`, because this is the path that
        // reads what is in the directory rather than only adding to it. A
        // directory another account can write to lets them drop in a log with
        // this workspace in its header and a name that sorts late, and
        // `--continue` replays whatever it finds as though the user had typed
        // it.
        privacy::directory(directory).map_err(|source| SessionError::Directory {
            at: directory.display().to_string().into(),
            source,
        })?;

        Self::continuing(&newest(directory, workspace)?)
    }

    /// Picks up the session `id` names, rather than the newest one.
    ///
    /// What `/resume` runs. The identifier comes from a list this build made of
    /// this directory's logs, so the log is checked to be one — a file that is
    /// gone, a workspace this is not, a format this build does not read — the
    /// same way [`Session::resume`] checks the one it found. Naming a session
    /// is a shorter way to reach one, not a way past what is asked of it.
    ///
    /// # Errors
    ///
    /// [`SessionError`] when no such session is recorded here, or when what is
    /// there cannot be read.
    pub fn reopen(
        directory: &Path,
        workspace: &Workspace,
        id: &SessionId,
    ) -> Result<(Self, Transcript), SessionError> {
        privacy::directory(directory).map_err(|source| SessionError::Directory {
            at: directory.display().to_string().into(),
            source,
        })?;

        // A log written by a build that spelled things differently fails here
        // rather than being dropped: `belongs` says so, and being told which
        // version wrote it is the answer worth having.
        let path = directory.join(format!("{}.{SUFFIX}", id.as_str()));

        if !path.is_file() || !belongs(&path, workspace)? {
            return Err(SessionError::Unknown {
                id: id.as_str().into(),
                at: workspace.root().display().to_string().into(),
            });
        }

        Self::continuing(&path)
    }

    /// The half of continuing a session that starts once the log is known.
    ///
    /// Both ways in reach it: the newest log for this directory, and the one a
    /// session was named by. What a continued session has to do to a log is the
    /// same either way, and it is the order of it that matters — claim, read,
    /// cut, and only then open the handle that appends.
    fn continuing(path: &Path) -> Result<(Self, Transcript), SessionError> {
        // Before the log is read, and long before it is cut. A session another
        // crucible still has open is a file that is still being appended to:
        // continuing it cuts it back to what was read here, which deletes lines
        // that process has already written and believes are there, and leaves
        // two of them appending to one log. See [`claim`].
        let held = match claim(path) {
            Ok(Some(held)) => Some(held),
            Ok(None) => {
                return Err(SessionError::Busy {
                    at: path.display().to_string().into(),
                });
            }
            // A filesystem with no locks cannot say a session is open either,
            // and refusing every `--continue` there would cost more than the
            // collision this guards against. What happens then is what happened
            // before there was a claim to take.
            Err(_) => None,
        };

        let (transcript, settled_at) = replay(path)?;

        // Before a single byte is appended, and before the handle that will
        // append them exists: whatever `replay` stopped at would otherwise have
        // the next turn written straight onto the end of it. See [`replay`].
        shorten(path, settled_at)?;

        let mut session = Self::writing(path.to_owned(), open(path)?);
        session.claim = held;

        Ok((session, transcript))
    }

    /// A session that records nothing, for a run that asked not to be kept.
    #[must_use]
    pub fn nowhere() -> Self {
        Self {
            path: PathBuf::new(),
            id: None,
            to: None,
            writer: None,
            claim: None,
            trouble: Trouble::default(),
        }
    }

    /// Which file this session is being written to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Which session this is.
    ///
    /// `None` in a session that records nothing: there is no log, so there is
    /// nothing for it to be named by. What reads this is a list somebody is
    /// picking from — the one they are already in is on it, and answering that
    /// with "open in another crucible" would name the wrong crucible.
    #[must_use]
    pub fn id(&self) -> Option<&SessionId> {
        self.id.as_ref()
    }

    /// Records one message.
    ///
    /// Returns without waiting. The write happens on the session's thread,
    /// which is the whole reason there is one.
    pub fn append(&self, message: &Message) {
        let Some(to) = &self.to else { return };
        drop(to.send(wire::line(message).into()));
    }

    /// Records that everything said so far has been forgotten.
    ///
    /// Written down rather than cut out. What was said still happened, and a
    /// log that quietly lost a stretch of it would be a worse record than one
    /// that says where the session started again.
    pub fn forgot(&self) {
        let Some(to) = &self.to else { return };
        drop(to.send(wire::forgotten().into()));
    }

    /// The first write that failed, if one has.
    ///
    /// A log that has quietly stopped recording is worse than no log, because
    /// the user finds out when they try to continue it.
    #[must_use]
    pub fn trouble(&self) -> Option<Box<str>> {
        self.trouble
            .lock()
            .ok()
            .and_then(|held| held.as_ref().cloned())
    }

    /// Ends the session and hands back the first write that failed, if one did.
    ///
    /// [`Session::trouble`] can only report what the writer thread has already
    /// reached. When a loop ends, the last turn is usually still in the queue —
    /// so the failure worth reporting most is the one nothing has had a chance
    /// to see. Dropping the sender and joining here is what turns that into an
    /// answer, and [`Drop`] alone would do the same draining with nobody left
    /// to tell.
    #[must_use]
    pub fn finish(mut self) -> Option<Box<str>> {
        self.to = None;
        if let Some(writer) = self.writer.take() {
            drop(writer.join());
        }

        self.trouble()
    }

    /// A session that records onto `sink` instead of a file.
    ///
    /// For proving what happens when a log stops working, from code that lives
    /// outside this crate — the wiring in the binary that tells the user is the
    /// one part of that path a test here cannot reach. Behind a feature no
    /// dependency turns on, so it is absent from a release build.
    #[cfg(feature = "proof")]
    pub fn onto<W: io::Write + Send + 'static>(path: PathBuf, sink: W) -> Self {
        Self::writing(path, sink)
    }

    /// Starts the thread that owns `sink` from here on.
    ///
    /// `sink` is a type parameter rather than a [`File`] so that a test can
    /// drive a log which has stopped working: what this module has to get right
    /// is what happens *after* a write fails, and a real file on a real disk
    /// will not fail on request.
    fn writing<W: io::Write + Send + 'static>(path: PathBuf, sink: W) -> Self {
        let (to, lines) = sync_channel(QUEUE);
        let trouble = Trouble::default();
        let mine = Arc::clone(&trouble);

        let writer = thread::spawn(move || log::write(sink, &lines, &mine));

        // Read back from the name rather than carried in, so that the two ways
        // to reach a log — minting a name, and finding one — cannot disagree
        // about which session is being written.
        let id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .and_then(|stem| SessionId::from_str(stem).ok());

        Self {
            path,
            id,
            to: Some(to),
            writer: Some(writer),
            claim: None,
            trouble,
        }
    }
}

impl Drop for Session {
    /// Waits for what is queued to reach the disk.
    ///
    /// Dropping the sender is what ends the writer's loop, and joining it is
    /// what makes "the log is complete once the process is gone" true rather
    /// than likely.
    fn drop(&mut self) {
        self.to = None;

        if let Some(writer) = self.writer.take() {
            drop(writer.join());
        }
    }
}

#[cfg(test)]
mod tests;
