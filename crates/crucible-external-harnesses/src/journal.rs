//! The account operations on one credential-sharing scope that have not
//! finished, kept so that a crash cannot hide one.
//!
//! A journal is one owner-only file per scope, in a directory
//! `crucible-privacy` keeps owner-only, replaced whole and durably at every
//! stage, so what a restart reads is the stage before a write or the stage
//! after it and never half of one. It is crash-recovery evidence and nothing
//! else: it records an operation's number, the work it does, the instance and
//! profile it acts for by their non-secret names, the generation it was begun
//! under and the stage it reached, and never a token, a code, a URL or
//! anything the vendor's program said. It holds at most one open operation and
//! [`HISTORY`] finished ones, and a file larger than [`JOURNAL_BYTES`] is not
//! read.
//!
//! Operations on one scope are serialized by a lock on a file beside the
//! journal, which every Crucible process meets: a second operation is refused
//! while one is held, rather than waited for, because the first may be
//! waiting on a person signing in.

use std::collections::VecDeque;
use std::fmt::{self, Write as _};
use std::fs::{self, File};
use std::io::{self, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crucible_execution::Ending;
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};

/// The most finished operations a journal keeps, for the account they give of
/// what happened last. Older ones are dropped first.
pub const HISTORY: usize = 16;

/// The largest journal read. Every record at its largest reference is well
/// under this, so a larger file is not one this crate wrote.
pub const JOURNAL_BYTES: usize = 64 * 1024;

/// The most bytes of a scope, instance or profile name kept: a name, never a
/// value.
pub const REFERENCE_BYTES: usize = 512;

/// The journal format this crate writes and reads.
const FORMAT: u64 = 1;

/// The work an account operation does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Work {
    /// Checking the account, signing in where it needs to, and enabling the
    /// instance.
    Connect,
    /// Taking the instance out of use, leaving the vendor's sign-in alone.
    Disconnect,
    /// Signing the vendor's account out.
    SignOut,
    /// Removing the instance's settings once it is out of use.
    Remove,
}

impl Work {
    /// Every work.
    pub const ALL: [Self; 4] = [Self::Connect, Self::Disconnect, Self::SignOut, Self::Remove];

    /// Its name in the journal.
    const fn name(self) -> &'static str {
        match self {
            Self::Connect => "connect",
            Self::Disconnect => "disconnect",
            Self::SignOut => "sign-out",
            Self::Remove => "remove",
        }
    }

    /// The stages it passes through, in order.
    #[must_use]
    pub const fn stages(self) -> &'static [Stage] {
        match self {
            Self::Connect => &[
                Stage::Begun,
                Stage::Mutating,
                Stage::Verifying,
                Stage::Committing,
            ],
            Self::Disconnect | Self::Remove => &[
                Stage::Begun,
                Stage::Closed,
                Stage::Quiesced,
                Stage::Committing,
            ],
            Self::SignOut => &[
                Stage::Begun,
                Stage::Closed,
                Stage::Quiesced,
                Stage::Mutating,
                Stage::Verifying,
            ],
        }
    }
}

/// How far an operation has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Stage {
    /// Its slot is held and this record is kept; nothing has changed yet.
    Begun,
    /// New work on the instances it affects is refused, and that refusal is
    /// kept.
    Closed,
    /// The work those instances had running has stopped.
    Quiesced,
    /// The vendor's program is changing the account, so it may have changed
    /// whether or not this finishes.
    Mutating,
    /// The outcome is being read back.
    Verifying,
    /// The configuration is being written.
    Committing,
}

impl Stage {
    /// Every stage, in order.
    pub const ALL: [Self; 6] = [
        Self::Begun,
        Self::Closed,
        Self::Quiesced,
        Self::Mutating,
        Self::Verifying,
        Self::Committing,
    ];

    /// Its name in the journal.
    const fn name(self) -> &'static str {
        match self {
            Self::Begun => "begun",
            Self::Closed => "closed",
            Self::Quiesced => "quiesced",
            Self::Mutating => "mutating",
            Self::Verifying => "verifying",
            Self::Committing => "committing",
        }
    }
}

/// One operation, as the journal keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    /// Its number on this scope.
    id: u64,
    /// What it does.
    work: Work,
    /// The instance it acts for.
    instance: Box<str>,
    /// The profile it acts on, where the route has one.
    profile: Option<Box<str>>,
    /// The instance's generation when it began.
    generation: u64,
    /// How far it got.
    stage: Stage,
    /// How it ended, once it has.
    ended: Option<Ending>,
}

impl Record {
    /// Its number on this scope.
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// What it does.
    #[must_use]
    pub const fn work(&self) -> Work {
        self.work
    }

    /// The instance it acts for.
    #[must_use]
    pub fn instance(&self) -> &str {
        &self.instance
    }

    /// The profile it acts on, where the route has one.
    #[must_use]
    pub fn profile(&self) -> Option<&str> {
        self.profile.as_deref()
    }

    /// The instance's generation when it began.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// How far it got.
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.stage
    }

    /// How it ended, or nothing while it is open.
    #[must_use]
    pub const fn ended(&self) -> Option<Ending> {
        self.ended
    }
}

/// What a journal holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entries {
    /// The operation not finished, where there is one.
    pub open: Option<Record>,
    /// Finished operations, oldest first, at most [`HISTORY`].
    pub history: VecDeque<Record>,
}

impl Entries {
    /// The number the next operation takes: one past every number kept.
    fn next(&self) -> u64 {
        self.open
            .iter()
            .chain(&self.history)
            .map(Record::id)
            .max()
            .map_or(1, |id| id.saturating_add(1))
    }
}

/// Why a journal could not be read or written.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// A name given to it is empty or longer than [`REFERENCE_BYTES`].
    #[error("an account name is empty or longer than crucible keeps")]
    Reference,
    /// The file is there and is not a journal this crate can read.
    #[error("the account journal at {} could not be read", .0.display())]
    Unreadable(PathBuf),
    /// The filesystem refused.
    #[error("the account journal could not be kept: {0}")]
    Io(#[from] io::Error),
}

/// The journal of one credential-sharing scope.
pub struct Journal {
    /// The owner-only directory it is kept in.
    directory: PathBuf,
    /// The journal's file.
    file: PathBuf,
    /// The file its operations are serialized on.
    lock: PathBuf,
}

impl fmt::Debug for Journal {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Journal")
            .field("file", &self.file)
            .finish_non_exhaustive()
    }
}

/// The lock on one scope, released when dropped.
#[derive(Debug)]
pub struct Hold(File);

impl Drop for Hold {
    fn drop(&mut self) {
        // The lock goes with the handle whatever this says.
        let _ = crucible_privacy::unlock_identity(&self.0);
    }
}

impl Journal {
    /// The journal of `scope`, kept in `directory`. Touches nothing.
    ///
    /// The file is named by a digest of the scope, which may be a path, so
    /// that no scope can name a file anywhere else.
    ///
    /// # Errors
    ///
    /// [`JournalError::Reference`] when `scope` is empty or too long.
    pub fn of(directory: PathBuf, scope: &str) -> Result<Self, JournalError> {
        bounded(scope)?;
        let digest = Sha256::digest(scope.as_bytes());
        let mut name = String::with_capacity(32);
        for byte in digest.iter().take(16) {
            let _ = write!(name, "{byte:02x}");
        }
        Ok(Self {
            file: directory.join(format!("{name}.journal")),
            lock: directory.join(format!("{name}.lock")),
            directory,
        })
    }

    /// The scope's lock, or nothing when another holder has it.
    ///
    /// # Errors
    ///
    /// [`JournalError::Io`] when the directory or lock cannot be made.
    pub fn hold(&self) -> Result<Option<Hold>, JournalError> {
        crucible_privacy::directory(&self.directory)
            .map_err(crucible_privacy::PrivacyError::into_io)?;
        let file =
            crucible_privacy::lock(&self.lock).map_err(crucible_privacy::PrivacyError::into_io)?;
        if crucible_privacy::try_lock_identity(&file)
            .map_err(crucible_privacy::PrivacyError::into_io)?
        {
            Ok(Some(Hold(file)))
        } else {
            Ok(None)
        }
    }

    /// What the journal holds now: nothing where it has never been written.
    ///
    /// # Errors
    ///
    /// [`JournalError::Unreadable`] for a file that is not a journal, and
    /// [`JournalError::Io`] where it cannot be opened.
    pub fn read(&self) -> Result<Entries, JournalError> {
        match crucible_privacy::tighten(&self.file) {
            Ok(_) => {}
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => {
                return Ok(Entries::default());
            }
            Err(problem) => return Err(problem.into_io().into()),
        }
        let opened = crucible_privacy::open_read(&self.file)
            .map_err(crucible_privacy::PrivacyError::into_io)?;
        let mut bytes = Vec::new();
        opened
            .take(
                u64::try_from(JOURNAL_BYTES)
                    .unwrap_or(u64::MAX)
                    .saturating_add(1),
            )
            .read_to_end(&mut bytes)?;
        if bytes.len() > JOURNAL_BYTES {
            return Err(JournalError::Unreadable(self.file.clone()));
        }
        decode(&bytes).ok_or_else(|| JournalError::Unreadable(self.file.clone()))
    }

    /// Replaces the journal with `entries`, durably, under `_hold`.
    ///
    /// # Errors
    ///
    /// [`JournalError::Io`] where it cannot be written; the journal is then
    /// as it was.
    pub fn write(&self, _hold: &Hold, entries: &Entries) -> Result<(), JournalError> {
        let bytes = encode(entries);
        if bytes.len() > JOURNAL_BYTES {
            return Err(JournalError::Unreadable(self.file.clone()));
        }
        let mut beside = Beside::new(&self.directory)?;
        let file = beside.file.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "the journal's draft closed")
        })?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(beside.file.take());
        crucible_privacy::replace(&beside.path, &self.file)
            .map_err(crucible_privacy::PrivacyError::into_io)?;
        beside.landed = true;
        Ok(())
    }

    /// Opens an operation of `work` for `instance` on `profile` under
    /// `generation`, as the next number after everything `entries` keeps.
    ///
    /// # Errors
    ///
    /// [`JournalError::Reference`] when a name is empty or too long.
    pub(crate) fn record(
        entries: &Entries,
        work: Work,
        instance: &str,
        profile: Option<&str>,
        generation: u64,
    ) -> Result<Record, JournalError> {
        bounded(instance)?;
        if let Some(profile) = profile {
            bounded(profile)?;
        }
        Ok(Record {
            id: entries.next(),
            work,
            instance: instance.into(),
            profile: profile.map(Into::into),
            generation,
            stage: Stage::Begun,
            ended: None,
        })
    }
}

/// Refuses a name that is empty or longer than [`REFERENCE_BYTES`].
const fn bounded(name: &str) -> Result<(), JournalError> {
    if name.is_empty() || name.len() > REFERENCE_BYTES {
        Err(JournalError::Reference)
    } else {
        Ok(())
    }
}

/// `record` finished as `ended`, kept in `entries`' history and no longer
/// open.
pub(crate) fn finished(entries: &mut Entries, mut record: Record, ended: Ending) {
    record.ended = Some(ended);
    entries.open = None;
    entries.history.push_back(record);
    while entries.history.len() > HISTORY {
        entries.history.pop_front();
    }
}

/// Moves `record` to `stage`.
pub(crate) const fn staged(record: &mut Record, stage: Stage) {
    record.stage = stage;
}

/// An ending's name in the journal.
const fn ending_name(ending: Ending) -> &'static str {
    match ending {
        Ending::Succeeded => "succeeded",
        Ending::Cancelled => "cancelled",
        Ending::Failed => "failed",
        Ending::Unknown => "unknown",
    }
}

/// The journal's bytes.
fn encode(entries: &Entries) -> Vec<u8> {
    let record = |record: &Record| {
        json!({
            "id": record.id,
            "work": record.work.name(),
            "instance": &*record.instance,
            "profile": record.profile.as_deref(),
            "generation": record.generation,
            "stage": record.stage.name(),
            "ended": record.ended.map(ending_name),
        })
    };
    let document = json!({
        "format": FORMAT,
        "open": entries.open.as_ref().map(record),
        "history": entries.history.iter().map(record).collect::<Vec<_>>(),
    });
    let mut bytes = document.to_string().into_bytes();
    bytes.push(b'\n');
    bytes
}

/// What `bytes` hold, or nothing where they are not a journal of this format.
fn decode(bytes: &[u8]) -> Option<Entries> {
    let document: Value = serde_json::from_slice(bytes).ok()?;
    let document = document.as_object()?;
    if document.len() != 3 || document.get("format")?.as_u64()? != FORMAT {
        return None;
    }
    let open = match document.get("open")? {
        Value::Null => None,
        Value::Object(fields) => {
            let record = decode_record(fields)?;
            record.ended.is_none().then_some(record)?.into()
        }
        _ => return None,
    };
    let history = document
        .get("history")?
        .as_array()?
        .iter()
        .map(|record| decode_record(record.as_object()?).filter(|record| record.ended.is_some()))
        .collect::<Option<VecDeque<_>>>()?;
    (history.len() <= HISTORY).then_some(Entries { open, history })
}

/// One record, or nothing where its fields are not a record's.
fn decode_record(fields: &Map<String, Value>) -> Option<Record> {
    if fields.len() != 7 {
        return None;
    }
    let name = |field: &str| -> Option<Option<&str>> {
        match fields.get(field)? {
            Value::Null => Some(None),
            Value::String(name) if !name.is_empty() && name.len() <= REFERENCE_BYTES => {
                Some(Some(name.as_str()))
            }
            _ => None,
        }
    };
    let work = Work::ALL
        .into_iter()
        .find(|work| Some(Some(work.name())) == name("work"))?;
    let stage = Stage::ALL
        .into_iter()
        .find(|stage| Some(Some(stage.name())) == name("stage"))?;
    let ended = match name("ended")? {
        None => None,
        Some(ended) => Some(
            Ending::ALL
                .into_iter()
                .find(|ending| ending_name(*ending) == ended)?,
        ),
    };
    Some(Record {
        id: fields.get("id")?.as_u64()?,
        work,
        instance: name("instance")??.into(),
        profile: name("profile")?.map(Into::into),
        generation: fields.get("generation")?.as_u64()?,
        stage,
        ended,
    })
}

/// A journal's draft beside it, removed unless it replaced the journal.
struct Beside {
    /// Where it is.
    path: PathBuf,
    /// The open draft, until it is closed to be renamed.
    file: Option<File>,
    /// Whether it replaced the journal.
    landed: bool,
}

impl Beside {
    /// A new owner-only draft in `directory`.
    fn new(directory: &Path) -> io::Result<Self> {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        for _ in 0..16 {
            let next = NEXT.fetch_add(1, Ordering::Relaxed);
            let path = directory.join(format!(".journal.{}.{next}.writing", std::process::id()));
            match crucible_privacy::create_write(&path) {
                Ok(file) => {
                    return Ok(Self {
                        path,
                        file: Some(file),
                        landed: false,
                    });
                }
                Err(problem) if problem.kind() == io::ErrorKind::AlreadyExists => {}
                Err(problem) => return Err(problem.into_io()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "no free name for the journal's draft",
        ))
    }
}

impl Drop for Beside {
    fn drop(&mut self) {
        if !self.landed {
            // A draft left behind is ignored by every reader.
            let _ = fs::remove_file(&self.path);
        }
    }
}
