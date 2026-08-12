//! One response, as deltas.
//!
//! The read half of the provider: events in through [`super::wire`], deltas
//! out. It exists apart from the request because it outlives it — the request
//! is over in a round trip, and this is what runs for as long as the model is
//! talking.
//!
//! A queue sits between the two because the parser answers with however many
//! deltas an event meant and the caller asks for them one at a time. This
//! endpoint narrates finely enough that the answer is at most one, and the queue
//! is what keeps that a fact about the wire rather than an assumption in the
//! loop. Everything an event yielded is delivered before another is read,
//! including after the user cancels: those deltas are already off the socket,
//! and dropping them would lose part of an answer that had arrived.

use std::collections::VecDeque;
use std::fmt;
use std::io::{BufReader, Read};

use crucible_core::{Cancel, Delta, DeltaStream, ProviderError, StopReason};

use crate::openai::wire::Open;
use crate::openai::{NAME, wire};
use crate::sse::Events;

/// A response being read.
pub(super) struct Stream {
    events: Events<BufReader<Box<dyn Read + Send>>>,
    cancel: Cancel,
    /// Deltas the last event yielded, not yet asked for.
    pending: VecDeque<Delta>,
    /// Which tool call is open, by the item id its vendor gave it.
    ///
    /// Here rather than in the parser because a call is opened in one event and
    /// its arguments arrive in the ones after it. A parser that forgets between
    /// events cannot tell a fragment of the open call from a fragment of
    /// another one, and hands both to the same call.
    open: Open,
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
            open: Open::default(),
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
    ///
    /// What was queued goes with it: the response is over, and handing out the
    /// rest of an event after saying the stream failed reorders the answer.
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

            // Between events rather than during one: the read below blocks
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

            match wire::deltas(&event, &mut self.open) {
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
pub(super) mod tests;
