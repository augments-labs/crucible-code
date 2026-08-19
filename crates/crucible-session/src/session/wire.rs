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

use std::path::Path;

use crucible_core::{Message, SessionId, StopReason, ToolCall, ToolId, ToolOutput, ToolResult};
use serde_json::{Value, json};

/// What wrote the file.
///
/// Read back and checked. A log from a build that spelled things differently
/// is refused rather than half-understood, which is the difference between
/// telling the user their session cannot be continued and silently continuing
/// a different one.
pub(crate) const FORMAT: u32 = 4;

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

/// The first line, which says what the file is and what it belongs to.
pub(crate) fn header(session: &SessionId, workspace: &Path) -> String {
    json!({
        "format": FORMAT,
        "session": session.as_str(),
        "workspace": workspace.display().to_string(),
    })
    .to_string()
}

/// What a header says, or `None` if this is not one.
pub(crate) fn opening(line: &str) -> Option<Opening> {
    let value: Value = serde_json::from_str(line).ok()?;

    Some(Opening {
        format: u32::try_from(value.get("format")?.as_u64()?).ok()?,
        workspace: value.get("workspace")?.as_str()?.to_owned(),
    })
}

/// What the first line of a log says about it.
pub(crate) struct Opening {
    pub(crate) format: u32,
    pub(crate) workspace: String,
}

/// One message as the line that records it.
pub(crate) fn line(message: &Message) -> String {
    let value = match message {
        Message::User(text) => json!({ "user": text.as_ref() }),
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

    if let Some(text) = value.get("user") {
        return Some(Message::User(text.as_str()?.into()));
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

fn answered(result: &ToolResult) -> Value {
    json!({
        "id": result.id.as_str(),
        "failed": result.output.is_failed(),
        "text": result.output.text(),
    })
}

fn result(value: &Value) -> Option<ToolResult> {
    let text = value.get("text")?.as_str()?;
    let failed = value.get("failed")?.as_bool()?;

    Some(ToolResult {
        id: ToolId::new(value.get("id")?.as_str()?),
        output: if failed {
            ToolOutput::failed(text)
        } else {
            ToolOutput::ok(text)
        },
    })
}
