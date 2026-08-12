//! The transcript: the ordered record of turns.
//!
//! One transcript per session, held in memory and sent to the provider on
//! every turn. It grows for the life of the session, so it owns its text
//! rather than borrowing, and nothing on the render path copies it.
//!
//! A turn is one prompt plus the exchange until the agent yields, so turns are
//! delimited by [`Message::User`]: everything after one, until the next, is
//! that turn.

use crate::ids::ToolId;
use crate::tool::{ToolCall, ToolOutput};

/// One message in the transcript.
///
/// A closed set. A provider translates each variant into its own wire shape,
/// so a new variant must break every provider that has not handled it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// What the user typed. Starts a turn.
    User(Box<str>),

    /// What the model produced: prose, tool calls, or both.
    Agent {
        /// The text, which may be empty when the model only called tools.
        text: Box<str>,
        /// The tools it asked for, in the order it asked.
        calls: Vec<ToolCall>,
    },

    /// What the tools produced, matched back to the calls that asked.
    ToolResults(Vec<ToolResult>),
}

/// One tool's answer, paired with the call it answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// The identifier from the call this answers.
    pub id: ToolId,
    /// What the tool produced.
    pub output: ToolOutput,
}

/// Why the model stopped producing output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// It finished its answer and is yielding to the user.
    Yielded,
    /// It is waiting for tool results before continuing.
    WantsTools,
    /// It hit the token limit for the response.
    OutOfTokens,
    /// The provider's filter cut the answer short.
    ///
    /// Truncation, like [`Self::OutOfTokens`], and separate from it because the
    /// remedy is different: no smaller request will help. Its own variant
    /// rather than folding into a finish, because an answer that stops here is
    /// incomplete and saying otherwise is the thing this enum exists to stop.
    Filtered,
    /// The provider paused a turn it expects to be asked to carry on.
    ///
    /// Not an ending, which it shares with [`Self::WantsTools`] and with no
    /// other variant here: the answer is unfinished, and what the provider is
    /// waiting for is the transcript back with this much of it already in.
    /// 0.0.x does not carry on by itself, so what this buys is the user being
    /// told rather than handed a paused answer that reads as a complete one.
    Paused,
    /// The user cancelled.
    Cancelled,
}

/// The ordered record of turns.
#[derive(Debug, Clone, Default)]
pub struct Transcript {
    messages: Vec<Message>,
}

impl Transcript {
    /// An empty transcript.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends a message.
    pub fn push(&mut self, message: Message) {
        self.messages.push(message);
    }

    /// Every message, in order — what a provider serialises.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Forgets everything said so far.
    ///
    /// The whole point is the memory as much as the meaning: this is the only
    /// thing here that grows with a session, and the peak-RSS budget is set to
    /// cover it. So the vector goes rather than being emptied — `Vec::clear`
    /// keeps the room the longest transcript ever needed, and a session cleared
    /// because it had become too large would carry that size to the end.
    pub fn forget(&mut self) {
        self.messages = Vec::new();
    }

    /// How many messages the transcript holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Whether the session has said anything yet.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// How many turns have started — the number of user prompts.
    #[must_use]
    pub fn turns(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| matches!(message, Message::User(_)))
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_transcript_is_empty() {
        let transcript = Transcript::new();
        assert!(transcript.is_empty());
        assert_eq!(transcript.len(), 0);
        assert_eq!(transcript.turns(), 0);
    }

    #[test]
    fn turns_are_counted_by_user_prompts() {
        let mut transcript = Transcript::new();
        transcript.push(Message::User("first".into()));
        transcript.push(Message::Agent {
            text: "answer".into(),
            calls: Vec::new(),
        });
        transcript.push(Message::User("second".into()));

        assert_eq!(transcript.turns(), 2);
        assert_eq!(transcript.len(), 3);
    }

    #[test]
    fn a_transcript_that_forgot_has_nothing_in_it_and_no_turns_behind_it() {
        let mut transcript = Transcript::new();
        transcript.push(Message::User("said".into()));
        transcript.push(Message::User("said again".into()));

        transcript.forget();

        assert!(transcript.is_empty());
        assert_eq!(transcript.len(), 0);
        assert_eq!(transcript.turns(), 0);
        assert_eq!(transcript.messages(), []);
    }
}
