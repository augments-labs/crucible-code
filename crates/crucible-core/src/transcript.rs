//! The transcript: the ordered record of turns.
//!
//! One transcript per session, held in memory and sent to the provider on
//! every turn. It grows for the life of the session, so it owns its text
//! rather than borrowing, and nothing on the render path copies it.
//!
//! A turn is one prompt plus the exchange until the agent yields, so turns are
//! delimited by [`Message::User`]: everything after one, until the next, is
//! that turn.

use std::fmt;

use crate::ids::ToolId;
use crate::tool::{ToolCall, ToolOutput};

/// One message in the transcript.
///
/// A closed set. A provider translates each variant into its own wire shape,
/// so a new variant must break every provider that has not handled it.
#[derive(Clone, PartialEq, Eq)]
pub enum Message {
    /// What the user typed. Starts a turn.
    User(Box<str>),

    /// What the model produced: prose, tool calls, or both.
    Agent {
        /// The text, which may be empty when the model only called tools.
        text: Box<str>,
        /// The tools it asked for, in the order it asked.
        calls: Vec<ToolCall>,
        /// How the answer ended, or `None` where it never reached an ending —
        /// a response that broke off part way through.
        ///
        /// Carried on the message rather than reported once and forgotten,
        /// because the transcript outlives the turn: it is written to the
        /// session log, read back by `--continue`, and sent to the model on
        /// every turn after this one. Without it a half-sentence goes back to
        /// the model as an answer it chose to end.
        stop: Option<StopReason>,
    },

    /// What the tools produced, matched back to the calls that asked.
    ToolResults(Vec<ToolResult>),
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(_) => f.debug_tuple("User").field(&"[redacted]").finish(),
            Self::Agent { calls, stop, .. } => f
                .debug_struct("Agent")
                .field("text", &"[redacted]")
                .field("calls", &calls.len())
                .field("stop", stop)
                .finish(),
            Self::ToolResults(results) => f
                .debug_tuple("ToolResults")
                .field(&format_args!("{} redacted", results.len()))
                .finish(),
        }
    }
}

/// One tool's answer, paired with the call it answers.
#[derive(Clone, PartialEq, Eq)]
pub struct ToolResult {
    /// The identifier from the call this answers.
    pub id: ToolId,
    /// What the tool produced.
    pub output: ToolOutput,
}

impl fmt::Debug for ToolResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolResult")
            .field("id", &"[redacted]")
            .field("output", &"[redacted]")
            .finish()
    }
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
    /// The request did not fit the model's window.
    ///
    /// Its own variant rather than sharing [`Self::OutOfTokens`], because the
    /// two are opposite failures wearing one word. That one is an answer that
    /// ran out of room to finish in, and the turn carries on around it. This is
    /// a request the model could not read at all, and no answer was produced —
    /// the turn cannot go anywhere until the session sent is made smaller.
    ///
    /// Folded together, the recoverable one gets a remedy it does not need and
    /// this one gets a remedy that cannot work: asking again, unchanged, for a
    /// request that did not fit the first time.
    WindowExceeded,
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
    /// 0.x does not carry on by itself, so what this buys is the user being
    /// told rather than handed a paused answer that reads as a complete one.
    Paused,
    /// The user cancelled.
    Cancelled,
    /// The provider named a reason this build has never heard of.
    ///
    /// Every other variant here says what happened; this one says only that
    /// nobody knows, which is why it exists rather than a fallback to
    /// [`Self::Yielded`]. A vendor's list grows, and the day it does, the arm
    /// that catches the new word decides whether an answer that was cut short
    /// arrives looking complete. Reading it as unfinished is wrong at worst
    /// about a turn that was fine; reading it as a finish is wrong about the
    /// one failure the user cannot see for themselves.
    ///
    /// It is not a licence to stop mapping reasons. A new one in a vendor's
    /// list is still an edit to that provider — this is what holds until the
    /// edit is made.
    Unknown,
}

impl StopReason {
    /// What the model has to be told about an agent turn that ended this way,
    /// or `None` where the turn ended the way the model meant it to.
    ///
    /// Takes the option because the absence of a reason is itself one of the
    /// answers: a response that broke off never said why it stopped, and a
    /// turn that never said is no more finished than one that said it was cut
    /// short.
    ///
    /// The sentence lives here rather than in each provider so that two
    /// providers cannot disagree about what a cut-off turn is. Where it goes
    /// on the wire differs between them, and that part each of them owns.
    #[must_use]
    pub fn cut(stop: Option<Self>) -> Option<&'static str> {
        match stop {
            // The answer stops where the model meant it to, and a turn waiting
            // on tools has the calls beside it saying what it is waiting for.
            Some(Self::Yielded | Self::WantsTools) => None,

            Some(Self::OutOfTokens) => Some("[the answer above was cut off at the token ceiling]"),
            Some(Self::WindowExceeded) => {
                Some("[there was no room left in the window for the request above]")
            }
            Some(Self::Filtered) => {
                Some("[the answer above was cut short by the provider's filter]")
            }
            Some(Self::Paused) => Some("[the answer above was paused and was never carried on]"),
            Some(Self::Cancelled) => Some("[the answer above was stopped by the user]"),
            Some(Self::Unknown) => {
                Some("[the answer above stopped for an unknown reason and may be unfinished]")
            }
            None => Some("[the answer above was cut off before it finished]"),
        }
    }
}

/// The ordered record of turns.
#[derive(Clone, Default)]
pub struct Transcript {
    messages: Vec<Message>,
}

impl fmt::Debug for Transcript {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Transcript")
            .field(
                "messages",
                &format_args!("{} redacted", self.messages.len()),
            )
            .finish()
    }
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

    /// Drops the last `many` messages.
    ///
    /// What a compaction leaves behind, replayed: the notes that stood in for
    /// them go on next, so this is the half that takes them off. Fewer than
    /// `many` present is a log that says it replaced more than it holds, which
    /// is damage rather than a shape to guess at — everything goes, and the
    /// notes stand for the session so far.
    pub fn behind(&mut self, many: usize) {
        let keeping = self.messages.len().saturating_sub(many);
        self.messages.truncate(keeping);
    }

    /// Takes the last message back off.
    ///
    /// For the one caller that puts a message on to ask something and must not
    /// leave it there: an instruction the session never gave, still standing in
    /// the transcript, is a question the model answers again every turn after.
    pub fn pop(&mut self) -> Option<Message> {
        self.messages.pop()
    }

    /// Hands the messages out, consuming the transcript.
    ///
    /// So that a caller rebuilding one can move what it keeps rather than
    /// cloning it. This is the only value here that grows with the session, and
    /// two of them alive at once is what the peak-memory budget refuses.
    #[must_use]
    pub fn into_messages(self) -> Vec<Message> {
        self.messages
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

    /// Clears the results named by `ids`, replacing each with a placeholder, and
    /// returns the bytes that freed.
    ///
    /// The mutation behind pruning. The runner decides which results are old
    /// enough to clear and names them here; the transcript owns the clearing so
    /// the change goes through one place rather than a hand-edited message. A
    /// name nothing answers to frees nothing — a result already dropped by a
    /// compaction, or an id a damaged log line carried, is simply absent. What
    /// the model is sent changes; the session log, which is the record, keeps
    /// the originals.
    pub fn prune(&mut self, ids: &[ToolId]) -> usize {
        let mut freed = 0;

        for message in &mut self.messages {
            if let Message::ToolResults(results) = message {
                for result in results.iter_mut() {
                    if ids.contains(&result.id) {
                        freed += result.output.prune();
                    }
                }
            }
        }

        freed
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
            stop: Some(StopReason::Yielded),
        });
        transcript.push(Message::User("second".into()));

        assert_eq!(transcript.turns(), 2);
        assert_eq!(transcript.len(), 3);
    }

    #[test]
    fn a_turn_that_ended_the_way_the_model_meant_it_to_is_not_marked() {
        // Every turn ends. A note under each one saying so would be spent on
        // the path taken every time, and the model would learn nothing from it.
        assert_eq!(StopReason::cut(Some(StopReason::Yielded)), None);
        assert_eq!(StopReason::cut(Some(StopReason::WantsTools)), None);
    }

    #[test]
    fn every_way_a_turn_can_be_cut_off_says_so_to_the_model() {
        // The half of this the live notice does not cover: the user is told on
        // screen, and the model is told here — on the next request, and on
        // every request after a session is continued. Listed by an exhaustive
        // `match` rather than an array, so a reason added to `StopReason` stops
        // the build here instead of being the one nobody worded.
        let every = [
            StopReason::OutOfTokens,
            StopReason::Filtered,
            StopReason::Paused,
            StopReason::Cancelled,
            StopReason::WindowExceeded,
            StopReason::Unknown,
        ];

        for stop in every {
            match stop {
                StopReason::OutOfTokens
                | StopReason::WindowExceeded
                | StopReason::Filtered
                | StopReason::Paused
                | StopReason::Cancelled
                | StopReason::Unknown => {}
                StopReason::Yielded | StopReason::WantsTools => continue,
            }

            assert!(
                StopReason::cut(Some(stop)).is_some(),
                "{stop:?} reads as a finished turn"
            );
        }
    }

    #[test]
    fn an_answer_that_never_reached_an_ending_is_marked_as_cut_off() {
        // A response that broke off part way says nothing about why it stopped.
        // Read as a finish it is a half-sentence the model is shown as a turn
        // it chose to end.
        assert!(StopReason::cut(None).is_some());
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

    #[test]
    fn transcript_debug_redacts_prompts_answers_and_tool_results() {
        let mut transcript = Transcript::new();
        transcript.push(Message::User("prompt-debug-canary".into()));
        transcript.push(Message::Agent {
            text: "answer-debug-canary".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });
        transcript.push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-debug-canary"),
            output: ToolOutput::ok("tool-debug-canary"),
        }]));

        let shown = format!("{transcript:?}");
        for canary in [
            "prompt-debug-canary",
            "answer-debug-canary",
            "call-debug-canary",
            "tool-debug-canary",
        ] {
            assert!(!shown.contains(canary), "{shown}");
        }
        assert!(shown.contains("redacted"));
    }
}
