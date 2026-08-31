//! Reading a log back into the transcript it recorded.
//!
//! The other half of this module writes; this half is the only thing that
//! reads, and everything it does is bounded by what a crashed process can
//! leave behind. A log can stop mid-line, name a workspace this run is not in,
//! or have been written by a build that spelled a message differently — so
//! finding the right log, refusing the wrong one and stopping at the first
//! line that cannot be read all live here together.

use std::collections::HashMap;
use std::fmt;
use std::fs::File;
use std::io::{self, BufRead as _, BufReader};
use std::path::{Path, PathBuf};
use std::str::{self, FromStr as _};

use crucible_core::{
    Calibration, ContextSnapshot, Message, SessionId, ToolId, Transcript, Workspace,
};

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

    if wire::readable(opening.format) {
        Ok(true)
    } else {
        Err(SessionError::Foreign {
            at: path.display().to_string().into(),
        })
    }
}

/// Everything a log holds, as the transcript it recorded, the offset the next
/// write has to start at, and what its last request carried.
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
///
/// What the last request carried comes back only where the line saying so is
/// the last thing in the file. Anything written after it — another message, a
/// compaction, a forgetting — happened to the transcript the reading covered,
/// so the reading is about a session that no longer exists and is dropped
/// rather than adjusted. There is nothing here to adjust it with.
///
/// How each answer ended comes back with it, unchanged and unjudged. Nothing
/// here decides what a turn that was cut off means — that is the provider's, on
/// the next request, and it is the whole reason the reason is on the line: a
/// transcript replayed without it hands the model a half-sentence as an answer
/// it chose to end.
pub(super) fn replay(path: &Path) -> Result<Replayed, SessionError> {
    let trouble = |source| SessionError::Log {
        at: path.display().to_string().into(),
        source,
    };

    let mut log = BufReader::new(File::open(path).map_err(trouble)?);
    let mut raw = Vec::new();

    // The header is not a message, but its bytes are where the messages start.
    let mut through = log.read_until(b'\n', &mut raw).map_err(trouble)? as u64;
    let format = str::from_utf8(&raw)
        .ok()
        .map(str::trim_end)
        .and_then(wire::opening)
        .map(|opening| opening.format);
    // The same, one message back, for when the last one turns out to be a
    // question the log never answers.
    let mut before = through;

    let mut transcript = Transcript::new();
    let mut pruned = Pruned::default();
    // Taken only where nothing follows it, which is why it is cleared by every
    // other line rather than kept until something replaces it. See [`Replayed`].
    let mut calibration = None;
    // A new format-10 log establishes that nothing has been sent yet. An old
    // header establishes no typed baseline; its first appended patch upgrades
    // it in place without rewriting the header.
    let mut context = (format == Some(wire::FORMAT)).then(ContextSnapshot::new);

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
            context = (format == Some(wire::FORMAT)).then(ContextSnapshot::new);
            calibration = None;
            before = through;
            through += read as u64;
            continue;
        }

        // Room having been made. The notes replace exactly the prefix count the
        // line carries. Earlier compaction markers have already transformed
        // the transcript, so the count applies to its current model-visible
        // shape rather than to physical lines above this one.
        if let Some((replaced, recap)) = whole.and_then(wire::made_room) {
            transcript.compacted(replaced, recap);
            calibration = None;
            through += read as u64;
            continue;
        }

        // Old tool results having been cleared. The named results are put back
        // as placeholders, so a continued session carries what the model was
        // actually sent rather than text the model stopped seeing. A name that
        // answers to nothing — cleared first and compacted away since — frees
        // nothing and is nobody's damage.
        if let Some(results) = whole.and_then(wire::cleared) {
            // Read first, because after the clearing the text is gone from the
            // only place it was. A result the clearing then leaves alone — too
            // small for a placeholder to be worth it — is held here too, and
            // that is harmless: it is still what the reader was shown, so
            // putting it back on the screen puts back what is already there.
            for message in transcript.messages() {
                if let Message::ToolResults(answers) = message {
                    for answer in answers {
                        if results.contains(&answer.id) {
                            pruned.keep(answer.id.clone(), answer.output.text().to_owned());
                        }
                    }
                }
            }

            transcript.prune(&results);
            calibration = None;
            through += read as u64;
            continue;
        }

        // What the request behind the answer above carried. Kept for now and
        // dropped by the next line of any other kind, since what makes it
        // usable is being the last thing written.
        if let Some(measured) = whole.and_then(wire::measure) {
            calibration = Some(measured);
            through += read as u64;
            continue;
        }

        if let Some(patch) = whole.and_then(wire::context_patch) {
            let context_patch = patch.map_err(|source| SessionError::Context {
                at: path.display().to_string().into(),
                source,
            })?;
            let prior = context.clone().unwrap_or_default();
            context =
                Some(
                    context_patch
                        .apply(&prior)
                        .map_err(|source| SessionError::Context {
                            at: path.display().to_string().into(),
                            source,
                        })?,
                );
            calibration = None;
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
        calibration = None;
        before = through;
        through += read as u64;
    }

    if outstanding(&transcript) {
        // The message being cut off is the one the reading covered, so what is
        // left is a shorter transcript than the number describes.
        return Ok(Replayed {
            transcript: without_last(&transcript),
            pruned,
            settled_at: before,
            calibration: None,
            context,
        });
    }

    Ok(Replayed {
        transcript,
        pruned,
        settled_at: through,
        calibration,
        context,
    })
}

/// What a log read back comes to.
///
/// The transcript, where the file settles after it, and what the last request
/// carried where the log still says — which it does only when that line is the
/// last thing in it. A reading with messages written after it covered a
/// transcript shorter than the one being handed back, and a load told a number
/// that covers less than it is about to send is a load that under-states
/// itself: the one direction that costs a turn rather than some context.
pub(super) struct Replayed {
    pub(super) transcript: Transcript,
    pub(super) pruned: Pruned,
    pub(super) settled_at: u64,
    pub(super) calibration: Option<Calibration>,
    pub(super) context: Option<ContextSnapshot>,
}

/// What results said before a pruning cleared them.
///
/// A pruning takes text out of the transcript because the model stopped being
/// sent it. The reader never stopped being shown it — the rows went down when
/// the call answered and are still what the session looks like — so a resumed
/// screen drawn from the transcript alone shows a reader a placeholder where
/// they remember an answer.
///
/// Beside the transcript rather than inside it, and this is the whole of why
/// the type exists. There is one transcript, it is the pruned one, and it is
/// what every request is built from; nothing that builds a request can reach
/// this. A second transcript holding the fuller text would be one wrong
/// argument away from re-sending what a session was told to stop sending.
///
/// Keyed by the id a result shares with the call it answered, which is what a
/// pruning line names and what the walk has in hand at the row it is drawing.
#[derive(Default)]
pub struct Pruned(HashMap<ToolId, String>);

/// Written by hand, because what is held here is what a tool printed.
///
/// Which is how a model reads a file and how it runs `env`. A key printed once
/// is a key in every `{:?}` this value reaches, and this one is reached from
/// further away than most: [`super::Session`] holds it and the runner holds
/// that, so a derived `Debug` here would put the whole of what a session ever
/// cleared into any line either of them was ever printed on.
impl fmt::Debug for Pruned {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Pruned")
            .field(&format_args!("{} redacted", self.0.len()))
            .finish()
    }
}

impl Pruned {
    /// Keeps what the result `call` answered with said, unless something
    /// already is.
    ///
    /// A log may clear one result twice — a session continued, pruned again,
    /// continued again — and by the second line the transcript holds the
    /// placeholder the first one left. First keep wins, so what is held is what
    /// the reader saw rather than what the last pruning found.
    ///
    /// Public because the walk that reads a log is not the only thing that
    /// fills one: what draws a session back onto a screen is judged against a
    /// side-table built by hand, and a table nothing outside this file could
    /// build would be one nothing outside this file could test.
    pub fn keep(&mut self, call: ToolId, text: String) {
        self.0.entry(call).or_insert(text);
    }

    /// What the result `call` answered with said, where a pruning cleared it.
    #[must_use]
    pub fn showed(&self, call: &ToolId) -> Option<&str> {
        self.0.get(call).map(String::as_str)
    }

    /// Whether nothing in this session was ever cleared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
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

#[cfg(test)]
mod tests {
    use crucible_core::{Message, ToolId};

    use super::{Pruned, replay};
    use crate::sample::Sample;
    use crate::session::wire::FORMAT;

    /// What the call in these cases answered with, before anything cleared it.
    ///
    /// Longer than [`crucible_core::ToolOutput::MIN_PRUNE_BYTES`] on purpose:
    /// under that a
    /// result is left alone, because the placeholder would cost more than the
    /// text it replaced. A shorter string here would make a case about pruning
    /// out of a log where no pruning happens.
    const WHOLE: &str =
        "every line the file had in it, and then every line it had after that, and more besides";

    /// A log holding one answered call and, after it, a line saying that result
    /// was cleared to make room.
    fn cleared_after_answering(name: &str) -> Sample {
        let sample = Sample::new(name);
        let id = "0000000000001-000001";

        sample.plant(
            id,
            &[
                sample.header(FORMAT, id),
                r#"{"user":"what is in this"}"#.to_owned(),
                r#"{"agent":"on it","calls":[{"args":"{}","id":"call-1","name":"read"}],"stop":"tools"}"#
                    .to_owned(),
                format!(r#"{{"results":[{{"failed":false,"id":"call-1","text":"{WHOLE}"}}]}}"#),
                r#"{"pruned":{"freed":80000,"results":["call-1"]}}"#.to_owned(),
            ],
        );

        sample
    }

    /// The one result in `messages`.
    fn only(messages: &[Message]) -> &crucible_core::ToolResult {
        messages
            .iter()
            .find_map(|message| match message {
                Message::ToolResults(results) => results.first(),
                _ => None,
            })
            .expect("the result the log recorded")
    }

    #[test]
    fn a_cleared_result_replays_as_a_placeholder_and_is_kept_beside_it_whole() {
        // Both halves of the same read, asserted together, because either alone
        // is the bug. The transcript is what every request is built from and it
        // has to hold the placeholder — a resumed session that re-sent what it
        // was told to stop sending would undo the pruning that made room. And
        // the reader watched that result come back, so a screen drawn from the
        // transcript alone shows them a placeholder where they remember an
        // answer.
        let sample = cleared_after_answering("replay-pruned");

        let read = replay(&sample.logs().join("0000000000001-000001.jsonl"))
            .expect("a log this build wrote");

        let result = only(read.transcript.messages());
        assert!(
            result.output.text().contains("cleared to make room"),
            "the transcript kept text the model stopped being sent: {}",
            result.output.text()
        );

        assert_eq!(
            read.pruned.showed(&ToolId::new("call-1")),
            Some(WHOLE),
            "what the reader was shown is gone with what the model was sent"
        );
    }

    #[test]
    fn a_log_that_cleared_nothing_keeps_nothing_beside_it() {
        // The cost of the side-table is what it holds, and an ordinary session
        // prunes nothing at all. This is the case that says so.
        let sample = Sample::new("replay-unpruned");
        let id = "0000000000001-000001";

        sample.plant(
            id,
            &[
                sample.header(FORMAT, id),
                r#"{"user":"what is in this"}"#.to_owned(),
            ],
        );

        let read =
            replay(&sample.logs().join(format!("{id}.jsonl"))).expect("a log this build wrote");

        assert!(read.pruned.is_empty(), "a session that pruned nothing");
    }

    #[test]
    fn what_a_pruning_cleared_does_not_reach_a_debug_line() {
        // The side-table holds what a tool printed, which is how a model reads
        // a file and how it runs `env`. Every other value in this tree that
        // carries that material writes its own `Debug` and redacts, and this
        // one is held by `Session`, which is held by the runner — so one
        // `{:?}` of either would otherwise print the whole of what a session
        // ever cleared.
        let mut pruned = Pruned::default();
        pruned.keep(ToolId::new("call-1"), WHOLE.to_owned());

        let shown = format!("{pruned:?}");
        assert!(
            !shown.contains("every line the file had"),
            "cleared text reached a log: {shown}"
        );
        assert!(shown.contains("redacted"), "{shown}");
    }
}
