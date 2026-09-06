//! Ordered Messages blocks for stateless Fable replay.
//!
//! Text and local inputs leave as ordinary deltas; only references and private
//! attributes are retained. A `message_stop` after closed blocks and a successful
//! stop reason is the sole point at which continuation is offered to the runner.

use crate::sse::SseEvent;
use crucible_core::{
    CONTINUATION_BYTES, CONTINUATION_PARTS, Continuation, ContinuationData, ContinuationPart,
    ContinuationScope, Delta, ProviderError, StopReason, TOOL_ARGUMENT_BYTES, TOOL_CALL_ID_BYTES,
    TOOL_NAME_BYTES, ToolId,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::io;

pub(super) const PROTOCOL: &str = "anthropic-messages-v1";

pub(super) fn problem(message: &'static str) -> ProviderError {
    ProviderError::Protocol {
        provider: super::NAME,
        problem: message.into(),
    }
}

pub(super) struct Blocks {
    state: Option<Continuation>,
    block: Option<Block>,
    next: usize,
    text: usize,
    calls: usize,
    private: usize,
    arguments: usize,
    ids: BTreeSet<Box<str>>,
    opened: bool,
    stopped: bool,
    reason: Option<StopReason>,
}

enum Block {
    Text {
        fields: Map<String, Value>,
        start: usize,
    },
    Tool {
        fields: Map<String, Value>,
        input: String,
        streamed: bool,
    },
    Private(Map<String, Value>),
}

impl Blocks {
    pub(super) fn new(
        model: &str,
        scope: ContinuationScope,
        effort: Option<crucible_core::Effort>,
    ) -> Result<Self, ProviderError> {
        let mut state = Continuation::new(PROTOCOL, model, scope)
            .map_err(|_| problem("invalid continuation identity"))?;
        state.push(ContinuationPart::Opaque(ContinuationData::new(&serde_json::json!({"request_effort":effort.map(crucible_core::Effort::as_str)}).to_string()).map_err(|_| problem("invalid request effort"))?))
            .map_err(|_| problem("invalid request effort"))?;
        Ok(Self {
            state: Some(state),
            block: None,
            next: 0,
            text: 0,
            calls: 0,
            private: 0,
            arguments: 0,
            ids: BTreeSet::new(),
            opened: false,
            stopped: false,
            reason: None,
        })
    }

    pub(super) fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        if !matches!(
            event.name.as_str(),
            "message_start"
                | "content_block_start"
                | "content_block_delta"
                | "content_block_stop"
                | "message_delta"
                | "message_stop"
                | "error"
        ) {
            // Additive SSE events and proxy heartbeats do not necessarily carry
            // JSON. Known content events remain strict so no replay part is lost.
            return Ok(Vec::new());
        }
        let mut value: Value =
            serde_json::from_str(&event.data).map_err(|_| problem("invalid message event JSON"))?;
        if value
            .get("type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != event.name)
            || self.stopped
        {
            return Err(problem("conflicting or late message event"));
        }
        if event.name == "error" {
            return Err(super::diagnostics::upstream(&value));
        }
        let mut deltas = Vec::new();
        match event.name.as_str() {
            "message_start" => {
                if self.opened {
                    return Err(problem("duplicate message start"));
                }
                self.opened = true;
                deltas.extend(super::wire::opened(&value)?);
                if let Some(message) = value.get("message") {
                    deltas.extend(super::diagnostics::transformations(message)?);
                }
            }
            "content_block_start" => {
                self.index(&value)?;
                if self.block.is_some() || self.reason.is_some() {
                    return Err(problem("overlapping message blocks"));
                }
                let fields = value
                    .get_mut("content_block")
                    .map(Value::take)
                    .ok_or_else(|| problem("missing message block"))?;
                self.block = Some(self.start(fields, &mut deltas)?);
            }
            "content_block_delta" => {
                self.index(&value)?;
                let delta = value
                    .get_mut("delta")
                    .map(Value::take)
                    .ok_or_else(|| problem("missing block delta"))?;
                self.more(delta, &mut deltas)?;
            }
            "content_block_stop" => {
                self.index(&value)?;
                self.close(&mut deltas)?;
                self.next += 1;
            }
            "message_delta" => {
                if !self.opened || self.block.is_some() || self.reason.is_some() {
                    return Err(problem("invalid message completion"));
                }
                for delta in super::diagnostics::completion(&value)? {
                    if let Delta::Stopped(reason) = delta {
                        self.reason = Some(reason);
                    } else {
                        deltas.push(delta);
                    }
                }
                deltas.extend(super::diagnostics::transformations(&value)?);
            }
            "message_stop" => {
                if !self.opened || self.block.is_some() {
                    return Err(problem("message stopped with an unfinished block"));
                }
                let reason = self
                    .reason
                    .ok_or_else(|| problem("message stopped without a reason"))?;
                if (reason == StopReason::WantsTools && self.calls == 0)
                    || (reason == StopReason::Yielded && self.calls != 0)
                {
                    return Err(problem("message stop contradicts its calls"));
                }
                if self.next > 0 && matches!(reason, StopReason::Yielded | StopReason::WantsTools) {
                    deltas.push(Delta::Continuation(
                        self.state
                            .take()
                            .ok_or_else(|| problem("missing continuation state"))?,
                    ));
                }
                self.stopped = true;
                deltas.push(Delta::Stopped(reason));
            }
            _ => return Err(problem("unsupported message event")),
        }
        if deltas.is_empty() && event.name.starts_with("content_block") {
            deltas.push(Delta::Progress);
        }
        Ok(deltas)
    }

    fn index(&self, value: &Value) -> Result<(), ProviderError> {
        if !self.opened
            || self.next >= CONTINUATION_PARTS
            || value.get("index").and_then(Value::as_u64) != Some(self.next as u64)
        {
            return Err(problem("invalid message block index"));
        }
        Ok(())
    }

    fn start(&mut self, value: Value, deltas: &mut Vec<Delta>) -> Result<Block, ProviderError> {
        let Value::Object(mut fields) = value else {
            return Err(problem("invalid message block"));
        };
        match fields.get("type").and_then(Value::as_str) {
            Some("text") => {
                let text = take_text(&mut fields, "text")?;
                claim(&mut self.private, weight_map(&fields), CONTINUATION_BYTES)?;
                let start = self.text;
                self.text(&text, deltas)?;
                Ok(Block::Text { fields, start })
            }
            Some("tool_use") => {
                let id = fields
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| problem("missing tool id"))?;
                let name = fields
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| problem("missing tool name"))?;
                identity(id, TOOL_CALL_ID_BYTES)?;
                identity(name, TOOL_NAME_BYTES)?;
                if self.ids.len() == 128 || self.ids.contains(id) {
                    return Err(problem("duplicate or excessive tool calls"));
                }
                claim(&mut self.private, id.len() + 64, CONTINUATION_BYTES)?;
                self.ids.insert(id.into());
                let input = fields
                    .remove("input")
                    .filter(Value::is_object)
                    .ok_or_else(|| problem("tool input is not an object"))?;
                let mut size = Size::new(TOOL_ARGUMENT_BYTES.saturating_sub(self.arguments));
                serde_json::to_writer(&mut size, &input)
                    .map_err(|_| problem("message arguments exceed their limit"))?;
                claim(&mut self.arguments, size.bytes, TOOL_ARGUMENT_BYTES)?;
                let input = input.to_string();
                claim(&mut self.private, weight_map(&fields), CONTINUATION_BYTES)?;
                Ok(Block::Tool {
                    fields,
                    input,
                    streamed: false,
                })
            }
            Some("thinking" | "redacted_thinking") => {
                claim(&mut self.private, weight_map(&fields), CONTINUATION_BYTES)?;
                Ok(Block::Private(fields))
            }
            _ => Err(problem("unsupported message content block")),
        }
    }

    fn text(&mut self, text: &str, deltas: &mut Vec<Delta>) -> Result<(), ProviderError> {
        claim(&mut self.text, text.len(), 8 * 1024 * 1024)?;
        if !text.is_empty() {
            deltas.push(Delta::Text(text.into()));
        }
        Ok(())
    }

    fn more(&mut self, value: Value, deltas: &mut Vec<Delta>) -> Result<(), ProviderError> {
        let Value::Object(mut fields) = value else {
            return Err(problem("invalid block delta"));
        };
        let kind = take_text(&mut fields, "type")?;
        match (self.block.as_mut(), kind.as_str()) {
            (Some(Block::Text { .. }), "text_delta") => {
                let text = take_text(&mut fields, "text")?;
                self.text(&text, deltas)?;
            }
            (Some(Block::Text { fields: block, .. }), "citations_delta") => {
                let citation = fields
                    .remove("citation")
                    .filter(Value::is_object)
                    .ok_or_else(|| problem("invalid citation delta"))?;
                claim(
                    &mut self.private,
                    weight(&citation).saturating_add(32),
                    CONTINUATION_BYTES,
                )?;
                let citations = block
                    .entry("citations")
                    .or_insert_with(|| Value::Array(Vec::new()));
                let Value::Array(citations) = citations else {
                    return Err(problem("invalid text citations"));
                };
                citations.reserve_exact(1);
                citations.push(citation);
            }
            (
                Some(Block::Tool {
                    input, streamed, ..
                }),
                "input_json_delta",
            ) => {
                let fragment = take_text(&mut fields, "partial_json")?;
                if !*streamed {
                    if input != "{}" {
                        return Err(problem("tool delta follows complete input"));
                    }
                    self.arguments -= input.len();
                    input.clear();
                    *streamed = true;
                }
                claim(&mut self.arguments, fragment.len(), TOOL_ARGUMENT_BYTES)?;
                input.reserve_exact(fragment.len());
                input.push_str(&fragment);
            }
            (Some(Block::Private(block)), "thinking_delta" | "signature_delta")
                if block.get("type").and_then(Value::as_str) == Some("thinking") =>
            {
                if kind == "thinking_delta"
                    && block
                        .get("signature")
                        .and_then(Value::as_str)
                        .is_some_and(|signature| !signature.is_empty())
                {
                    return Err(problem("thinking delta follows its signature"));
                }
                let key = if kind == "thinking_delta" {
                    "thinking"
                } else {
                    "signature"
                };
                let fragment = take_text(&mut fields, key)?;
                // Charge the field allocation once, not once per fragment: a
                // proxy's packet boundaries do not change retained payload size.
                let field_bytes = if block.contains_key(key) {
                    0
                } else {
                    key.len() + 96
                };
                claim(
                    &mut self.private,
                    fragment.len().saturating_add(field_bytes),
                    CONTINUATION_BYTES,
                )?;
                let text = block
                    .entry(key)
                    .or_insert_with(|| Value::String(String::new()));
                let Value::String(text) = text else {
                    return Err(problem("invalid thinking field"));
                };
                text.reserve_exact(fragment.len());
                text.push_str(&fragment);
            }
            _ => return Err(problem("delta does not match its content block")),
        }
        if !fields.is_empty() {
            return Err(problem("unsupported block delta fields"));
        }
        Ok(())
    }

    fn close(&mut self, deltas: &mut Vec<Delta>) -> Result<(), ProviderError> {
        let block = self
            .block
            .take()
            .ok_or_else(|| problem("block stopped without a start"))?;
        let part = match block {
            Block::Text { fields, start } => ContinuationPart::Text {
                start,
                end: self.text,
                data: data(&fields)?,
            },
            Block::Tool {
                mut fields, input, ..
            } => {
                if !serde_json::from_str::<Value>(&input).is_ok_and(|input| input.is_object()) {
                    return Err(problem("incomplete tool input"));
                }
                let id = take_text(&mut fields, "id")?;
                let name = take_text(&mut fields, "name")?;
                let part = ContinuationPart::Call {
                    index: self.calls,
                    data: data(&fields)?,
                };
                self.calls += 1;
                deltas.push(Delta::ToolStarted {
                    id: ToolId::new(id),
                    name: name.into(),
                });
                deltas.push(Delta::ToolArgs(input.into()));
                part
            }
            Block::Private(fields) => {
                private(&fields)?;
                ContinuationPart::Opaque(data(&fields)?)
            }
        };
        self.state
            .as_mut()
            .ok_or_else(|| problem("missing continuation state"))?
            .push(part)
            .map_err(|_| problem("message continuation exceeds its limit"))
    }
}

pub(super) fn private(fields: &Map<String, Value>) -> Result<(), ProviderError> {
    let valid = match fields.get("type").and_then(Value::as_str) {
        Some("thinking") => {
            fields.get("thinking").is_some_and(Value::is_string)
                && fields
                    .get("signature")
                    .and_then(Value::as_str)
                    .is_some_and(|s| !s.is_empty())
        }
        Some("redacted_thinking") => fields
            .get("data")
            .and_then(Value::as_str)
            .is_some_and(|s| !s.is_empty()),
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(problem("invalid private thinking block"))
    }
}

pub(super) fn identity(value: &str, limit: usize) -> Result<(), ProviderError> {
    if value.is_empty() || value.len() > limit || value.chars().any(char::is_control) {
        Err(problem("invalid tool identity"))
    } else {
        Ok(())
    }
}

fn take_text(fields: &mut Map<String, Value>, key: &str) -> Result<String, ProviderError> {
    match fields.remove(key) {
        Some(Value::String(text)) => Ok(text),
        _ => Err(problem("missing or invalid string field")),
    }
}

fn data(fields: &Map<String, Value>) -> Result<ContinuationData, ProviderError> {
    let mut size = Size::new(CONTINUATION_BYTES);
    serde_json::to_writer(&mut size, fields)
        .map_err(|_| problem("message continuation exceeds its limit"))?;
    ContinuationData::new(
        &serde_json::to_string(fields).map_err(|_| problem("invalid block attributes"))?,
    )
    .map_err(|_| problem("message continuation exceeds its limit"))
}

/// Count the escaped JSON representation before allocating its string. The
/// decoded wire budget alone cannot bound expansion of control characters.
struct Size {
    bytes: usize,
    limit: usize,
}

impl Size {
    const fn new(limit: usize) -> Self {
        Self { bytes: 0, limit }
    }
}

impl io::Write for Size {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes) {
            return Err(io::Error::other("message JSON exceeds its limit"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn claim(used: &mut usize, amount: usize, cap: usize) -> Result<(), ProviderError> {
    let next = used.saturating_add(amount);
    if next > cap {
        return Err(problem("message response exceeds its resource limit"));
    }
    *used = next;
    Ok(())
}

fn weight_map(fields: &Map<String, Value>) -> usize {
    fields.iter().fold(64usize, |n, (key, value)| {
        n.saturating_add(key.len())
            .saturating_add(64)
            .saturating_add(weight(value))
    })
}
fn weight(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len().saturating_add(32),
        Value::Array(items) => items
            .iter()
            .fold(32usize, |n, item| n.saturating_add(weight(item))),
        Value::Object(fields) => weight_map(fields),
        _ => 32,
    }
}
