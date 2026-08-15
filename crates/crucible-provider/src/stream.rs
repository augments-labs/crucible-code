//! One response, as deltas.
//!
//! The read half of every provider: events in through that vendor's `wire`,
//! deltas out. It exists apart from the request because it outlives it — the
//! request is over in a round trip, and this is what runs for as long as the
//! model is talking.
//!
//! It sits here rather than in each provider because none of what it does is a
//! vendor's business. When the cancel is looked at, what a body that stops
//! arriving means, and what happens to deltas already read when the stream
//! fails are answers this program owes its user, and a copy of them per
//! provider is a chance per provider to answer differently. What *is* a
//! vendor's business is behind [`Wire`]: which events mean something, and what.
//!
//! The response also retains an opaque credential filter. A gateway already
//! saw the applied header and may repeat it in an error event; every typed wire
//! failure passes through that filter before it reaches formatting.
//!
//! A queue sits between the two because the parser answers with however many
//! deltas an event meant and the caller asks for them one at a time. Everything
//! an event yielded is delivered before another is read, including after the
//! user cancels: those deltas are already off the socket, and dropping them
//! would lose part of an answer that had arrived.
//!
//! The cancel is looked at between events, and what makes that prompt is that
//! the framing below gives up waiting and hands the turn back rather than
//! blocking on the socket. So a provider that has stopped talking costs one
//! bounded wait and not an indefinite one, and a wait that expired ends
//! nothing: the response is still open, and only the user or the socket closes
//! it.

use std::collections::VecDeque;
use std::fmt;
use std::io::{BufReader, Read};

use crucible_core::{Cancel, Delta, DeltaStream, ProviderError, Redactions, StopReason};

use crate::sse::{Events, Framed, SseEvent};

/// What one vendor's events mean.
///
/// The state a vendor needs between events lives in the implementor, because
/// only it knows whether there is any: a protocol that opens a tool call in one
/// event and streams its arguments in the ones after has to remember which call
/// is open, and one that carries a whole call in a single event has nothing to
/// remember at all.
///
/// Whatever it remembers starts empty, which is what `Default` says here — a
/// response begins with no call open and nothing half-read, so the loop builds
/// its own parser and a caller never passes one in.
pub(crate) trait Wire: Default + Send {
    /// What this provider is called, in the failures this loop reports.
    const PROVIDER: &'static str;

    /// What `event` means, or nothing where it means nothing here.
    ///
    /// A `Vec` because one event can mean several deltas and most mean none. The
    /// empty case does not allocate, and the rest are a smaller cost than the
    /// parse that produced them.
    ///
    /// # Errors
    ///
    /// Whatever the vendor's parser reports: a payload that does not parse, an
    /// event that contradicts what is open, or the provider reporting a failure
    /// inside a response it had already started.
    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError>;
}

/// A response being read.
pub(crate) struct Response<W: Wire> {
    events: Events<BufReader<Box<dyn Read + Send>>>,
    cancel: Cancel,
    /// Exact credentials the gateway saw, available only as a filter.
    redactions: Redactions,
    /// Deltas the last event yielded, not yet asked for.
    pending: VecDeque<Delta>,
    /// The vendor's parser, and whatever it remembers between events.
    wire: W,
    /// Whether the model said why it stopped.
    stopped: bool,
    /// Whether there is nothing further to read.
    finished: bool,
}

impl<W: Wire> Response<W> {
    /// Reads `body` until it ends or `cancel` is raised.
    pub(crate) fn new(body: Box<dyn Read + Send>, cancel: Cancel, redactions: Redactions) -> Self {
        Self {
            events: Events::new(BufReader::new(body)),
            cancel,
            redactions,
            pending: VecDeque::new(),
            wire: W::default(),
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
            provider: W::PROVIDER,
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
        Err(problem.redacted(&self.redactions))
    }
}

impl<W: Wire> fmt::Debug for Response<W> {
    /// By hand: a socket part-way through a response cannot be shown without
    /// consuming it, and what is queued is response content.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Response")
            .field("provider", &W::PROVIDER)
            .field("pending", &self.pending.len())
            .field("stopped", &self.stopped)
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl<W: Wire> DeltaStream for Response<W> {
    fn next(&mut self) -> Option<Result<Delta, ProviderError>> {
        loop {
            if let Some(delta) = self.pending.pop_front() {
                return Some(Ok(delta));
            }

            if self.finished {
                return None;
            }

            // Between events rather than during one, which is prompt because
            // the read below comes back whether or not anything arrived.
            if self.cancel.requested() {
                self.finished = true;
                return Some(Ok(Delta::Stopped(StopReason::Cancelled)));
            }

            let event = match self.events.next() {
                None => return self.ended(),
                Some(Err(problem)) => {
                    return Some(self.fail(ProviderError::Transport {
                        provider: W::PROVIDER,
                        problem: problem.to_string().into(),
                    }));
                }
                // Nothing yet. Round the loop to the cancel above, which is the
                // whole of what a bounded wait is for.
                Some(Ok(Framed::Quiet)) => continue,
                Some(Ok(Framed::Event(event))) => event,
            };

            match self.wire.deltas(&event) {
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
