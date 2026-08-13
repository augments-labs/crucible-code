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
    use crucible_core::{Cancel, Delta, DeltaStream, ProviderError, StopReason};

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
