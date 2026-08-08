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
use std::io::{self, BufRead, BufReader, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use crucible_core::{Message, SessionId, Transcript, Workspace};

mod wire;

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
        std::fs::create_dir_all(directory).map_err(|source| SessionError::Directory {
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
fn open(path: &Path) -> Result<File, SessionError> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|source| SessionError::Log {
            at: path.display().to_string().into(),
            source,
        })
}

/// The newest log recorded for `workspace`.
///
/// Session identifiers sort by start time as text, so the newest is found by
/// name and only the first line of each candidate is read.
fn newest(directory: &Path, workspace: &Workspace) -> Result<PathBuf, SessionError> {
    let mut logs = Vec::new();

    let entries = std::fs::read_dir(directory).map_err(|source| SessionError::Directory {
        at: directory.display().to_string().into(),
        source,
    })?;

    for entry in entries.flatten() {
        let path = entry.path();
        let named = path.file_stem().and_then(|stem| stem.to_str());

        if path.extension().is_some_and(|end| end == SUFFIX)
            && named.is_some_and(|stem| SessionId::from_str(stem).is_ok())
        {
            logs.push(path);
        }
    }

    logs.sort_unstable();

    for path in logs.into_iter().rev() {
        if belongs(&path, workspace)? {
            return Ok(path);
        }
    }

    Err(SessionError::Nothing {
        at: workspace.root().display().to_string().into(),
    })
}

/// Whether `path` is a log of a session in `workspace`.
///
/// A log this build does not understand is refused rather than skipped: the
/// answer to "continue my last session" must never be a different one.
fn belongs(path: &Path, workspace: &Workspace) -> Result<bool, SessionError> {
    let trouble = |source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    };

    let mut first = String::new();
    BufReader::new(File::open(path).map_err(trouble)?)
        .read_line(&mut first)
        .map_err(trouble)?;

    let Some(opening) = wire::opening(first.trim_end()) else {
        return Ok(false);
    };

    if Path::new(&opening.workspace) != workspace.root() {
        return Ok(false);
    }

    if opening.format == wire::FORMAT {
        Ok(true)
    } else {
        Err(SessionError::Foreign {
            at: path.display().to_string().into(),
        })
    }
}

/// Everything a log holds, as the transcript it recorded.
///
/// Reading stops at the first line that cannot be read as a message. A session
/// that ended when the process did leaves a half-written last line, and a
/// prefix of the conversation is a conversation; one with a hole in it is not.
fn replay(path: &Path) -> Result<Transcript, SessionError> {
    let file = File::open(path).map_err(|source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    })?;

    let mut transcript = Transcript::new();
    for line in BufReader::new(file).lines().skip(1).map_while(Result::ok) {
        let Some(message) = wire::message(&line) else {
            break;
        };
        transcript.push(message);
    }

    Ok(settled(transcript))
}

/// The transcript without a last message that is still waiting on tools.
///
/// A process that died between asking for a tool and recording its result
/// leaves calls nothing ever answered. Sending that on is not a cosmetic
/// problem: a provider is entitled to reject a transcript whose last word is
/// an unanswered question.
fn settled(transcript: Transcript) -> Transcript {
    let outstanding = matches!(
        transcript.messages().last(),
        Some(Message::Agent { calls, .. }) if !calls.is_empty()
    );

    if !outstanding {
        return transcript;
    }

    let mut settled = Transcript::new();
    for message in transcript.messages().iter().rev().skip(1).rev() {
        settled.push(message.clone());
    }

    settled
}

#[cfg(test)]
mod tests;
