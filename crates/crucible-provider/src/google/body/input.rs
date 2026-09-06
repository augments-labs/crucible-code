//! Borrowed local history lowered to stateless Interactions input.
//!
//! Only same-recipient Google state is native. Other providers, unsigned old
//! logs and recap requests become descriptive user context with no executable
//! function-call framing. Native local calls must each be answered once before
//! another conversation step. Prefix compaction does not rewrite retained steps.

use super::super::{PROTOCOL, protocol};
use crate::json::{Array, Object};
use crucible_core::{
    Attached, Content, ContinuationPart, ContinuationScope, Message, Modality,
    ProviderContinuation, ProviderError, Request, RequestPurpose, ToolCall,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub(super) fn write(
    input: &mut Array<'_>,
    request: &Request<'_>,
    scope: ContinuationScope,
) -> Result<(), ProviderError> {
    // Borrow at most the128 IDs already validated by native(), not another
    // history-sized index. Some(empty) still rejects a duplicate result after
    // the native group has been fully answered; a foreign Agent resets it.
    let mut pending: Option<BTreeSet<&str>> = None;
    for (nth, message) in request.transcript.messages().iter().enumerate() {
        if request.purpose == RequestPurpose::Turn {
            if !matches!(message, Message::ToolResults(_)) {
                answered(pending.as_ref())?;
            }
            if let Message::Agent { .. } = message {
                pending = None;
            }
            if let Message::Agent {
                text,
                calls,
                continuation: Some(state),
                ..
            } = message
                && compatible(state, scope, request.model)
            {
                state
                    .validate(text, calls.len())
                    .map_err(|_| protocol("invalid continuation references"))?;
                native(input, state, text, calls)?;
                pending = Some(calls.iter().map(|call| call.id.as_str()).collect());
                continue;
            }
            if let Message::ToolResults(results) = message
                && let Some(waiting) = pending.as_mut()
            {
                for result in results {
                    if !waiting.remove(result.id.as_str()) {
                        return Err(protocol(
                            "function result does not match an unanswered call",
                        ));
                    }
                }
                super::results::write(input, results, request.attached, nth)?;
                continue;
            }
        }
        let mut outcome = Ok(());
        input.object(|step| {
            step.text("type", "user_input");
            step.array("content", |content| {
                content.object(|part| {
                    part.text("type", "text");
                    match (request.purpose, message) {
                        (RequestPurpose::Turn, Message::User { text, .. }) => {
                            part.text("text", text);
                        }
                        (RequestPurpose::Turn, Message::Context(fragment)) => {
                            part.text("text", fragment.text());
                        }
                        _ => {
                            part.text_with("text", |write| crate::history::visible(message, write));
                        }
                    }
                });
                if request.purpose == RequestPurpose::Turn {
                    for file in request.attached.iter().filter(|file| file.message == nth) {
                        if outcome.is_ok() {
                            content.object(|part| outcome = attachment(part, file));
                        }
                    }
                }
            });
        });
        outcome?;
    }
    answered(pending.as_ref())
}

fn answered(pending: Option<&BTreeSet<&str>>) -> Result<(), ProviderError> {
    if pending.is_some_and(|waiting| !waiting.is_empty()) {
        return Err(protocol("history has unanswered function calls"));
    }
    Ok(())
}

pub(super) fn attachment(part: &mut Object<'_>, file: &Attached<'_>) -> Result<(), ProviderError> {
    match file.content {
        Content::Instead(line) => {
            part.text("type", "text");
            part.text("text", line);
        }
        Content::Bytes(bytes) => {
            let kind = match file.modality {
                Modality::Text => {
                    let text = std::str::from_utf8(bytes)
                        .map_err(|_| protocol("text attachment is not valid UTF-8"))?;
                    part.text("type", "text");
                    part.text("text", text);
                    return Ok(());
                }
                Modality::Image => "image",
                Modality::Pdf => "document",
                Modality::Video => "video",
                Modality::Audio => "audio",
            };
            part.text("type", kind);
            part.text("mime_type", file.media_type);
            part.encoded("data", bytes);
        }
    }
    Ok(())
}

fn compatible(state: &ProviderContinuation, scope: ContinuationScope, model: &str) -> bool {
    state.protocol() == PROTOCOL
        && state.scope() == scope
        && state.model().starts_with("gemini-")
        && model.starts_with("gemini-")
}

fn native(
    input: &mut Array<'_>,
    state: &ProviderContinuation,
    text: &str,
    calls: &[ToolCall],
) -> Result<(), ProviderError> {
    let mut parts = state.parts().iter().peekable();
    let mut identities = super::super::calls::Calls::default();
    let mut retained = state.retained_bytes();
    while let Some(part) = parts.next() {
        match part {
            ContinuationPart::Opaque(data) => {
                let mut value = parse(data.as_str())?;
                if let Some(output) = value.remove("output") {
                    if !value.is_empty() {
                        return Err(protocol("invalid output grouping"));
                    }
                    let Value::Object(output) = output else {
                        return Err(protocol("invalid output grouping"));
                    };
                    if output.get("type").and_then(Value::as_str) != Some("model_output")
                        || output.contains_key("content")
                    {
                        return Err(protocol("invalid output attributes"));
                    }
                    let mut outcome = Ok(());
                    input.object(|step| {
                        attributes(step, &output);
                        step.array("content", |content| {
                            while let Some(ContinuationPart::Text { start, end, data }) =
                                parts.peek()
                            {
                                let result = text_part(content, text, *start, *end, data.as_str());
                                parts.next();
                                if let Err(error) = result {
                                    outcome = Err(error);
                                    break;
                                }
                            }
                        });
                    });
                    outcome?;
                } else {
                    super::super::shape::native(&value)?;
                    identities.native(&value, &mut retained)?;
                    input.value(&Value::Object(value));
                }
            }
            ContinuationPart::Call { index, data } => {
                let fields = parse(data.as_str())?;
                if fields.get("type").and_then(Value::as_str) != Some("function_call")
                    || ["id", "name", "arguments"]
                        .iter()
                        .any(|key| fields.contains_key(*key))
                {
                    return Err(protocol("invalid call attributes"));
                }
                let call = calls
                    .get(*index)
                    .ok_or_else(|| protocol("invalid call reference"))?;
                identities.function(call.id.as_str(), &mut retained)?;
                let arguments: Value = serde_json::from_str(call.args.as_str())
                    .map_err(|_| protocol("invalid replay arguments"))?;
                if !arguments.is_object() {
                    return Err(protocol("invalid replay arguments"));
                }
                input.object(|step| {
                    attributes(step, &fields);
                    step.text("id", call.id.as_str());
                    step.text("name", &call.name);
                    step.value("arguments", &arguments);
                });
            }
            ContinuationPart::Text { .. } => return Err(protocol("text without output grouping")),
        }
    }
    identities.completed()
}

fn text_part(
    content: &mut Array<'_>,
    text: &str,
    start: usize,
    end: usize,
    data: &str,
) -> Result<(), ProviderError> {
    let fields = parse(data)?;
    if fields.get("type").and_then(Value::as_str) != Some("text") || fields.contains_key("text") {
        return Err(protocol("invalid replay text attributes"));
    }
    let text = text
        .get(start..end)
        .ok_or_else(|| protocol("invalid replay text range"))?;
    content.object(|part| {
        attributes(part, &fields);
        part.text("text", text);
    });
    Ok(())
}

fn parse(data: &str) -> Result<Map<String, Value>, ProviderError> {
    serde_json::from_str(data).map_err(|_| protocol("invalid private interaction data"))
}

fn attributes(object: &mut Object<'_>, fields: &Map<String, Value>) {
    for (key, value) in fields {
        object.value(key, value);
    }
}
