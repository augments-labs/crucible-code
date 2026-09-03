//! Calling one tool a server offered, and reading back what it produced.
//!
//! The call is gated on an [`Offered`] rather than a name, for the same reason
//! a catalogue is gated on a [`Greeting`](crate::Greeting): holding one is the
//! proof that this server said it has this tool. A name assembled anywhere else
//! is a name crucible would be trying on a server that never mentioned it.
//!
//! What comes back is somebody else's program's text on its way into a model's
//! context, so it is bounded here, where the bytes are first retained. Unlike a
//! catalogue, a result that runs past its bound is cut rather than refused: a
//! short catalogue is a lie about which tools exist, while a cut result is
//! still the answer to the question that was asked, and [`Answered::omitted`]
//! is how it says it is not all of it.
//!
//! A server reporting that the tool itself failed is not an error of crucible's
//! making. The model asked for something, the something did not work, and that
//! is a result to react to rather than a conversation to end — so it arrives as
//! [`Answered::failed`] and the words come with it.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

use crate::catalogue::Offered;
use crate::talking::{Talking, Trouble};

/// The most bytes of one result crucible retains.
///
/// Every one of them goes into a model's context and into the transcript that
/// is kept of the turn, so this is a bound on what one call by one server can
/// cost every request that follows it.
pub const RESULT_BYTES: usize = 64 * 1024;

/// The most content blocks crucible reads from one result.
///
/// [`RESULT_BYTES`] already bounds the bytes; this bounds the work of walking
/// them, so a server answering with a million empty blocks is a sentence rather
/// than a pause nobody can explain.
pub const BLOCKS: usize = 1024;

/// What the middle of a cut result is replaced by.
///
/// In the text rather than beside it. The bytes the model is shown have to say
/// on their own that they are not contiguous, because nothing downstream of
/// here is obliged to render [`Answered::omitted`] and a model reading a
/// sentence joined to an unrelated one has been actively misled.
pub const CUT: &str = "\n[…crucible cut this result…]\n";

/// Why a tool call produced nothing to hand back.
#[derive(Debug, thiserror::Error)]
pub enum Unanswered {
    /// The conversation itself failed.
    #[error(transparent)]
    Talking(#[from] Trouble),

    /// A member the protocol requires was absent, or was the wrong kind.
    #[error("the server answered the call without {field}, which every result has to carry")]
    Missing {
        /// Which member.
        field: &'static str,
    },
}

/// What one tool call produced.
///
/// Inert text and two facts about it. Nothing here decides what a model is
/// shown or what a reader sees; that belongs above this crate, which is why a
/// cut is reported rather than rendered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answered {
    /// What the server produced, cut to [`RESULT_BYTES`].
    text: Box<str>,
    /// Whether the server said the tool itself failed.
    failed: bool,
    /// Bytes of the server's own text that are not in [`Self::text`].
    omitted: usize,
}

impl Answered {
    /// What the server produced.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Whether the server said the tool itself failed.
    #[must_use]
    pub const fn failed(&self) -> bool {
        self.failed
    }

    /// How many bytes of the server's own text are missing from this one.
    ///
    /// Zero is the whole result. Anything else is a result that was cut, and a
    /// caller showing it to a model owes the model that fact.
    #[must_use]
    pub const fn omitted(&self) -> usize {
        self.omitted
    }
}

/// Calls one tool the server offered, and reads back what it produced.
///
/// # Errors
///
/// [`Unanswered`] where the conversation fails or the answer is not the shape
/// the protocol gives. A tool that ran and failed is not an error here.
pub fn call<R: BufRead, W: Write>(
    talking: &mut Talking<R, W>,
    tool: &Offered,
    arguments: &Value,
) -> Result<Answered, Unanswered> {
    let answer = talking.ask(
        "tools/call",
        &json!({ "name": tool.name(), "arguments": arguments }),
    )?;

    let Some(content) = answer.get("content").and_then(Value::as_array) else {
        return Err(Unanswered::Missing { field: "content" });
    };

    // One walk, so what was left out is counted by the same step that decided
    // to leave it out. Two loops would let a bound be removed without the count
    // that reports it noticing.
    let mut said = String::new();
    let mut omitted = 0usize;
    for (read, block) in content.iter().enumerate() {
        let shown = shown(block);
        if read >= BLOCKS {
            omitted = omitted.saturating_add(shown.len());
            continue;
        }
        if !said.is_empty() {
            said.push('\n');
        }
        said.push_str(&shown);
    }

    let (text, cut) = cut(said);
    Ok(Answered {
        text,
        // Absent is a result that worked. The member says a tool failed, and a
        // server that leaves it off has not said anything went wrong.
        failed: answer
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        omitted: omitted.saturating_add(cut),
    })
}

/// One content block, as the text crucible has for it.
///
/// A kind crucible cannot show becomes its own name and nothing else. Naming it
/// rather than dropping it is the difference between a model reading a shorter
/// answer and a model reading an answer it does not know is shorter; carrying
/// its bytes is how a megabyte of base64 gets into a context as prose.
fn shown(block: &Value) -> String {
    let kind = block
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match block.get("text").and_then(Value::as_str) {
        Some(said) if kind == "text" => said.to_owned(),
        _ => format!("[{kind}, which crucible has no way to show]"),
    }
}

/// Holds `said` to [`RESULT_BYTES`], taking any excess out of the middle.
///
/// The middle, because the two ends are the parts that carry meaning: a tool
/// says what it was doing at the start and how it went at the end, and a result
/// cut off at the ceiling loses the half that says whether it worked.
fn cut(said: String) -> (Box<str>, usize) {
    if said.len() <= RESULT_BYTES {
        return (said.into(), 0);
    }
    // The marker is part of what is retained, so the two halves have to leave
    // room for it or the result would come back over its own ceiling.
    let room = RESULT_BYTES.saturating_sub(CUT.len());
    let head = floor(&said, room / 2);
    let tail = ceiling(&said, said.len() - (room - room / 2));
    let mut kept = String::with_capacity(RESULT_BYTES);
    kept.push_str(said.get(..head).unwrap_or_default());
    kept.push_str(CUT);
    kept.push_str(said.get(tail..).unwrap_or_default());
    (
        kept.into(),
        said.len().saturating_sub(head + (said.len() - tail)),
    )
}

/// The nearest character boundary at or below `at`.
fn floor(said: &str, at: usize) -> usize {
    let mut at = at.min(said.len());
    while at > 0 && !said.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// The nearest character boundary at or above `at`.
fn ceiling(said: &str, at: usize) -> usize {
    let mut at = at.min(said.len());
    while at < said.len() && !said.is_char_boundary(at) {
        at += 1;
    }
    at
}

#[cfg(test)]
mod tests;
