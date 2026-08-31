//! The session log's line shape.
//!
//! One JSON object per line, in the order the messages happened. The shape is
//! owned here rather than derived from the domain types, for the same reason
//! each provider owns its own: a 0.x file format may change in any release,
//! and pinning [`Message`] to one would make every such change a change to the
//! domain everything else is written against.
//!
//! Reading is total. Anything a line cannot be read as comes back as `None`,
//! and the caller stops there rather than replaying a transcript with a hole
//! in it.

use std::fmt::Write as _;
use std::path::Path;

use crucible_core::{
    Attachment, Calibration, Carried, Changed, Fragment, Message, Modality, SessionId, Spend,
    StopReason, ToolCall, ToolId, ToolOutput, ToolResult,
};
use serde_json::{Value, json};

/// What wrote the file.
///
/// Read back and checked. A log from a build that spelled things differently
/// is refused rather than half-understood, which is the difference between
/// telling the user their session cannot be continued and silently continuing
/// a different one.
pub(crate) const FORMAT: u32 = 10;

/// The formats this build reads, newest first.
///
/// A log is refused rather than half-understood, and that is what this list is
/// careful about: a format is on it only where every line an older build could
/// have written still means here exactly what it meant there. Formats 3 and 4
/// are, because each newer one only *added* — a word for a stop reason that
/// build never produced, and line kinds it never wrote. Nothing was renamed and
/// nothing changed meaning, so a log from either replays whole; what it is
/// missing is a line saying what its last request carried, and a session with
/// no such line is a session that measures itself again on its next answer.
/// Format 7 is, because format 8 only added a branch to the header, and a
/// header without one already means "branch unknown". Format 8 is, because
/// format 9 only added a key to a line of tool results — the count a change came
/// to — and a result written without one already means a call that changed no
/// file, which is what a build with nowhere to say it could only ever have
/// meant. Format 9 is, because format 10 only adds typed context and its
/// snapshot patches; a log without either replays as context-unknown rather
/// than pretending it recorded state it could not have written.
///
/// A format that changed the meaning of a line does not go on this list however
/// small the change looks. What it would buy is somebody's history; what it
/// would cost is a session that looks fine and is missing turns, which is the
/// failure the refusal exists for.
pub(crate) const READS: &[u32] = &[10, 9, 8, 7, 6, 5, 4, 3];

/// Whether this build can replay a log written under `format`.
pub(crate) fn readable(format: u32) -> bool {
    READS.contains(&format)
}

/// Whether this line says everything above it was forgotten.
///
/// Read and never written. `/clear` forgot in place once, and a session that
/// did could not rewrite its own log — it is append-only, written by a thread
/// that never seeks and left behind by a process that may have crashed — so
/// what it forgot was recorded as something that happened at a point in it,
/// the same as a message. That command starts a session of its own now, but
/// the logs holding the marker are still logs of this format, and replay
/// starts the transcript again where it finds one.
pub(crate) fn forgets(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| value.get("forgotten").and_then(Value::as_bool))
        .unwrap_or_default()
}

/// A line saying room was made, and what the notes stand in place of.
///
/// Written where a compaction happened, the way the forgetting marker above is,
/// and for the same reason: the log is append-only and cannot be rewritten, so
/// what happened *to* the transcript is recorded as another thing that happened
/// in it. Every message it replaced stays in the file — that is the record —
/// and replay is what leaves them out of the transcript.
///
/// It carries **what it replaced** rather than standing for everything above
/// it. A count is a fact about this compaction; a position is a fact about the
/// file, and a file that ever holds more than one thread of messages has no
/// meaningful "everything above".
pub(crate) fn compacted(replaced: usize, recap: &str) -> String {
    json!({ "compacted": { "replaced": replaced, "recap": recap } }).to_string()
}

/// What a compaction line says, or `None` if this is not one.
pub(crate) fn made_room(line: &str) -> Option<(usize, String)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let made = value.get("compacted")?;

    Some((
        usize::try_from(made.get("replaced")?.as_u64()?).ok()?,
        made.get("recap")?.as_str()?.to_owned(),
    ))
}

/// A line saying old tool results were cleared, and which.
///
/// The same append-only shape as the compaction line, for the same reason: a
/// result is cleared from what the model is sent, not from the record, so the
/// clearing is written down as another thing that happened. It names the
/// results rather than a span, because a pruning does not move a boundary the
/// way a compaction does — the same results stay where they were, holding a
/// placeholder instead of what they held. `freed` is how much it recovered, so
/// a reader can tell a pruning that paid from one that did not.
pub(crate) fn pruned(freed: usize, results: &[ToolId]) -> String {
    json!({
        "pruned": {
            "freed": freed,
            "results": results.iter().map(ToolId::as_str).collect::<Vec<_>>(),
        }
    })
    .to_string()
}

/// What a pruning line says, or `None` if this is not one: the results it
/// cleared, by the id each shares with the call it answered.
pub(crate) fn cleared(line: &str) -> Option<Vec<ToolId>> {
    let value: Value = serde_json::from_str(line).ok()?;
    let results = value.get("pruned")?.get("results")?.as_array()?;

    results
        .iter()
        .map(|one| Some(ToolId::new(one.as_str()?)))
        .collect()
}

/// A line saying what the request that produced the answer above it carried.
///
/// Written after the message it belongs to, and read the same way, because
/// order is the only thing that says which transcript it covers: everything
/// above it was sent, and anything below it was not. That is why a reader takes
/// it only where it is the last thing in the file — see [`super::replay`].
///
/// It records lengths and token counts and nothing else. Not the model's name,
/// not the instructions, not a hash of them: the fixed content of a request is
/// compared by its size, and a number this build computed from its own hasher
/// would be a number another build is free to compute differently.
pub(crate) fn measured(calibration: &Calibration) -> String {
    json!({
        "carried": {
            "tokens": calibration.carried.tokens(),
            "spent": calibration.spent.tokens(),
            "bytes": calibration.sent,
            "overhead": calibration.overhead,
        }
    })
    .to_string()
}

/// What a carried line says, or `None` if this is not one.
pub(crate) fn measure(line: &str) -> Option<Calibration> {
    let value: Value = serde_json::from_str(line).ok()?;
    let measured = value.get("carried")?;

    Some(Calibration {
        carried: Carried::new(measured.get("tokens")?.as_u64()?),
        spent: Spend::new(measured.get("spent")?.as_u64()?),
        sent: measured.get("bytes")?.as_u64()?,
        overhead: measured.get("overhead")?.as_u64()?,
    })
}

/// The first line, which says what the file is and what it belongs to.
///
/// The branch is whatever the caller says the workspace's version control had
/// checked out when the session began — this crate does not run git, so the
/// caller that can look supplies it and a caller that cannot passes `None`,
/// and the key is left off the line rather than written null.
pub(crate) fn header(session: &SessionId, workspace: &Path, branch: Option<&str>) -> String {
    let mut value = json!({
        "format": FORMAT,
        "session": session.as_str(),
        "workspace": workspace.display().to_string(),
    });
    if let (Some(branch), Some(object)) = (branch, value.as_object_mut()) {
        object.insert("branch".to_owned(), Value::String(branch.to_owned()));
    }
    value.to_string()
}

/// What a header says, or `None` if this is not one.
pub(crate) fn opening(line: &str) -> Option<Opening> {
    let value: Value = serde_json::from_str(line).ok()?;

    Some(Opening {
        format: u32::try_from(value.get("format")?.as_u64()?).ok()?,
        workspace: value.get("workspace")?.as_str()?.to_owned(),
        branch: value
            .get("branch")
            .and_then(Value::as_str)
            .map(str::to_owned),
    })
}

/// What the first line of a log says about it.
pub(crate) struct Opening {
    pub(crate) format: u32,
    pub(crate) workspace: String,
    pub(crate) branch: Option<String>,
}

/// One message as the line that records it.
pub(crate) fn line(message: &Message) -> String {
    let value = match message {
        Message::Context(fragment) => json!({
            "context": {
                "section": fragment.section(),
                "text": fragment.text(),
            }
        }),
        // Written without the key when there are no files, so a text-only
        // session's log is byte for byte what format 5 wrote.
        Message::User { text, attachments } if attachments.is_empty() => {
            json!({ "user": text.as_ref() })
        }
        Message::User { text, attachments } => json!({
            "user": text.as_ref(),
            "attached": attachments.iter().map(attached).collect::<Vec<_>>(),
        }),
        Message::Agent { text, calls, stop } => json!({
            "agent": text.as_ref(),
            "calls": calls.iter().map(called).collect::<Vec<_>>(),
            "stop": stop.map(stopped),
        }),
        Message::ToolResults(results) => json!({
            "results": results.iter().map(answered).collect::<Vec<_>>(),
        }),
    };

    value.to_string()
}

/// One line as the message it records, or `None` if it is not one.
pub(crate) fn message(line: &str) -> Option<Message> {
    let value: Value = serde_json::from_str(line).ok()?;

    if let Some(context) = value.get("context") {
        return Some(Message::Context(Fragment::new(
            context.get("section")?.as_str()?,
            context.get("text")?.as_str()?,
        )));
    }

    if let Some(text) = value.get("user") {
        let attachments = match value.get("attached") {
            Some(attached) => read(attached, attachment)?.into(),
            None => Box::default(),
        };

        return Some(Message::User {
            text: text.as_str()?.into(),
            attachments,
        });
    }

    if let Some(text) = value.get("agent") {
        return Some(Message::Agent {
            text: text.as_str()?.into(),
            calls: read(value.get("calls")?, call)?,
            stop: stops(value.get("stop")),
        });
    }

    Some(Message::ToolResults(read(value.get("results")?, result)?))
}

/// One attached file as it is written down.
///
/// The path and never the bytes. A log grows for the life of a session and is
/// read back by a build that may not be this one; base64 in it would be the
/// unbounded file the bound log exists to prevent, and a picture nobody can
/// read without the session it came from.
fn attached(attachment: &Attachment) -> Value {
    json!({
        "path": attachment.path.as_ref(),
        "modality": modal(attachment.modality),
        "media_type": attachment.media_type.as_ref(),
        "hash": hashed(&attachment.hash),
    })
}

fn attachment(value: &Value) -> Option<Attachment> {
    Some(Attachment {
        path: value.get("path")?.as_str()?.into(),
        modality: modality(value.get("modality")?)?,
        media_type: value.get("media_type")?.as_str()?.into(),
        hash: hash(value.get("hash")?)?,
    })
}

/// What kind of file it is, as the log spells it.
///
/// Spelled here rather than taken from [`Modality::as_str`], for the reason the
/// rest of this shape is spelled here: those words answer to a generated model
/// database, and a vocabulary that moved there would silently change what every
/// log written afterwards says.
fn modal(modality: Modality) -> &'static str {
    match modality {
        Modality::Text => "text",
        Modality::Image => "image",
        Modality::Pdf => "pdf",
        Modality::Video => "video",
        Modality::Audio => "audio",
    }
}

/// A word this build has no name for makes the line unreadable, which is the
/// opposite of what an unknown stop reason does one function below.
///
/// There is no `Unknown` to fall back to, and no useful guess: a file read as
/// the wrong kind is a file sent to a model in a shape it did not agree to. A
/// line nobody can read stops the replay, and stopping is the honest answer to
/// a log this build does not understand.
fn modality(value: &Value) -> Option<Modality> {
    Some(match value.as_str()? {
        "text" => Modality::Text,
        "image" => Modality::Image,
        "pdf" => Modality::Pdf,
        "video" => Modality::Video,
        "audio" => Modality::Audio,
        _ => return None,
    })
}

/// A hash as the log spells it: 64 lowercase hexadecimal characters.
fn hashed(hash: &[u8; 32]) -> String {
    let mut out = String::with_capacity(hash.len() * 2);
    for byte in hash {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn hash(value: &Value) -> Option<[u8; 32]> {
    let mut hash = [0; 32];
    let mut written = value.as_str()?.bytes();

    for byte in &mut hash {
        *byte = (nibble(written.next()?)? << 4) | nibble(written.next()?)?;
    }

    // Anything left over is a hash of the wrong length, which is a line this
    // build did not write and will not guess at.
    written.next().is_none().then_some(hash)
}

/// Lowercase only: uppercase is spelling this file has never used, so a line
/// carrying it came from somewhere other than a session log.
fn nibble(written: u8) -> Option<u8> {
    match written {
        b'0'..=b'9' => Some(written - b'0'),
        b'a'..=b'f' => Some(written - b'a' + 10),
        _ => None,
    }
}

/// How an answer ended, as the log spells it.
///
/// Spelled here rather than taken from the enum's `Debug`, for the same reason
/// the rest of this shape is: a rename in the domain would silently change what
/// every log written afterwards says.
///
/// Every variant is named rather than caught by a rest pattern — a reason added
/// to [`StopReason`] is a reason the log has to learn a word for, and the
/// build stopping here is what asks for one.
fn stopped(stop: StopReason) -> &'static str {
    match stop {
        StopReason::Yielded => "yielded",
        StopReason::WantsTools => "tools",
        StopReason::OutOfTokens => "tokens",
        StopReason::WindowExceeded => "window",
        StopReason::Filtered => "filtered",
        StopReason::Paused => "paused",
        StopReason::Cancelled => "cancelled",
        StopReason::Unknown => "unknown",
    }
}

/// What an agent line says about how the answer ended.
///
/// Null — and, for a line damaged in a way that still parses, absent — is an
/// answer that never reached an ending, which is what the runner records for a
/// response that broke off part way.
///
/// A word this build has no name for reads as [`StopReason::Unknown`] rather
/// than making the line unreadable. It is the same answer the providers give a
/// vendor's new word, and it costs a session nothing; what it must never become
/// is a finish, which is the one reading that would put a truncated turn back
/// in front of the model as a whole one.
fn stops(value: Option<&Value>) -> Option<StopReason> {
    Some(match value.and_then(Value::as_str)? {
        "yielded" => StopReason::Yielded,
        "tools" => StopReason::WantsTools,
        "tokens" => StopReason::OutOfTokens,
        "window" => StopReason::WindowExceeded,
        "filtered" => StopReason::Filtered,
        "paused" => StopReason::Paused,
        "cancelled" => StopReason::Cancelled,
        _ => StopReason::Unknown,
    })
}

/// Every element of a JSON array, read the same way, or `None` if any of them
/// cannot be.
fn read<T>(value: &Value, each: fn(&Value) -> Option<T>) -> Option<Vec<T>> {
    value.as_array()?.iter().map(each).collect()
}

/// A tool call as it is written down.
///
/// The arguments stay the text the model wrote. They are the model's own JSON
/// and are parsed by the tool that owns them, so re-encoding them here would
/// be a second chance to change what it said.
fn called(call: &ToolCall) -> Value {
    json!({
        "id": call.id.as_str(),
        "name": call.name.as_ref(),
        "args": call.args.as_str(),
    })
}

fn call(value: &Value) -> Option<ToolCall> {
    Some(ToolCall {
        id: ToolId::new(value.get("id")?.as_str()?),
        name: value.get("name")?.as_str()?.into(),
        args: crucible_core::ToolArgs::new(value.get("args")?.as_str()?),
    })
}

/// One tool result as the line that records it.
///
/// The files go down beside the text under the key a prompt's do, and the count
/// a change came to goes down beside them. Both are left off entirely where
/// there is nothing to say — so a session whose tools showed nothing and changed
/// nothing is written exactly as format 6 wrote it, and one that only changed
/// nothing exactly as format 8 did.
///
/// The count and not the lines. A log is a file on disk that outlives the
/// session, and a line of a file that held a key is a key; two integers name
/// nothing and are the whole of what a change header is written from.
fn answered(result: &ToolResult) -> Value {
    let mut object = serde_json::Map::new();
    object.insert("id".to_owned(), json!(result.id.as_str()));
    object.insert("failed".to_owned(), json!(result.output.is_failed()));
    object.insert("text".to_owned(), json!(result.output.text()));

    let attachments = result.output.attachments();
    if !attachments.is_empty() {
        object.insert(
            "attached".to_owned(),
            json!(attachments.iter().map(attached).collect::<Vec<_>>()),
        );
    }

    // A call that left the file as it was is a call with no header to draw, and
    // writing a count of nothing would say there was one.
    if let Some(changed) = result.output.changed().filter(|counts| !counts.is_empty()) {
        object.insert(
            "change".to_owned(),
            json!({ "added": changed.added(), "removed": changed.removed() }),
        );
    }

    Value::Object(object)
}

/// One count off a `change`, or `None` where it is not one.
fn counted(change: &Value, name: &str) -> Option<usize> {
    usize::try_from(change.get(name)?.as_u64()?).ok()
}

fn result(value: &Value) -> Option<ToolResult> {
    let text = value.get("text")?.as_str()?;
    let failed = value.get("failed")?.as_bool()?;

    let output = if failed {
        ToolOutput::failed(text)
    } else {
        ToolOutput::ok(text)
    };

    // Restored rather than admitted again. The verdict that let this tool read
    // is over and no log holds one; what says it was reached is that this build
    // wrote the line at all.
    let output = match value.get("attached") {
        Some(attached) => output.replayed(read(attached, attachment)?),
        None => output,
    };

    // A line with no `change` is a call that changed no file, or one an older
    // build recorded before there was anywhere to say so. Neither has a header,
    // and both fall through to the words instead.
    let output = match value.get("change") {
        Some(change) => output.counting(Changed::new(
            counted(change, "added")?,
            counted(change, "removed")?,
        )),
        None => output,
    };

    Some(ToolResult {
        id: ToolId::new(value.get("id")?.as_str()?),
        output,
    })
}

#[cfg(test)]
mod tests {
    use crucible_core::{
        Approved, Ask, Attachment, Modality, Permission, Remember, Sensitivity, Settled, Target,
        ToolArgs, Verdict,
    };

    use super::*;

    /// Nobody to ask. A read is settled without a question in every mode, so a
    /// test that reaches this has stopped testing what it meant to.
    struct Unasked;

    impl Ask for Unasked {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            (Verdict::Deny, Remember::Never)
        }
    }

    /// A verdict about the very read that found the file.
    fn permitted() -> Approved {
        let call = ToolCall {
            id: ToolId::new("call-1"),
            name: "read".into(),
            args: ToolArgs::new("{}"),
        };
        let settled = Permission::new().decide(
            &call,
            &Sensitivity::ReadOnly {
                target: Target::unresolved(),
            },
            &mut Unasked,
        );

        let Settled::Approved(approved) = settled else {
            panic!("a read is allowed without a question")
        };
        approved
    }

    fn holiday() -> Attachment {
        Attachment {
            path: "pictures/holiday.png".into(),
            modality: Modality::Image,
            media_type: "image/png".into(),
            hash: [0xab; 32],
        }
    }

    /// A result for a call that rewrote a file, counted and no longer showing.
    fn rewrote(changed: Changed) -> Message {
        Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: ToolOutput::ok("rewrote main.rs").counting(changed),
        }])
    }

    #[test]
    fn the_count_a_change_came_to_survives_the_line_and_a_count_of_nothing_does_not() {
        // Two cases, and they end differently on purpose. A call that moved
        // lines writes what it moved and reads back as the same message. A call
        // that moved none writes no key at all -- which is what keeps a
        // diff-less line the bytes the format before this one wrote -- so it
        // comes back saying there was no count rather than saying the count was
        // zero, and the message it comes back as is not the one that went in.
        let moved = rewrote(Changed::new(2, 1));
        assert_eq!(message(&line(&moved)).as_ref(), Some(&moved));

        let still = rewrote(Changed::new(0, 0));
        let read = message(&line(&still)).expect("the line to read back as a message");
        assert_ne!(read, still, "a count of nothing came back as one");

        let Message::ToolResults(results) = &read else {
            panic!("a line of results reads back as results")
        };
        let [only] = results.as_slice() else {
            panic!("one result went in")
        };
        assert_eq!(only.output.changed(), None);
        assert_eq!(only.output.text(), "rewrote main.rs");
    }

    #[test]
    fn a_count_goes_down_beside_the_result_and_not_inside_its_words() {
        assert_eq!(
            line(&rewrote(Changed::new(2, 1))),
            r#"{"results":[{"change":{"added":2,"removed":1},"failed":false,"id":"call-1","text":"rewrote main.rs"}]}"#
        );
    }

    #[test]
    fn a_prompt_and_the_files_beside_it_survive_the_line_that_records_them() {
        let asked = Message::User {
            text: "what is in this".into(),
            attachments: Box::new([holiday()]),
        };

        let read = message(&line(&asked));

        assert_eq!(read.as_ref(), Some(&asked));
    }

    #[test]
    fn every_modality_has_a_word_in_the_log_and_reads_back_as_itself() {
        for modality in Modality::EVERY {
            let attachment = Attachment {
                modality,
                ..holiday()
            };
            let written = Message::User {
                text: "look".into(),
                attachments: Box::new([attachment]),
            };

            assert_eq!(message(&line(&written)).as_ref(), Some(&written));
        }
    }

    #[test]
    fn a_prompt_with_nothing_attached_is_written_exactly_as_format_five_wrote_it() {
        // The reason `attached` is left off rather than written empty: a
        // text-only session's log must not change at all under format 6.
        assert_eq!(line(&Message::said("hello")), r#"{"user":"hello"}"#);
    }

    #[test]
    fn a_context_fragment_survives_the_line_that_records_it() {
        let fragment = Message::Context(Fragment::new("workspace", "Workspace: /src"));

        assert_eq!(
            line(&fragment),
            r#"{"context":{"section":"workspace","text":"Workspace: /src"}}"#
        );
        assert_eq!(message(&line(&fragment)).as_ref(), Some(&fragment));
    }

    #[test]
    fn a_line_a_format_five_build_wrote_still_reads_as_the_prompt_it_recorded() {
        // Frozen bytes, not a round trip: a round trip would agree with itself
        // however the shape moved.
        assert_eq!(
            message(r#"{"user":"what did I ask"}"#),
            Some(Message::said("what did I ask"))
        );
        assert!(readable(5), "a format 5 log must still replay");
    }

    #[test]
    fn a_tool_result_and_the_file_it_showed_survive_the_line_that_records_them() {
        let found = Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: ToolOutput::ok("one match").with_attachments(&permitted(), [holiday()]),
        }]);

        let read = message(&line(&found)).expect("the line to read back as a message");
        let Message::ToolResults(results) = &read else {
            panic!("a line of results reads back as results")
        };
        let [only] = results.as_slice() else {
            panic!("one result went in")
        };
        let [file] = only.output.attachments() else {
            panic!("the file the result showed did not survive the log")
        };

        assert_eq!(file, &holiday());
        assert_eq!(read, found, "and nothing else about the result moved");
    }

    #[test]
    fn a_tool_result_that_showed_nothing_is_written_exactly_as_format_six_wrote_it() {
        // The same reason the prompt's key is left off rather than written
        // empty, one format later: a session whose tools showed nothing must
        // not change at all under format 7.
        let quiet = Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: ToolOutput::ok("one match"),
        }]);

        assert_eq!(
            line(&quiet),
            r#"{"results":[{"failed":false,"id":"call-1","text":"one match"}]}"#
        );
    }

    #[test]
    fn the_format_moves_with_the_line_shape() {
        assert_eq!(FORMAT, 10);
        assert!(readable(FORMAT));
    }

    #[test]
    fn the_branch_the_caller_supplied_survives_the_header() {
        let id: SessionId = "018bcfe5-687b-7abc-8def-0123456789ab"
            .parse()
            .expect("a uuid session id parses");

        let written = header(&id, Path::new("/somewhere"), Some("feature/picker"));
        let read = opening(&written).expect("the header this build writes opens");

        assert_eq!(read.branch.as_deref(), Some("feature/picker"));
        assert_eq!(read.workspace, "/somewhere");
        assert_eq!(read.format, FORMAT);
    }

    #[test]
    fn a_header_without_a_branch_opens_with_none() {
        // Frozen bytes, not a round trip: this is the header a format 7 build
        // wrote, and a session that never learned its branch writes the same
        // absence today.
        let old = r#"{"format":7,"session":"0000000000001-000001","workspace":"/w"}"#;

        let read = opening(old).expect("a format 7 header still opens");

        assert_eq!(read.branch, None);
        assert_eq!(read.workspace, "/w");
        assert!(readable(read.format), "a format 7 log must still replay");
    }
}
