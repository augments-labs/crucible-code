//! The transcript: the ordered record of turns.
//!
//! One transcript per session, held in memory and sent to the provider on
//! every turn. It grows for the life of the session, so it owns its text
//! rather than borrowing, and nothing on the render path copies it. A file
//! attached to a prompt is the one thing it does not own: it holds the path and
//! the bytes stay on disk, because what the transcript holds it holds for every
//! turn that follows.
//!
//! A turn is one prompt plus the exchange until the agent yields, so turns are
//! delimited by [`Message::User`]: everything after one, until the next, is
//! that turn.

use std::fmt;

use crate::context::Fragment;
use crate::ids::ToolId;
use crate::modality::Modality;
use crate::tool::{ToolCall, ToolOutput};

/// A file the user put in front of the model, named rather than carried.
///
/// The bytes stay on disk. A transcript is held whole for the life of a
/// session and is what the memory budget bounds, so a picture that lives in it
/// is a picture paid for on every turn after the one that sent it; a path is
/// paid for once. What reads the file is the request being built, and it is
/// read again for each one.
///
/// `hash` is taken when the file is attached and is the record of what was
/// sent. A session read back later can compare it and say the file has changed
/// since, rather than quietly sending different bytes under the same prompt. It
/// is not a cache key: nothing is stored under it and nothing looks a file up
/// by it.
#[derive(Clone, PartialEq, Eq)]
pub struct Attachment {
    /// Where the file is, already resolved against the workspace.
    pub path: Box<str>,
    /// Which kind of content it is, which is what decides whether a model can
    /// be sent it at all.
    pub modality: Modality,
    /// The media type the provider will label it with — `image/png` and its
    /// like. Determined from the file rather than from its name.
    pub media_type: Box<str>,
    /// SHA-256 of the file's bytes as they were when it was attached.
    pub hash: [u8; 32],
}

/// The path goes; the kind stays.
///
/// A path is user content — it names directories, and on most machines it names
/// the person. What kind of file it was is a fact about the shape of the turn
/// and gives away nothing about whose file it is, so a panic payload can still
/// say an image was attached. The hash goes with the path: it is not reversible,
/// but it confirms which file somebody was holding to anyone who already has a
/// copy, and a backtrace is the wrong place to settle that.
impl fmt::Debug for Attachment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Attachment")
            .field("path", &"[redacted]")
            .field("modality", &self.modality)
            .field("media_type", &self.media_type)
            .field("hash", &"[redacted]")
            .finish()
    }
}

/// One message in the transcript.
///
/// A closed set. A provider translates each variant into its own wire shape,
/// so a new variant must break every provider that has not handled it.
#[derive(Clone, PartialEq, Eq)]
pub enum Message {
    /// A typed harness fact rendered for this request and retained thereafter.
    ///
    /// Separate from [`Self::User`] so it starts no turn, is never attributed
    /// to the developer, and can be recognized after compaction without a
    /// user being able to forge its ownership by typing the same words.
    Context(Fragment),

    /// What the user typed, and the files they put beside it. Starts a turn.
    User {
        /// The prompt.
        text: Box<str>,
        /// The files named with it, in the order they were named. Empty for
        /// almost every prompt, which is why [`Message::said`] exists.
        attachments: Box<[Attachment]>,
    },

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

impl Message {
    /// A prompt with nothing attached to it.
    ///
    /// Most prompts are this, and the constructor is what keeps them reading
    /// the way they did before a message could carry a file.
    #[must_use]
    pub fn said(text: impl Into<Box<str>>) -> Self {
        Self::User {
            text: text.into(),
            attachments: Box::new([]),
        }
    }

    /// Every file this message holds, in the order it holds them.
    ///
    /// A prompt names its own; a message of tool results holds whatever each
    /// result attached, one result after the next. Flat rather than per
    /// result, because a place in the message is the whole of an attachment's
    /// address — what reads this weighs bytes against one request's ceiling
    /// and has no use for which call inside the message found which file.
    #[must_use]
    pub fn attachments(&self) -> Vec<&Attachment> {
        match self {
            Self::User { attachments, .. } => attachments.iter().collect(),
            Self::Context(_) | Self::Agent { .. } => Vec::new(),
            Self::ToolResults(results) => results
                .iter()
                .flat_map(|result| result.output.attachments())
                .collect(),
        }
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Context(fragment) => f.debug_tuple("Context").field(&fragment.section()).finish(),
            Self::User { attachments, .. } => f
                .debug_struct("User")
                .field("text", &"[redacted]")
                .field("attachments", attachments)
                .finish(),
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

    /// Removes every typed context fragment after history was summarized.
    ///
    /// A retained delta is not evidence that the full fragment it amended also
    /// survived. Dropping the complete family at the rewrite boundary makes
    /// reconciliation conservative and forces one fresh full rendering before
    /// the next provider request. User, agent, tool, and recap messages stay
    /// byte-for-byte as compaction selected them.
    pub fn forget_context(&mut self) {
        self.messages
            .retain(|message| !matches!(message, Message::Context(_)));
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
            .filter(|message| matches!(message, Message::User { .. }))
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
        transcript.push(Message::said("first"));
        transcript.push(Message::Agent {
            text: "answer".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });
        transcript.push(Message::said("second"));

        assert_eq!(transcript.turns(), 2);
        assert_eq!(transcript.len(), 3);
    }

    #[test]
    fn context_is_retained_without_starting_a_user_turn() {
        let mut transcript = Transcript::new();
        transcript.push(Message::Context(Fragment::new(
            "workspace",
            "Workspace: /private/project",
        )));

        assert_eq!(transcript.turns(), 0);
        assert_eq!(transcript.len(), 1);
        assert!(
            transcript
                .messages()
                .first()
                .expect("the context fragment")
                .attachments()
                .is_empty()
        );
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
        transcript.push(Message::said("said"));
        transcript.push(Message::said("said again"));

        transcript.forget();

        assert!(transcript.is_empty());
        assert_eq!(transcript.len(), 0);
        assert_eq!(transcript.turns(), 0);
        assert_eq!(transcript.messages(), []);
    }

    #[test]
    fn a_history_rewrite_can_remove_context_without_touching_the_conversation() {
        let mut transcript = Transcript::new();
        transcript.push(Message::said("keep the prompt"));
        transcript.push(Message::Context(Fragment::new("workspace", "private")));
        transcript.push(Message::Agent {
            text: "keep the answer".into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
        });

        transcript.forget_context();

        assert_eq!(transcript.len(), 2);
        assert!(matches!(
            transcript.messages().first(),
            Some(Message::User { .. })
        ));
        assert!(matches!(
            transcript.messages().get(1),
            Some(Message::Agent { .. })
        ));
    }

    #[test]
    fn transcript_debug_redacts_prompts_answers_and_tool_results() {
        let mut transcript = Transcript::new();
        transcript.push(Message::Context(Fragment::new(
            "workspace",
            "context-debug-canary",
        )));
        transcript.push(Message::said("prompt-debug-canary"));
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
            "context-debug-canary",
            "prompt-debug-canary",
            "answer-debug-canary",
            "call-debug-canary",
            "tool-debug-canary",
        ] {
            assert!(!shown.contains(canary), "{shown}");
        }
        assert!(shown.contains("redacted"));
    }

    #[test]
    fn debug_redacts_an_attachments_path_along_with_the_prompt_it_came_with() {
        // A path is the user's content as much as the prompt is: it names
        // directories, and on a home machine it usually names the person.
        let message = Message::User {
            text: "what is in this".into(),
            attachments: Box::new([Attachment {
                path: "/home/path-debug-canary/holiday.png".into(),
                modality: Modality::Image,
                media_type: "image/png".into(),
                hash: [0; 32],
            }]),
        };

        let shown = format!("{message:?}");

        assert!(!shown.contains("path-debug-canary"), "{shown}");
        assert!(!shown.contains("what is in this"), "{shown}");
        // What it may say is that there is one, and what kind: that is the
        // shape of the turn rather than anything the user wrote.
        assert!(shown.contains("Image"), "{shown}");
    }

    #[test]
    fn a_prompt_with_nothing_attached_reads_as_one() {
        assert_eq!(
            Message::said("go"),
            Message::User {
                text: "go".into(),
                attachments: Box::new([]),
            }
        );
    }
}
