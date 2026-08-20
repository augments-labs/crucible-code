//! One `MoonshotAI` response, as deltas.
//!
//! The loop belongs to [`crate::stream`] and is the same for every provider.
//! What is this provider's is which events mean something, which lives in
//! [`super::wire`]; what is here is the pairing of the two, and the tests that
//! read a recorded response end to end.

use crate::moonshot::wire::Completions;
use crate::stream::Response;

/// A response being read, as this endpoint narrates one.
pub(super) type Stream = Response<Completions>;

#[cfg(test)]
pub(super) mod tests {
    use crucible_core::{
        Cancel, Carried, Delta, DeltaStream, ProviderError, Spend, StopReason, ToolId,
    };

    use super::*;

    /// A complete answer, as the API streams one.
    ///
    /// No `event:` lines: this endpoint names nothing and puts every event in
    /// its payload, and the sentinel that closes the stream is not JSON.
    pub(in crate::moonshot) const ANSWER: &str = concat!(
        r#"data: {"id":"chat-1","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"role":"assistant","content":""},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":"Hello"},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{"content":", world"},"finish_reason":null}]}"#,
        "\n\n",
        r#"data: {"id":"chat-1","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]}"#,
        "\n\n",
        r#"data: {"id":"chat-1","choices":[],"usage":{"prompt_tokens":9,"completion_tokens":4}}"#,
        "\n\n",
        "data: [DONE]\n\n",
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
    pub(in crate::moonshot) fn deltas(stream: &mut dyn DeltaStream) -> Vec<Delta> {
        let mut out = Vec::new();
        while let Some(delta) = stream.next() {
            out.push(delta.unwrap());
        }
        out
    }

    /// One chunk, wrapped in the framing this endpoint sends it under.
    fn chunk(delta: &str, finish: &str) -> String {
        format!(
            "data: {{\"choices\":[{{\"index\":0,\"delta\":{delta},\"finish_reason\":{finish}}}]}}\n\n"
        )
    }

    #[test]
    fn an_answer_arrives_as_text_a_stop_and_then_what_it_cost() {
        // In that order, because that is the order this endpoint sends them.
        // Both counts come last, in a chunk with no choice in it at all — after
        // the reason the model stopped, which is the one place a reader is
        // tempted to stop reading. What the request carried is read from the
        // same chunk as what the answer cost.
        let mut stream = reading(ANSWER, &Cancel::new());

        assert_eq!(
            deltas(&mut stream),
            vec![
                Delta::Text("Hello".into()),
                Delta::Text(", world".into()),
                Delta::Stopped(StopReason::Yielded),
                Delta::Carried(Carried::new(9)),
                Delta::Spent(Spend::new(4)),
            ]
        );
    }

    #[test]
    fn the_chunk_that_carries_the_counts_says_what_the_request_carried_too() {
        // `prompt_tokens` sits beside `completion_tokens` in the one chunk this
        // endpoint sends them in, and only one of them was ever read.
        let mut stream = reading(ANSWER, &Cancel::new());

        assert!(
            deltas(&mut stream).contains(&Delta::Carried(Carried::new(9))),
            "the usage chunk carries prompt_tokens and it is not reported"
        );
    }

    #[test]
    fn the_sentinel_that_closes_the_stream_is_not_read_as_a_payload() {
        // `[DONE]` is not JSON. Parsed as one it fails the turn at the last
        // event, after a complete answer has already arrived — and every turn
        // this provider ever runs ends with it.
        let mut stream = reading(
            &format!(
                "{}data: [DONE]\n\n",
                chunk(r#"{"content":"Hi"}"#, r#""stop""#)
            ),
            &Cancel::new(),
        );

        assert_eq!(
            deltas(&mut stream),
            vec![
                Delta::Text("Hi".into()),
                Delta::Stopped(StopReason::Yielded),
            ]
        );
    }

    #[test]
    fn a_tool_call_is_named_once_and_then_arrives_in_fragments() {
        // The name and the identity come with the first fragment and never
        // again; the ones after it carry the index alone.
        let body = format!(
            "{}{}{}{}",
            chunk(
                r#"{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"read","arguments":""}}]}"#,
                "null"
            ),
            chunk(
                r#"{"tool_calls":[{"index":0,"function":{"arguments":"{\"path\":"}}]}"#,
                "null"
            ),
            chunk(
                r#"{"tool_calls":[{"index":0,"function":{"arguments":"\"a.rs\"}"}}]}"#,
                "null"
            ),
            chunk("{}", r#""tool_calls""#),
        );
        let mut stream = reading(&body, &Cancel::new());

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
    fn arguments_for_a_call_that_was_never_opened_are_refused() {
        // Assembled onto whatever is open, they are one tool running on another
        // tool's arguments — a wrong file read rather than a failure anyone can
        // see.
        let mut stream = reading(
            &chunk(
                r#"{"tool_calls":[{"index":3,"function":{"arguments":"{}"}}]}"#,
                "null",
            ),
            &Cancel::new(),
        );

        let problem = stream.next().unwrap().unwrap_err();

        assert!(
            matches!(problem, ProviderError::Protocol { .. }),
            "expected a stray fragment to be refused, got {problem:?}"
        );
    }

    #[test]
    fn a_failure_reported_mid_stream_stops_the_stream() {
        let body = format!(
            "{}data: {{\"error\":{{\"type\":\"rate_limit_reached_error\",\"message\":\"too many requests\"}}}}\n\n",
            chunk(r#"{"content":"Hel"}"#, "null")
        );
        let mut stream = reading(&body, &Cancel::new());

        assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("Hel".into()));
        let problem = stream.next().unwrap().unwrap_err();

        assert_eq!(
            problem.to_string(),
            "moonshot: rate_limit_reached_error: too many requests"
        );
        assert!(
            stream.next().is_none(),
            "the stream continued past a failure"
        );
    }

    #[test]
    fn a_ceiling_and_a_filter_are_not_reported_as_a_finish() {
        // Both cut an answer short, and an answer that stops mid-sentence is
        // indistinguishable from a finished one unless the turn says so.
        for (said, meant) in [
            (r#""length""#, StopReason::OutOfTokens),
            (r#""content_filter""#, StopReason::Filtered),
            (r#""something_new""#, StopReason::Unknown),
        ] {
            let mut stream = reading(&chunk(r#"{"content":"Hel"}"#, said), &Cancel::new());

            assert_eq!(
                deltas(&mut stream),
                vec![Delta::Text("Hel".into()), Delta::Stopped(meant)],
                "{said} was read as something else"
            );
        }
    }

    #[test]
    fn a_response_that_stops_arriving_is_a_failure_and_not_a_finished_turn() {
        let mut stream = reading(&chunk(r#"{"content":"Hel"}"#, "null"), &Cancel::new());

        assert_eq!(stream.next().unwrap().unwrap(), Delta::Text("Hel".into()));
        let problem = stream.next().unwrap().unwrap_err();

        assert!(
            matches!(problem, ProviderError::Transport { .. }),
            "expected a truncated response to be reported, got {problem:?}"
        );
        assert!(stream.next().is_none());
    }

    #[test]
    fn cancelling_mid_answer_stops_the_stream_rather_than_failing_it() {
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
}
