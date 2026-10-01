//! What a dialect's own tests share: a provider over recorded bytes, a
//! request to send it, and what it sent and read.

use std::sync::Arc;

use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{Delta, Effort, Provider, ProviderError, Request, RequestPurpose};
use crucible_runtime::Cancel;
use crucible_types::{Message, ToolSchema, Transcript};
use serde_json::Value;

use super::{Chat, Dialect};
use crate::endpoint::Endpoint;
use crate::transport::Replay;

/// The one tool a dialect's conversations offer.
pub(crate) const TOOLS: &[ToolSchema<'static>] = &[ToolSchema {
    name: "lookup",
    schema: r#"{"type":"object","description":"Looks a word up."}"#,
}];

/// `D` at `endpoint`, signed with a fabricated key and answering every
/// request with `status` and `body`.
pub(crate) fn at<D: Dialect>(
    endpoint: Endpoint,
    status: u16,
    body: &str,
) -> (Chat<D>, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    (
        Chat::at(
            endpoint,
            Box::new(HeaderKey::new(
                ApiKey::new("fabricated-dialect-key"),
                Header::bearer(),
            )),
            Box::new(Arc::clone(&replay)),
        ),
        replay,
    )
}

/// A question and nothing else.
pub(crate) fn question() -> Transcript {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("Define cat."))
        .expect("a valid transcript");
    transcript
}

/// `transcript`, asked of `model` at `effort`, with the tool where `tools`.
pub(crate) fn asking(
    model: &'static str,
    transcript: Transcript,
    tools: bool,
    effort: Option<Effort>,
) -> Request<'static> {
    Request {
        purpose: RequestPurpose::Turn,
        model,
        transcript: Box::leak(Box::new(transcript)),
        tools: if tools { TOOLS } else { &[] },
        attached: &[],
        max_tokens: 512,
        system: None,
        effort,
        prompt_cache: None,
    }
}

/// Every delta of one response to `request`, or the first thing that failed.
pub(crate) fn read<D: Dialect>(
    provider: &Chat<D>,
    request: Request<'static>,
) -> Result<Vec<Delta>, ProviderError> {
    let cancel = Cancel::new();
    let mut stream = crucible_runtime::answered!(provider.stream(request, &cancel))?;
    let mut deltas = Vec::new();
    while let Some(delta) = crucible_runtime::answered!(stream.next()) {
        deltas.push(delta?);
    }
    Ok(deltas)
}

/// What sits at `pointer` in `value`, or null where nothing does.
pub(crate) fn field(value: &Value, pointer: &str) -> Value {
    value.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// The body `replay` last received.
pub(crate) fn sent(replay: &Replay) -> Value {
    serde_json::from_str(&replay.sent().body).expect("the body is JSON")
}

/// The header `name` `replay` last received.
pub(crate) fn header(replay: &Replay, name: &str) -> Option<String> {
    replay
        .sent()
        .headers
        .iter()
        .find(|(header, _)| header.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

/// Server-sent events framing `chunks`, each one an event's payload, closed
/// as this wire closes a stream.
pub(crate) fn framed(chunks: &[Value]) -> String {
    let mut body = String::new();
    for chunk in chunks {
        body.push_str("data: ");
        body.push_str(&chunk.to_string());
        body.push_str("\n\n");
    }
    body.push_str("data: [DONE]\n\n");
    body
}
