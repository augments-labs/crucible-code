//! Events: the one thing that crosses a thread boundary.
//!
//! Every worker — the provider stream, a running tool, the input reader —
//! posts events to a single channel, and the main thread is the only reader.
//! That is what keeps the terminal owned by exactly one thread without a lock
//! anywhere on the render path.
//!
//! A closed set, deliberately. Adding an event must break every `match` that
//! decides how to draw one.

use crate::ids::{ToolId, TurnId};
use crate::provider::{ProviderError, Spend};
use crate::tool::{Summary, ToolCall, ToolError, ToolOutput};
use crate::transcript::StopReason;

/// Why a turn ended badly.
///
/// Owned by core rather than by the runner, because [`Event`] is owned by core
/// and an event that names a runner type would invert the dependency.
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// The provider failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// A tool could not be carried out.
    #[error(transparent)]
    Tool(#[from] ToolError),

    /// The model asked for a tool the user refused.
    #[error("{0} was not allowed")]
    Refused(Box<str>),

    /// Tool results crossed the turn's retained-output boundary.
    #[error("tool results exceeded the {maximum}-byte per-turn limit")]
    ToolOutputBytes {
        /// The most tool-output text one turn retains.
        maximum: usize,
    },
}

/// Where a worker reports what happened.
///
/// A trait for the same reason [`crate::Ask`] is one: the runner drives it and
/// must not name what is on the other end. The wiring decides that — a channel
/// in the binary, a vector in a test — and a channel of something wider than
/// [`Event`] is then a wrapper rather than a change to the runner.
pub trait Post {
    /// Reports one event.
    ///
    /// Cannot fail. Nothing a worker does depends on anyone still listening,
    /// and a worker that stopped to handle a closed channel would be stopping
    /// for the one condition that already means the process is leaving.
    fn post(&self, event: Event);
}

/// The ordinary case: events go to the thread that draws.
impl Post for std::sync::mpsc::Sender<Event> {
    fn post(&self, event: Event) {
        drop(self.send(event));
    }
}

/// One record of something that happened.
#[derive(Debug)]
pub enum Event {
    /// A turn began.
    TurnStarted {
        /// Which turn.
        turn: TurnId,
    },

    /// Prose arrived from the model.
    Delta {
        /// The text, to be appended to the live tail.
        text: Box<str>,
    },

    /// The model asked for a tool, and the call is now complete.
    ToolRequested {
        /// The assembled call.
        call: ToolCall,
        /// What the tool that owns those arguments says the call is about —
        /// asked where the tools are in reach, since the arguments are opaque
        /// to whatever draws the row.
        summary: Summary,
        /// What several of this tool's calls are counted in, for a drawer that
        /// folds a run of them into one line. Asked here for the same reason
        /// the summary is: the tools are in reach on this side and nowhere
        /// downstream of it.
        counted: &'static str,
    },

    /// A tool finished.
    ToolFinished {
        /// Which call this answers.
        call: ToolId,
        /// What it produced.
        output: ToolOutput,
    },

    /// What the turn has spent so far, every response of it added up.
    Spent {
        /// The running total, not the reading that moved it.
        spend: Spend,
    },

    /// A turn ended.
    TurnFinished {
        /// Which turn.
        turn: TurnId,
        /// Why the model stopped.
        stop: StopReason,
    },

    /// A turn ended badly.
    Failed {
        /// What went wrong.
        error: TurnError,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_provider_failure_reads_as_itself_through_a_turn_error() {
        let error = TurnError::from(ProviderError::Transport {
            provider: "anthropic",
            problem: "connection reset".into(),
        });

        // `transparent` on purpose: a user reading this wants the provider's
        // message, not "turn error: provider error: connection reset".
        assert_eq!(error.to_string(), "anthropic: connection reset");
    }

    #[test]
    fn a_refusal_names_the_tool_that_was_refused() {
        let error = TurnError::Refused("bash".into());
        assert_eq!(error.to_string(), "bash was not allowed");
    }

    #[test]
    fn a_tool_output_limit_names_its_exact_boundary() {
        let error = TurnError::ToolOutputBytes { maximum: 4096 };
        assert_eq!(
            error.to_string(),
            "tool results exceeded the 4096-byte per-turn limit"
        );
    }

    #[test]
    fn an_event_can_cross_a_thread() {
        // The channel is the whole concurrency design, so this is the property
        // that matters about `Event` more than any of its contents.
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            tx.send(Event::TurnStarted {
                turn: TurnId::FIRST,
            })
        })
        .join()
        .unwrap()
        .unwrap();

        assert!(matches!(rx.recv().unwrap(), Event::TurnStarted { .. }));
    }

    #[test]
    fn posting_where_nobody_is_listening_is_not_a_failure() {
        // The receiver is gone only when the process is on its way out, and a
        // worker that stopped to handle that would be stopping for the one
        // condition that is already handled.
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);

        tx.post(Event::TurnStarted {
            turn: TurnId::FIRST,
        });
    }

    #[test]
    fn what_is_posted_is_what_arrives() {
        let (tx, rx) = std::sync::mpsc::channel();

        tx.post(Event::Delta {
            text: "streamed".into(),
        });

        assert!(matches!(rx.recv().unwrap(), Event::Delta { text } if &*text == "streamed"));
    }
}
