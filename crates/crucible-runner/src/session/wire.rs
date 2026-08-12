//! The session log's line shape.
//!
//! One JSON object per line, in the order the messages happened. The shape is
//! owned here rather than derived from the domain types, for the same reason
//! each provider owns its own: a 0.0.x file format may change in any release,
//! and pinning [`Message`] to one would make every such change a change to the
//! domain everything else is written against.
//!
//! Reading is total. Anything a line cannot be read as comes back as `None`,
//! and the caller stops there rather than replaying a transcript with a hole
//! in it.

use std::path::Path;

use crucible_core::{Message, SessionId, ToolCall, ToolId, ToolOutput, ToolResult};
use serde_json::{Value, json};

/// What wrote the file.
///
/// Read back and checked. A log from a build that spelled things differently
/// is refused rather than half-understood, which is the difference between
/// telling the user their session cannot be continued and silently continuing
/// a different one.
pub(crate) const FORMAT: u32 = 2;

/// The line that says everything before it was forgotten.
///
/// A marker rather than a rewrite. The log is append-only — it is written by a
/// thread that never seeks, and it is what a crashed process leaves behind —
/// so what a session forgets is recorded as something that happened at a point
/// in it, the same as a message. Replay reads it and starts the transcript
/// again from there.
pub(crate) fn forgotten() -> String {
    json!({ "forgotten": true }).to_string()
}

/// Whether this line is that marker.
pub(crate) fn forgets(line: &str) -> bool {
    serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|value| value.get("forgotten").and_then(Value::as_bool))
        .unwrap_or_default()
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
        Message::Agent { text, calls } => json!({
            "agent": text.as_ref(),
            "calls": calls.iter().map(called).collect::<Vec<_>>(),
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
        });
    }

    Some(Message::ToolResults(read(value.get("results")?, result)?))
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
