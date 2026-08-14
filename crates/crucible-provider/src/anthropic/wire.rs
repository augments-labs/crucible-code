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
use crate::refusal::SILENT;
use crate::sse::SseEvent;
use crate::stream::Wire;

/// The Messages API, being narrated.
///
/// Nothing is carried between events. This endpoint names a tool call in full
/// in the event that opens it, so a fragment arriving afterwards never has to
/// be matched against something read earlier — which is the state the other
/// protocol has no choice but to keep.
#[derive(Debug, Default)]
pub(super) struct Messages;

impl Wire for Messages {
    const PROVIDER: &'static str = NAME;

    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        Ok(delta(event)?.into_iter().collect())
    }
}

/// What an event means, or nothing if it means nothing to us.
///
/// The name decides before the payload is touched. Read the other way round, an
/// event this build has no name for fails the turn on a payload nobody was
/// going to look at — a proxy's heartbeat under a name of its own carries no
/// data line at all — and the arm that exists to skip it is never reached.
///
/// # Errors
///
/// [`ProviderError::Upstream`] when the event is the provider reporting a
/// failure inside a response it had already started, and
/// [`ProviderError::Protocol`] when an event that should carry a payload does
/// not parse, or announces a tool call by half its identity.
pub(super) fn delta(event: &SseEvent) -> Result<Option<Delta>, ProviderError> {
    match event.name.as_str() {
        "content_block_start" => started(&parse(&event.data)?),
        "content_block_delta" => Ok(content(&parse(&event.data)?)),
        "message_delta" => Ok(stopped(&parse(&event.data)?)),
        "error" => Err(upstream(&parse(&event.data)?)),

        // Everything else, and none of it parsed. `ping` is the keep-alive and
        // `message_start`, `content_block_stop` and `message_stop` are brackets
        // around content already delivered; a type this build has never heard
        // of is a newer API talking to an older client, or a proxy spelling its
        // own heartbeat however it likes — which arrives with no data line at
        // all. Skipping the lot keeps the answer flowing; the alternative is
        // failing a turn over something that was additive.
        _ => Ok(None),
    }
}

/// The beginning of a content block.
fn started(payload: &Value) -> Result<Option<Delta>, ProviderError> {
    let Some(block) = payload.get("content_block") else {
        return Ok(None);
    };

    // Text blocks announce themselves and then arrive as deltas; there is
    // nothing to show yet. Only a tool call carries its identity up front.
    if block.get("type").and_then(Value::as_str) != Some("tool_use") {
        return Ok(None);
    }

    // Half an identity. Skipped, the block's arguments would be assembled onto
    // the call before it — one tool running on another tool's arguments — and
    // the call that was half announced would leave no trace at all.
    let (Some(id), Some(name)) = (text(block, "id"), text(block, "name")) else {
        return Err(ProviderError::Protocol {
            provider: NAME,
            problem: "a tool call arrived with an id or a name but not both".into(),
        });
    };

    Ok(Some(Delta::ToolStarted {
        id: ToolId::new(id),
        name: name.into(),
    }))
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

        // The two ways this API says a turn ended.
        "end_turn" | "stop_sequence" => StopReason::Yielded,

        // A reason added to the vendor's list after this build shipped. Named
        // as unknown rather than folded into a finish: this arm fires on the
        // day nobody is watching, and an answer cut short by a reason with no
        // arm yet would otherwise arrive looking complete. A new reason is
        // still an edit here — this is what holds until it is made.
        _ => StopReason::Unknown,
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
            .unwrap_or(SILENT)
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
    fn a_tool_call_carrying_half_an_identity_is_refused_rather_than_skipped() {
        // Skipped, it is a call the runner never opens, so the fragments that
        // follow are assembled onto the call before it — a tool running on
        // another tool's arguments — and the one that was half announced
        // vanishes without anything saying so.
        for half in [
            r#"{"index":1,"content_block":{"type":"tool_use","id":"toolu_1"}}"#,
            r#"{"index":1,"content_block":{"type":"tool_use","name":"read"}}"#,
        ] {
            let problem = delta(&event("content_block_start", half)).unwrap_err();

            assert!(
                matches!(problem, ProviderError::Protocol { .. }),
                "expected a protocol failure, got {problem:?}"
            );
        }
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
        // Some proxies send `event: ping` with no data line at all, and a proxy
        // that spells its heartbeat differently sends the same empty payload
        // under a name this build has never heard of. Parsing either as JSON
        // would fail a turn over a keep-alive.
        assert_eq!(of("ping", ""), None);
        assert_eq!(of("keep-alive", ""), None);
    }

    #[test]
    fn an_unknown_event_type_is_skipped_rather_than_fatal() {
        assert_eq!(of("something_new", r#"{"whatever":true}"#), None);
    }

    #[test]
    fn an_unknown_event_is_skipped_without_its_payload_being_read() {
        // The order this reads in is the whole of it. Parsed first, an event
        // with a name nothing handles fails the turn on a payload nobody was
        // going to look at — and the arm above, which exists to skip it, is
        // never reached at all.
        assert_eq!(of("something_new", "not json at all"), None);
    }

    #[test]
    fn a_stop_reason_this_build_has_not_heard_of_is_not_reported_as_a_finish() {
        // The day the vendor adds one is the day nobody is watching. Read as a
        // finish, an answer cut short by it arrives looking complete, which is
        // the one failure the user cannot see for themselves.
        assert_eq!(
            of(
                "message_delta",
                r#"{"delta":{"stop_reason":"something_new"}}"#
            ),
            Some(Delta::Stopped(StopReason::Unknown))
        );
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
