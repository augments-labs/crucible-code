//! Ordered, bounded Responses items for stateless replay.
//!
//! One response's assembly is temporary. Text leaves as deltas; completed items
//! are checked against their fragments and the terminal output, then lowered to
//! references into the runner's text/calls. No second transcript is retained.

use crate::sse::SseEvent;
use crucible_core::{
    CONTINUATION_BYTES, CONTINUATION_PARTS, Continuation, ContinuationData, ContinuationPart,
    ContinuationScope, Delta, ProviderError, Request, StopReason, TOOL_ARGUMENT_BYTES,
    TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, ToolId,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io;

pub(super) const PROTOCOL: &str = "openai-responses-v1";
const TEXT_BYTES: usize = 8 * 1024 * 1024;

pub(super) fn problem(message: &'static str) -> ProviderError {
    ProviderError::Protocol {
        provider: super::NAME,
        problem: message.into(),
    }
}

/// Only status and typed window/cancel failures may escape private replay.
pub(super) fn refusal(error: ProviderError) -> ProviderError {
    match error {
        ProviderError::Refused { status, .. } => ProviderError::Refused {
            provider: super::NAME,
            status,
            message: match status {
                401 | 403 => "check the OpenAI credential and model access",
                404 => "check the OpenAI model name and endpoint",
                408 | 429 | 500..=599 => "OpenAI is temporarily unable to serve this request",
                _ => {
                    "check the OpenAI model and request settings; private response details omitted"
                }
            }
            .into(),
        },
        error => error,
    }
}

pub(super) struct Output {
    items: BTreeMap<usize, Item>,
    frontier: usize,
    budget: Budget,
    ids: BTreeSet<String>,
    calls: BTreeSet<String>,
    state: Option<Continuation>,
    opened: Option<String>,
    stopped: bool,
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Output([redacted])")
    }
}

struct Item {
    id: String,
    kind: String,
    text: BTreeMap<usize, Piece>,
    arguments: String,
    arguments_complete: bool,
    call: Option<Call>,
    done: Option<Value>,
    delivered: bool,
}

struct Call {
    id: String,
    name: String,
}

#[derive(Default)]
struct Piece {
    text: String,
    sent: usize,
    complete: bool,
}

#[derive(Default)]
struct Budget {
    text: usize,
    arguments: usize,
    private: usize,
}

impl Output {
    pub(super) fn new(
        request: &Request<'_>,
        scope: ContinuationScope,
    ) -> Result<Self, ProviderError> {
        let mut state = Continuation::new(PROTOCOL, request.model, scope)
            .map_err(|_| problem("invalid continuation identity"))?;
        push(
            &mut state,
            ContinuationPart::Opaque(data(
                &json!({"request_effort":request.effort.map(crucible_core::Effort::as_str)}),
            )?),
        )?;
        Ok(Self {
            items: BTreeMap::new(),
            frontier: 0,
            budget: Budget::default(),
            ids: BTreeSet::new(),
            calls: BTreeSet::new(),
            state: Some(state),
            opened: None,
            stopped: false,
        })
    }

    pub(super) fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        if event.data.trim().is_empty() {
            return Ok(vec![]);
        }
        let mut value: Value = serde_json::from_str(&event.data)
            .map_err(|_| problem("invalid response event JSON"))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if self.stopped {
            return Err(problem("response event after completion"));
        }
        if !event.name.is_empty() && event.name != "message" && event.name != kind {
            return Err(problem("conflicting response event types"));
        }
        let mut deltas = Vec::new();
        match kind.as_str() {
            "response.created" => {
                let id = value
                    .pointer("/response/id")
                    .and_then(Value::as_str)
                    .ok_or_else(|| problem("missing response identity"))?;
                identity(id, TOOL_CALL_ID_BYTES)?;
                if self.opened.replace(id.into()).is_some() {
                    return Err(problem("duplicate response start"));
                }
            }
            "response.output_item.added" => self.start(&mut value)?,
            "response.output_text.delta" | "response.refusal.delta" => self.text(&value)?,
            "response.output_text.done" | "response.refusal.done" => self.text_done(&value)?,
            "response.function_call_arguments.delta" => {
                let item = self.item(&value)?;
                if item.kind != "function_call" || item.arguments_complete {
                    return Err(problem("arguments on a non-function item"));
                }
                let fragment = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| problem("missing function argument delta"))?;
                claim(
                    &mut self.budget.arguments,
                    fragment.len(),
                    TOOL_ARGUMENT_BYTES,
                )?;
                self.item(&value)?.arguments.push_str(fragment);
            }
            "response.function_call_arguments.done" => self.arguments_done(&value)?,
            "response.output_item.done" => self.finish(&mut value)?,
            "response.completed" => return self.completed(&value),
            "response.incomplete" => {
                self.stopped = true;
                deltas.extend(usage(&value)?);
                deltas.push(Delta::Stopped(super::wire::cut(&value)));
            }
            "response.failed" | "error" => {
                return Err(upstream(&value));
            }
            // These additive events narrate attributes repeated in the complete
            // item. Private summaries never become user-visible text.
            _ => {}
        }
        self.flush(&mut deltas)?;
        if deltas.is_empty() && !self.items.is_empty() {
            deltas.push(Delta::Progress);
        }
        Ok(deltas)
    }

    fn start(&mut self, value: &mut Value) -> Result<(), ProviderError> {
        if self.opened.is_none() {
            return Err(problem("item before response start"));
        }
        let index = index(value, "output_index")?;
        if self.items.contains_key(&index) {
            return Err(problem("duplicate response item"));
        }
        let item = value
            .get("item")
            .ok_or_else(|| problem("missing response item"))?;
        let id = field(item, "id")?;
        let kind = field(item, "type")?;
        identity(id, TOOL_CALL_ID_BYTES)?;
        if !matches!(
            kind,
            "message" | "function_call" | "reasoning" | "web_search_call" | "compaction"
        ) {
            return Err(problem("unsupported response item type"));
        }
        let call = if kind == "function_call" {
            let id = field(item, "call_id")?;
            let name = field(item, "name")?;
            identity(id, TOOL_CALL_ID_BYTES)?;
            identity(name, TOOL_NAME_BYTES)?;
            claim(
                &mut self.budget.private,
                id.len().saturating_add(name.len()).saturating_add(128),
                CONTINUATION_BYTES,
            )?;
            Some(Call {
                id: id.into(),
                name: name.into(),
            })
        } else {
            None
        };
        claim(
            &mut self.budget.private,
            id.len().saturating_add(kind.len()).saturating_add(256),
            CONTINUATION_BYTES,
        )?;
        if !self.ids.insert(id.into()) {
            return Err(problem("duplicate response item identity"));
        }
        self.items.insert(
            index,
            Item {
                id: id.into(),
                kind: kind.into(),
                text: BTreeMap::new(),
                arguments: String::new(),
                arguments_complete: false,
                call,
                done: None,
                delivered: false,
            },
        );
        Ok(())
    }

    fn item(&mut self, value: &Value) -> Result<&mut Item, ProviderError> {
        let index = index(value, "output_index")?;
        let item = self
            .items
            .get_mut(&index)
            .ok_or_else(|| problem("delta without response item"))?;
        if item.done.is_some()
            || value.get("item_id").and_then(Value::as_str) != Some(item.id.as_str())
        {
            return Err(problem("delta for a closed or different response item"));
        }
        Ok(item)
    }

    fn text(&mut self, value: &Value) -> Result<(), ProviderError> {
        let content = index(value, "content_index")?;
        let fragment = field(value, "delta")?;
        if self.item(value)?.kind != "message" {
            return Err(problem("text on a non-message item"));
        }
        if self
            .item(value)?
            .text
            .get(&content)
            .is_some_and(|piece| piece.complete)
        {
            return Err(problem("text delta after completed text"));
        }
        claim(&mut self.budget.text, fragment.len(), TEXT_BYTES)?;
        if !self.item(value)?.text.contains_key(&content) {
            claim(&mut self.budget.private, 128, CONTINUATION_BYTES)?;
        }
        let piece = self.item(value)?.text.entry(content).or_default();
        piece.text.push_str(fragment);
        Ok(())
    }

    fn text_done(&mut self, value: &Value) -> Result<(), ProviderError> {
        let content = index(value, "content_index")?;
        let key = if field(value, "type")? == "response.refusal.done" {
            "refusal"
        } else {
            "text"
        };
        let complete = field(value, key)?;
        let item = self.item(value)?;
        if item.kind != "message" {
            return Err(problem("text on a non-message item"));
        }
        if let Some(piece) = item.text.get(&content) {
            if piece.complete || !complete.starts_with(&piece.text) {
                return Err(problem("contradictory completed text"));
            }
        } else {
            claim(&mut self.budget.private, 128, CONTINUATION_BYTES)?;
        }
        let length = self
            .item(value)?
            .text
            .get(&content)
            .map_or(0, |piece| piece.text.len());
        claim(&mut self.budget.text, complete.len() - length, TEXT_BYTES)?;
        let piece = self.item(value)?.text.entry(content).or_default();
        piece.text.push_str(
            complete
                .get(length..)
                .ok_or_else(|| problem("invalid text boundary"))?,
        );
        piece.complete = true;
        Ok(())
    }

    fn finish(&mut self, value: &mut Value) -> Result<(), ProviderError> {
        let index = index(value, "output_index")?;
        let finished = value
            .get_mut("item")
            .map(Value::take)
            .ok_or_else(|| problem("missing completed item"))?;
        let item = self
            .items
            .get_mut(&index)
            .ok_or_else(|| problem("completion without response item"))?;
        if item.done.is_some()
            || field(&finished, "id")? != item.id
            || field(&finished, "type")? != item.kind
        {
            return Err(problem("conflicting response item completion"));
        }
        validate(&finished)?;
        match item.kind.as_str() {
            "message" => {
                let parts = finished
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| problem("missing message content"))?;
                if item.text.keys().any(|index| *index >= parts.len()) {
                    return Err(problem("missing completed text part"));
                }
                for (index, part) in parts.iter().enumerate() {
                    let text = visible(part)?;
                    let streamed = item
                        .text
                        .get(&index)
                        .map_or("", |piece| piece.text.as_str());
                    if !text.starts_with(streamed)
                        || item
                            .text
                            .get(&index)
                            .is_some_and(|piece| piece.complete && text != streamed)
                    {
                        return Err(problem("completed text contradicts streamed text"));
                    }
                    claim(
                        &mut self.budget.text,
                        text.len() - streamed.len(),
                        TEXT_BYTES,
                    )?;
                }
                for piece in item.text.values_mut() {
                    piece.text = String::new();
                }
            }
            "function_call" => {
                let arguments = field(&finished, "arguments")?;
                let call = item
                    .call
                    .as_ref()
                    .ok_or_else(|| problem("missing function identity"))?;
                if field(&finished, "call_id")? != call.id || field(&finished, "name")? != call.name
                {
                    return Err(problem("function identity changed at completion"));
                }
                if !arguments.starts_with(&item.arguments)
                    || item.arguments_complete && arguments != item.arguments
                {
                    return Err(problem("completed arguments contradict streamed arguments"));
                }
                claim(
                    &mut self.budget.arguments,
                    arguments.len() - item.arguments.len(),
                    TOOL_ARGUMENT_BYTES,
                )?;
                if self.calls.len() == 128
                    || !self.calls.insert(field(&finished, "call_id")?.into())
                {
                    return Err(problem("too many or duplicate function calls"));
                }
                item.arguments.clear();
                item.arguments.shrink_to_fit();
            }
            _ => {}
        }
        claim(
            &mut self.budget.private,
            private_weight(&finished),
            CONTINUATION_BYTES,
        )?;
        item.done = Some(finished);
        Ok(())
    }

    fn arguments_done(&mut self, value: &Value) -> Result<(), ProviderError> {
        let complete = field(value, "arguments")?;
        let item = self.item(value)?;
        if item.kind != "function_call"
            || item.arguments_complete
            || !complete.starts_with(&item.arguments)
        {
            return Err(problem("contradictory completed arguments"));
        }
        let length = item.arguments.len();
        claim(
            &mut self.budget.arguments,
            complete.len() - length,
            TOOL_ARGUMENT_BYTES,
        )?;
        let item = self.item(value)?;
        item.arguments.push_str(
            complete
                .get(length..)
                .ok_or_else(|| problem("invalid argument boundary"))?,
        );
        item.arguments_complete = true;
        Ok(())
    }

    fn flush(&mut self, deltas: &mut Vec<Delta>) -> Result<(), ProviderError> {
        while let Some(item) = self.items.get_mut(&self.frontier) {
            let Some(done) = &item.done else {
                // The frontier may have moved since these fragments arrived.
                // Drain every unsent byte, not only the next network fragment.
                if let Some(piece) = item.text.get_mut(&0)
                    && piece.sent < piece.text.len()
                {
                    deltas.push(Delta::Text(
                        piece
                            .text
                            .get(piece.sent..)
                            .ok_or_else(|| problem("invalid text boundary"))?
                            .into(),
                    ));
                    piece.sent = piece.text.len();
                }
                break;
            };
            match item.kind.as_str() {
                "message" => {
                    let parts = done
                        .get("content")
                        .and_then(Value::as_array)
                        .ok_or_else(|| problem("missing message content"))?;
                    for (index, part) in parts.iter().enumerate() {
                        let text = visible(part)?;
                        let sent = item.text.get(&index).map_or(0, |piece| piece.sent);
                        let rest = text
                            .get(sent..)
                            .ok_or_else(|| problem("invalid streamed text boundary"))?;
                        if !rest.is_empty() {
                            deltas.push(Delta::Text(rest.into()));
                        }
                    }
                    item.text.clear();
                }
                "function_call" => {
                    deltas.push(Delta::ToolStarted {
                        id: ToolId::new(field(done, "call_id")?),
                        name: field(done, "name")?.into(),
                    });
                    deltas.push(Delta::ToolArgs(field(done, "arguments")?.into()));
                }
                _ => {}
            }
            item.delivered = true;
            self.frontier += 1;
        }
        Ok(())
    }

    fn completed(&mut self, value: &Value) -> Result<Vec<Delta>, ProviderError> {
        let response = value
            .get("response")
            .ok_or_else(|| problem("missing completed response"))?;
        if self.opened.as_deref() != Some(field(response, "id")?)
            || field(response, "status")? != "completed"
            || self.frontier != self.items.len()
            || self.items.values().any(|item| !item.delivered)
        {
            return Err(problem(
                "response completed with unfinished or conflicting items",
            ));
        }
        let output = response
            .get("output")
            .and_then(Value::as_array)
            .ok_or_else(|| problem("missing response output"))?;
        // The subscription service has already narrated its items and repeats
        // an empty output list. A nonempty repeat must agree exactly.
        if !output.is_empty()
            && (output.len() != self.items.len()
                || output
                    .iter()
                    .zip(self.items.values())
                    .any(|(value, item)| item.done.as_ref() != Some(value)))
        {
            return Err(problem("terminal output contradicts completed items"));
        }
        let mut deltas = Vec::new();
        deltas.extend(usage(value)?);
        let stop = if self.calls.is_empty() {
            StopReason::Yielded
        } else {
            StopReason::WantsTools
        };
        if !self.items.is_empty() {
            let mut state = self
                .state
                .take()
                .ok_or_else(|| problem("missing continuation state"))?;
            let mut text = 0;
            let mut calls = 0;
            for (_, item) in std::mem::take(&mut self.items) {
                capture(
                    &mut state,
                    item.done
                        .ok_or_else(|| problem("unfinished response item"))?,
                    &mut text,
                    &mut calls,
                )?;
            }
            deltas.push(Delta::Continuation(state));
        }
        self.stopped = true;
        deltas.push(Delta::Stopped(stop));
        Ok(deltas)
    }
}

fn capture(
    state: &mut Continuation,
    value: Value,
    text: &mut usize,
    calls: &mut usize,
) -> Result<(), ProviderError> {
    let Value::Object(mut fields) = value else {
        return Err(problem("invalid response item"));
    };
    match fields.get("type").and_then(Value::as_str) {
        Some("message") => {
            let Some(Value::Array(content)) = fields.remove("content") else {
                return Err(problem("missing message content"));
            };
            push(
                state,
                ContinuationPart::Opaque(data(&json!({"message":fields}))?),
            )?;
            for part in content {
                let Value::Object(mut fields) = part else {
                    return Err(problem("invalid text part"));
                };
                let key = if fields.get("type").and_then(Value::as_str) == Some("refusal") {
                    "refusal"
                } else {
                    "text"
                };
                let Some(Value::String(value)) = fields.remove(key) else {
                    return Err(problem("missing message text"));
                };
                let start = *text;
                *text += value.len();
                push(
                    state,
                    ContinuationPart::Text {
                        start,
                        end: *text,
                        data: data(&Value::Object(fields))?,
                    },
                )?;
            }
        }
        Some("function_call") => {
            for key in ["call_id", "name", "arguments"] {
                fields.remove(key);
            }
            push(
                state,
                ContinuationPart::Call {
                    index: *calls,
                    data: data(&Value::Object(fields))?,
                },
            )?;
            *calls += 1;
        }
        _ => push(
            state,
            ContinuationPart::Opaque(data(&Value::Object(fields))?),
        )?,
    }
    Ok(())
}

pub(super) fn header(value: &Value) -> Result<(), ProviderError> {
    identity(field(value, "id")?, TOOL_CALL_ID_BYTES)?;
    if value
        .get("status")
        .is_some_and(|status| status.as_str() != Some("completed"))
    {
        return Err(problem("unfinished response item"));
    }
    if field(value, "type")? == "message" {
        if field(value, "role")? != "assistant" {
            return Err(problem("non-assistant output message"));
        }
        if value.get("phase").is_some_and(|phase| {
            !phase.is_null() && !matches!(phase.as_str(), Some("commentary" | "final_answer"))
        }) {
            return Err(problem("invalid assistant phase"));
        }
    }
    Ok(())
}

pub(super) fn validate(value: &Value) -> Result<(), ProviderError> {
    header(value)?;
    match field(value, "type")? {
        "message" => {
            let content = value
                .get("content")
                .and_then(Value::as_array)
                .filter(|parts| parts.len() < CONTINUATION_PARTS)
                .ok_or_else(|| problem("invalid message content"))?;
            for part in content {
                visible(part)?;
            }
        }
        "function_call" => {
            identity(field(value, "call_id")?, TOOL_CALL_ID_BYTES)?;
            identity(field(value, "name")?, TOOL_NAME_BYTES)?;
            let args = field(value, "arguments")?;
            if args.len() > TOOL_ARGUMENT_BYTES {
                return Err(problem("function arguments exceed their limit"));
            }
            if !serde_json::from_str::<Value>(args).is_ok_and(|value| value.is_object()) {
                return Err(problem("invalid function arguments"));
            }
        }
        "reasoning" | "compaction" => {
            if field(value, "encrypted_content")?.is_empty() {
                return Err(problem("missing encrypted continuation"));
            }
        }
        "web_search_call" => {}
        _ => return Err(problem("unsupported response item type")),
    }
    Ok(())
}

fn usage(value: &Value) -> Result<Option<Delta>, ProviderError> {
    if let Some(usage) = value
        .pointer("/response/usage")
        .filter(|value| !value.is_null())
    {
        counts(usage, &["input_tokens", "output_tokens", "total_tokens"])?;
        for (field, names) in [
            (
                "input_tokens_details",
                &["cached_tokens", "cache_write_tokens"][..],
            ),
            ("output_tokens_details", &["reasoning_tokens"][..]),
        ] {
            if let Some(details) = usage.get(field).filter(|value| !value.is_null()) {
                counts(details, names)?;
            }
        }
    }
    super::wire::usage(value, true)
}

fn counts(value: &Value, names: &[&str]) -> Result<(), ProviderError> {
    let fields = value
        .as_object()
        .ok_or_else(|| problem("invalid usage object"))?;
    for name in names {
        if fields
            .get(*name)
            .is_some_and(|value| !value.is_null() && value.as_u64().is_none())
        {
            return Err(problem("invalid usage count"));
        }
    }
    Ok(())
}

fn upstream(value: &Value) -> ProviderError {
    let code = value
        .pointer("/response/error/code")
        .or_else(|| value.get("code"))
        .and_then(Value::as_str);
    let kind = match code {
        Some("server_error") => "server_error",
        Some("rate_limit_exceeded") => "rate_limit_exceeded",
        Some("context_length_exceeded") => {
            return ProviderError::WindowExceeded {
                provider: super::NAME,
            };
        }
        _ => return problem("OpenAI reported an unfinished response; private details omitted"),
    };
    ProviderError::Upstream {
        provider: super::NAME,
        kind: kind.into(),
        message: "OpenAI could not finish this request; private details omitted".into(),
    }
}

fn visible(value: &Value) -> Result<&str, ProviderError> {
    match field(value, "type")? {
        "output_text" => field(value, "text"),
        "refusal" => field(value, "refusal"),
        _ => Err(problem("unsupported message content")),
    }
}

pub(super) fn field<'a>(value: &'a Value, key: &str) -> Result<&'a str, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| problem("missing or invalid response field"))
}

pub(super) fn identity(value: &str, cap: usize) -> Result<(), ProviderError> {
    if value.is_empty() || value.len() > cap || value.chars().any(char::is_control) {
        return Err(problem("invalid response identity"));
    }
    Ok(())
}

fn index(value: &Value, key: &str) -> Result<usize, ProviderError> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n < CONTINUATION_PARTS)
        .ok_or_else(|| problem("invalid response index"))
}

fn claim(used: &mut usize, bytes: usize, cap: usize) -> Result<(), ProviderError> {
    let next = used.saturating_add(bytes);
    if next > cap {
        return Err(problem("response exceeds its resource limit"));
    }
    *used = next;
    Ok(())
}

fn private_weight(value: &Value) -> usize {
    match value {
        Value::String(text) => text.len().saturating_add(32),
        Value::Array(items) => items
            .iter()
            .fold(32, |n, item| n.saturating_add(private_weight(item))),
        Value::Object(fields) => fields.iter().fold(64, |n, (key, value)| {
            let visible = matches!(
                fields.get("type").and_then(Value::as_str),
                Some("output_text")
            ) && key == "text"
                || fields.get("type").and_then(Value::as_str) == Some("refusal")
                    && key == "refusal"
                || fields.get("type").and_then(Value::as_str) == Some("function_call")
                    && key == "arguments";
            n.saturating_add(key.len())
                .saturating_add(64)
                .saturating_add(if visible { 32 } else { private_weight(value) })
        }),
        _ => 32,
    }
}

fn data(value: &Value) -> Result<ContinuationData, ProviderError> {
    // The bounded writer rejects escaping expansion before allocating it.
    struct Bounded(Vec<u8>);
    impl io::Write for Bounded {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if bytes.len() > CONTINUATION_BYTES.saturating_sub(self.0.len()) {
                return Err(io::Error::other("continuation exceeds its limit"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let mut buffer = Bounded(Vec::new());
    serde_json::to_writer(&mut buffer, value)
        .map_err(|_| problem("continuation exceeds its limit"))?;
    let text =
        std::str::from_utf8(&buffer.0).map_err(|_| problem("invalid continuation encoding"))?;
    ContinuationData::new(text).map_err(|_| problem("continuation exceeds its limit"))
}

fn push(state: &mut Continuation, part: ContinuationPart) -> Result<(), ProviderError> {
    state
        .push(part)
        .map_err(|_| problem("continuation exceeds its limit"))
}
