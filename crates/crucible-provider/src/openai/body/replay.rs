//! Native item grouping is borrowed from one compatible assistant at a time.
//!
//! Foreign/unsigned history and recaps remain visible descriptive context, never
//! encrypted state or executable call framing. Retained complete items survive
//! client prefix compaction unchanged; every native call must be answered once.

use crate::json::{Array, Object};
use crate::openai::continuation::{
    PROTOCOL, field, header as validate_header, identity, problem, validate,
};
use crucible_core::{
    ContinuationPart, ContinuationScope, Message, ProviderContinuation, ProviderError, Request,
    RequestPurpose, TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, ToolCall,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub(super) fn compatible(state: &ProviderContinuation, scope: ContinuationScope) -> bool {
    state.protocol() == PROTOCOL && state.scope() == scope && state.model() == crate::openai::ASTRA
}

pub(super) fn write(
    input: &mut Array<'_>,
    request: &Request<'_>,
    scope: ContinuationScope,
    explicit: Option<usize>,
) -> Result<(), ProviderError> {
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
                && compatible(state, scope)
            {
                native(input, state, text, calls)?;
                pending = Some(calls.iter().map(|call| call.id.as_str()).collect());
                continue;
            }
            if let Message::ToolResults(results) = message
                && let Some(waiting) = pending.as_mut()
            {
                for result in results {
                    if !waiting.remove(result.id.as_str()) {
                        return Err(problem("function result does not match an unanswered call"));
                    }
                }
                super::append(input, message, nth, request.attached, explicit == Some(nth));
                continue;
            }
            if matches!(message, Message::User { .. } | Message::Context(_)) {
                super::append(input, message, nth, request.attached, explicit == Some(nth));
                continue;
            }
        }
        input.object(|item| {
            item.text("role", "user");
            if explicit == Some(nth) {
                item.array("content", |content| {
                    content.object(|part| {
                        part.text("type", "input_text");
                        part.text_with("text", |write| crate::history::visible(message, write));
                        part.object("prompt_cache_breakpoint", |marker| {
                            marker.text("mode", "explicit");
                        });
                    });
                });
            } else {
                item.text_with("content", |write| crate::history::visible(message, write));
            }
        });
    }
    answered(pending.as_ref())
}

fn answered(pending: Option<&BTreeSet<&str>>) -> Result<(), ProviderError> {
    if pending.is_some_and(|waiting| !waiting.is_empty()) {
        return Err(problem("history has unanswered function calls"));
    }
    Ok(())
}

fn native(
    input: &mut Array<'_>,
    state: &ProviderContinuation,
    text: &str,
    calls: &[ToolCall],
) -> Result<(), ProviderError> {
    state
        .validate(text, calls.len())
        .map_err(|_| problem("invalid continuation references"))?;
    let mut identities = BTreeSet::new();
    if calls.len() > 128 {
        return Err(problem("too many replay calls"));
    }
    for call in calls {
        identity(call.id.as_str(), TOOL_CALL_ID_BYTES)?;
        identity(&call.name, TOOL_NAME_BYTES)?;
        if !identities.insert(call.id.as_str()) {
            return Err(problem("duplicate replay call id"));
        }
    }
    let mut parts = state.parts().iter().peekable();
    let Some(ContinuationPart::Opaque(header)) = parts.next() else {
        return Err(problem("missing request effort record"));
    };
    let header = parse(header.as_str())?;
    if header.len() != 1
        || !header.get("request_effort").is_some_and(|value| {
            value.is_null()
                || matches!(
                    value.as_str(),
                    Some("low" | "medium" | "high" | "xhigh" | "max")
                )
        })
    {
        return Err(problem("invalid request effort record"));
    }
    while let Some(part) = parts.next() {
        match part {
            ContinuationPart::Opaque(data) => {
                let mut fields = parse(data.as_str())?;
                if let Some(message) = fields.remove("message") {
                    validate_header(&message)?;
                    let message = message
                        .as_object()
                        .ok_or_else(|| problem("invalid message attributes"))?;
                    if !fields.is_empty()
                        || message.contains_key("content")
                        || message.get("type").and_then(Value::as_str) != Some("message")
                        || message.get("role").and_then(Value::as_str) != Some("assistant")
                    {
                        return Err(problem("invalid message grouping"));
                    }
                    let mut outcome = Ok(());
                    input.object(|item| {
                        attributes(item, message);
                        item.array("content", |content| {
                            while let Some(ContinuationPart::Text { start, end, data }) =
                                parts.peek()
                            {
                                if outcome.is_ok() {
                                    outcome = text_part(content, text, *start, *end, data.as_str());
                                }
                                parts.next();
                            }
                        });
                    });
                    outcome?;
                } else {
                    let value = Value::Object(fields);
                    if !matches!(
                        field(&value, "type")?,
                        "reasoning" | "web_search_call" | "compaction"
                    ) {
                        return Err(problem("invalid opaque item"));
                    }
                    validate(&value)?;
                    input.value(&value);
                }
            }
            ContinuationPart::Call { index, data } => {
                let value = Value::Object(parse(data.as_str())?);
                validate_header(&value)?;
                let fields = value
                    .as_object()
                    .ok_or_else(|| problem("invalid call attributes"))?;
                if fields.get("type").and_then(Value::as_str) != Some("function_call")
                    || ["call_id", "name", "arguments"]
                        .iter()
                        .any(|key| fields.contains_key(*key))
                {
                    return Err(problem("invalid call attributes"));
                }
                let call = calls
                    .get(*index)
                    .ok_or_else(|| problem("invalid call reference"))?;
                if !serde_json::from_str::<Value>(call.args.as_str())
                    .is_ok_and(|value| value.is_object())
                {
                    return Err(problem("invalid replay arguments"));
                }
                input.object(|item| {
                    attributes(item, fields);
                    item.text("call_id", call.id.as_str());
                    item.text("name", &call.name);
                    item.text("arguments", call.args.as_str());
                });
            }
            ContinuationPart::Text { .. } => return Err(problem("text without message grouping")),
        }
    }
    Ok(())
}

fn text_part(
    content: &mut Array<'_>,
    text: &str,
    start: usize,
    end: usize,
    data: &str,
) -> Result<(), ProviderError> {
    let fields = parse(data)?;
    if fields.contains_key("text") || fields.contains_key("refusal") {
        return Err(problem("invalid replay text attributes"));
    }
    let key = match fields.get("type").and_then(Value::as_str) {
        Some("output_text") => "text",
        Some("refusal") => "refusal",
        _ => return Err(problem("invalid replay text type")),
    };
    let text = text
        .get(start..end)
        .ok_or_else(|| problem("invalid replay text range"))?;
    content.object(|part| {
        attributes(part, &fields);
        part.text(key, text);
    });
    Ok(())
}

fn parse(data: &str) -> Result<Map<String, Value>, ProviderError> {
    match serde_json::from_str(data) {
        Ok(Value::Object(fields)) => Ok(fields),
        _ => Err(problem("invalid continuation JSON")),
    }
}

fn attributes(item: &mut Object<'_>, fields: &Map<String, Value>) {
    for (key, value) in fields {
        item.value(key, value);
    }
}
