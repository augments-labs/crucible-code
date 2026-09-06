//! Bounded assembly of one native step, independent of interleaving.
//!
//! Visible text is drained as soon as its step reaches the delivery frontier.
//! Only byte ranges and native attributes survive in continuation. Tool argument
//! fragments stay private until the complete object is validated at step stop.

use super::protocol;
use crucible_core::{
    CONTINUATION_BYTES, CONTINUATION_PARTS, Continuation, ContinuationData, ContinuationPart,
    Delta, ProviderError, TOOL_ARGUMENT_BYTES, TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, ToolId,
};
use serde_json::{Map, Value, json};
use std::io;

const TEXT_BYTES: usize = 8 * 1024 * 1024;

/// Conservative decoded-wire admission, separate from the exact final envelope
/// budget. Counters are checked before an event grows retained assembly state.
#[derive(Default)]
pub(super) struct Budget {
    private: usize,
    text: usize,
    arguments: usize,
    calls: super::super::calls::Calls,
}

impl Budget {
    fn private(&mut self, value: &Value) -> Result<(), ProviderError> {
        claim(&mut self.private, weight(value), CONTINUATION_BYTES)
    }
    fn text(&mut self, value: &str) -> Result<(), ProviderError> {
        claim(&mut self.text, value.len(), TEXT_BYTES)
    }

    fn call(&mut self, id: &str) -> Result<(), ProviderError> {
        self.calls.function(id, &mut self.private)
    }

    pub(super) fn completed(&self) -> Result<(), ProviderError> {
        self.calls.completed()
    }
}

fn claim(used: &mut usize, amount: usize, cap: usize) -> Result<(), ProviderError> {
    let next = used.saturating_add(amount);
    if next > cap {
        return Err(protocol("interaction response exceeds its resource limit"));
    }
    *used = next;
    Ok(())
}

fn weight(value: &Value) -> usize {
    match value {
        Value::String(s) => s.len().saturating_add(32),
        Value::Array(a) => a.iter().fold(32, |n, v| n.saturating_add(weight(v))),
        Value::Object(o) => o.iter().fold(64, |n, (k, v)| {
            n.saturating_add(k.len())
                .saturating_add(64)
                .saturating_add(weight(v))
        }),
        _ => 32,
    }
}

struct Text {
    attributes: Map<String, Value>,
    pending: String,
    start: Option<usize>,
    end: usize,
}

pub(super) struct Step {
    native: Map<String, Value>,
    content: Vec<Text>,
    arguments: Option<String>,
    initial_argument_bytes: usize,
    pub(super) stopped: bool,
}

impl Step {
    pub(super) fn new(value: Value, budget: &mut Budget) -> Result<Self, ProviderError> {
        let Value::Object(mut native) = value else {
            return Err(protocol("invalid interaction step"));
        };
        let mut content = Vec::new();
        let mut initial_argument_bytes = 0;
        match native.get("type").and_then(Value::as_str) {
            Some("model_output") => {
                if let Some(value) = native.remove("content") {
                    let Value::Array(parts) = value else {
                        return Err(protocol("invalid model output content"));
                    };
                    if parts.len() > CONTINUATION_PARTS {
                        return Err(protocol("too many model output parts"));
                    }
                    for part in parts {
                        content.push(text(part, budget)?);
                    }
                }
            }
            Some("function_call") => {
                identity(&native, "id", TOOL_CALL_ID_BYTES)?;
                identity(&native, "name", TOOL_NAME_BYTES)?;
                let arguments = native
                    .get("arguments")
                    .ok_or_else(|| protocol("missing function arguments"))?;
                if !arguments.is_object() {
                    return Err(protocol("function arguments are not an object"));
                }
                initial_argument_bytes = argument_bytes(arguments)?;
                claim(
                    &mut budget.arguments,
                    initial_argument_bytes,
                    TOOL_ARGUMENT_BYTES,
                )?;
                budget.call(
                    native
                        .get("id")
                        .and_then(Value::as_str)
                        .ok_or_else(|| protocol("missing function call identity"))?,
                )?;
            }
            Some(kind) if opaque(kind) => {}
            _ => return Err(protocol("unsupported interaction step type")),
        }
        // Arguments have their own bound and do not consume private-state room.
        if native.get("type").and_then(Value::as_str) == Some("function_call") {
            let arguments = native
                .remove("arguments")
                .ok_or_else(|| protocol("missing function arguments"))?;
            budget.private(&Value::Object(native.clone()))?;
            native.insert("arguments".into(), arguments);
        } else {
            budget.private(&Value::Object(native.clone()))?;
        }
        Ok(Self {
            native,
            content,
            arguments: None,
            initial_argument_bytes,
            stopped: false,
        })
    }

    pub(super) fn apply(&mut self, delta: Value, budget: &mut Budget) -> Result<(), ProviderError> {
        let Value::Object(mut fields) = delta else {
            return Err(protocol("invalid interaction delta"));
        };
        let kind = fields
            .remove("type")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or_else(|| protocol("missing interaction delta type"))?;
        let step = self
            .native
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("");
        match (step, kind.as_str()) {
            ("model_output", "text") => {
                let value = fields
                    .remove("text")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .ok_or_else(|| protocol("missing text delta"))?;
                budget.text(&value)?;
                if !fields.is_empty() {
                    return Err(protocol("unsupported text delta attributes"));
                }
                if self.content.is_empty() {
                    self.content.push(Text {
                        attributes: Map::from_iter([("type".into(), json!("text"))]),
                        pending: String::new(),
                        start: None,
                        end: 0,
                    });
                }
                let last = self
                    .content
                    .last_mut()
                    .ok_or_else(|| protocol("missing text content"))?;
                last.pending.reserve_exact(value.len());
                last.pending.push_str(&value);
            }
            ("model_output", "text_annotation_delta") => {
                let annotations = fields
                    .remove("annotations")
                    .ok_or_else(|| protocol("missing text annotations"))?;
                if !annotations.is_array() || !fields.is_empty() {
                    return Err(protocol("invalid text annotations"));
                }
                budget.private(&annotations)?;
                let last = self
                    .content
                    .last_mut()
                    .ok_or_else(|| protocol("annotation without text content"))?;
                merge_field(&mut last.attributes, "annotations".into(), annotations)?;
            }
            ("function_call", "arguments_delta") => {
                let value = fields
                    .remove("arguments")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .ok_or_else(|| protocol("missing function argument delta"))?;
                if !fields.is_empty() {
                    return Err(protocol("unsupported function argument attributes"));
                }
                if self.arguments.is_none()
                    && !self
                        .native
                        .get("arguments")
                        .and_then(Value::as_object)
                        .is_some_and(Map::is_empty)
                {
                    return Err(protocol("argument delta follows complete arguments"));
                }
                if self.arguments.is_none() {
                    // The initial empty object is a placeholder, replaced by
                    // streamed JSON rather than prepended to it.
                    budget.arguments -= self.initial_argument_bytes;
                }
                claim(&mut budget.arguments, value.len(), TOOL_ARGUMENT_BYTES)?;
                let args = self.arguments.get_or_insert_with(String::new);
                if args.len().saturating_add(value.len()) > TOOL_ARGUMENT_BYTES {
                    return Err(protocol("function arguments exceed their limit"));
                }
                args.reserve_exact(value.len());
                args.push_str(&value);
            }
            ("thought", "thought_signature") => {
                let signature = fields
                    .remove("signature")
                    .ok_or_else(|| protocol("missing thought signature"))?;
                if !signature.is_string() || !fields.is_empty() {
                    return Err(protocol("invalid thought signature"));
                }
                budget.private(&signature)?;
                merge_field(&mut self.native, "signature".into(), signature)?;
            }
            ("thought", "thought_summary") => {
                let content = fields
                    .remove("content")
                    .ok_or_else(|| protocol("missing thought summary"))?;
                if !content.is_object() || !fields.is_empty() {
                    return Err(protocol("invalid thought summary"));
                }
                budget.private(&content)?;
                merge_field(
                    &mut self.native,
                    "summary".into(),
                    Value::Array(vec![content]),
                )?;
            }
            (native, delta) if native == delta && opaque(native) => {
                budget.private(&Value::Object(fields.clone()))?;
                for (key, value) in fields {
                    merge_field(&mut self.native, key, value)?;
                }
            }
            _ => return Err(protocol("delta does not match interaction step")),
        }
        Ok(())
    }

    pub(super) fn flush(&mut self, offset: &mut usize, deltas: &mut Vec<Delta>) {
        for part in &mut self.content {
            if part.start.is_none() || !part.pending.is_empty() {
                part.start.get_or_insert(*offset);
                *offset += part.pending.len();
                part.end = *offset;
            }
            if !part.pending.is_empty() {
                deltas.push(Delta::Text(std::mem::take(&mut part.pending).into()));
            }
        }
    }

    pub(super) fn finish(
        mut self,
        state: &mut Continuation,
        calls: &mut usize,
        deltas: &mut Vec<Delta>,
        budget: &mut Budget,
    ) -> Result<(), ProviderError> {
        match self.native.get("type").and_then(Value::as_str) {
            Some("model_output") => {
                push(
                    state,
                    ContinuationPart::Opaque(data(&json!({"output":self.native}))?),
                )?;
                for part in self.content {
                    push(
                        state,
                        ContinuationPart::Text {
                            start: part
                                .start
                                .ok_or_else(|| protocol("undelivered text content"))?,
                            end: part.end,
                            data: data(&Value::Object(part.attributes))?,
                        },
                    )?;
                }
            }
            Some("function_call") => {
                let id = self
                    .native
                    .remove("id")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .ok_or_else(|| protocol("missing call id"))?;
                let name = self
                    .native
                    .remove("name")
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .ok_or_else(|| protocol("missing call name"))?;
                let initial = self
                    .native
                    .remove("arguments")
                    .ok_or_else(|| protocol("missing call arguments"))?;
                let arguments = self.arguments.unwrap_or_else(|| initial.to_string());
                if arguments.len() > TOOL_ARGUMENT_BYTES
                    || !serde_json::from_str::<Value>(&arguments).is_ok_and(|v| v.is_object())
                {
                    return Err(protocol("function arguments are incomplete or invalid"));
                }
                push(
                    state,
                    ContinuationPart::Call {
                        index: *calls,
                        data: data(&Value::Object(self.native))?,
                    },
                )?;
                *calls += 1;
                deltas.push(Delta::ToolStarted {
                    id: ToolId::new(id),
                    name: name.into(),
                });
                deltas.push(Delta::ToolArgs(arguments.into()));
            }
            _ => {
                super::super::shape::native(&self.native)?;
                budget.calls.native(&self.native, &mut budget.private)?;
                push(
                    state,
                    ContinuationPart::Opaque(data(&Value::Object(self.native))?),
                )?;
            }
        }
        Ok(())
    }
}

/// Measure the argument representation without allocating a second JSON body.
/// Initial objects and streamed argument strings share one exact byte budget.
fn argument_bytes(value: &Value) -> Result<usize, ProviderError> {
    struct Size(usize);
    impl io::Write for Size {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > TOOL_ARGUMENT_BYTES.saturating_sub(self.0) {
                return Err(io::Error::other("function arguments exceed their limit"));
            }
            self.0 += bytes.len();
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut size = Size(0);
    serde_json::to_writer(&mut size, value)
        .map_err(|_| protocol("function arguments exceed their limit"))?;
    Ok(size.0)
}

fn text(value: Value, budget: &mut Budget) -> Result<Text, ProviderError> {
    let Value::Object(mut attributes) = value else {
        return Err(protocol("invalid text content"));
    };
    if attributes.get("type").and_then(Value::as_str) != Some("text") {
        return Err(protocol("unsupported model output content"));
    }
    let pending = attributes
        .remove("text")
        .and_then(|v| v.as_str().map(str::to_owned))
        .ok_or_else(|| protocol("missing initial text"))?;
    budget.text(&pending)?;
    budget.private(&Value::Object(attributes.clone()))?;
    Ok(Text {
        attributes,
        pending,
        start: None,
        end: 0,
    })
}

fn identity(native: &Map<String, Value>, key: &str, cap: usize) -> Result<(), ProviderError> {
    if !native
        .get(key)
        .and_then(Value::as_str)
        .is_some_and(|s| !s.is_empty() && s.len() <= cap && !s.chars().any(char::is_control))
    {
        return Err(protocol("invalid function call identity"));
    }
    Ok(())
}

pub(in crate::google) fn opaque(kind: &str) -> bool {
    matches!(
        kind,
        "thought"
            | "google_search_call"
            | "google_search_result"
            | "url_context_call"
            | "url_context_result"
            | "code_execution_call"
            | "code_execution_result"
            | "processing_call"
            | "processing_result"
    )
}

fn merge_field(
    fields: &mut Map<String, Value>,
    key: String,
    value: Value,
) -> Result<(), ProviderError> {
    if let Some(current) = fields.get_mut(&key) {
        if matches!(key.as_str(), "id" | "call_id") {
            // Identity is repeated or supplied once, never a string fragment.
            // Concatenating it can silently bind a result to a different call.
            if *current != value {
                return Err(protocol("conflicting native step identity"));
            }
        } else {
            merge(current, value)?;
        }
    } else {
        fields.insert(key, value);
    }
    Ok(())
}

fn merge(current: &mut Value, value: Value) -> Result<(), ProviderError> {
    match (current, value) {
        (Value::String(a), Value::String(b)) => {
            a.reserve_exact(b.len());
            a.push_str(&b);
        }
        (Value::Array(a), Value::Array(b)) => {
            a.reserve_exact(b.len());
            a.extend(b);
        }
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in b {
                merge_field(a, k, v)?;
            }
        }
        (a, b) if *a == b => {}
        _ => return Err(protocol("conflicting native step attributes")),
    }
    Ok(())
}

fn data(value: &Value) -> Result<ContinuationData, ProviderError> {
    ContinuationData::new(&value.to_string())
        .map_err(|_| protocol("interaction continuation exceeds its limit"))
}

fn push(state: &mut Continuation, part: ContinuationPart) -> Result<(), ProviderError> {
    state
        .push(part)
        .map_err(|_| protocol("interaction continuation exceeds its limit"))
}
