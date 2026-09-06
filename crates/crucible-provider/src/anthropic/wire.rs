//! A response, from Anthropic's shape.
//!
//! One event in, however many deltas out. Zero for most of them: they are
//! bookkeeping — a ping, the start of a block, the close of one — so the caller
//! keeps reading rather than deciding what to skip. Two for the event that ends
//! a response, which says what it cost and why the model stopped at once.
//!
//! Read by field lookup rather than into mirror structs. The payload is
//! consumed once, here, and a struct per event type would be more code to say
//! the same thing while still needing a fallback for the types this does not
//! know.

use crucible_core::{
    Delta, InputTokenUsage, ProviderError, ProviderNumericDetail, ProviderUsage, StopReason,
    ToolId, UsageError,
};
use serde_json::Value;

use crate::anthropic::NAME;
use crate::refusal::SILENT;
use crate::sse::SseEvent;
use crate::stream::Wire;

/// The Messages API, being narrated.
///
/// Legacy projections remain stateless. Fable's request-bound parser retains
/// ordered blocks until the complete message closes, without exposing thinking.
#[derive(Default)]
pub(super) struct Messages {
    blocks: Option<super::continuation::Blocks>,
}

impl Messages {
    pub(super) fn for_request(
        model: &str,
        scope: crucible_core::ContinuationScope,
        effort: Option<crucible_core::Effort>,
    ) -> Result<Self, ProviderError> {
        Ok(Self {
            blocks: if model == super::FABLE_51 {
                Some(super::continuation::Blocks::new(model, scope, effort)?)
            } else {
                None
            },
        })
    }
}

impl Wire for Messages {
    const PROVIDER: &'static str = NAME;

    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        if let Some(blocks) = &mut self.blocks {
            blocks.deltas(event)
        } else {
            deltas(event)
        }
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
fn deltas(event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
    match event.name.as_str() {
        "content_block_start" => Ok(started(&parse(&event.data)?)?.into_iter().collect()),
        "content_block_delta" => Ok(content(&parse(&event.data)?).into_iter().collect()),
        "message_delta" => ended(&parse(&event.data)?),
        "message_start" => Ok(opened(&parse(&event.data)?)?.into_iter().collect()),
        "error" => Err(upstream(&parse(&event.data)?)),

        // Everything else, and none of it parsed. `ping` is the keep-alive and
        // `content_block_stop` and `message_stop` are brackets around content
        // already delivered; a type this build has never heard
        // of is a newer API talking to an older client, or a proxy spelling its
        // own heartbeat however it likes — which arrives with no data line at
        // all. Skipping the lot keeps the answer flowing; the alternative is
        // failing a turn over something that was additive.
        _ => Ok(Vec::new()),
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

/// What the event that closes a response says.
///
/// Two things, and either can be absent: this endpoint sends one of these for
/// the counts alone while the answer is still arriving, and sends the last one
/// with the reason beside them. The cost goes first, because the stop is the
/// thing a reader is entitled to treat as the last word.
pub(super) fn ended(payload: &Value) -> Result<Vec<Delta>, ProviderError> {
    Ok(output_usage(payload)?
        .into_iter()
        .chain(stopped(payload))
        .collect())
}

/// A partial output reading; the input report arrived at message start.
fn output_usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
    let Some(tokens) = payload
        .get("usage")
        .and_then(|usage| usage.get("output_tokens"))
        .and_then(Value::as_u64)
    else {
        return Ok(None);
    };
    let detail = ProviderNumericDetail::new("output_tokens", tokens).map_err(usage_problem)?;
    let usage = ProviderUsage::new(
        InputTokenUsage::UNKNOWN,
        Some(tokens),
        None,
        None,
        &[detail],
    )
    .map_err(usage_problem)?;
    Ok(Some(Delta::Usage(usage)))
}

/// What the request this response answers carried.
///
/// Sent once, in the event that opens the response, because it is settled
/// before the model has written a word — which is the same reason [`spent`]
/// arrives repeatedly and this does not.
///
/// Anthropic reports uncached input, cache writes, and cache reads as separate
/// fields. All three occupy the request's context window, so the carried count
/// is their sum. The cache fields may appear even where crucible did not mark a
/// block itself — a gateway or provider feature can cache the request — and
/// omitting them makes a mostly cached session look almost empty.
///
/// Absent rather than zero only where none of the three counts is present, for
/// the reason [`spent`] gives about its own: a request whose size nobody
/// reported and one that carried nothing are different facts.
pub(super) fn opened(payload: &Value) -> Result<Option<Delta>, ProviderError> {
    let Some(usage) = payload
        .get("message")
        .and_then(|message| message.get("usage"))
    else {
        return Ok(None);
    };
    let uncached = number(usage, "input_tokens");
    let write = number(usage, "cache_creation_input_tokens");
    let read = number(usage, "cache_read_input_tokens");
    if uncached.is_none() && write.is_none() && read.is_none() {
        return Ok(None);
    }
    let mut details = Vec::new();
    for (label, value) in [
        ("input_tokens", uncached),
        ("cache_creation_input_tokens", write),
        ("cache_read_input_tokens", read),
    ] {
        if let Some(value) = value {
            details.push(ProviderNumericDetail::new(label, value).map_err(usage_problem)?);
        }
    }
    let input = InputTokenUsage::disjoint(uncached, read, write).map_err(usage_problem)?;
    let usage = ProviderUsage::new(input, None, None, None, &details).map_err(usage_problem)?;
    Ok(Some(Delta::Usage(usage)))
}

fn number(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

fn usage_problem(problem: UsageError) -> ProviderError {
    ProviderError::Protocol {
        provider: NAME,
        problem: format!("invalid usage accounting: {problem}").into(),
    }
}

/// The model saying it has stopped, and why.
fn stopped(payload: &Value) -> Option<Delta> {
    let reason = payload
        .get("delta")
        .and_then(|delta| text(delta, "stop_reason"))?;

    Some(Delta::Stopped(match reason {
        "tool_use" => StopReason::WantsTools,

        // An answer that ran out of room to finish in. The turn goes on around
        // it, and asking for less is what gets a whole one.
        "max_tokens" => StopReason::OutOfTokens,

        // And a request that did not fit at all, which produced no answer. Its
        // remedy is the opposite direction — the session has to get smaller
        // before the same question can be asked again — so the two are never
        // folded together however alike the words look.
        "model_context_window_exceeded" => StopReason::WindowExceeded,

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
    use crate::fake::{disjoint_input_usage, output_usage};

    fn event(name: &str, data: &str) -> SseEvent {
        SseEvent {
            name: name.to_owned(),
            data: data.to_owned(),
        }
    }

    /// The one delta an event meant, for the events that mean at most one.
    fn of(name: &str, data: &str) -> Option<Delta> {
        let mut meant = deltas(&event(name, data)).unwrap();

        assert!(meant.len() <= 1, "{name} meant more than one delta");
        meant.pop()
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
            let problem = deltas(&event("content_block_start", half)).unwrap_err();

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
    fn the_two_ceilings_are_told_apart_because_only_one_of_them_is_recoverable() {
        // An answer that ran out of room is finished early and the turn goes
        // on. A request that did not fit is a turn that cannot proceed at all
        // until the session is made smaller — opposite failures, and folding
        // them together means the recoverable one gets no remedy and the other
        // gets the wrong one.
        assert_eq!(
            of("message_delta", r#"{"delta":{"stop_reason":"max_tokens"}}"#),
            Some(Delta::Stopped(StopReason::OutOfTokens))
        );
        assert_eq!(
            of(
                "message_delta",
                r#"{"delta":{"stop_reason":"model_context_window_exceeded"}}"#
            ),
            Some(Delta::Stopped(StopReason::WindowExceeded))
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
    fn the_event_that_opens_a_response_says_what_the_request_carried() {
        // `message_start` was read as a bracket around content and skipped
        // whole. It is where this endpoint puts the other half of the usage
        // object, and that half is what says how full the window is.
        assert_eq!(
            deltas(&event(
                "message_start",
                r#"{"message":{"id":"msg_1","usage":{"input_tokens":1200}}}"#,
            ))
            .unwrap(),
            vec![disjoint_input_usage(Some(1200), None, None)]
        );
    }

    #[test]
    fn cached_input_is_part_of_what_an_anthropic_request_carried() {
        // These are disjoint usage buckets in the Messages API. Counting only
        // `input_tokens` makes a request served mostly from cache look almost
        // empty even though cache reads and writes occupy the same window.
        assert_eq!(
            deltas(&event(
                "message_start",
                r#"{"message":{"id":"msg_1","usage":{"input_tokens":200,"cache_creation_input_tokens":300,"cache_read_input_tokens":700}}}"#,
            ))
            .unwrap(),
            vec![disjoint_input_usage(Some(200), Some(300), Some(700))]
        );
    }

    #[test]
    fn an_opening_event_that_says_nothing_about_the_request_reports_nothing() {
        // The same rule the cost already keeps: nothing here invents a zero. A
        // request whose size nobody reported and one that carried nothing are
        // different facts, and compaction is decided on this one.
        assert_eq!(
            deltas(&event("message_start", r#"{"message":{"id":"msg_1"}}"#)).unwrap(),
            vec![]
        );
    }

    #[test]
    fn what_a_response_has_cost_arrives_while_it_is_still_being_written() {
        // This endpoint sends the counts more than once, and an interim reading
        // carries no reason beside it. Read as a stop it would end the response
        // in the middle of the answer; skipped, the number nobody is watching a
        // long turn by only arrives once the turn is over.
        assert_eq!(
            deltas(&event("message_delta", r#"{"usage":{"output_tokens":12}}"#)).unwrap(),
            vec![output_usage(12)]
        );
    }

    #[test]
    fn the_event_that_ends_a_response_says_what_it_cost_before_saying_it_ended() {
        // Both in one event. The cost goes first because the stop is the thing a
        // reader is entitled to treat as the last word.
        assert_eq!(
            deltas(&event(
                "message_delta",
                r#"{"delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":40}}"#,
            ))
            .unwrap(),
            vec![output_usage(40), Delta::Stopped(StopReason::Yielded),]
        );
    }

    #[test]
    fn a_response_that_says_nothing_about_what_it_cost_is_not_reported_as_free() {
        // Nothing here invents a zero. A turn whose provider never sends the
        // counts says nothing about them, which is what lets the row above the
        // box leave the segment out rather than draw a number that is wrong.
        assert_eq!(
            of("message_delta", r#"{"delta":{"stop_reason":"end_turn"}}"#),
            Some(Delta::Stopped(StopReason::Yielded))
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
        let problem = deltas(&event(
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
        let problem = deltas(&event("content_block_delta", "not json at all")).unwrap_err();

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
