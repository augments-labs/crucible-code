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

use crucible_core::{Delta, ProviderError, StopReason, ToolId};
use serde_json::Value;

use crate::moonshot::NAME;
use crate::refusal::SILENT;
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
pub(super) struct Completions {
    open: Open,
}

impl Wire for Completions {
    const PROVIDER: &'static str = NAME;

    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        deltas(event, &mut self.open)
    }
}

/// The tool call the response has open.
///
/// Carried between events because a call is named in the event that opens it
/// and its arguments arrive in the ones after, which carry the index alone. The
/// index is the whole of the identity a fragment has, so this is what a
/// fragment is checked against.
#[derive(Debug, Default)]
struct Open {
    /// The index a fragment belongs to, or `None` where no call is open.
    call: Option<u64>,
}

/// What an event means, or nothing if it means nothing to us.
///
/// # Errors
///
/// [`ProviderError::Upstream`] when the event is the provider reporting a
/// failure inside a response it had already started, and
/// [`ProviderError::Protocol`] when an event does not parse, announces a tool
/// call by part of its identity, or contradicts what is open.
fn deltas(event: &SseEvent, open: &mut Open) -> Result<Vec<Delta>, ProviderError> {
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

        calls(delta, open, &mut deltas)?;
    }

    if let Some(reason) = text(choice, "finish_reason") {
        deltas.push(Delta::Stopped(stop(reason)));
    }

    Ok(deltas)
}

/// The tool calls one event carries, opened or continued.
///
/// One event can hold several, which is why this appends rather than returns:
/// a call finishing and the next one opening arrive together.
fn calls(delta: &Value, open: &mut Open, deltas: &mut Vec<Delta>) -> Result<(), ProviderError> {
    let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else {
        return Ok(());
    };

    for call in calls {
        // The only identity a fragment has. Without it there is nothing to
        // decide which call the arguments below belong to, and guessing at the
        // one in hand is one tool running on another tool's arguments.
        let Some(index) = call.get("index").and_then(Value::as_u64) else {
            return Err(ProviderError::Protocol {
                provider: NAME,
                problem: "a tool call arrived without the index its fragments are keyed by".into(),
            });
        };

        let function = call.get("function");
        let name = function
            .and_then(|function| text(function, "name"))
            .filter(|name| !name.is_empty());

        // A name is what opens a call: it is sent once, with the identity a
        // result is answered against, and never again. Everything after it is a
        // fragment of the call that name opened.
        if let Some(name) = name {
            let Some(id) = text(call, "id").filter(|id| !id.is_empty()) else {
                // Skipped instead, nothing opens and the fragments that follow
                // are assembled onto the call before it, while the call that
                // was half announced leaves no trace — so the turn ends looking
                // like a clean finish with a tool the model asked for never run.
                return Err(ProviderError::Protocol {
                    provider: NAME,
                    problem: "a tool call was announced without the identity its result is \
                              answered against"
                        .into(),
                });
            };

            open.call = Some(index);
            deltas.push(Delta::ToolStarted {
                id: ToolId::new(id),
                name: name.into(),
            });
        } else if open.call != Some(index) {
            return Err(ProviderError::Protocol {
                provider: NAME,
                problem: "arguments arrived for a tool call other than the one open".into(),
            });
        }

        if let Some(arguments) = function
            .and_then(|function| text(function, "arguments"))
            .filter(|arguments| !arguments.is_empty())
        {
            deltas.push(Delta::ToolArgs(arguments.into()));
        }
    }

    Ok(())
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
        message: text(error, "message").unwrap_or(SILENT).into(),
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
