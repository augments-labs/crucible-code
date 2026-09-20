//! What a turn reports while it runs: provisional, and only ever that.
//!
//! A [`Progress`] says something is happening. It is not what happened: words
//! arrive before the turn that says them is recorded, a tool is asked for
//! before anybody has allowed it, and any of it may be followed by a failure
//! that leaves the conversation as it was. What is true afterwards is a
//! [`Snapshot`](crate::Snapshot) and the [`Outcome`](crate::Outcome) the
//! command comes back as, which are different types so that neither can be
//! taken for the other.
//!
//! Progress may be dropped. A host is free to drop it rather than let a queue
//! grow, and a client that did not ask for
//! [`Capability::Progress`](crate::Capability::Progress) is handed none, so
//! nothing a client needs in order to be correct is carried here.

use serde_json::Value;

use crate::bounds::Text;
use crate::error::{ErrorCode, Refusal};
use crate::outcome::{Problem, Stop};
use crate::wire::{Fields, Writing, frame, parsed};

/// One thing a running turn reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    /// A turn started.
    Started {
        /// Its ordinal in the session.
        turn: u64,
    },
    /// Words the model said, as they arrived.
    Delta {
        /// The words, cut to the ceiling.
        text: Text,
    },
    /// The model asked for a tool.
    ToolRequested {
        /// The provider's identifier for the call.
        call: Text,
        /// The tool.
        tool: Text,
        /// What the tool says it would do, in its own words.
        summary: Text,
    },
    /// A tool call ended.
    ToolFinished {
        /// The provider's identifier for the call.
        call: Text,
        /// Whether it ended in failure.
        failed: bool,
    },
    /// A request is being made again.
    Retrying,
    /// Room is being made in the conversation.
    Compacting {
        /// Which part of the recap is being asked for.
        part: u64,
    },
    /// Room was made.
    Compacted {
        /// How many messages the recap stands in place of.
        replaced: u64,
    },
    /// Tokens were spent.
    Spent {
        /// How many.
        tokens: u64,
    },
    /// A turn reported that it finished.
    Finished {
        /// Its ordinal in the session.
        turn: u64,
        /// Why the model stopped.
        stop: Stop,
    },
    /// A turn reported that it failed.
    Failed(Problem),
}

impl Progress {
    /// Every kind of progress, by the word it crosses as.
    pub const KINDS: [&'static str; 10] = [
        "started",
        "delta",
        "tool_requested",
        "tool_finished",
        "retrying",
        "compacting",
        "compacted",
        "spent",
        "finished",
        "failed",
    ];

    /// The word this kind crosses as.
    #[must_use]
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Started { .. } => "started",
            Self::Delta { .. } => "delta",
            Self::ToolRequested { .. } => "tool_requested",
            Self::ToolFinished { .. } => "tool_finished",
            Self::Retrying => "retrying",
            Self::Compacting { .. } => "compacting",
            Self::Compacted { .. } => "compacted",
            Self::Spent { .. } => "spent",
            Self::Finished { .. } => "finished",
            Self::Failed(_) => "failed",
        }
    }

    /// The value this travels as.
    #[must_use]
    pub fn written(&self) -> Value {
        let object = Writing::new().with("progress", self.kind());
        match self {
            Self::Started { turn } => object.with("turn", *turn),
            Self::Delta { text } => object.text("text", text),
            Self::ToolRequested {
                call,
                tool,
                summary,
            } => object
                .text("call", call)
                .text("tool", tool)
                .text("summary", summary),
            Self::ToolFinished { call, failed } => {
                object.text("call", call).with("failed", *failed)
            }
            Self::Retrying => object,
            Self::Compacting { part } => object.with("part", *part),
            Self::Compacted { replaced } => object.with("replaced", *replaced),
            Self::Spent { tokens } => object.with("tokens", *tokens),
            Self::Finished { turn, stop } => object.with("turn", *turn).with("stop", stop.as_str()),
            Self::Failed(problem) => object.with("problem", problem.written()),
        }
        .finish()
    }

    /// The frame this progress travels as.
    ///
    /// # Errors
    ///
    /// [`ErrorCode::TooLarge`] where the frame would be over the ceiling.
    pub fn encode(&self) -> Result<Vec<u8>, Refusal> {
        frame(&self.written())
    }

    /// The progress `bytes` spell.
    ///
    /// A frame that is a snapshot or a response is refused here: progress is
    /// named by its own field, which neither of those has.
    ///
    /// # Errors
    ///
    /// [`Refusal`] for anything but one whole, bounded progress report.
    pub fn decode(bytes: &[u8]) -> Result<Self, Refusal> {
        let mut fields = Fields::of(parsed(bytes)?)?;
        let progress = match fields.string("progress")?.as_str() {
            "started" => Self::Started {
                turn: fields.number("turn")?,
            },
            "delta" => Self::Delta {
                text: fields.text("text")?,
            },
            "tool_requested" => Self::ToolRequested {
                call: fields.text("call")?,
                tool: fields.text("tool")?,
                summary: fields.text("summary")?,
            },
            "tool_finished" => Self::ToolFinished {
                call: fields.text("call")?,
                failed: fields.flag("failed")?,
            },
            "retrying" => Self::Retrying,
            "compacting" => Self::Compacting {
                part: fields.number("part")?,
            },
            "compacted" => Self::Compacted {
                replaced: fields.number("replaced")?,
            },
            "spent" => Self::Spent {
                tokens: fields.number("tokens")?,
            },
            "finished" => Self::Finished {
                turn: fields.number("turn")?,
                stop: Stop::named(&fields.string("stop")?)?,
            },
            "failed" => Self::Failed(Problem::read(fields.take("problem")?)?),
            _ => return Err(ErrorCode::Malformed.into()),
        };
        fields.done()?;
        Ok(progress)
    }
}
