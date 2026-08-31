//! One file per session, one line per message, appended in order. Nothing
//! already written is ever changed, which is what makes a crash cost the last
//! line rather than the file: there is no offset to seek to, no length to
//! update, and nothing half-changed to leave behind.
//!
//! One file *per session* is a claim about the name as much as about the
//! writing. A session starting creates its log rather than opening it, so the
//! name it recorded under is one no other crucible was handed — see [`taking`],
//! which is where the two ways of already having a name are refused.
//!
//! The one cut is where continuing starts. `--continue` shortens the file to
//! the end of the last message the replay could settle on — before a line a
//! crash tore in half, before a tool call nothing ever answered — and does it
//! before the handle that appends exists. A log already ending there loses
//! nothing, which is every ordinary run. It is a truncation and not a rewrite:
//! what survives is byte for byte what was written.

use std::fs::File;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{SyncSender, sync_channel};
use std::thread::{self, JoinHandle};

use crucible_core::{
    Calibration, ContextError, ContextPatch, ContextSnapshot, Message, SessionId, Transcript,
    Workspace,
};

mod beside;
mod claim;
mod glimpse;
mod index;
mod log;
mod privacy;
mod prompts;
mod recent;
mod replay;
mod wire;

use claim::{Claim, Claimed, claim};
pub use glimpse::{Glimpse, glimpse};
use log::{Trouble, make, open, shorten};
pub use prompts::{PROMPTS, prompts, remember};
pub use recent::{Recorded, recent};
pub use replay::Pruned;
use replay::{Replayed, belongs, newest, replay};

/// How many lines may be waiting to be written.
///
/// A local file drains far faster than turns produce messages, so this is
/// never reached in practice. When it is, recording blocks — which is the
/// right answer for a durable log, and a wrong one for a queue that drops.
const QUEUE: usize = 256;

/// What a session log is called.
const SUFFIX: &str = "jsonl";

/// How many names a new session tries before it gives up.
///
/// A name carries seventy-four bits of randomness inside one millisecond, so a
/// second attempt is already a rarity and a third is one nobody will see. What
/// the bound is really for is the other reason every name comes back taken — a
/// directory that has stopped behaving — where a loop with no end to it is a
/// start-up that hangs with nothing on screen instead of a session that says
/// what went wrong.
const NAMES: usize = 8;

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

    /// The bounded newest-session index could not be read or replaced.
    #[error("could not use the session index {at}: {source}")]
    Index {
        /// Which file.
        at: Box<str>,
        /// What the operating system said.
        source: io::Error,
    },

    /// The bounded prompt history could not be read or replaced.
    ///
    /// Separate from [`SessionError::Index`] because they are separate files
    /// with separate bounds, and the name in the message is the whole of what
    /// tells a reader which one stopped working.
    #[error("could not use the prompt history {at}: {source}")]
    History {
        /// Which file.
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

    /// The mark that says a session is open could not be made, so whether
    /// anything holds the log was never established.
    #[error("could not claim the session log {at}: {source}")]
    Claim {
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

    /// Every name a new session tried was already spoken for.
    ///
    /// A name carries a millisecond and seventy-four bits of randomness, so one
    /// collision is rare and a run of them says something else is wrong with
    /// the directory. Reported rather than retried for ever: a session that
    /// cannot be named is one the user can be told about, and a loop that never
    /// gives up is a start-up that hangs with nothing on screen.
    #[error("could not find a free name for a new session in {at}")]
    Taken {
        /// The directory it tried in.
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

    /// A persisted context patch could not reconstruct a valid snapshot.
    #[error("could not replay context in {at}: {source}")]
    Context {
        /// Which log contained the invalid patch.
        at: Box<str>,
        /// Why the patch could not represent typed context.
        source: ContextError,
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
    /// What the log said its last request carried, where it was picked up and
    /// where the log still says. `None` in a session that was started rather
    /// than continued, and in one whose log stopped before it could say.
    calibration: Option<Calibration>,
    /// Typed model-visible state reconstructed from context patches.
    ///
    /// `None` is the compatibility state for a pre-context session that has
    /// not yet written its first patch. It means unknown, not empty.
    context: Option<ContextSnapshot>,
    /// How many conversation messages this session holds, kept as they are
    /// appended.
    ///
    /// A continued session starts from what the replay handed back, so the
    /// number is what a continue would replay except internal context records
    /// — a compacted log counts its conversation transcript, not its lines.
    /// Written to the index once, from [`Drop`], which is also what repairs the
    /// zero a legacy session was indexed with.
    messages: AtomicUsize,
    /// What the results a pruning cleared said, for whatever draws the session
    /// back onto a screen. Empty in a session that was started rather than
    /// continued, and in one whose log never pruned anything.
    pruned: Pruned,
    trouble: Trouble,
}

impl Session {
    /// Begins recording a new session in `directory`.
    ///
    /// `branch` is what the workspace's version control had checked out, where
    /// the caller could look — this crate does not run git. It goes into the
    /// header once and is served back by [`recent`]; a caller that cannot say
    /// passes `None` and the header simply never learns one.
    ///
    /// # Errors
    ///
    /// [`SessionError`] when the directory or the file cannot be made, and
    /// [`SessionError::Taken`] where every name it minted was already spoken
    /// for.
    pub fn start(
        directory: &Path,
        workspace: &Workspace,
        branch: Option<&str>,
    ) -> Result<Self, SessionError> {
        privacy::directory(directory).map_err(|source| SessionError::Directory {
            at: directory.display().to_string().into(),
            source,
        })?;

        // A legacy scan happens here, after the first frame, and once. The
        // welcome itself reads only the fixed index this makes.
        index::ensure(directory)?;

        Self::naming(directory, workspace, branch, SessionId::new)
    }

    /// Starts a session under the first name `name` mints that nothing holds.
    ///
    /// The minting is a parameter so that a test can hand over one that is
    /// already taken. A collision needs two of seventy-four random bits in one
    /// millisecond, so this is the path no ordinary run reaches — and code
    /// nothing has ever run is code nobody knows the behaviour of.
    fn naming(
        directory: &Path,
        workspace: &Workspace,
        branch: Option<&str>,
        mut name: impl FnMut() -> SessionId,
    ) -> Result<Self, SessionError> {
        for _ in 0..NAMES {
            let id = name();
            let path = directory.join(format!("{}.{SUFFIX}", id.as_str()));

            let Some((mut file, held)) = taking(&path)? else {
                continue;
            };

            // Before the header: a crash between these steps leaves a name the
            // index reader validates and skips. The opposite order could leave
            // a complete session no bounded lookup could discover.
            if let Err(problem) = index::record(directory, &id) {
                drop(file);
                drop(held);
                let _ = std::fs::remove_file(&path);
                return Err(problem);
            }

            let header = wire::header(&id, workspace.root(), branch);
            writeln!(file, "{header}").map_err(|source| SessionError::Log {
                at: path.display().to_string().into(),
                source,
            })?;

            let mut session = Self::writing(path, file);
            session.claim = held;

            return Ok(session);
        }

        Err(SessionError::Taken {
            at: directory.display().to_string().into(),
        })
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

        // An upgraded `--continue` preserves flat legacy sessions. This scan
        // runs after the first frame and is replaced by fixed index reads from
        // then on.
        index::ensure(directory)?;

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
        let held = match claimed(path)? {
            Claimed::Taken(held) => Some(held),
            Claimed::Busy => {
                return Err(SessionError::Busy {
                    at: path.display().to_string().into(),
                });
            }
            // A filesystem with no locks cannot say a session is open either,
            // and refusing every `--continue` there would cost more than the
            // collision this guards against. What happens then is what happened
            // before there was a claim to take.
            Claimed::Lockless => None,
        };

        let Replayed {
            transcript,
            settled_at,
            calibration,
            context,
            pruned,
        } = replay(path)?;

        // Before a single byte is appended, and before the handle that will
        // append them exists: whatever `replay` stopped at would otherwise have
        // the next turn written straight onto the end of it. See [`replay`].
        shorten(path, settled_at)?;

        let mut session = Self::writing(path.to_owned(), open(path)?);
        session.claim = held;
        session.calibration = calibration;
        session.context = context;
        session.pruned = pruned;
        // What a continue replays is what the session now holds, except typed
        // context records are harness state rather than messages somebody
        // said: a stale or legacy index count is repaired from here when the
        // session ends.
        session.messages = AtomicUsize::new(
            transcript
                .messages()
                .iter()
                .filter(|message| is_conversation_message(message))
                .count(),
        );

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
            calibration: None,
            context: Some(ContextSnapshot::new()),
            messages: AtomicUsize::new(0),
            pruned: Pruned::default(),
            trouble: Trouble::default(),
        }
    }

    /// What the results a pruning cleared said, before it cleared them.
    ///
    /// Taken rather than borrowed, and taken once. A pruning exists to give
    /// back the bytes a long conversation is carrying, and this is those exact
    /// bytes: leaving them on the session would hold, for as long as the run
    /// lasts, what the pruning was run to let go of. The screen wants them at
    /// the replay and never again, so it takes them and drops them there.
    ///
    /// Empty in a session that was started rather than continued, and in one
    /// whose log never cleared anything.
    pub fn take_pruned(&mut self) -> Pruned {
        std::mem::take(&mut self.pruned)
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
        if is_conversation_message(message) {
            self.messages.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Records one per-pass merge patch and advances the reconstructed state.
    ///
    /// A legacy session has no baseline, so its first patch applies to empty
    /// state and establishes one. Only the patch is written; the full snapshot
    /// remains an in-memory replay product.
    ///
    /// # Errors
    ///
    /// [`ContextError`] if a caller supplied a persisted-style patch that
    /// cannot produce a valid snapshot.
    pub fn contextual(&mut self, patch: &ContextPatch) -> Result<(), ContextError> {
        let prior = self.context.clone().unwrap_or_default();
        let current = patch.apply(&prior)?;
        if let Some(to) = &self.to {
            drop(to.send(wire::contextual(patch).into()));
        }
        self.context = Some(current);
        Ok(())
    }

    /// The typed state reconstructed for this session.
    ///
    /// `None` means a pre-context session whose model-visible fragments have
    /// unknown vintage. It must never be read as a known empty snapshot.
    #[must_use]
    pub const fn context_snapshot(&self) -> Option<&ContextSnapshot> {
        self.context.as_ref()
    }

    /// Records that room was made, and what the notes stand in place of.
    ///
    /// The messages it replaced stay in the file. This log is what happened;
    /// the transcript is what the model is sent, and here is where the two stop
    /// being the same thing. A session continued later reads this line and
    /// leaves those messages out of the transcript without losing them from the
    /// record.
    pub fn compacted(&self, replaced: usize, recap: &str) {
        let Some(to) = &self.to else { return };
        drop(to.send(wire::compacted(replaced, recap).into()));
    }

    /// Records that old tool results were cleared, and which.
    ///
    /// The results stay in the file holding what they held — the log is the
    /// record — and a session continued later reads this line and clears them
    /// from the transcript again, so what the model is sent matches across the
    /// continue. Written the way the compaction line is, for the same reason.
    pub fn pruned(&self, freed: usize, results: &[crucible_core::ToolId]) {
        let Some(to) = &self.to else { return };
        drop(to.send(wire::pruned(freed, results).into()));
    }

    /// Records what the request behind the answer just written carried.
    ///
    /// Written after that message and never beside it, because order is what
    /// says which transcript the reading covers: a log is read forwards, and
    /// everything above this line is what was sent to get the answer above it.
    ///
    /// One line per answer, and only where the numbers are exact. A response
    /// that reported half of itself, or a model changed since, writes nothing —
    /// a session that comes back estimating is the behaviour this replaces, and
    /// it is a great deal better than one that comes back sure and wrong.
    pub fn measured(&self, calibration: &Calibration) {
        let Some(to) = &self.to else { return };
        drop(to.send(wire::measured(calibration).into()));
    }

    /// What the log this session was picked up from last said it carried.
    ///
    /// `None` for a session that was started here, and for one whose log ends
    /// with anything other than that line — see `replay`.
    #[must_use]
    pub const fn calibrated(&self) -> Option<Calibration> {
        self.calibration
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
            calibration: None,
            context: Some(ContextSnapshot::new()),
            messages: AtomicUsize::new(0),
            pruned: Pruned::default(),
            trouble,
        }
    }
}

/// Whether one persisted transcript entry belongs in the picker-facing count.
fn is_conversation_message(message: &Message) -> bool {
    !matches!(message, Message::Context(_))
}

/// Saves `title` as what every later listing calls the session `id` names.
///
/// The override sits in the index rather than the log: the log is the record
/// of what happened, and what a row is called is presentation. It is flattened
/// and bounded where it is written — see [`Recorded::title`] for how it is
/// served back — and a title that flattens to nothing clears the override. A
/// session the fixed newest window has let go of is left as it is.
///
/// # Errors
///
/// [`SessionError`] when the index could not be read or replaced.
pub fn retitle(directory: &Path, id: &SessionId, title: &str) -> Result<(), SessionError> {
    index::retitle(directory, id, title)
}

/// Takes `path` for a session starting now, or `None` where the name is
/// already spoken for.
///
/// Two things have to be true of a name, and they are two because they fail
/// apart. Nothing may be holding a claim on it: a mark another crucible has
/// locked is a session that name belongs to, whatever the directory shows,
/// since the log it belongs to can be renamed or deleted underneath it.
/// `--continue` meets that same answer and stops, because there the busy
/// session is the one the user asked for; nobody asked for this one, so it is a
/// name to step over rather than a refusal to report. Read instead as the third
/// answer — a filesystem with no locks — a session would start unguarded on a
/// name another crucible believes is its own.
///
/// And no log may stand there already. That is the filesystem's own answer
/// rather than a lock's, which is what makes it the one that holds between two
/// crucibles minting the same name in the same millisecond: exactly one of them
/// creates the file, and the loser is told so. It is also the only guard left
/// where locks are not to be had.
///
/// The claim goes first so that a name refused here leaves nothing behind to
/// clean up. The mark it makes sits beside a log that already exists, or beside
/// the one written a line later; a log created before a claim was tried would
/// have to be deleted again, which is the one operation on a session directory
/// that can take somebody else's file with it.
///
/// # Errors
///
/// [`SessionError`] when the claim could not be attempted at all — see
/// [`claim`] — or when the log could not be made for any reason other than
/// already being there. A mark that cannot be made stops the session rather
/// than costing it a name: it goes in a directory the caller has just made, so
/// what failed is that directory, and every name minted after this one would
/// fail in the same place.
fn taking(path: &Path) -> Result<Option<(File, Option<Claim>)>, SessionError> {
    let held = match claimed(path)? {
        Claimed::Taken(held) => Some(held),
        Claimed::Busy => return Ok(None),
        Claimed::Lockless => None,
    };

    Ok(make(path)?.map(|file| (file, held)))
}

/// Claims `log`, with the one thing that is not an answer about it named as an
/// error.
///
/// Both ways into a session go through here, so neither of them can be the one
/// that reads a claim it could not attempt as a claim it attempted.
fn claimed(log: &Path) -> Result<Claimed, SessionError> {
    claim(log).map_err(|source| SessionError::Claim {
        at: log.display().to_string().into(),
        source,
    })
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

        // After the join, so the count the index keeps is of lines that made
        // it to the disk. Ignored on failure like every write the index gets:
        // it is decoration on the welcome screen, not the record.
        if let (Some(id), Some(directory)) = (self.id.as_ref(), self.path.parent()) {
            let _ = index::tally(directory, id, self.messages.load(Ordering::Relaxed));
        }
    }
}

#[cfg(test)]
mod tests;
