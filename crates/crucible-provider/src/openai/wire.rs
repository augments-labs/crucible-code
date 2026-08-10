//! A response, from OpenAI's shape.
//!
//! One chunk in, however many deltas out. Zero for the keep-alives and for the
//! marker that closes the stream; more than one when a chunk both opens a tool
//! call and starts its arguments, which the first chunk of every call does.
//! That plural is the difference from the other protocol, where a delta is an
//! event and an event is a delta.
//!
//! Read by field lookup rather than into mirror structs. The payload is
//! consumed once, here, and a struct per chunk shape would be more code to say
//! the same thing while still needing a fallback for what it does not know.

use crucible_core::{Delta, ProviderError, StopReason, ToolId};
use serde_json::Value;

use crate::openai::NAME;
use crate::sse::SseEvent;

/// The marker that closes a stream. Not JSON, and not a stop reason.
const DONE: &str = "[DONE]";

/// What a chunk means, or nothing if it means nothing to us.
///
/// `open` is which tool call the response has open, by the index its vendor
/// gave it. The caller carries it between chunks because a call is opened in
/// one chunk and its arguments arrive in the ones after it — so no single chunk
/// can tell whether a fragment belongs where it is about to be assembled.
///
/// # Errors
///
/// [`ProviderError::Upstream`] when the chunk is the provider reporting a
/// failure inside a response it had already started, and
/// [`ProviderError::Protocol`] when a chunk does not parse.
pub(super) fn deltas(
    event: &SseEvent,
    open: &mut Option<i64>,
) -> Result<Vec<Delta>, ProviderError> {
    // Not JSON, and the one thing every response ends with. Reading it as a
    // chunk would fail every turn at the last moment.
    if event.data.trim() == DONE {
        return Ok(Vec::new());
    }

    // A heartbeat, which a proxy may spell any way it likes and may send with
    // no data line at all. There is nothing to parse; reading it as a chunk
    // fails the turn and discards the answer that had already arrived.
    if event.data.trim().is_empty() {
        return Ok(Vec::new());
    }

    let payload = parse(&event.data)?;

    if let Some(error) = payload.get("error").filter(|error| !error.is_null()) {
        return Err(upstream(error));
    }

    let mut out = Vec::new();

    // One choice, because one is what a request without `n` asks for. A chunk
    // with none carries a usage report, which this build never asks for and
    // which the endpoint sends only to a request that opted in. Other servers
    // speaking this protocol send one anyway, so it is skipped rather than
    // refused.
    let Some(choice) = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(out);
    };

    if let Some(delta) = choice.get("delta") {
        said(delta, &mut out, open)?;
    }

    // Last, and in the same chunk as whatever came with it: a reason shares a
    // chunk with the final word or arrives in one of its own, and both shapes
    // are ordinary. Pushing it after the content is what makes either safe.
    if let Some(reason) = text(choice, "finish_reason") {
        out.push(Delta::Stopped(stop(reason)));
    }

    Ok(out)
}

/// What the model produced in one chunk.
fn said(delta: &Value, out: &mut Vec<Delta>, open: &mut Option<i64>) -> Result<(), ProviderError> {
    // Empty is what the opening chunk carries, the one that only announces the
    // role. Emitting it would put a blank line in front of every answer.
    if let Some(content) = text(delta, "content").filter(|content| !content.is_empty()) {
        out.push(Delta::Text(content.into()));
    }

    // A model that declines says so here, and leaves `content` null for the
    // whole response. Unread, a refusal is a turn that shows nothing at all
    // and then reports that it finished normally.
    if let Some(refusal) = text(delta, "refusal").filter(|refusal| !refusal.is_empty()) {
        out.push(Delta::Text(refusal.into()));
    }

    for call in delta
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        calling(call, out, open)?;
    }

    Ok(())
}

/// One entry of a chunk's tool calls: an opening, a fragment, or both.
///
/// Both is why this appends rather than returns. Nothing on this wire promises
/// that the chunk carrying a call's name carries no arguments with it, and
/// dropping that first fragment leaves the model's arguments unparsable.
///
/// Fragments carry no identity, so the runner appends them to whichever call it
/// opened last. That works because a provider sends each call's fragments
/// before opening the next one — an assumption, and the one this parser rests
/// on. What it will not do is assemble a fragment onto a call that is visibly
/// the wrong one, which is what the two guards below are for. `open` is the
/// call the fragments belong to, and outlives the chunk it was opened in.
fn calling(
    call: &Value,
    out: &mut Vec<Delta>,
    open: &mut Option<i64>,
) -> Result<(), ProviderError> {
    let function = call.get("function");
    let index = call.get("index").and_then(Value::as_i64);

    let id = text(call, "id").filter(|id| !id.is_empty());
    let name = function
        .and_then(|function| text(function, "name"))
        .filter(|name| !name.is_empty());

    match (id, name) {
        (Some(id), Some(name)) => {
            *open = index;
            out.push(Delta::ToolStarted {
                id: ToolId::new(id),
                name: name.into(),
            });
        }

        // Half an identity. Letting it through would send the arguments below
        // to whichever call was open before it, which is one tool running on
        // another tool's arguments rather than a failure anyone can see.
        (Some(_), None) | (None, Some(_)) => {
            return Err(ProviderError::Protocol {
                provider: NAME,
                problem: "a tool call arrived with an id or a name but not both".into(),
            });
        }

        // Neither: a fragment of a call already open.
        (None, None) => {}
    }

    let args = function
        .and_then(|function| text(function, "arguments"))
        .filter(|args| !args.is_empty());
    let Some(args) = args else { return Ok(()) };

    // Arguments under any index but the open call's would be assembled onto
    // that call. The call may have been opened chunks ago, which is the whole
    // reason `open` is not a local of this parse.
    if let (Some(open), Some(index)) = (*open, index)
        && open != index
    {
        return Err(ProviderError::Protocol {
            provider: NAME,
            problem: "arguments arrived for a tool call other than the one open".into(),
        });
    }

    out.push(Delta::ToolArgs(args.into()));
    Ok(())
}

/// The model saying why it stopped.
fn stop(reason: &str) -> StopReason {
    match reason {
        "tool_calls" | "function_call" => StopReason::WantsTools,
        "length" => StopReason::OutOfTokens,
        // Content was withheld, so the answer on screen is not the whole one.
        // Reporting it as a finish would show a cut-off answer as a complete
        // one, which is the same mistake `length` is spelled out to avoid.
        "content_filter" => StopReason::Filtered,
        // `stop`, and anything added later. A reason this build has not heard
        // of is treated as a finish, which is the only safe guess: it hands the
        // turn back rather than asking the provider for more.
        _ => StopReason::Yielded,
    }
}

/// A failure the provider reported mid-response.
fn upstream(error: &Value) -> ProviderError {
    ProviderError::Upstream {
        provider: NAME,
        kind: text(error, "type").unwrap_or("error").into(),
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
        // The payload itself is not carried: it is up to a whole chunk long and
        // this message ends up in front of a user.
        problem: format!("a chunk was not JSON: {problem}").into(),
    })
}

#[cfg(test)]
mod tests;
