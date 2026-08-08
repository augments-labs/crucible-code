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
/// # Errors
///
/// [`ProviderError::Upstream`] when the chunk is the provider reporting a
/// failure inside a response it had already started, and
/// [`ProviderError::Protocol`] when a chunk does not parse.
pub(super) fn deltas(event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
    // Not JSON, and the one thing every response ends with. Reading it as a
    // chunk would fail every turn at the last moment.
    if event.data.trim() == DONE {
        return Ok(Vec::new());
    }

    let payload = parse(&event.data)?;

    if let Some(error) = payload.get("error").filter(|error| !error.is_null()) {
        return Err(upstream(error));
    }

    let mut out = Vec::new();

    // One choice, because one is what a request without `n` asks for. A chunk
    // with none is the usage report that some accounts get after the last one.
    let Some(choice) = payload
        .get("choices")
        .and_then(Value::as_array)
        .and_then(|choices| choices.first())
    else {
        return Ok(out);
    };

    if let Some(delta) = choice.get("delta") {
        said(delta, &mut out);
    }

    // Last, and in the same chunk as whatever came with it: the reason arrives
    // alongside the final word about half the time.
    if let Some(reason) = text(choice, "finish_reason") {
        out.push(Delta::Stopped(stop(reason)));
    }

    Ok(out)
}

/// What the model produced in one chunk.
fn said(delta: &Value, out: &mut Vec<Delta>) {
    // Empty is what the opening chunk carries, the one that only announces the
    // role. Emitting it would put a blank line in front of every answer.
    if let Some(content) = text(delta, "content").filter(|content| !content.is_empty()) {
        out.push(Delta::Text(content.into()));
    }

    for call in delta
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        calling(call, out);
    }
}

/// One entry of a chunk's tool calls: an opening, a fragment, or both.
///
/// Both is why this appends rather than returns. Nothing on this wire promises
/// that the chunk carrying a call's name carries no arguments with it, and
/// dropping that first fragment leaves the model's arguments unparsable.
fn calling(call: &Value, out: &mut Vec<Delta>) {
    let function = call.get("function");

    // An identifier means a call is opening. Only the first chunk of each one
    // carries it; the fragments after it are identified by position alone,
    // which the runner handles by assembling whatever call is open.
    let name = function.and_then(|function| text(function, "name"));
    if let (Some(id), Some(name)) = (text(call, "id"), name) {
        out.push(Delta::ToolStarted {
            id: ToolId::new(id),
            name: name.into(),
        });
    }

    let args = function
        .and_then(|function| text(function, "arguments"))
        .filter(|args| !args.is_empty());
    if let Some(args) = args {
        out.push(Delta::ToolArgs(args.into()));
    }
}

/// The model saying why it stopped.
fn stop(reason: &str) -> StopReason {
    match reason {
        "tool_calls" | "function_call" => StopReason::WantsTools,
        "length" => StopReason::OutOfTokens,
        // `stop`, `content_filter`, and anything added later. Every one of them
        // means the model is finished and it is the user's move.
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
mod tests {
    use super::*;

    fn event(data: &str) -> SseEvent {
        SseEvent {
            // Chunks arrive on this wire as bare `data:` lines with no type in
            // front of them, which the framing reports as an unnamed event.
            name: String::new(),
            data: data.to_owned(),
        }
    }

    fn of(data: &str) -> Vec<Delta> {
        deltas(&event(data)).unwrap()
    }

    #[test]
    fn text_arrives_as_it_is_produced() {
        let out = of(r#"{"choices":[{"index":0,"delta":{"content":"Hel"},"finish_reason":null}]}"#);

        assert_eq!(out, vec![Delta::Text("Hel".into())]);
    }

    #[test]
    fn the_first_chunk_of_an_answer_carries_a_role_and_no_words() {
        // Emitting something for it would put an empty line in front of every
        // answer.
        let out = of(r#"{"choices":[{"index":0,"delta":{"role":"assistant","content":""}}]}"#);

        assert_eq!(out, Vec::new());
    }

    #[test]
    fn a_tool_call_announces_its_identity_before_its_arguments() {
        // The name has to arrive first: the fragments that follow carry an
        // index and nothing else saying which call they belong to.
        let out = of(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":""}}]}}]}"#,
        );

        assert_eq!(
            out,
            vec![Delta::ToolStarted {
                id: ToolId::new("call_1"),
                name: "read".into(),
            }]
        );
    }

    #[test]
    fn a_call_that_opens_with_arguments_already_in_it_yields_both() {
        // Nothing on this wire promises the opening chunk is empty of them, and
        // dropping the first fragment leaves the model's arguments unparsable.
        let out = of(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"name":"read","arguments":"{\"pa"}}]}}]}"#,
        );

        assert_eq!(
            out,
            vec![
                Delta::ToolStarted {
                    id: ToolId::new("call_1"),
                    name: "read".into(),
                },
                Delta::ToolArgs("{\"pa".into()),
            ]
        );
    }

    #[test]
    fn tool_arguments_arrive_in_fragments() {
        let out = of(
            r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"th\":\"a.rs\"}"}}]}}]}"#,
        );

        assert_eq!(out, vec![Delta::ToolArgs("th\":\"a.rs\"}".into())]);
    }

    #[test]
    fn two_calls_opened_in_one_chunk_are_two_calls() {
        let out = of(r#"{"choices":[{"index":0,"delta":{"tool_calls":[
                {"index":0,"id":"call_1","function":{"name":"read","arguments":""}},
                {"index":1,"id":"call_2","function":{"name":"glob","arguments":""}}]}}]}"#);

        assert_eq!(
            out,
            vec![
                Delta::ToolStarted {
                    id: ToolId::new("call_1"),
                    name: "read".into(),
                },
                Delta::ToolStarted {
                    id: ToolId::new("call_2"),
                    name: "glob".into(),
                },
            ]
        );
    }

    #[test]
    fn wanting_tools_is_distinguished_from_yielding() {
        // The runner's whole loop turns on this: one runs the tools and goes
        // back to the model, the other hands control to the user.
        assert_eq!(
            of(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}"#),
            vec![Delta::Stopped(StopReason::WantsTools)]
        );
        assert_eq!(
            of(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#),
            vec![Delta::Stopped(StopReason::Yielded)]
        );
    }

    #[test]
    fn running_out_of_tokens_is_its_own_reason() {
        // A truncated answer looks finished. It is the one stop the user has to
        // be told about.
        assert_eq!(
            of(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#),
            vec![Delta::Stopped(StopReason::OutOfTokens)]
        );
    }

    #[test]
    fn a_last_word_and_a_stop_in_one_chunk_keeps_both() {
        let out = of(r#"{"choices":[{"index":0,"delta":{"content":"!"},"finish_reason":"stop"}]}"#);

        assert_eq!(
            out,
            vec![Delta::Text("!".into()), Delta::Stopped(StopReason::Yielded)]
        );
    }

    #[test]
    fn the_marker_that_closes_the_stream_is_not_parsed_as_a_chunk() {
        // It is the literal text `[DONE]`. Reading it as JSON fails a turn on
        // the one thing every response ends with.
        assert_eq!(of(DONE), Vec::new());
    }

    #[test]
    fn a_chunk_with_no_choices_is_not_a_failure() {
        // The usage report that some accounts get after the last choice.
        assert_eq!(
            of(r#"{"choices":[],"usage":{"total_tokens":12}}"#),
            Vec::new()
        );
    }

    #[test]
    fn an_unknown_field_is_skipped_rather_than_fatal() {
        assert_eq!(
            of(r#"{"choices":[{"index":0,"delta":{"refusal":null,"whatever":true}}]}"#),
            Vec::new()
        );
    }

    #[test]
    fn a_failure_inside_the_response_carries_what_the_provider_called_it() {
        // This arrives on a 200. Being over a rate limit is the usual cause,
        // and the kind is what tells a caller it is worth trying again.
        let problem = deltas(&event(
            r#"{"error":{"type":"server_error","message":"The server had an error"}}"#,
        ))
        .unwrap_err();

        assert_eq!(
            problem.to_string(),
            "openai: server_error: The server had an error"
        );
    }

    #[test]
    fn a_chunk_that_is_not_json_is_a_protocol_failure_that_does_not_quote_it() {
        // The payload can be an entire chunk long, and this message is shown to
        // a user.
        let problem = deltas(&event("not json at all")).unwrap_err();

        assert!(
            matches!(problem, ProviderError::Protocol { .. }),
            "expected a protocol failure, got {problem:?}"
        );
        assert!(
            !problem.to_string().contains("not json at all"),
            "the payload was quoted back: {problem}"
        );
    }
}
