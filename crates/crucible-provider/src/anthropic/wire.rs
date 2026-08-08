//! A response, from Anthropic's shape.
//!
//! One event in, at most one delta out. Most events are bookkeeping — a ping,
//! the start of a block, the close of one — and produce nothing, so the caller
//! keeps reading rather than deciding what to skip.
//!
//! Read by field lookup rather than into mirror structs. The payload is
//! consumed once, here, and a struct per event type would be more code to say
//! the same thing while still needing a fallback for the types this does not
//! know.

use crucible_core::{Delta, ProviderError, StopReason, ToolId};
use serde_json::Value;

use crate::anthropic::NAME;
use crate::sse::SseEvent;

/// What an event means, or nothing if it means nothing to us.
///
/// # Errors
///
/// [`ProviderError::Upstream`] when the event is the provider reporting a
/// failure inside a response it had already started, and
/// [`ProviderError::Protocol`] when an event that should carry a payload does
/// not parse.
pub(super) fn delta(event: &SseEvent) -> Result<Option<Delta>, ProviderError> {
    // Types that carry nothing worth parsing. `ping` is the keep-alive, and the
    // rest are brackets around content that has already been delivered.
    if matches!(
        event.name.as_str(),
        "ping" | "message_start" | "content_block_stop" | "message_stop"
    ) {
        return Ok(None);
    }

    let payload = parse(&event.data)?;

    match event.name.as_str() {
        "content_block_start" => Ok(started(&payload)),
        "content_block_delta" => Ok(content(&payload)),
        "message_delta" => Ok(stopped(&payload)),
        "error" => Err(upstream(&payload)),
        // An unknown type is a newer API talking to an older client. Skipping
        // it keeps the answer flowing; the alternative is failing a turn over
        // something that was additive.
        _ => Ok(None),
    }
}

/// The beginning of a content block.
fn started(payload: &Value) -> Option<Delta> {
    let block = payload.get("content_block")?;

    // Text blocks announce themselves and then arrive as deltas; there is
    // nothing to show yet. Only a tool call carries its identity up front.
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return None;
    }

    Some(Delta::ToolStarted {
        id: ToolId::new(text(block, "id")?),
        name: text(block, "name")?.into(),
    })
}

/// More of the current content block.
fn content(payload: &Value) -> Option<Delta> {
    let block = payload.get("delta")?;

    match block.get("type").and_then(Value::as_str)? {
        "text_delta" => Some(Delta::Text(text(block, "text")?.into())),
        "input_json_delta" => Some(Delta::ToolArgs(text(block, "partial_json")?.into())),
        // Thinking blocks, and whatever comes after them. Nothing here asks for
        // them, and showing a reasoning trace nobody requested is worse than
        // showing nothing.
        _ => None,
    }
}

/// The model saying it has stopped, and why.
fn stopped(payload: &Value) -> Option<Delta> {
    let reason = payload
        .get("delta")
        .and_then(|delta| text(delta, "stop_reason"))?;

    Some(Delta::Stopped(match reason {
        "tool_use" => StopReason::WantsTools,

        // Two different ceilings, one remedy: ask for less and the answer
        // arrives whole.
        "max_tokens" | "model_context_window_exceeded" => StopReason::OutOfTokens,

        // Stopped by a classifier rather than by the model. No shorter request
        // helps, which is why it is not folded in with the ceilings above.
        "refusal" => StopReason::Filtered,

        // A long turn the model expects to be asked to carry on. Left to the
        // arm below it would read as a finish, which is the one way an
        // unfinished answer reaches the user looking whole.
        "pause_turn" => StopReason::Paused,

        // `end_turn` and `stop_sequence`. A reason added later lands here too
        // and will read as a finish — so a new one in the vendor's list is a
        // change to make here, not something this arm can be trusted to cover.
        _ => StopReason::Yielded,
    }))
}

/// A failure the provider reported mid-response.
fn upstream(payload: &Value) -> ProviderError {
    let error = payload.get("error");

    ProviderError::Upstream {
        provider: NAME,
        kind: error
            .and_then(|error| text(error, "type"))
            .unwrap_or("error")
            .into(),
        message: error
            .and_then(|error| text(error, "message"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn event(name: &str, data: &str) -> SseEvent {
        SseEvent {
            name: name.to_owned(),
            data: data.to_owned(),
        }
    }

    fn of(name: &str, data: &str) -> Option<Delta> {
        delta(&event(name, data)).unwrap()
    }

    #[test]
    fn text_arrives_as_it_is_produced() {
        let out = of(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"text_delta","text":"Hel"}}"#,
        );

        assert_eq!(out, Some(Delta::Text("Hel".into())));
    }

    #[test]
    fn a_tool_call_announces_its_identity_before_its_arguments() {
        // The name has to arrive first: the arguments that follow are fragments
        // with nothing in them saying which call they belong to.
        let out = of(
            "content_block_start",
            r#"{"index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"read","input":{}}}"#,
        );

        assert_eq!(
            out,
            Some(Delta::ToolStarted {
                id: ToolId::new("toolu_1"),
                name: "read".into(),
            })
        );
    }

    #[test]
    fn tool_arguments_arrive_in_fragments() {
        let out = of(
            "content_block_delta",
            r#"{"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"pa"}}"#,
        );

        assert_eq!(out, Some(Delta::ToolArgs("{\"pa".into())));
    }

    #[test]
    fn a_text_block_starting_is_not_a_delta() {
        // It carries no text; the text follows. Emitting something here would
        // put an empty line in front of every answer.
        let out = of(
            "content_block_start",
            r#"{"index":0,"content_block":{"type":"text","text":""}}"#,
        );

        assert_eq!(out, None);
    }

    #[test]
    fn wanting_tools_is_distinguished_from_yielding() {
        // The runner's whole loop turns on this: one runs the tools and goes
        // back to the model, the other hands control to the user.
        assert_eq!(
            of("message_delta", r#"{"delta":{"stop_reason":"tool_use"}}"#),
            Some(Delta::Stopped(StopReason::WantsTools))
        );
        assert_eq!(
            of("message_delta", r#"{"delta":{"stop_reason":"end_turn"}}"#),
            Some(Delta::Stopped(StopReason::Yielded))
        );
    }

    #[test]
    fn running_out_of_tokens_is_its_own_reason() {
        // A truncated answer looks finished. It is the one stop the user has to
        // be told about.
        assert_eq!(
            of("message_delta", r#"{"delta":{"stop_reason":"max_tokens"}}"#),
            Some(Delta::Stopped(StopReason::OutOfTokens))
        );
    }

    #[test]
    fn an_answer_the_provider_withheld_is_not_reported_as_a_finished_one() {
        // A classifier stopping the response mid-sentence is the case the
        // reason exists for: no shorter request helps, so saying "finished"
        // would be telling the user the opposite of what happened.
        assert_eq!(
            of("message_delta", r#"{"delta":{"stop_reason":"refusal"}}"#),
            Some(Delta::Stopped(StopReason::Filtered))
        );
    }

    #[test]
    fn running_past_the_context_window_is_a_ceiling_like_any_other() {
        // Different ceiling from `max_tokens`, same remedy — a shorter request
        // is what gets a whole answer — so it reports the same reason.
        assert_eq!(
            of(
                "message_delta",
                r#"{"delta":{"stop_reason":"model_context_window_exceeded"}}"#
            ),
            Some(Delta::Stopped(StopReason::OutOfTokens))
        );
    }

    #[test]
    fn a_paused_turn_is_not_a_finished_one() {
        // The provider is waiting to be asked to carry on. Reported as a finish
        // it becomes an answer that stops mid-thought and says nothing about
        // why, which is the failure the user cannot diagnose for themselves.
        assert_eq!(
            of("message_delta", r#"{"delta":{"stop_reason":"pause_turn"}}"#),
            Some(Delta::Stopped(StopReason::Paused))
        );
    }

    #[test]
    fn a_stop_sequence_yields_like_any_other_ending() {
        assert_eq!(
            of(
                "message_delta",
                r#"{"delta":{"stop_reason":"stop_sequence"}}"#
            ),
            Some(Delta::Stopped(StopReason::Yielded))
        );
    }

    #[test]
    fn a_usage_only_message_delta_is_not_a_stop() {
        // The final `message_delta` carries token counts alongside the reason,
        // but an interim one carries counts alone.
        assert_eq!(
            of("message_delta", r#"{"usage":{"output_tokens":12}}"#),
            None
        );
    }

    #[test]
    fn bookkeeping_events_produce_nothing() {
        for (name, data) in [
            ("ping", "{}"),
            ("message_start", r#"{"message":{"id":"msg_1"}}"#),
            ("content_block_stop", r#"{"index":0}"#),
            ("message_stop", "{}"),
        ] {
            assert_eq!(of(name, data), None, "{name} produced a delta");
        }
    }

    #[test]
    fn a_keep_alive_with_no_payload_is_not_a_parse_failure() {
        // Some proxies send `event: ping` with no data line at all. Parsing
        // that as JSON would fail a turn over a heartbeat.
        assert_eq!(of("ping", ""), None);
    }

    #[test]
    fn an_unknown_event_type_is_skipped_rather_than_fatal() {
        assert_eq!(of("something_new", r#"{"whatever":true}"#), None);
    }

    #[test]
    fn a_failure_inside_the_response_carries_what_the_provider_called_it() {
        // This arrives on a 200. Being overloaded is the usual cause, and the
        // kind is what tells a caller it is worth trying again.
        let problem = delta(&event(
            "error",
            r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#,
        ))
        .unwrap_err();

        assert_eq!(
            problem.to_string(),
            "anthropic: overloaded_error: Overloaded"
        );
    }

    #[test]
    fn an_event_that_is_not_json_is_a_protocol_failure_that_does_not_quote_it() {
        // The payload can be an entire event long, and this message is shown to
        // a user.
        let problem = delta(&event("content_block_delta", "not json at all")).unwrap_err();

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
