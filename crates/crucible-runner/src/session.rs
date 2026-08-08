//! The session log: what makes a conversation survive the process.
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

use std::fs::{File, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crucible_core::{Message, SessionId, Transcript, Workspace};

mod replay;
mod wire;

use replay::{newest, replay};

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

    /// Somewhere to keep sessions could not be worked out.
    #[error("no home directory: set XDG_DATA_HOME or HOME")]
    Homeless,
}

/// Where the first write that failed is left for the main thread to find.
type Trouble = Arc<Mutex<Option<Box<str>>>>;

/// One session's durable record.
#[derive(Debug)]
pub struct Session {
    path: PathBuf,
    /// `None` in a session that records nothing.
    to: Option<SyncSender<Box<str>>>,
    /// Taken by `drop`, which is where the queue is waited for.
    writer: Option<JoinHandle<()>>,
    trouble: Trouble,
}

impl Session {
    /// Begins recording a new session in `directory`.
    ///
    /// # Errors
    ///
    /// [`SessionError`] when the directory or the file cannot be made.
    pub fn start(directory: &Path, workspace: &Workspace) -> Result<Self, SessionError> {
        // 0700 for the same reason the logs themselves are 0600: the names
        // alone say which directories were worked in and when.
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|source| SessionError::Directory {
                at: directory.display().to_string().into(),
                source,
            })?;

        // As with the logs, the mode above applies only to a directory this
        // call creates — a sessions directory left by an earlier build keeps
        // whatever it was made with until something narrows it.
        let permissions = std::fs::Permissions::from_mode(0o700);
        std::fs::set_permissions(directory, permissions).map_err(|source| {
            SessionError::Directory {
                at: directory.display().to_string().into(),
                source,
            }
        })?;

        let id = SessionId::new();
        let path = directory.join(format!("{}.{SUFFIX}", id.as_str()));
        let mut file = open(&path)?;

        let header = wire::header(&id, workspace.root());
        writeln!(file, "{header}").map_err(|source| SessionError::Log {
            at: path.display().to_string().into(),
            source,
        })?;

        Ok(Self::writing(path, file))
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
        let path = newest(directory, workspace)?;
        let transcript = replay(&path)?;

        Ok((Self::writing(path.clone(), open(&path)?), transcript))
    }

    /// A session that records nothing, for a run that asked not to be kept.
    #[must_use]
    pub fn nowhere() -> Self {
        Self {
            path: PathBuf::new(),
            to: None,
            writer: None,
            trouble: Trouble::default(),
        }
    }

    /// Which file this session is being written to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Records one message.
    ///
    /// Returns without waiting. The write happens on the session's thread,
    /// which is the whole reason there is one.
    pub fn append(&self, message: &Message) {
        let Some(to) = &self.to else { return };
        drop(to.send(wire::line(message).into()));
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

    /// Where sessions are kept, from the environment.
    ///
    /// Read here rather than deeper down: this is the boundary, and everything
    /// below it is handed a path.
    ///
    /// # Errors
    ///
    /// [`SessionError::Homeless`] when neither `XDG_DATA_HOME` nor `HOME` says
    /// anywhere to put them.
    pub fn directory() -> Result<PathBuf, SessionError> {
        let under = |name: &str, rest: &str| {
            std::env::var_os(name)
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
                .map(|path| path.join(rest))
        };

        under("XDG_DATA_HOME", "crucible/sessions")
            .or_else(|| under("HOME", ".local/share/crucible/sessions"))
            .ok_or(SessionError::Homeless)
    }

    /// Starts the thread that owns `file` from here on.
    fn writing(path: PathBuf, file: File) -> Self {
        let (to, lines) = sync_channel(QUEUE);
        let trouble = Trouble::default();
        let mine = Arc::clone(&trouble);

        let writer = thread::spawn(move || write(file, &lines, &mine));

        Self {
            path,
            to: Some(to),
            writer: Some(writer),
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

/// Appends every line that arrives until the session is dropped.
///
/// A failure is recorded once and the loop goes on, because the senders are
/// not waiting for an answer: stopping here would fill the queue and block the
/// turn instead of losing a log nobody can write anyway.
fn write(mut file: File, lines: &Receiver<Box<str>>, trouble: &Trouble) {
    for line in lines {
        let Err(problem) = writeln!(file, "{line}") else {
            continue;
        };

        if let Ok(mut held) = trouble.lock() {
            held.get_or_insert_with(|| problem.to_string().into());
        }
    }
}

/// Opens a log for appending, making it if it is not there.
///
/// The mode is the user's alone. A log holds what was typed, what the model
/// said, the contents of files that were read and everything a command printed
/// — so the default 0644 would put a session on a shared machine within reach
/// of anybody with an account on it, and a group-writable umask would let them
/// append lines that `--continue` later replays as though the user had typed
/// them.
fn open(path: &Path) -> Result<File, SessionError> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .map_err(|source| SessionError::Log {
            at: path.display().to_string().into(),
            source,
        })?;

    // `mode` applies only when the call creates the file, so a log written
    // before this rule existed keeps its old permissions until it is resumed.
    let permissions = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(path, permissions).map_err(|source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    })?;

    Ok(file)
}

#[cfg(test)]
mod tests;
