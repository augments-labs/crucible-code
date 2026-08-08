//! What one chunk of the wire means.
//!
//! Separate from the parser next door only because the parser reached the
//! per-file cap. Everything here is about `deltas` and the two functions under
//! it.

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
fn a_call_carrying_half_an_identity_is_refused_rather_than_guessed() {
    // Either half alone leaves the fragments that follow with nowhere they
    // provably belong, and the parser would append them to whichever call was
    // open before. That is one tool running on another tool's arguments —
    // a `write` given a `read`'s path — rather than a failure anyone can see.
    for half in [
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_1","function":{"arguments":""}}]}}]}"#,
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"name":"read","arguments":""}}]}}]}"#,
    ] {
        let problem = deltas(&event(half)).unwrap_err();

        assert!(
            matches!(problem, ProviderError::Protocol { .. }),
            "expected a protocol failure, got {problem:?}"
        );
    }
}

#[test]
fn arguments_for_a_call_the_chunk_did_not_open_are_refused() {
    // Fragments carry no identity of their own, so they are assembled onto the
    // call opened last. A chunk that opens one call and then carries arguments
    // under a different index is the case where that rule is visibly wrong, and
    // the arguments would land on the call above.
    let problem = deltas(&event(
        r#"{"choices":[{"index":0,"delta":{"tool_calls":[
                {"index":0,"id":"call_1","function":{"name":"read","arguments":""}},
                {"index":1,"function":{"arguments":"{\"path\":\"a.rs\"}"}}]}}]}"#,
    ))
    .unwrap_err();

    assert!(
        matches!(problem, ProviderError::Protocol { .. }),
        "expected a protocol failure, got {problem:?}"
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
fn a_withheld_answer_is_not_reported_as_a_finished_one() {
    // Named apart from the ceiling rather than folded into `stop`, because the
    // two have opposite remedies: a shorter question fixes a ceiling and buys
    // nothing here. Falling through to the catch-all would report the answer
    // the filter cut as one the model chose to end.
    assert_eq!(
        of(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"content_filter"}]}"#),
        vec![Delta::Stopped(StopReason::Filtered)]
    );
}

#[test]
fn a_refusal_is_shown_rather_than_left_as_a_silent_finish() {
    // A model that declines writes it here and leaves `content` null for the
    // whole response. Read only from `content`, the turn shows nothing at all
    // and then reports that it finished normally.
    let out = of(
        r#"{"choices":[{"index":0,"delta":{"content":null,"refusal":"I can't help with that."},"finish_reason":"stop"}]}"#,
    );

    assert_eq!(
        out,
        vec![
            Delta::Text("I can't help with that.".into()),
            Delta::Stopped(StopReason::Yielded),
        ]
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
