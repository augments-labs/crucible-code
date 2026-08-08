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
use crate::provider::ProviderError;
use crate::tool::{ToolCall, ToolError, ToolOutput};
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
    },

    /// A tool finished.
    ToolFinished {
        /// Which call this answers.
        call: ToolId,
        /// What it produced.
        output: ToolOutput,
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
}
