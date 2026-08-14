//! A response, from OpenAI's shape.
//!
//! One event in, however many deltas out. Zero for most of them: this endpoint
//! narrates a response item by item, and the majority of what it sends is the
//! narration rather than the answer.
//!
//! Read by field lookup rather than into mirror structs. The payload is
//! consumed once, here, and a struct per event shape would be more code to say
//! the same thing while still needing a fallback for what it does not know.
//!
//! Every event says its own `type`, so that is what is read rather than the SSE
//! event name beside it. One name, one place it is spelled.

use crucible_core::{Delta, ProviderError, StopReason, ToolId};
use serde_json::Value;

use crate::openai::NAME;
use crate::refusal::SILENT;
use crate::sse::SseEvent;
use crate::stream::Wire;

/// The Responses API, being narrated.
#[derive(Debug, Default)]
pub(super) struct Responses {
    open: Open,
}

impl Wire for Responses {
    const PROVIDER: &'static str = NAME;

    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        deltas(event, &mut self.open)
    }
}

/// The tool call the response has open, and whether its arguments have started
/// arriving.
///
/// Carried between events because a call is opened in one and its arguments
/// arrive in the ones after it — so no single event can tell whether a fragment
/// belongs where it is about to be assembled. The flag is what decides whether
/// the finished item repeats arguments already streamed or supplies ones that
/// never were.
#[derive(Debug, Default)]
struct Open {
    /// The item id this endpoint keys its fragments by, empty where none is
    /// open.
    item: String,
    /// Whether a fragment has arrived for it.
    streamed: bool,
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
    // A heartbeat, which a proxy may spell any way it likes and may send with
    // no data line at all. There is nothing to parse; reading it as an event
    // fails the turn and discards the answer that had already arrived.
    if event.data.trim().is_empty() {
        return Ok(Vec::new());
    }

    let payload = parse(&event.data)?;
    let Some(kind) = text(&payload, "type") else {
        return Ok(Vec::new());
    };

    match kind {
        // The answer as it is written, and the other thing a model writes in
        // its place. A model that declines produces no output text at all, and
        // a refusal left unread is a turn that shows nothing and then reports
        // that it finished normally.
        "response.output_text.delta" | "response.refusal.delta" => Ok(said(&payload)),

        "response.output_item.added" => started(&payload, open),
        "response.function_call_arguments.delta" => arguing(&payload, open),
        "response.output_item.done" => finished(&payload, open),

        // The three ways a response ends. `completed` is the only one that is
        // not a failure, and which of the two finishes it is depends on what
        // the response turned out to hold.
        "response.completed" => Ok(vec![Delta::Stopped(stop(&payload))]),
        "response.incomplete" => Ok(vec![Delta::Stopped(cut(&payload))]),
        "response.failed" => Err(failed(&payload)),

        // A failure outside any response, which arrives flat rather than under
        // one.
        "error" => Err(upstream(&payload)),

        // Everything else this endpoint narrates: the response opening, content
        // parts being framed, reasoning being done. A stream that failed on an
        // event it had not heard of would fail every turn the day a field is
        // added.
        _ => Ok(Vec::new()),
    }
}

/// A fragment of what the model is saying.
fn said(payload: &Value) -> Vec<Delta> {
    match text(payload, "delta").filter(|delta| !delta.is_empty()) {
        Some(delta) => vec![Delta::Text(delta.into())],
        None => Vec::new(),
    }
}

/// An item the response has started, which is a tool call or is not.
///
/// A message item opening is nothing to report: its text arrives as fragments
/// and this would put a delta in front of it saying so.
fn started(payload: &Value, open: &mut Open) -> Result<Vec<Delta>, ProviderError> {
    let Some(item) = payload.get("item") else {
        return Ok(Vec::new());
    };
    if text(item, "type") != Some("function_call") {
        return Ok(Vec::new());
    }

    // The two identities this endpoint gives a call, and they are not
    // interchangeable. `id` is what its own fragments are keyed by; `call_id`
    // is what a result is answered against, and it is the one the transcript
    // has to carry.
    //
    // A call missing either of them, or its name, is refused rather than
    // skipped. Skipped, nothing opens: the fragments that follow are assembled
    // onto the call before it — one tool running on another tool's arguments —
    // and the call that was half announced leaves no trace, so the turn ends
    // looking like a clean finish with a tool the model asked for never run.
    let (Some(id), Some(name), Some(call)) =
        (text(item, "id"), text(item, "name"), text(item, "call_id"))
    else {
        return Err(ProviderError::Protocol {
            provider: NAME,
            problem: "a tool call arrived without both of its identities and a name".into(),
        });
    };

    *open = Open {
        item: id.to_owned(),
        streamed: false,
    };

    Ok(vec![Delta::ToolStarted {
        id: ToolId::new(call),
        name: name.into(),
    }])
}

/// A fragment of the open call's arguments.
///
/// Fragments carry the item they belong to, so unlike the older endpoint this
/// can say when one does not belong to the call in hand. It refuses rather than
/// assembling it anyway, which would be one tool running on another tool's
/// arguments rather than a failure anyone can see.
fn arguing(payload: &Value, open: &mut Open) -> Result<Vec<Delta>, ProviderError> {
    let Some(delta) = text(payload, "delta").filter(|delta| !delta.is_empty()) else {
        return Ok(Vec::new());
    };

    if text(payload, "item_id").is_none_or(|item| item != open.item) {
        return Err(ProviderError::Protocol {
            provider: NAME,
            problem: "arguments arrived for a tool call other than the one open".into(),
        });
    }

    open.streamed = true;
    Ok(vec![Delta::ToolArgs(delta.into())])
}

/// An item the response has finished.
///
/// The finished call carries its whole argument text. Where fragments arrived it
/// is what they add up to and repeating it would double the arguments; where
/// none did — a server that narrates only the ends of things — it is the only
/// copy there is.
///
/// Which of those it is can only be answered about the call in hand, so the item
/// is checked against the open one first — the same check [`arguing`] makes, for
/// the same reason. Taken on trust, the arguments of one call would be emitted
/// under whichever call happens to be open and against the `streamed` flag of
/// that other call: one tool running on another tool's arguments, and the flag
/// deciding whether they arrive twice or not at all.
fn finished(payload: &Value, open: &mut Open) -> Result<Vec<Delta>, ProviderError> {
    let Some(item) = payload.get("item") else {
        return Ok(Vec::new());
    };
    if text(item, "type") != Some("function_call") {
        return Ok(Vec::new());
    }

    if text(item, "id").is_none_or(|item| item != open.item) {
        return Err(ProviderError::Protocol {
            provider: NAME,
            problem: "a tool call finished that was not the one open".into(),
        });
    }

    let streamed = std::mem::take(open).streamed;
    let arguments = text(item, "arguments").filter(|arguments| !arguments.is_empty());

    Ok(match arguments {
        Some(arguments) if !streamed => vec![Delta::ToolArgs(arguments.into())],
        _ => Vec::new(),
    })
}

/// Why a response that finished finished.
///
/// There is no field for it: a response that wants tools and a response that
/// has answered both complete, and what tells them apart is whether the output
/// holds a call. Read from the finished response rather than remembered from
/// the items, so the two cannot disagree.
fn stop(payload: &Value) -> StopReason {
    let calls = payload
        .get("response")
        .and_then(|response| response.get("output"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|item| text(item, "type") == Some("function_call"));

    if calls {
        StopReason::WantsTools
    } else {
        StopReason::Yielded
    }
}

/// Why a response that did not finish stopped.
///
/// Neither of these is a finish. An answer cut off by a ceiling or withheld by a
/// filter reads as a complete answer unless the turn says otherwise, and that is
/// the one failure the user cannot see for themselves.
fn cut(payload: &Value) -> StopReason {
    let reason = payload
        .get("response")
        .and_then(|response| response.get("incomplete_details"))
        .and_then(|details| text(details, "reason"));

    match reason {
        Some("content_filter") => StopReason::Filtered,
        // `max_output_tokens`, and any reason this build has not heard of.
        // Falling through to a ceiling rather than to a finish is the whole
        // point: the response has already said it is incomplete, and the only
        // question left is which way to say so.
        _ => StopReason::OutOfTokens,
    }
}

/// A response the provider gave up on part-way through.
///
/// `error` is a field that can be present and null — a vendor spelling "no
/// error" that way rather than by leaving it out — so null is absent here. The
/// alternative is reading a code and a message off nothing, finding neither,
/// and reporting that the provider said nothing about a response that had said
/// its status and why it stopped.
fn failed(payload: &Value) -> ProviderError {
    let response = payload.get("response");
    let error = response
        .and_then(|response| response.get("error"))
        .filter(|error| !error.is_null());

    if let Some(error) = error {
        return upstream(error);
    }

    // Nothing under `error`, so what is left is what the response says about
    // itself: why it is incomplete, and failing that its own status as the kind.
    ProviderError::Upstream {
        provider: NAME,
        kind: response
            .and_then(|response| text(response, "status"))
            .unwrap_or("error")
            .into(),
        message: response
            .and_then(|response| response.get("incomplete_details"))
            .and_then(|details| text(details, "reason"))
            .unwrap_or(SILENT)
            .into(),
    }
}

/// A failure the provider reported mid-response.
fn upstream(error: &Value) -> ProviderError {
    ProviderError::Upstream {
        provider: NAME,
        kind: text(error, "code")
            .or_else(|| text(error, "type"))
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

#[cfg(test)]
mod tests;
