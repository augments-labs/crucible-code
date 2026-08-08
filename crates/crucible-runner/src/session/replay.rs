//! Reading a log back into the transcript it recorded.
//!
//! The other half of this module writes; this half is the only thing that
//! reads, and everything it does is bounded by what a crashed process can
//! leave behind. A log can stop mid-line, name a workspace this run is not in,
//! or have been written by a build that spelled a message differently — so
//! finding the right log, refusing the wrong one and stopping at the first
//! line that cannot be read all live here together.

use std::fs::File;
use std::io::{BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::str::FromStr as _;

use crucible_core::{Message, SessionId, Transcript, Workspace};

use super::{SUFFIX, SessionError, wire};

/// The newest log recorded for `workspace`.
///
/// Session identifiers sort by start time as text, so the newest is found by
/// name and only the first line of each candidate is read.
pub(super) fn newest(directory: &Path, workspace: &Workspace) -> Result<PathBuf, SessionError> {
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
/// Reading stops at the first line that cannot be read *as a message*. A
/// session that ended when the process did leaves a half-written last line, and
/// a prefix of the conversation is a conversation; one with a hole in it is not.
///
/// A line that cannot be read at all is the other case and fails outright.
/// Bytes that are not text stop the read wherever they sit, so treating that as
/// the end of the log would drop every turn after a damaged one and hand back a
/// conversation missing its middle with nothing to say so.
pub(super) fn replay(path: &Path) -> Result<Transcript, SessionError> {
    let trouble = |source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    };

    let file = File::open(path).map_err(trouble)?;

    let mut transcript = Transcript::new();
    for line in BufReader::new(file).lines().skip(1) {
        let Some(message) = wire::message(&line.map_err(trouble)?) else {
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

    // A transcript-sized copy, once per `--continue` and never again: this runs
    // before the first turn, on a transcript already read whole from the disk,
    // and the alternative is a `Transcript` that can have its last message taken
    // off — a method the running loop would then also be able to reach for.
    let mut settled = Transcript::new();
    for message in transcript.messages().iter().rev().skip(1).rev() {
        settled.push(message.clone());
    }

    settled
}
