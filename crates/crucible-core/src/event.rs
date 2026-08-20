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
use crate::tool::{Summary, ToolCall, ToolError, ToolOutput, Wrote};
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

    /// The turn produced more than it was allowed to.
    ///
    /// Only where somebody asked for a ceiling. What it bounds is what a
    /// runaway turn actually consumes, rather than a count of calls standing in
    /// for it — a turn making progress is not one to stop for being long.
    #[error("this turn produced more than the {ceiling}-token ceiling it was given")]
    Spent {
        /// The ceiling that was set.
        ceiling: u64,
    },

    /// Making room did not make enough of it.
    #[error("there is no room left in the model's window, and compacting it freed none")]
    NoRoom,

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
    },

    /// More of what a running tool has printed, in the order it arrived.
    ///
    /// One piece of it rather than all of it: a command that has printed a
    /// megabyte has posted many of these, and nothing here holds on to what an
    /// earlier one carried. What the model is finally sent is the
    /// [`ToolOutput`] on [`Event::ToolFinished`], bounded by the tool; this is
    /// for the reader, while there is still something to watch.
    ///
    /// Named by its call because a piece of output that did not say whose it
    /// was could be drawn under the wrong call the moment two of them can run
    /// at once — and because whatever coalesces these has to know where one
    /// call's output stops.
    Wrote {
        /// Which call printed it.
        call: ToolId,
        /// The text, in the order the command produced it.
        text: Wrote,
    },

    /// A tool finished.
    ToolFinished {
        /// Which call this answers.
        call: ToolId,
        /// What it produced.
        output: ToolOutput,
    },

    /// The response the turn was waiting on failed before it said anything, and
    /// the same request is going out again.
    ///
    /// Only ever posted about a response nobody read a word of, so nothing on
    /// screen is being taken back. What it replaces is a turn that ended over a
    /// socket the provider closed while the tools ran.
    Retrying,

    /// How much of the model's window is left, where a window is known.
    ///
    /// `None` where none is: nothing draws a fraction of a number nobody
    /// stated, and a session on a model this build has never heard of is one
    /// where the reading is simply absent.
    Carried {
        /// The percentage still free, rounded down.
        left: Option<u8>,
    },

    /// Room is being made, and the turn has not ended.
    ///
    /// Reported again as the notes are written, so the row saying so can move
    /// rather than sit still for the length of one request.
    Compacting {
        /// What asked for it.
        why: crate::Compacting,
        /// How much of the notes has been written, as a percentage of the room
        /// they were given.
        ///
        /// A fraction of what was *asked for*, not of how long it will take —
        /// nothing here knows that. It is honest about being an answer arriving
        /// rather than a clock running down, and it is why the room a recap is
        /// given is a figure this program chooses rather than the model's own
        /// ceiling.
        part: u8,
    },

    /// Room was made, and by how much.
    Compacted {
        /// What it took.
        compacted: crate::Compacted,
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

    /// A line typed while the turn ran was worked into it.
    ///
    /// Posted where the line joins the turn — between one pass and the next —
    /// so the screen can commit it where it belongs: as the reader's own words,
    /// in the order they arrived, rather than as something the model said.
    Steered {
        /// The line, whole.
        line: String,
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
    fn what_a_call_wrote_arrives_under_the_call_that_wrote_it() {
        let (tx, rx) = std::sync::mpsc::channel();

        tx.post(Event::Wrote {
            call: ToolId::new("a"),
            text: Wrote::new("Compiling crucible-core v0.5.0\n"),
        });

        let Event::Wrote { call, text } = rx.recv().unwrap() else {
            panic!("the event that arrived was not the one posted");
        };
        assert_eq!(call, ToolId::new("a"));
        assert_eq!(text.as_str(), "Compiling crucible-core v0.5.0\n");
    }

    #[test]
    fn an_event_never_shows_what_a_command_printed() {
        // A command is how a model reads a file it was refused and how it runs
        // `env`, so what one prints is redacted for the reason a tool's result
        // is. `Delta` is the neighbouring case and is deliberately not: that is
        // the model's own prose on its way to the screen.
        let event = Event::Wrote {
            call: ToolId::new("a"),
            text: Wrote::new("wrote-debug-canary"),
        };

        let shown = format!("{event:?}");
        assert!(!shown.contains("wrote-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"));
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
