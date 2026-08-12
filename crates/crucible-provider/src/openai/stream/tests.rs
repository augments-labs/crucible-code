//! What a whole response does, chunk after chunk.
//!
//! Separate from the stream next door only because it reached the per-file cap.
//! Everything here is about `Stream` and the queue under it.

use crucible_core::ToolId;

use super::*;

/// A complete answer, as the API streams one.
pub(in crate::openai) const ANSWER: &str = concat!(
    "data: {\"id\":\"chatcmpl_1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n",
    "data: {\"id\":\"chatcmpl_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hello\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl_1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\", world\"},\"finish_reason\":null}]}\n\n",
    "data: {\"id\":\"chatcmpl_1\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
    "data: [DONE]\n\n",
);

/// A stream over recorded bytes.
fn reading(body: &str, cancel: &Cancel) -> Stream {
    Stream::new(
        Box::new(std::io::Cursor::new(body.to_owned().into_bytes())),
        cancel.clone(),
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
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"type\":\"function\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"a.rs\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let mut stream = reading(body, &Cancel::new());

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
    // The ordinary shape of two calls: each is opened and then filled in
    // before the next one starts, and the index every fragment carries is
    // the one just opened.
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"glob\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"function\":{\"arguments\":\"{\\\"pattern\\\":\\\"*.rs\\\"}\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: [DONE]\n\n",
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
    // A fragment names its call by an index and nothing else, and the
    // runner assembles it onto whichever call was opened last. A server
    // that goes back to an earlier index is where that is visibly wrong —
    // and which call is open is not something one chunk can see, so this is
    // the guard that has to outlive one.
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":1,\"id\":\"call_2\",\"function\":{\"name\":\"glob\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"{\\\"path\\\":\\\"a.rs\\\"}\"}}]}}]}\n\n",
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
    // with no data line at all. Read as a chunk it is empty rather than
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
fn a_chunk_worth_several_deltas_is_delivered_one_at_a_time() {
    // The queue is the only part of this file the other protocol does not
    // need, so it is the part worth watching.
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"!\"},\"finish_reason\":\"stop\"}]}\n\n",
        "data: [DONE]\n\n",
    );
    let mut stream = reading(body, &Cancel::new());

    assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("!".into()));
    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::Stopped(StopReason::Yielded)
    );
    assert!(stream.next().is_none());
}

#[test]
fn a_failure_reported_mid_stream_stops_the_stream() {
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n",
        "data: {\"error\":{\"type\":\"server_error\",\"message\":\"The server had an error\"}}\n\n",
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
    let body = "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"}}]}\n\n";
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
fn what_a_chunk_already_yielded_survives_the_cancel_that_follows_it() {
    // The deltas are off the socket and in the queue by the time the user
    // asks to stop, so dropping them loses part of an answer that had
    // already arrived — and a tool call lost this way leaves the transcript
    // holding arguments for a call it never opened.
    let body = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"content\":\"one moment\",\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"read\",\"arguments\":\"\"}}]}}]}\n\n",
        "data: [DONE]\n\n",
    );
    let cancel = Cancel::new();
    let mut stream = reading(body, &cancel);

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::Text("one moment".into())
    );
    cancel.request();

    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::ToolStarted {
            id: ToolId::new("call_1"),
            name: "read".into(),
        },
        "the queue was thrown away instead of drained"
    );
    assert_eq!(
        stream.next().unwrap().unwrap(),
        Delta::Stopped(StopReason::Cancelled)
    );
    assert!(stream.next().is_none());
}

#[test]
fn a_stream_shows_no_response_content_in_its_debug() {
    // It is held by the runner for the length of a turn, so it appears in
    // whatever that prints.
    let shown = format!("{:?}", reading(ANSWER, &Cancel::new()));

    assert!(!shown.contains("Hello"), "the response leaked: {shown}");
}
