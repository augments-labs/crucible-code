//! What `crucible sessions list --json` writes: the sessions recorded for the
//! directory it was started in, newest first, as one bounded document.
//!
//! Like the [`doctor`](crate::doctor) report beside it, this is read once, by
//! a person or a script, so it says its own [`FORMAT_VERSION`] and [`KIND`], no
//! request asks for it, a field a reader does not know refuses the document,
//! and the list is refused over [`ITEMS`] sessions rather than cut. The host
//! lists no more than that and counts the rest in `omitted`.
//!
//! A [`Session`] says only what the host's index and the first line of its
//! log say: its id, which names when it started, how many messages it has,
//! the branch it was started on and the title it was given. Nothing anybody
//! wrote in a session is a field here. The branch and the title are a
//! [`Text`], so neither runs past the ceiling unmarked.
//!
//! A list is `incomplete` wherever it may not be every session there is:
//! sessions were left out past the window, logs did not read, the host's
//! index holds all the sessions it keeps, the directory has sessions but no
//! index yet, or a branch or title was cut. Each of those is a field of its
//! own, so a reader knows which. A list that could not be made at all is
//! `failed`, says why in words that name no path, and is the only one that
//! exits 1.

use std::collections::BTreeSet;
use std::str::FromStr as _;
use std::time::UNIX_EPOCH;

use crucible_types::SessionId;

use crate::bounds::{ITEMS, Text};
use crate::error::{ErrorCode, Refusal};
use crate::wire::{Fields, Writing, frame, parsed, text, written};

/// The number every document here says it is written in.
pub const FORMAT_VERSION: u64 = 1;

/// The word every document here is named by.
pub const KIND: &str = "sessions";

/// One session the list names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// Which session, by the id `--resume` takes. It names when the session
    /// started, which crosses beside it as milliseconds since the epoch.
    pub id: SessionId,
    /// The branch it was started on, where it was started on one.
    pub branch: Option<Text>,
    /// How many messages it holds, as its host's index counts them.
    pub messages: u64,
    /// The title it was given, where it was given one.
    pub title: Option<Text>,
}

impl Session {
    /// When the session started, in milliseconds since the epoch: what its id
    /// names, and the epoch for an id that names nothing this machine holds.
    fn started(&self) -> u64 {
        self.id
            .started()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| {
                u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
            })
    }

    fn truncated(&self) -> bool {
        self.branch.as_ref().is_some_and(Text::truncated)
            || self.title.as_ref().is_some_and(Text::truncated)
    }
}

/// The sessions that were listed, and what may have been left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listing {
    /// The newest sessions, newest first, at most [`ITEMS`] of them.
    pub sessions: Vec<Session>,
    /// How many more there were past those.
    pub omitted: u64,
    /// How many logs the index names whose first line did not read, said
    /// nothing of.
    pub unreadable: u64,
    /// Whether the host's index holds all the sessions it keeps, so older ones
    /// may have fallen out of it.
    pub index_full: bool,
    /// Whether the session directory is there with no index in it yet, so
    /// whatever it holds was not listed.
    pub unindexed: bool,
}

impl Listing {
    fn whole(&self) -> bool {
        self.omitted == 0
            && self.unreadable == 0
            && !self.index_full
            && !self.unindexed
            && !self.truncated()
    }

    fn truncated(&self) -> bool {
        self.sessions.iter().any(Session::truncated)
    }

    /// Whether each session is said once.
    fn distinct(&self) -> bool {
        let mut ids = BTreeSet::new();
        self.sessions
            .iter()
            .all(|session| ids.insert(session.id.as_str()))
    }
}

/// What `crucible sessions list --json` writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    /// The sessions were read.
    Listed(Listing),
    /// They could not be, for the reason given.
    Failed(Text),
}

impl Report {
    /// The status word: `failed` where nothing could be listed, `incomplete`
    /// where something may have been left out, and `complete` otherwise.
    #[must_use]
    pub fn status(&self) -> &'static str {
        match self {
            Self::Failed(_) => "failed",
            Self::Listed(listing) if listing.whole() => "complete",
            Self::Listed(_) => "incomplete",
        }
    }

    /// The code `crucible sessions list` exits with: 1 where nothing could be
    /// listed, and 0 otherwise, an incomplete list included.
    #[must_use]
    pub const fn exit(&self) -> u8 {
        match self {
            Self::Failed(_) => 1,
            Self::Listed(_) => 0,
        }
    }

    fn truncated(&self) -> bool {
        match self {
            Self::Failed(problem) => problem.truncated(),
            Self::Listed(listing) => listing.truncated(),
        }
    }

    /// The document, as one line ending in a newline.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] for more than [`ITEMS`] sessions or a document
    /// over the frame ceiling, and [`ErrorCode::Malformed`] for a session said
    /// twice.
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        let written = match self {
            Self::Failed(problem) => Writing::new().with("problem", written(problem)),
            Self::Listed(listing) => {
                if listing.sessions.len() > ITEMS {
                    return Err(ErrorCode::TooLarge.into());
                }
                if !listing.distinct() {
                    return Err(ErrorCode::Malformed.into());
                }
                let sessions: Vec<serde_json::Value> = listing
                    .sessions
                    .iter()
                    .map(|session| {
                        Writing::new()
                            .with("id", session.id.as_str())
                            .with("started", session.started())
                            .with("messages", session.messages)
                            .maybe("branch", session.branch.as_ref().map(written))
                            .maybe("title", session.title.as_ref().map(written))
                            .finish()
                    })
                    .collect();
                Writing::new()
                    .with("sessions", sessions)
                    .with("omitted", listing.omitted)
                    .with("unreadable", listing.unreadable)
                    .with("index_full", listing.index_full)
                    .with("unindexed", listing.unindexed)
            }
        };
        let mut bytes = frame(
            &written
                .with("truncated", self.truncated())
                .with("format_version", FORMAT_VERSION)
                .with(crate::wire::KIND, KIND)
                .with("status", self.status())
                .finish(),
        )?;
        bytes.push(b'\n');
        Ok(bytes)
    }

    /// The report `bytes` spell.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::UnsupportedVersion`] for another format version, and
    /// [`Refusal`] for anything but one whole, bounded document that says what
    /// it may: a status or a truncation flag that disagrees with the list, a
    /// start that is not the one its id names, and a report
    /// [`encode`](Self::encode) would refuse, are all refused.
    pub fn decode(bytes: &[u8]) -> Result<Self, Refusal> {
        let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
        let mut fields = Fields::of(parsed(bytes)?)?;
        if fields.number("format_version")? != FORMAT_VERSION {
            return Err(ErrorCode::UnsupportedVersion.into());
        }
        if fields.kind()? != KIND {
            return Err(ErrorCode::Malformed.into());
        }
        let status = fields.string("status")?;
        let truncated = fields.flag("truncated")?;
        let report = if status == "failed" {
            Self::Failed(fields.text("problem")?)
        } else {
            let sessions = fields
                .list("sessions")?
                .into_iter()
                .map(session)
                .collect::<Result<_, Refusal>>()?;
            let listing = Listing {
                sessions,
                omitted: fields.number("omitted")?,
                unreadable: fields.number("unreadable")?,
                index_full: fields.flag("index_full")?,
                unindexed: fields.flag("unindexed")?,
            };
            if !listing.distinct() {
                return Err(ErrorCode::Malformed.into());
            }
            Self::Listed(listing)
        };
        fields.done()?;
        if report.truncated() != truncated || report.status() != status {
            return Err(ErrorCode::Malformed.into());
        }
        Ok(report)
    }
}

/// The session `value` is, refused where its start is not the one its id
/// names.
fn session(value: serde_json::Value) -> Result<Session, Refusal> {
    let mut one = Fields::of(value)?;
    let id = SessionId::from_str(&one.string("id")?).map_err(|_| ErrorCode::Malformed)?;
    let started = one.number("started")?;
    let session = Session {
        id,
        branch: one.maybe("branch").map(text).transpose()?,
        messages: one.number("messages")?,
        title: one.maybe("title").map(text).transpose()?,
    };
    one.done()?;
    if session.started() != started {
        return Err(ErrorCode::Malformed.into());
    }
    Ok(session)
}

#[cfg(test)]
pub(crate) mod tests;
