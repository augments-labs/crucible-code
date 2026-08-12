//! Reading a log back into the transcript it recorded.
//!
//! The other half of this module writes; this half is the only thing that
//! reads, and everything it does is bounded by what a crashed process can
//! leave behind. A log can stop mid-line, name a workspace this run is not in,
//! or have been written by a build that spelled a message differently — so
//! finding the right log, refusing the wrong one and stopping at the first
//! line that cannot be read all live here together.

use std::fs::File;
use std::io::{self, BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::str::{self, FromStr as _};

use crucible_core::{Message, SessionId, Transcript, Workspace};

use super::{SUFFIX, SessionError, wire};

/// Every session log in `directory`, oldest first.
///
/// Session identifiers sort by start time as text, so this is time order and
/// nothing has to be opened to put it in that order. Anything else in the
/// directory is left out here rather than failing later: the name is the only
/// thing that says a file is a log at all.
///
/// # Errors
///
/// [`SessionError::Directory`] when the directory cannot be read.
pub(super) fn logs(directory: &Path) -> Result<Vec<PathBuf>, SessionError> {
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
    Ok(logs)
}

/// The newest log recorded for `workspace`.
///
/// Only the first line of each candidate is read, newest first, so a directory
/// full of other directories' sessions costs a header apiece rather than a
/// replay apiece.
pub(super) fn newest(directory: &Path, workspace: &Workspace) -> Result<PathBuf, SessionError> {
    for path in logs(directory)?.into_iter().rev() {
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
pub(super) fn belongs(path: &Path, workspace: &Workspace) -> Result<bool, SessionError> {
    let trouble = |source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    };

    let mut first = String::new();
    BufReader::new(File::open(path).map_err(trouble)?)
        .read_line(&mut first)
        .map_err(trouble)?;

    // A first line the process never finished is a log with nothing whole in
    // it: the header is written before a session can record anything, so one
    // that stopped there holds no turns. Read as a header it would be worse
    // than useless — the next turn would be appended onto the header itself,
    // and the welded line names no workspace, so from then on nothing could
    // find the session at all.
    if !first.ends_with('\n') {
        return Ok(false);
    }

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

/// Everything a log holds, as the transcript it recorded and the offset the
/// next write has to start at.
///
/// Reading stops at the first line that is not a whole message. A session that
/// ended when the process did leaves a half-written last line, and a prefix of
/// the transcript is a transcript; one with a hole in it is not.
///
/// A line this build cannot read with more of the log after it is a different
/// thing, and fails outright: treating damage in the middle as the end would
/// hand back a transcript missing its middle with nothing to say so, and the
/// caller cuts the file to what was read — so every turn recorded after the
/// damage would leave the disk as well. What tells the two apart is whether
/// anything follows, because the end of a log is also the end of its file.
///
/// The offset is what the caller must cut the file to before appending, and it
/// is the reason this returns one at all: everything from there on is a
/// fragment or a line no replay will read again, so a log continued without the
/// cut welds the next turn onto the wreckage. That costs either the continued
/// turn, silently, or — once the weld is no longer the last line — the whole
/// session. Cutting touches nothing that was replayed, so what is on disk
/// afterwards reads back as exactly the transcript returned here.
pub(super) fn replay(path: &Path) -> Result<(Transcript, u64), SessionError> {
    let trouble = |source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    };

    let mut log = BufReader::new(File::open(path).map_err(trouble)?);
    let mut raw = Vec::new();

    // The header is not a message, but its bytes are where the messages start.
    let mut through = log.read_until(b'\n', &mut raw).map_err(trouble)? as u64;
    // The same, one message back, for when the last one turns out to be a
    // question the log never answers.
    let mut before = through;

    let mut transcript = Transcript::new();

    loop {
        raw.clear();
        let read = log.read_until(b'\n', &mut raw).map_err(trouble)?;

        if read == 0 || !raw.ends_with(b"\n") {
            break;
        }

        let whole = str::from_utf8(&raw).ok().map(str::trim_end);

        // A line with nothing in it is neither a message nor damage: it is what
        // the writer lays down to end a line a failed write may have cut short,
        // before starting the next one. Nothing was recorded there, so nothing
        // is missing.
        if whole == Some("") {
            through += read as u64;
            continue;
        }

        // A session that forgot what it had said. Everything above this line
        // stays in the file — it is what happened, and the log is the record of
        // that — and none of it is replayed, because the model was never going
        // to be told it again.
        if whole.is_some_and(wire::forgets) {
            transcript.forget();
            before = through;
            through += read as u64;
            continue;
        }

        let Some(message) = whole.and_then(wire::message) else {
            // Where the damage sits is what decides what to do about it. At the
            // end of the file it is where the log stops, and what came before
            // is still a transcript. With turns recorded after it, stopping
            // here would drop every one of them — from the transcript handed
            // back, and from the file the caller cuts to match.
            if log.fill_buf().map_err(trouble)?.is_empty() {
                break;
            }

            return Err(trouble(io::Error::new(
                io::ErrorKind::InvalidData,
                "a line that is not a message, with the log continuing past it",
            )));
        };

        transcript.push(message);
        before = through;
        through += read as u64;
    }

    if outstanding(&transcript) {
        return Ok((without_last(&transcript), before));
    }

    Ok((transcript, through))
}

/// Whether the last message is still waiting on tools.
///
/// A process that died between asking for a tool and recording its result
/// leaves calls nothing ever answered. Sending that on is not a cosmetic
/// problem: a provider is entitled to reject a transcript whose last word is
/// an unanswered question.
fn outstanding(transcript: &Transcript) -> bool {
    matches!(
        transcript.messages().last(),
        Some(Message::Agent { calls, .. }) if !calls.is_empty()
    )
}

/// A copy without the final message.
///
/// A transcript-sized copy, once per `--continue` and never again: this runs
/// before the first turn, on a transcript already read whole from the disk, and
/// the alternative is a `Transcript` that can have its last message taken off —
/// a method the running loop would then also be able to reach for.
fn without_last(transcript: &Transcript) -> Transcript {
    let mut settled = Transcript::new();
    for message in transcript.messages().iter().rev().skip(1).rev() {
        settled.push(message.clone());
    }

    settled
}
