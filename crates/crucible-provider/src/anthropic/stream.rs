//! One response, as deltas.
//!
//! The read half of the provider: events in through [`super::wire`], deltas
//! out. It exists apart from the request because it outlives it — the request
//! is over in a round trip, and this is what runs for as long as the model is
//! talking.

use std::fmt;
use std::io::{BufReader, Read};

use crucible_core::{Cancel, Delta, DeltaStream, ProviderError, StopReason};

use crate::anthropic::{NAME, wire};
use crate::sse::Events;

/// A response being read.
pub(super) struct Stream {
    events: Events<BufReader<Box<dyn Read + Send>>>,
    cancel: Cancel,
    /// Whether the model said why it stopped.
    stopped: bool,
    /// Whether there is nothing further to deliver.
    finished: bool,
}

impl Stream {
    /// Reads `body` until it ends or `cancel` is raised.
    pub(super) fn new(body: Box<dyn Read + Send>, cancel: Cancel) -> Self {
        Self {
            events: Events::new(BufReader::new(body)),
            cancel,
            stopped: false,
            finished: false,
        }
    }

    /// What to deliver when the events run out.
    ///
    /// A response that stops arriving part-way through looks exactly like a
    /// finished one from here — same silence, no error. Saying so is the
    /// difference between a visibly failed turn and an answer that was quietly
    /// cut in half.
    fn ended(&mut self) -> Option<Result<Delta, ProviderError>> {
        self.finished = true;

        if self.stopped {
            return None;
        }

        Some(Err(ProviderError::Transport {
            provider: NAME,
            problem: "the response ended before the model finished".into(),
        }))
    }

    /// Stops delivering, and reports why.
    fn fail(&mut self, problem: ProviderError) -> Result<Delta, ProviderError> {
        self.finished = true;
        Err(problem)
    }
}

impl fmt::Debug for Stream {
    /// By hand: a socket part-way through a response cannot be shown without
    /// consuming it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("stopped", &self.stopped)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl DeltaStream for Stream {
    fn next(&mut self) -> Option<Result<Delta, ProviderError>> {
        loop {
            if self.finished {
                return None;
            }

            // Between events rather than during one: the read below blocks
            // until the provider says something, and the keep-alive it sends
            // while thinking is what makes that a short wait.
            if self.cancel.requested() {
                self.finished = true;
                return Some(Ok(Delta::Stopped(StopReason::Cancelled)));
            }

            let event = match self.events.next() {
                None => return self.ended(),
                Some(Err(problem)) => {
                    return Some(self.fail(ProviderError::Transport {
                        provider: NAME,
                        problem: problem.to_string().into(),
                    }));
                }
                Some(Ok(event)) => event,
            };

            match wire::delta(&event) {
                Err(problem) => return Some(self.fail(problem)),
                Ok(Some(delta)) => {
                    self.stopped |= matches!(delta, Delta::Stopped(_));
                    return Some(Ok(delta));
                }
                Ok(None) => {}
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use crucible_core::ToolId;

    use super::*;

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
    fn a_stream_shows_no_response_content_in_its_debug() {
        // It is held by the runner for the length of a turn, so it appears in
        // whatever that prints.
        let shown = format!("{:?}", reading(ANSWER, &Cancel::new()));

        assert!(!shown.contains("Hello"), "the response leaked: {shown}");
    }
}
