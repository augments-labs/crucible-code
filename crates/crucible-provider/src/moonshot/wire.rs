//! A response, from `MoonshotAI`'s shape.
//!
//! One event in, however many deltas out. Unlike the newer protocol this
//! endpoint narrates nothing: every event is a piece of the answer, and one of
//! them can carry text, a tool call and the reason the model stopped at once.
//!
//! Read by field lookup rather than into mirror structs. The payload is
//! consumed once, here, and a struct per shape would be more code to say the
//! same thing while still needing a fallback for what it does not know.
//!
//! Events are unnamed — the SSE `event:` line is never sent — so what an event
//! is is decided by what its payload holds rather than by a word beside it.

use crucible_core::{Delta, ProviderError, StopReason};
use serde_json::Value;

use crate::moonshot::NAME;
use crate::sse::SseEvent;
use crate::stream::Wire;

/// What closes a stream on this endpoint.
///
/// Not JSON, and the last thing every response sends. Parsed as a payload it
/// fails every turn this provider ever runs, at the moment a complete answer
/// has just finished arriving.
const DONE: &str = "[DONE]";

/// Chat Completions, being narrated.
#[derive(Debug, Default)]
pub(super) struct Completions;

impl Wire for Completions {
    const PROVIDER: &'static str = NAME;

    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        deltas(event)
    }
}

/// What an event means, or nothing if it means nothing to us.
///
/// # Errors
///
/// [`ProviderError::Upstream`] when the event is the provider reporting a
/// failure inside a response it had already started, and
/// [`ProviderError::Protocol`] when an event does not parse.
fn deltas(event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
    // A heartbeat, which a proxy may send with no data line at all, and the
    // sentinel above. Neither is JSON and neither means anything here.
    let data = event.data.trim();
    if data.is_empty() || data == DONE {
        return Ok(Vec::new());
    }

    let payload = parse(data)?;

    // A failure inside a response already started. It arrives in place of the
    // choices rather than beside them.
    if let Some(error) = payload.get("error").filter(|error| !error.is_null()) {
        return Err(upstream(error));
    }

    let choice = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first());
    let Some(choice) = choice else {
        // Usage, and whatever else this endpoint sends without a choice in it.
        return Ok(Vec::new());
    };

    let mut deltas = Vec::new();

    if let Some(delta) = choice.get("delta") {
        // `reasoning_content` sits beside this on a model that thinks first.
        // Nothing displays it, and reading it as the answer would put the
        // model's working in front of the user as though it were one.
        if let Some(said) = text(delta, "content").filter(|said| !said.is_empty()) {
            deltas.push(Delta::Text(said.into()));
        }
    }

    if let Some(reason) = text(choice, "finish_reason") {
        deltas.push(Delta::Stopped(stop(reason)));
    }

    Ok(deltas)
}

/// Why the model stopped.
///
/// A word this build has not heard of reads as unfinished rather than as a
/// finish. Wrong that way it is wrong about a turn that was fine; wrong the
/// other way it is an answer that was cut short arriving looking complete,
/// which is the one failure the user cannot see for themselves.
fn stop(reason: &str) -> StopReason {
    match reason {
        "stop" => StopReason::Yielded,
        "tool_calls" | "function_call" => StopReason::WantsTools,
        "length" | "max_tokens" => StopReason::OutOfTokens,
        "content_filter" => StopReason::Filtered,
        _ => StopReason::Unknown,
    }
}

/// A failure the provider reported mid-response.
fn upstream(error: &Value) -> ProviderError {
    ProviderError::Upstream {
        provider: NAME,
        kind: text(error, "type")
            .or_else(|| text(error, "code"))
            .unwrap_or("error")
            .into(),
        message: text(error, "message")
            .unwrap_or("the provider reported a failure and did not say what")
            .into(),
    }
}

/// One string field.
fn text<'a>(value: &'a Value, field: &str) -> Option<&'a str> {
    value.get(field).and_then(Value::as_str)
}

/// The payload as JSON.
fn parse(data: &str) -> Result<Value, ProviderError> {
    serde_json::from_str(data).map_err(|problem| ProviderError::Protocol {
        provider: NAME,
        // The payload itself is not carried: it is up to a whole event long and
        // this message ends up in front of a user.
        problem: format!("an event was not JSON: {problem}").into(),
    })
}
