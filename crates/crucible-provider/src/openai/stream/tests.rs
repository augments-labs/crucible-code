//! What a whole response does, event after event.
//!
//! Separate from the stream next door only because it reached the per-file cap.
//! Everything here is about `Stream` and the queue under it.

use crucible_core::{Cancel, Delta, DeltaStream, ProviderError, StopReason, ToolId};

use super::*;
use crate::transport::{Paused, Said};

/// One delta, and then the model stops talking.
const HALF: &str =
    "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"Hel\"}\n\n";

/// A complete answer, as the API streams one.
pub(in crate::openai) const ANSWER: &str = concat!(
    "event: response.created\n",
    "data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_1\",\"status\":\"in_progress\"}}\n\n",
    "event: response.output_item.added\n",
    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"message\",\"id\":\"msg_1\",\"role\":\"assistant\",\"content\":[]}}\n\n",
    "event: response.output_text.delta\n",
    "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"Hello\"}\n\n",
    "event: response.output_text.delta\n",
    "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\", world\"}\n\n",
    "event: response.completed\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Hello, world\"}]}]}}\n\n",
);

/// One tool call, opened and filled in and finished.
const CALLED: &str = concat!(
    "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"\"}}\n\n",
    "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"path\\\":\"}\n\n",
    "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"\\\"a.rs\\\"}\"}\n\n",
    "data: {\"type\":\"response.function_call_arguments.done\",\"item_id\":\"fc_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}\n\n",
    "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}}\n\n",
    "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}]}}\n\n",
);

/// A stream over recorded bytes.
fn reading(body: &str, cancel: &Cancel) -> Stream {
    Stream::new(
        Box::new(std::io::Cursor::new(body.to_owned().into_bytes())),
        cancel.clone(),
        crucible_core::Redactions::default(),
    )
}

/// Every delta a response produces.
pub(in crate::openai) fn deltas(stream: &mut dyn DeltaStream) -> Vec<Delta> {
    let mut out = Vec::new();
    while let Some(delta) = stream.next() {
        out.push(delta.unwrap());
    }
    out
}

#[test]
fn an_answer_arrives_as_text_and_a_stop() {
    let mut stream = reading(ANSWER, &Cancel::new());

    assert_eq!(
        deltas(&mut stream),
        vec![
            Delta::Text("Hello".into()),
            Delta::Text(", world".into()),
            Delta::Stopped(StopReason::Yielded),
        ]
    );
}

#[test]
fn a_tool_call_arrives_named_and_then_in_fragments() {
    let mut stream = reading(CALLED, &Cancel::new());

    assert_eq!(
        deltas(&mut stream),
        vec![
            Delta::ToolStarted {
                id: ToolId::new("call_1"),
                name: "read".into(),
            },
            Delta::ToolArgs("{\"path\":".into()),
            Delta::ToolArgs("\"a.rs\"}".into()),
            Delta::Stopped(StopReason::WantsTools),
        ]
    );
}

#[test]
fn a_second_call_takes_the_fragments_that_follow_it() {
    // The ordinary shape of two calls: each is opened, filled in and finished
    // before the next one starts.
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_2\",\"name\":\"glob\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_2\",\"delta\":\"{\\\"pattern\\\":\\\"*.rs\\\"}\"}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_2\",\"name\":\"glob\",\"arguments\":\"{\\\"pattern\\\":\\\"*.rs\\\"}\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"call_1\"},{\"type\":\"function_call\",\"call_id\":\"call_2\"}]}}\n\n",
    );
    let mut stream = reading(body, &Cancel::new());

    assert_eq!(
        deltas(&mut stream),
        vec![
            Delta::ToolStarted {
                id: ToolId::new("call_1"),
                name: "read".into(),
            },
            Delta::ToolArgs("{\"path\":\"a.rs\"}".into()),
            Delta::ToolStarted {
                id: ToolId::new("call_2"),
                name: "glob".into(),
            },
            Delta::ToolArgs("{\"pattern\":\"*.rs\"}".into()),
            Delta::Stopped(StopReason::WantsTools),
        ]
    );
}

#[test]
fn arguments_for_a_call_the_response_no_longer_has_open_are_refused() {
    // Which call is open is not something one event can see, so this is the
    // guard that has to outlive one. Assembled anyway, the fragment would run
    // one tool on another tool's arguments.
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"read\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_item.added\",\"output_index\":1,\"item\":{\"type\":\"function_call\",\"id\":\"fc_2\",\"call_id\":\"call_2\",\"name\":\"glob\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_1\",\"delta\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}\n\n",
    );
    let mut stream = reading(body, &Cancel::new());

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::ToolStarted {
            id: ToolId::new("call_1"),
            name: "read".into(),
        }
    );
    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::ToolStarted {
            id: ToolId::new("call_2"),
            name: "glob".into(),
        }
    );

    let problem = stream.next().unwrap().unwrap_err();

    assert!(
        matches!(problem, ProviderError::Protocol { .. }),
        "a fragment was assembled onto the wrong call instead of refused: {problem:?}"
    );
    assert!(stream.next().is_none());
}

#[test]
fn a_heartbeat_with_no_payload_does_not_end_the_turn() {
    // A proxy's keep-alive can be spelled any way it likes and can arrive
    // with no data line at all. Read as an event it is empty rather than
    // JSON, which would fail the turn and discard the answer so far.
    let mut stream = reading(&format!("event: keep-alive\n\n{ANSWER}"), &Cancel::new());

    assert_eq!(
        deltas(&mut stream),
        vec![
            Delta::Text("Hello".into()),
            Delta::Text(", world".into()),
            Delta::Stopped(StopReason::Yielded),
        ]
    );
}

#[test]
fn an_event_worth_several_deltas_is_delivered_one_at_a_time() {
    // The queue is the only part of this file the other protocol does not
    // need, so it is the part worth watching. One event opens the call and
    // the next finishes it with the whole of its arguments.
    let body = concat!(
        "data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"clock\",\"arguments\":\"\"}}\n\n",
        "data: {\"type\":\"response.output_item.done\",\"output_index\":0,\"item\":{\"type\":\"function_call\",\"id\":\"fc_1\",\"call_id\":\"call_1\",\"name\":\"clock\",\"arguments\":\"{}\"}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"function_call\",\"call_id\":\"call_1\"}]}}\n\n",
    );
    let mut stream = reading(body, &Cancel::new());

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::ToolStarted {
            id: ToolId::new("call_1"),
            name: "clock".into(),
        }
    );
    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::ToolArgs("{}".into())
    );
    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::Stopped(StopReason::WantsTools)
    );
    assert!(stream.next().is_none());
}

#[test]
fn a_failure_reported_mid_stream_stops_the_stream() {
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"item_id\":\"msg_1\",\"delta\":\"Hel\"}\n\n",
        "data: {\"type\":\"error\",\"code\":\"server_error\",\"message\":\"The server had an error\"}\n\n",
    );
    let mut stream = reading(body, &Cancel::new());

    assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("Hel".into()));
    let problem = stream.next().unwrap().unwrap_err();

    assert_eq!(
        problem.to_string(),
        "openai: server_error: The server had an error"
    );
    assert!(
        stream.next().is_none(),
        "the stream continued past a failure"
    );
}

#[test]
fn a_response_that_stops_arriving_is_a_failure_and_not_a_finished_turn() {
    // Half an answer and then silence looks identical to a complete one
    // from the socket's side. Only the missing stop reason says otherwise.
    let body = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\"}\n\n";
    let mut stream = reading(body, &Cancel::new());

    assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("Hel".into()));
    let problem = stream.next().unwrap().unwrap_err();

    assert!(
        matches!(problem, ProviderError::Transport { .. }),
        "expected a truncated response to be reported, got {problem:?}"
    );
    assert!(stream.next().is_none());
}

#[test]
fn a_complete_response_ends_without_inventing_a_failure() {
    let mut stream = reading(ANSWER, &Cancel::new());

    deltas(&mut stream);

    assert!(stream.next().is_none());
}

#[test]
fn cancelling_mid_answer_stops_the_stream_rather_than_failing_it() {
    // The user asked; nothing went wrong. The runner keeps what arrived.
    let cancel = Cancel::new();
    let mut stream = reading(ANSWER, &cancel);

    assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("Hello".into()));
    cancel.request();

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::Stopped(StopReason::Cancelled)
    );
    assert!(stream.next().is_none(), "the stream continued after a stop");
}

#[test]
fn a_call_cancelled_between_its_name_and_its_arguments_is_not_left_half_open() {
    // The transcript would otherwise hold a call with no arguments, and the
    // next turn would send the model a call it never finished making. The stop
    // is what closes it.
    let cancel = Cancel::new();
    let mut stream = reading(CALLED, &cancel);

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::ToolStarted {
            id: ToolId::new("call_1"),
            name: "read".into(),
        }
    );
    cancel.request();

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::Stopped(StopReason::Cancelled)
    );
    assert!(stream.next().is_none());
}

#[test]
fn a_cancel_raised_while_nothing_is_arriving_stops_the_stream() {
    // The provider went quiet mid-answer and the user pressed stop. Raised from
    // inside the wait, because that is where a user raises one and the only
    // place the answer was ever in doubt: a cancel seen before the read is one
    // the read never had to be interrupted for.
    let cancel = Cancel::new();
    let raise = cancel.clone();
    let silent = Paused::saying([
        Said::Bytes(HALF.into()),
        Said::Nothing,
        Said::Nothing,
        Said::Nothing,
    ])
    .meanwhile(move || raise.request());
    let mut stream = Stream::new(
        Box::new(silent),
        cancel,
        crucible_core::Redactions::default(),
    );

    assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("Hel".into()));

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::Stopped(StopReason::Cancelled),
        "the stream waited out a silent provider with a cancel raised"
    );
    assert!(stream.next().is_none());
}

#[test]
fn a_response_that_pauses_while_the_model_thinks_is_not_a_failed_turn() {
    // The regression a bounded wait buys its promptness with, if the wait
    // expiring is read as a connection that broke: every pause in a long answer
    // becomes a failed turn. Nothing here fails, and this body pauses between
    // every five bytes of itself.
    let mut stream = Stream::new(
        Box::new(Paused::dawdling(ANSWER, 5)),
        Cancel::new(),
        crucible_core::Redactions::default(),
    );

    assert_eq!(
        deltas(&mut stream),
        vec![
            Delta::Text("Hello".into()),
            Delta::Text(", world".into()),
            Delta::Stopped(StopReason::Yielded),
        ]
    );
}

#[test]
fn a_stream_shows_no_response_content_in_its_debug() {
    // It is held by the runner for the length of a turn, so it appears in
    // whatever that prints.
    let shown = format!("{:?}", reading(ANSWER, &Cancel::new()));

    assert!(!shown.contains("Hello"), "the response leaked: {shown}");
}
