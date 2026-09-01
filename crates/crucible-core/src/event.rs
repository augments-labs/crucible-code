//! Events: the one thing that crosses a thread boundary.
//!
//! Every worker — the provider stream, a running tool, the input reader —
//! posts events to a single channel, and the main thread is the only reader.
//! That is what keeps the terminal owned by exactly one thread without a lock
//! anywhere on the render path.
//!
//! A closed set, deliberately. Adding an event must break every `match` that
//! decides how to draw one.

use crate::ContextError;
use crate::ids::{RunId, ToolId, TurnId};
use crate::provider::{ProviderError, Spend};
use crate::run::Ancestry;
use crate::tool::{Looking, Summary, ToolCall, ToolError, ToolOutput, Wrote};
use crate::toolset::{ToolReceipt, ToolsetError};
use crate::transcript::{Attachment, StopReason};

/// Why a turn ended badly.
///
/// Owned by core rather than by the runner, because [`Event`] is owned by core
/// and an event that names a runner type would invert the dependency.
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// Typed request context could not be snapshotted or patched.
    #[error(transparent)]
    Context(#[from] ContextError),

    /// The exact provider-facing stable prefix could not be described safely.
    #[error(transparent)]
    PromptCacheProjection(#[from] crate::PromptCacheProjectionError),

    /// Prompt-cache policy required a control that could not be prepared.
    #[error(transparent)]
    PromptCachePreparation(#[from] crate::PromptCachePreparationError),

    /// An explicitly authorized persistent cache resource could not be prepared.
    #[error(transparent)]
    PromptCacheResource(#[from] crate::PromptCacheResourceError),

    /// The provider failed.
    #[error(transparent)]
    Provider(#[from] ProviderError),

    /// A tool could not be carried out.
    #[error(transparent)]
    Tool(#[from] ToolError),

    /// The live tool roster could not be materialized.
    #[error(transparent)]
    Toolset(#[from] ToolsetError),

    /// A toolset lifecycle failed and its required cleanup failed as well.
    #[error("{primary}; toolset cleanup also failed: {cleanup}")]
    ToolsetCleanup {
        /// The failure that ended the work.
        primary: Box<TurnError>,
        /// The independent cleanup failure that must not be lost.
        cleanup: ToolsetError,
    },

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

    /// The model stopped before a complete structured recap was available.
    #[error(
        "compaction did not produce a complete structured recap; the original context was kept"
    )]
    RecapIncomplete,

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
/// in the binary, a vector in a test — and what travels is an
/// [`EventEnvelope`], so a destination that only draws can drop the attribution
/// on the way in without the runner knowing it did.
///
/// Widening what is carried is not free: it is this method, every destination
/// that implements it and every place that reports. The envelope is the answer
/// to that, and adding to it is the cheap change; adding beside it is not.
pub trait Post: Send + Sync {
    /// Reports one event, and which execution produced it.
    ///
    /// Cannot fail. Nothing a worker does depends on anyone still listening,
    /// and a worker that stopped to handle a closed channel would be stopping
    /// for the one condition that already means the process is leaving.
    fn post(&self, reported: EventEnvelope);
}

/// The ordinary case: events go to the thread that draws.
impl Post for std::sync::mpsc::Sender<EventEnvelope> {
    fn post(&self, reported: EventEnvelope) {
        drop(self.send(reported));
    }
}

/// One event, and the execution it came from.
///
/// A wrapper rather than a field on every variant, so that the set of events
/// stays a statement about what happened and attribution stays a statement
/// about who it happened to. A reader that only draws still matches on
/// [`Event`]; a reader that has to tell two runs apart reaches past it.
#[derive(Debug)]
pub struct EventEnvelope {
    ancestry: Ancestry,
    event: Event,
}

impl EventEnvelope {
    /// One event, attributed to `ancestry`.
    #[must_use]
    pub const fn new(ancestry: Ancestry, event: Event) -> Self {
        Self { ancestry, event }
    }

    /// Which execution produced it.
    #[must_use]
    pub const fn run(&self) -> RunId {
        self.ancestry.run()
    }

    /// Where that execution sits among the ones around it.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// What happened.
    #[must_use]
    pub const fn event(&self) -> &Event {
        &self.event
    }

    /// What happened, with the attribution dropped.
    ///
    /// For a destination that has nothing to tell apart. Named so that giving
    /// up the attribution is something a reader can see being done.
    #[must_use]
    pub fn into_event(self) -> Event {
        self.event
    }
}

/// A destination, and the execution everything sent to it came from.
///
/// The stamp lives here rather than at each of the several dozen places that
/// report something, so that a worker still says what happened and nothing
/// else. `Copy`, because it is an [`Ancestry`] and a borrow and holding one by
/// reference would tie a caller to where it was made for no gain.
#[derive(Clone, Copy)]
pub struct Reporter<'a> {
    ancestry: Ancestry,
    to: &'a dyn Post,
}

impl<'a> Reporter<'a> {
    /// Everything sent through this will be attributed to `ancestry`.
    #[must_use]
    pub const fn new(ancestry: Ancestry, to: &'a dyn Post) -> Self {
        Self { ancestry, to }
    }

    /// Execution attribution fixed on every event this reporter posts.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// Reports one event as this execution's.
    pub fn post(&self, event: Event) {
        self.to.post(EventEnvelope::new(self.ancestry, event));
    }
}

impl std::fmt::Debug for Reporter<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reporter")
            .field("ancestry", &self.ancestry)
            .finish_non_exhaustive()
    }
}

/// One record of something that happened.
pub enum Event {
    /// A turn began.
    TurnStarted {
        /// Which turn.
        turn: TurnId,
    },

    /// One bounded immutable prompt-cache preparation or provider fact.
    PromptCache {
        /// The typed fact; ancestry is supplied by the surrounding envelope.
        fact: crate::PromptCacheFact,
    },

    /// One bounded sandbox lifecycle fact for a tool invocation.
    Sandbox {
        /// The fixed provider call identity that owns the sandbox.
        call: ToolId,
        /// Redacted typed fact; ancestry is supplied by the envelope.
        fact: crate::SandboxFact,
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
        /// Whether the running call can be left to finish in the background.
        backgroundable: bool,
        /// Whether this is the only call of its pass.
        ///
        /// One response can ask for several tools at once, and they are
        /// answered as they finish rather than in the order they were asked
        /// for. A reader is shown a result under the call it answers, and a
        /// transcript that only ever grows at the end can keep that promise
        /// for one outstanding call and not for four. So the count is
        /// reported here, where it is known, rather than guessed at by
        /// whatever draws the rows: the calls of a pass are all announced
        /// before any of them runs, so nothing downstream can tell the first
        /// of four from the only one.
        alone: bool,
        /// What kind of looking-around this call is, where it is only that —
        /// asked of the tool beside [`Event::ToolRequested::summary`], and for
        /// the same reason: the arguments are opaque to whatever draws the row,
        /// and the tool is the only code that can read them.
        ///
        /// `None` is a call the transcript names. `Some` is a call it may fold
        /// into a count of the ones like it, which is what keeps a turn that
        /// read forty files from being forty rows of a reader's screen.
        looking: Option<Looking>,
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
        /// Bounded audit and usage evidence from the invocation pipeline.
        ///
        /// `None` is reserved for synthetic renderer events. The runner emits
        /// `Some` for every recorded call.
        receipt: Option<ToolReceipt>,
    },

    /// The response the turn was waiting on failed before it said anything, and
    /// the same request is going out again.
    ///
    /// Only ever posted about a response nobody read a word of, so nothing on
    /// screen is being taken back. What it replaces is a turn that ended over a
    /// socket the provider closed while the tools ran.
    Retrying,

    /// How much usable room remains before compaction, where a window is known.
    ///
    /// The room reserved for the answer and its tool results is outside this
    /// percentage, so zero is the safe compaction boundary rather than the
    /// model's literal last token. `None` where no window is known: nothing draws
    /// a fraction of a number nobody stated.
    Carried {
        /// The usable percentage still free, rounded down.
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

    /// Files a request went out without, because it had no room for them.
    ///
    /// A request has a ceiling and a transcript does not, so the older
    /// attachments lose their bytes rather than the turn losing its answer.
    /// That is the design working, and it is invisible: without this the
    /// answers quietly get less to look at with nothing on screen saying so.
    ///
    /// Posted once per request rather than once per turn. A retry is a second
    /// answer that went out short, and a reader watching it arrive is owed the
    /// same sentence about it.
    Aged {
        /// The attachments whose content was not carried, in transcript order.
        ///
        /// The attachment rather than its path, so what a reader is shown and
        /// what a backtrace may print stay the two different things
        /// [`Attachment`]'s own `Debug` already keeps apart.
        files: Box<[Attachment]>,
    },

    /// Files a request went out without, because the model does not read them.
    ///
    /// A transcript outlives a model: a picture named while one model was being
    /// asked is still there when another is, and a provider with no word for a
    /// kind writes it as something it is not rather than refusing it. So the
    /// bytes stay behind, and the answer that arrives has not seen them.
    ///
    /// Apart from [`Event::Aged`] because the two differ in the one part a
    /// reader can act on: an aged file goes out if it is asked for again, and
    /// this one does not go out until the model does.
    ///
    /// Posted once per request, for the reason [`Event::Aged`] is.
    Unread {
        /// The attachments whose content was not carried, in transcript order.
        files: Box<[Attachment]>,
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

/// By hand, because two variants carry conversation text of their own and they
/// go opposite ways: a steered line is the reader's, redacted the way
/// [`crate::Message::User`] redacts the same words, while a delta's prose is
/// deliberately shown — it is the model's own prose on its way to the screen.
/// Everything else delegates, and what needs redacting redacts itself.
impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TurnStarted { turn } => {
                f.debug_struct("TurnStarted").field("turn", turn).finish()
            }
            Self::PromptCache { fact } => {
                f.debug_struct("PromptCache").field("fact", fact).finish()
            }
            Self::Sandbox { call: _, fact } => f
                .debug_struct("Sandbox")
                .field("call", &"[redacted]")
                .field("fact", fact)
                .finish(),
            Self::Delta { text } => f.debug_struct("Delta").field("text", text).finish(),
            Self::ToolRequested {
                call,
                summary,
                backgroundable,
                looking,
                alone,
            } => f
                .debug_struct("ToolRequested")
                .field("call", call)
                .field("summary", summary)
                .field("backgroundable", backgroundable)
                .field("looking", looking)
                .field("alone", alone)
                .finish(),
            Self::Wrote { call, text } => f
                .debug_struct("Wrote")
                .field("call", call)
                .field("text", text)
                .finish(),
            Self::ToolFinished {
                call,
                output,
                receipt,
            } => f
                .debug_struct("ToolFinished")
                .field("call", call)
                .field("output", output)
                .field("receipt", receipt)
                .finish(),
            Self::Retrying => f.write_str("Retrying"),
            Self::Carried { left } => f.debug_struct("Carried").field("left", left).finish(),
            Self::Compacting { why, part } => f
                .debug_struct("Compacting")
                .field("why", why)
                .field("part", part)
                .finish(),
            Self::Compacted { compacted } => f
                .debug_struct("Compacted")
                .field("compacted", compacted)
                .finish(),
            Self::Spent { spend } => f.debug_struct("Spent").field("spend", spend).finish(),
            Self::TurnFinished { turn, stop } => f
                .debug_struct("TurnFinished")
                .field("turn", turn)
                .field("stop", stop)
                .finish(),
            Self::Failed { error } => f.debug_struct("Failed").field("error", error).finish(),
            Self::Aged { files } => f.debug_struct("Aged").field("files", files).finish(),
            Self::Unread { files } => f.debug_struct("Unread").field("files", files).finish(),
            Self::Steered { line: _ } => f
                .debug_struct("Steered")
                .field("line", &"[redacted]")
                .finish(),
        }
    }
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

        let sent = std::thread::spawn(move || {
            tx.send(Event::TurnStarted {
                turn: TurnId::FIRST,
            })
            .is_ok()
        })
        .join()
        .unwrap();

        assert!(sent, "the event receiver went away before the send");
        assert!(matches!(rx.recv().unwrap(), Event::TurnStarted { .. }));
    }

    #[test]
    fn what_a_request_went_out_without_says_no_path() {
        // The event crosses a channel and can end up in a panic payload, and a
        // path names directories and usually the person. It carries the
        // attachment itself so that the redaction is the one already written
        // for it, rather than a second one that can disagree.
        let event = Event::Aged {
            files: Box::new([Attachment {
                path: "/home/aged-debug-canary/holiday.png".into(),
                modality: crate::Modality::Image,
                media_type: "image/png".into(),
                hash: [0; 32],
            }]),
        };

        let shown = format!("{event:?}");

        assert!(!shown.contains("aged-debug-canary"), "{shown}");
        assert!(shown.contains("Image"), "{shown}");
    }

    #[test]
    fn posting_where_nobody_is_listening_is_not_a_failure() {
        // The receiver is gone only when the process is on its way out, and a
        // worker that stopped to handle that would be stopping for the one
        // condition that is already handled.
        let (tx, rx) = std::sync::mpsc::channel();
        drop(rx);

        Reporter::new(Ancestry::new(), &tx).post(Event::TurnStarted {
            turn: TurnId::FIRST,
        });
    }

    #[test]
    fn what_a_call_wrote_arrives_under_the_call_that_wrote_it() {
        let (tx, rx) = std::sync::mpsc::channel();

        Reporter::new(Ancestry::new(), &tx).post(Event::Wrote {
            call: ToolId::new("a"),
            text: Wrote::new("Compiling crucible-core v0.5.0\n"),
        });

        let Event::Wrote { call, text } = rx.recv().unwrap().into_event() else {
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
    fn an_event_never_shows_what_the_reader_typed() {
        // A steered line is the reader's own words, and the transcript's
        // [`Message::User`] already redacts the same words once they are a
        // message. The moment between typing and joining the turn is not a
        // moment they stop being theirs.
        let event = Event::Steered {
            line: "steered-debug-canary".to_owned(),
        };

        let shown = format!("{event:?}");
        assert!(!shown.contains("steered-debug-canary"), "{shown}");
        assert!(shown.contains("redacted"), "{shown}");
    }

    #[test]
    fn what_is_posted_is_what_arrives() {
        let (tx, rx) = std::sync::mpsc::channel();

        Reporter::new(Ancestry::new(), &tx).post(Event::Delta {
            text: "streamed".into(),
        });

        assert!(
            matches!(rx.recv().unwrap().into_event(), Event::Delta { text } if &*text == "streamed")
        );
    }
}
