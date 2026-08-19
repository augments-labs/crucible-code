//! What one event means, event shape by event shape.
//!
//! Separate from the parser next door only because the parser reached the
//! per-file cap.

use super::*;

/// One event, as the framing above hands it over.
///
/// The name is left empty throughout: this endpoint repeats the type inside
/// every payload, and the payload is what the parser reads. A test that set
/// both could pass while the parser read a field nothing sends.
fn event(data: &str) -> SseEvent {
    SseEvent {
        name: String::new(),
        data: data.to_owned(),
    }
}

/// What one event yields, with nothing open before it.
fn out(data: &str) -> Vec<Delta> {
    deltas(&event(data), &mut Responses::default()).expect("an event that parses")
}

/// A response finishing, repeating back whatever `output` says.
///
/// The two services this endpoint serves differ here and the parser may not:
/// the published API lists every item the response produced, and the backend a
/// `ChatGPT` plan is served by lists none of them.
fn completed(output: &str) -> String {
    format!(r#"{{"type":"response.completed","response":{{"output":{output}}}}}"#)
}

/// A tool call opening, with the two identities this endpoint gives one.
const OPENED: &str = r#"{"type":"response.output_item.added","output_index":0,
    "item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read","arguments":""}}"#;

#[test]
fn a_fragment_of_the_answer_is_text() {
    assert_eq!(
        out(r#"{"type":"response.output_text.delta","delta":"Hello"}"#),
        vec![Delta::Text("Hello".into())]
    );
}

#[test]
fn an_empty_fragment_says_nothing() {
    // The opening fragment of some responses carries nothing. Emitting it
    // would put a blank line in front of every answer.
    assert!(out(r#"{"type":"response.output_text.delta","delta":""}"#).is_empty());
}

#[test]
fn a_refusal_is_shown_rather_than_swallowed() {
    // A model that declines produces no output text at all. Unread, a refusal
    // is a turn that shows nothing and then reports that it finished.
    assert_eq!(
        out(r#"{"type":"response.refusal.delta","delta":"I can't help with that"}"#),
        vec![Delta::Text("I can't help with that".into())]
    );
}

#[test]
fn a_tool_call_opens_under_the_identity_a_result_is_answered_against() {
    // Two identities and they are not interchangeable: `id` keys the
    // fragments, `call_id` is what the next turn's result names. Sending the
    // wrong one back is a result the vendor cannot match to a call.
    assert_eq!(
        out(OPENED),
        vec![Delta::ToolStarted {
            id: ToolId::new("call_1"),
            name: "read".into(),
        }]
    );
}

#[test]
fn a_message_opening_is_not_a_tool_call() {
    let said = r#"{"type":"response.output_item.added","output_index":0,
        "item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}"#;

    assert!(out(said).is_empty());
}

#[test]
fn reasoning_is_narrated_and_never_drawn() {
    // The item exists on every turn these models take. It is not the answer,
    // and a trace nobody asked for is worse than none.
    let thinking = r#"{"type":"response.output_item.added","output_index":0,
        "item":{"type":"reasoning","id":"rs_1","summary":[]}}"#;

    assert!(out(thinking).is_empty());
    assert!(out(r#"{"type":"response.reasoning_text.delta","delta":"hmm"}"#).is_empty());
}

#[test]
fn arguments_follow_the_call_they_were_opened_under() {
    let mut response = Responses::default();

    deltas(&event(OPENED), &mut response).expect("a call opens");
    let fragment = r#"{"type":"response.function_call_arguments.delta",
        "item_id":"fc_1","delta":"{\"path\":"}"#;

    assert_eq!(
        deltas(&event(fragment), &mut response).expect("a fragment of it"),
        vec![Delta::ToolArgs("{\"path\":".into())]
    );
}

#[test]
fn arguments_for_a_call_that_is_not_open_are_refused() {
    // Assembled anyway they would be one tool running on another tool's
    // arguments, which is a failure nobody can see.
    let mut response = Responses::default();
    deltas(&event(OPENED), &mut response).expect("a call opens");

    let elsewhere = r#"{"type":"response.function_call_arguments.delta",
        "item_id":"fc_2","delta":"{}"}"#;
    let problem = deltas(&event(elsewhere), &mut response).expect_err("a fragment of another call");

    assert!(
        matches!(problem, ProviderError::Protocol { .. }),
        "{problem:?}"
    );
}

#[test]
fn arguments_arriving_before_any_call_are_refused() {
    let orphan = r#"{"type":"response.function_call_arguments.delta",
        "item_id":"fc_1","delta":"{}"}"#;

    let problem = deltas(&event(orphan), &mut Responses::default())
        .expect_err("a fragment of nothing at all");

    assert!(
        matches!(problem, ProviderError::Protocol { .. }),
        "{problem:?}"
    );
}

#[test]
fn a_finished_call_does_not_repeat_the_arguments_that_were_streamed() {
    // The finished item carries the whole argument text. Emitted after the
    // fragments it would double them, and the JSON the model wrote would not
    // parse.
    let mut response = Responses::default();
    deltas(&event(OPENED), &mut response).expect("a call opens");
    deltas(
        &event(
            r#"{"type":"response.function_call_arguments.delta",
                "item_id":"fc_1","delta":"{\"path\":\"a.rs\"}"}"#,
        ),
        &mut response,
    )
    .expect("its arguments");

    let done = r#"{"type":"response.output_item.done","output_index":0,
        "item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read",
                "arguments":"{\"path\":\"a.rs\"}"}}"#;

    assert!(
        deltas(&event(done), &mut response)
            .expect("the call finishing")
            .is_empty()
    );
}

#[test]
fn a_finished_call_whose_arguments_never_streamed_supplies_them() {
    // A server that narrates only the ends of things sends no fragments at
    // all. Without this the tool would run on no arguments.
    let mut response = Responses::default();
    deltas(&event(OPENED), &mut response).expect("a call opens");

    let done = r#"{"type":"response.output_item.done","output_index":0,
        "item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read",
                "arguments":"{\"path\":\"a.rs\"}"}}"#;

    assert_eq!(
        deltas(&event(done), &mut response).expect("the call finishing"),
        vec![Delta::ToolArgs("{\"path\":\"a.rs\"}".into())]
    );
}

#[test]
fn a_call_finishing_that_is_not_the_one_open_is_refused() {
    // The finished item carries the whole argument text, and taken on trust it
    // would be emitted under whichever call is open — one tool running on
    // another tool's arguments, and read against that other call's `streamed`
    // flag, so the arguments arrive twice or not at all.
    let mut response = Responses::default();
    deltas(&event(OPENED), &mut response).expect("a call opens");

    let elsewhere = r#"{"type":"response.output_item.done","output_index":1,
        "item":{"type":"function_call","id":"fc_2","call_id":"call_2","name":"write",
                "arguments":"{\"path\":\"b.rs\"}"}}"#;
    let problem = deltas(&event(elsewhere), &mut response).expect_err("another call finishing");

    assert!(
        matches!(problem, ProviderError::Protocol { .. }),
        "{problem:?}"
    );
}

#[test]
fn a_call_finishing_with_nothing_open_is_refused() {
    let done = r#"{"type":"response.output_item.done","output_index":0,
        "item":{"type":"function_call","id":"fc_1","call_id":"call_1","name":"read",
                "arguments":"{}"}}"#;

    let problem =
        deltas(&event(done), &mut Responses::default()).expect_err("a call nothing ever opened");

    assert!(
        matches!(problem, ProviderError::Protocol { .. }),
        "{problem:?}"
    );
}

#[test]
fn a_message_finishing_is_not_a_tool_call_and_leaves_the_open_one_alone() {
    // The response narrates the end of every item, and a call is opened before
    // the message beside it is closed. Read as a call, that would end the open
    // one and the fragments still to come would have nothing to belong to.
    let mut response = Responses::default();
    deltas(&event(OPENED), &mut response).expect("a call opens");

    let said = r#"{"type":"response.output_item.done","output_index":0,
        "item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}"#;

    assert!(
        deltas(&event(said), &mut response)
            .expect("a message finishing")
            .is_empty()
    );

    let fragment = r#"{"type":"response.function_call_arguments.delta",
        "item_id":"fc_1","delta":"{}"}"#;
    assert_eq!(
        deltas(&event(fragment), &mut response).expect("the open call's arguments"),
        vec![Delta::ToolArgs("{}".into())]
    );
}

#[test]
fn a_tool_call_carrying_half_an_identity_is_refused_rather_than_skipped() {
    // Skipped, nothing opens: the fragments that follow are assembled onto the
    // call before it, the half-announced call leaves no trace, and the turn
    // ends looking like a clean finish with a tool the model asked for never
    // run.
    for half in [
        r#"{"type":"response.output_item.added",
            "item":{"type":"function_call","call_id":"call_1","name":"read"}}"#,
        r#"{"type":"response.output_item.added",
            "item":{"type":"function_call","id":"fc_1","name":"read"}}"#,
        r#"{"type":"response.output_item.added",
            "item":{"type":"function_call","id":"fc_1","call_id":"call_1"}}"#,
    ] {
        let problem = deltas(&event(half), &mut Responses::default()).expect_err("half a call");

        assert!(
            matches!(problem, ProviderError::Protocol { .. }),
            "{problem:?}"
        );
    }
}

#[test]
fn a_response_that_asked_for_nothing_yielded() {
    let output = r#"[{"type":"message","role":"assistant","content":[]}]"#;

    assert_eq!(
        out(&completed(output)),
        vec![Delta::Stopped(StopReason::Yielded)]
    );
}

#[test]
fn a_response_that_asked_for_a_tool_wants_tools() {
    // There is no field for it: both shapes complete, and what tells them apart
    // is whether a call was asked for along the way. Read the wrong way the turn
    // would end with the tool the model asked for never run.
    let mut response = Responses::default();
    deltas(&event(OPENED), &mut response).expect("a call opens");

    let listed = r#"[{"type":"reasoning","id":"rs_1"},
                     {"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}]"#;

    assert_eq!(
        deltas(&event(&completed(listed)), &mut response).expect("the response finishing"),
        vec![Delta::Stopped(StopReason::WantsTools)]
    );
}

#[test]
fn a_call_a_response_finishes_without_listing_still_wants_tools() {
    // What the backend a plan is served by sends: every item narrated, and then
    // a finish that lists none of them. Read from the list, every tool call a
    // plan makes is a clean finish — the call is streamed, the turn is told it
    // is over, the tool never runs, and the user sees a turn that drew nothing
    // at all.
    let mut response = Responses::default();
    deltas(&event(OPENED), &mut response).expect("a call opens");

    assert_eq!(
        deltas(&event(&completed("[]")), &mut response).expect("the response finishing"),
        vec![Delta::Stopped(StopReason::WantsTools)]
    );
}

#[test]
fn a_response_cut_short_says_which_way_it_was_cut() {
    // Neither is a finish. An answer stopped by a ceiling or withheld by a
    // filter reads as a complete one unless the turn says otherwise.
    let ceiling = r#"{"type":"response.incomplete","response":
        {"incomplete_details":{"reason":"max_output_tokens"}}}"#;
    let filtered = r#"{"type":"response.incomplete","response":
        {"incomplete_details":{"reason":"content_filter"}}}"#;

    assert_eq!(out(ceiling), vec![Delta::Stopped(StopReason::OutOfTokens)]);
    assert_eq!(out(filtered), vec![Delta::Stopped(StopReason::Filtered)]);
}

#[test]
fn a_reason_this_build_has_not_heard_of_is_still_not_a_finish() {
    // The response already said it is incomplete. The only question left is
    // which way to say so, and a finish is the one answer that is wrong.
    let strange = r#"{"type":"response.incomplete","response":
        {"incomplete_details":{"reason":"something-new"}}}"#;

    assert_eq!(out(strange), vec![Delta::Stopped(StopReason::OutOfTokens)]);
}

#[test]
fn what_a_response_cost_arrives_under_the_response_that_finished() {
    // Once, at the end, and inside the response rather than beside it. The cost
    // goes first because the stop is the thing a reader is entitled to treat as
    // the last word.
    let done = r#"{"type":"response.completed","response":
        {"output":[],"usage":{"input_tokens":900,"output_tokens":58}}}"#;

    assert_eq!(
        out(done),
        vec![
            Delta::Carried(Carried::new(900)),
            Delta::Spent(Spend::new(58)),
            Delta::Stopped(StopReason::Yielded),
        ]
    );
}

#[test]
fn a_response_cut_short_still_says_what_it_cost() {
    // Tokens produced before a ceiling stopped the answer are tokens produced.
    // Read only off a clean finish, the truncated turn is the one that reports
    // having cost nothing.
    let cut = r#"{"type":"response.incomplete","response":
        {"incomplete_details":{"reason":"max_output_tokens"},"usage":{"output_tokens":4096}}}"#;

    assert_eq!(
        out(cut),
        vec![
            Delta::Spent(Spend::new(4096)),
            Delta::Stopped(StopReason::OutOfTokens),
        ]
    );
}

#[test]
fn a_response_that_failed_carries_what_the_provider_said() {
    let said = r#"{"type":"response.failed","response":{"error":
        {"code":"server_error","message":"the model is overloaded"}}}"#;

    let problem = deltas(&event(said), &mut Responses::default()).expect_err("a failure");

    assert_eq!(
        problem.to_string(),
        "openai: server_error: the model is overloaded"
    );
}

#[test]
fn a_null_error_is_an_absent_one_and_the_response_says_the_rest() {
    // `"error": null` is a field that is there and holds nothing, which is not
    // the same as a field that is absent — and a code and a message read off it
    // find neither. This came out as "did not say what" for a response that had
    // said its status and why it stopped.
    let said = r#"{"type":"response.failed","response":{"status":"failed",
        "error":null,"incomplete_details":{"reason":"unsupported reasoning.effort"}}}"#;

    let problem = deltas(&event(said), &mut Responses::default()).expect_err("a failure");

    assert_eq!(
        problem.to_string(),
        "openai: failed: unsupported reasoning.effort"
    );
}

#[test]
fn a_failure_the_provider_named_nothing_about_says_where_to_look() {
    // Nothing under `error`, nothing under `incomplete_details`. What is left
    // has to be a sentence somebody can act on, and the likeliest thing behind
    // a response failing silently is a request asking for what this model does
    // not serve — which is the one thing crucible can point at itself.
    let said = r#"{"type":"response.failed","response":{"status":"failed"}}"#;

    let problem = deltas(&event(said), &mut Responses::default()).expect_err("a failure");

    assert_eq!(
        problem.to_string(),
        "openai: failed: gave up on the response and named no reason; \
         check that this model serves what was asked of it"
    );
}

#[test]
fn a_failure_outside_any_response_arrives_flat() {
    let said = r#"{"type":"error","code":"rate_limit_exceeded","message":"slow down"}"#;

    let problem = deltas(&event(said), &mut Responses::default()).expect_err("a failure");

    assert_eq!(
        problem.to_string(),
        "openai: rate_limit_exceeded: slow down"
    );
}

#[test]
fn an_event_this_build_has_not_heard_of_is_ignored() {
    // Vendors add events. A stream that failed on one would fail every turn
    // at the last moment, the day a field is added.
    assert!(out(r#"{"type":"response.something.new","delta":"x"}"#).is_empty());
    assert!(out(r#"{"type":"response.created","response":{"id":"resp_1"}}"#).is_empty());
}

#[test]
fn a_keep_alive_with_nothing_in_it_is_not_a_failure() {
    // A proxy may spell a heartbeat any way it likes and may send it with no
    // data at all. Read as an event it fails the turn and discards the answer
    // that had already arrived.
    for blank in ["", "   ", "\n"] {
        assert!(out(blank).is_empty(), "{blank:?}");
    }
}

#[test]
fn an_event_that_is_not_json_is_a_protocol_failure_naming_no_payload() {
    let problem =
        deltas(&event("not json at all"), &mut Responses::default()).expect_err("a failure");

    assert!(
        matches!(problem, ProviderError::Protocol { .. }),
        "{problem:?}"
    );
    assert!(
        !problem.to_string().contains("not json at all"),
        "the payload reached the user: {problem}"
    );
}

#[test]
fn a_finished_response_says_what_the_request_carried() {
    // The other half of the same usage object, and the half that says how full
    // the window is. It goes before the cost, which goes before the stop: what
    // was sent, what came back, that it ended.
    let done = r#"{"type":"response.completed","response":
        {"output":[],"usage":{"input_tokens":900,"output_tokens":58}}}"#;

    assert_eq!(
        out(done),
        vec![
            Delta::Carried(Carried::new(900)),
            Delta::Spent(Spend::new(58)),
            Delta::Stopped(StopReason::Yielded),
        ]
    );
}
