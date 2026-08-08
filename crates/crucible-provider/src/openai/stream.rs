//! One response, as deltas.
//!
//! The read half of the provider: chunks in through [`super::wire`], deltas
//! out. It exists apart from the request because it outlives it — the request
//! is over in a round trip, and this is what runs for as long as the model is
//! talking.
//!
//! A queue sits between the two because one chunk can mean several deltas, and
//! the caller asks for them one at a time. Everything a chunk yielded is
//! delivered before another chunk is read, including after the user cancels:
//! those deltas are already off the socket, and dropping them would lose part
//! of an answer that had arrived.

use std::collections::VecDeque;
use std::fmt;
use std::io::{BufReader, Read};

use crucible_core::{Cancel, Delta, DeltaStream, ProviderError, StopReason};

use crate::openai::{NAME, wire};
use crate::sse::Events;

/// A response being read.
pub(super) struct Stream {
    events: Events<BufReader<Box<dyn Read + Send>>>,
    cancel: Cancel,
    /// Deltas the last chunk yielded, not yet asked for.
    pending: VecDeque<Delta>,
    /// Whether the model said why it stopped.
    stopped: bool,
    /// Whether there is nothing further to read.
    finished: bool,
}

impl Stream {
    /// Reads `body` until it ends or `cancel` is raised.
    pub(super) fn new(body: Box<dyn Read + Send>, cancel: Cancel) -> Self {
        Self {
            events: Events::new(BufReader::new(body)),
            cancel,
            pending: VecDeque::new(),
            stopped: false,
            finished: false,
        }
    }

    /// What to deliver when the chunks run out.
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
    ///
    /// What was queued goes with it: the response is over, and handing out the
    /// rest of a chunk after saying the stream failed reorders the answer.
    fn fail(&mut self, problem: ProviderError) -> Result<Delta, ProviderError> {
        self.finished = true;
        self.pending.clear();
        Err(problem)
    }
}

impl fmt::Debug for Stream {
    /// By hand: a socket part-way through a response cannot be shown without
    /// consuming it, and what is queued is response content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("pending", &self.pending.len())
            .field("stopped", &self.stopped)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl DeltaStream for Stream {
    fn next(&mut self) -> Option<Result<Delta, ProviderError>> {
        loop {
            if let Some(delta) = self.pending.pop_front() {
                return Some(Ok(delta));
            }

            if self.finished {
                return None;
            }

            // Between chunks rather than during one: the read below blocks
            // until the provider says something.
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

            match wire::deltas(&event) {
                Err(problem) => return Some(self.fail(problem)),
                Ok(deltas) => {
                    self.stopped |= deltas
                        .iter()
                        .any(|delta| matches!(delta, Delta::Stopped(_)));
                    self.pending.extend(deltas);
                }
            }
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
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
}
