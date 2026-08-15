//! One Anthropic response, as deltas.
//!
//! The loop belongs to [`crate::stream`] and is the same for every provider.
//! What is this provider's is which events mean something, which lives in
//! [`super::wire`]; what is here is the pairing of the two, and the tests that
//! read a recorded response end to end.

use crate::anthropic::wire::Messages;
use crate::stream::Response;

/// A response being read, as this endpoint narrates one.
pub(super) type Stream = Response<Messages>;

#[cfg(test)]
pub(super) mod tests {
    use crucible_core::{Cancel, Delta, DeltaStream, ProviderError, StopReason, ToolId};

    use super::*;
    use crate::transport::{Paused, Said};

    /// One delta, and then the model stops talking.
    const HALF: &str = "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n";

    /// A complete answer, as the API streams one.
    pub(in crate::anthropic) const ANSWER: &str = concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\"}}\n\n",
        "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        "event: ping\ndata: {\"type\":\"ping\"}\n\n",
        "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hello\"}}\n\n",
        "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\", world\"}}\n\n",
        "event: content_block_stop\ndata: {\"index\":0}\n\n",
        "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":4}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
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
    pub(in crate::anthropic) fn deltas(stream: &mut dyn DeltaStream) -> Vec<Delta> {
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
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\"}}\n\n",
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"\\\"a.rs\\\"}\"}}\n\n",
            "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
        );
        let mut stream = reading(body, &Cancel::new());

        assert_eq!(
            deltas(&mut stream),
            vec![
                Delta::ToolStarted {
                    id: ToolId::new("toolu_1"),
                    name: "read".into(),
                },
                Delta::ToolArgs("{\"path\":".into()),
                Delta::ToolArgs("\"a.rs\"}".into()),
                Delta::Stopped(StopReason::WantsTools),
            ]
        );
    }

    #[test]
    fn a_heartbeat_with_no_payload_does_not_end_the_turn() {
        // A proxy holding the connection open spells its keep-alive however it
        // likes and may send no data line with it. Read as a payload it is
        // empty rather than JSON, which would fail the turn mid-answer and
        // discard everything the model had said in it.
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
    fn a_failure_reported_mid_stream_stops_the_stream() {
        let body = concat!(
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n",
            "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n",
        );
        let mut stream = reading(body, &Cancel::new());

        assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("Hel".into()));
        let problem = stream.next().unwrap().unwrap_err();

        assert_eq!(
            problem.to_string(),
            "anthropic: overloaded_error: Overloaded"
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
        let body = "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"Hel\"}}\n\n";
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
    fn a_cancel_raised_while_nothing_is_arriving_stops_the_stream() {
        // The provider went quiet mid-answer and the user pressed stop. Raised
        // from inside the wait, because that is where a user raises one and the
        // only place the answer was ever in doubt: a cancel seen before the
        // read is one the read never had to be interrupted for.
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
        // expiring is read as a connection that broke: every pause in a long
        // answer becomes a failed turn. Nothing here fails, and this body
        // pauses between every five bytes of itself.
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
}
