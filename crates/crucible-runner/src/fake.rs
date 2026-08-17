//! Stand-ins for the collaborators a turn needs.
//!
//! The runner names nothing concrete, so a test can hand it a provider that
//! answers from a list and tools that answer from a field. What is exercised is
//! the loop itself: what it sends, what it records, and when it stops.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};

use crucible_core::{
    Approved, Ask, Cancel, Delta, DeltaStream, Diff, Effort, Message, Provider, ProviderError,
    Remember, Request, Sensitivity, Summary, Target, Tool, ToolArgs, ToolCall, ToolError,
    ToolOutput, ToolSchema, Verdict,
};

/// The name a scripted provider answers to.
const SCRIPT: &str = "script";

/// Every request a provider was given, shared with the test that made it.
pub(crate) type Sent = Arc<Mutex<Vec<SentRequest>>>;

/// Fixed-size request evidence retained after its borrowed view is gone.
#[derive(Debug)]
pub(crate) struct SentRequest {
    pub(crate) transcript_len: usize,
    agent_text: Vec<u64>,
    pub(crate) tools: Vec<ToolSchema>,
    pub(crate) effort: Option<Effort>,
}

impl SentRequest {
    /// Whether the request carried an agent answer with exactly this text.
    pub(crate) fn carried(&self, text: &str) -> bool {
        self.agent_text.contains(&fingerprint(text))
    }
}

fn fingerprint(text: &str) -> u64 {
    let mut hash = DefaultHasher::new();
    text.hash(&mut hash);
    hash.finish()
}

/// A provider that answers from a script, one round per request.
pub(crate) struct Script {
    rounds: Mutex<VecDeque<Vec<Delta>>>,
    sent: Sent,
    refuses: bool,
    breaks: bool,
}

impl Script {
    /// Answers each request with the next round, and with nothing once the
    /// rounds run out.
    pub(crate) fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            rounds: Mutex::new(rounds.into()),
            sent: Sent::default(),
            refuses: false,
            breaks: false,
        }
    }

    /// A provider that refuses every request.
    pub(crate) fn failing() -> Self {
        Self {
            refuses: true,
            ..Self::new(Vec::new())
        }
    }

    /// A provider whose connection breaks once the round's deltas have been
    /// handed over.
    ///
    /// The failure the loop cannot treat as an ending: the deltas were posted,
    /// so the user has already read them, and nothing the provider sends after
    /// them says how the answer was meant to finish.
    pub(crate) fn breaking(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            breaks: true,
            ..Self::new(rounds)
        }
    }

    /// A handle on what it was asked, kept by the test before it hands the
    /// provider over.
    pub(crate) fn sent(&self) -> Sent {
        Arc::clone(&self.sent)
    }
}

impl Provider for Script {
    fn name(&self) -> &'static str {
        SCRIPT
    }

    fn stream(
        &self,
        request: Request<'_>,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        self.sent.lock().unwrap().push(SentRequest {
            transcript_len: request.transcript.len(),
            agent_text: request
                .transcript
                .messages()
                .iter()
                .filter_map(|message| match message {
                    Message::Agent { text, .. } => Some(fingerprint(text)),
                    Message::User(_) | Message::ToolResults(_) => None,
                })
                .collect(),
            tools: request.tools.to_vec(),
            effort: request.effort,
        });

        if self.refuses {
            return Err(ProviderError::Refused {
                provider: SCRIPT,
                status: 401,
                message: "no".into(),
            });
        }

        let round = self.rounds.lock().unwrap().pop_front().unwrap_or_default();
        Ok(Box::new(Recited {
            deltas: round.into(),
            breaks: self.breaks,
        }))
    }
}

/// A round already in memory, handed out one delta at a time.
struct Recited {
    deltas: VecDeque<Delta>,
    /// Whether running out of deltas is a broken connection rather than the end
    /// of the answer.
    breaks: bool,
}

impl DeltaStream for Recited {
    fn next(&mut self) -> Option<Result<Delta, ProviderError>> {
        if let Some(delta) = self.deltas.pop_front() {
            return Some(Ok(delta));
        }

        self.breaks.then(|| {
            self.breaks = false;
            Err(ProviderError::Transport {
                provider: SCRIPT,
                problem: "the connection went away".into(),
            })
        })
    }
}

/// A tool whose answer is decided before it runs.
pub(crate) struct Fixed {
    name: &'static str,
    answer: Box<str>,
    problem: Option<Box<str>>,
    cancels: bool,
    sensitivity: Sensitivity,
    diff: Option<Diff>,
}

impl Fixed {
    /// A read-only tool that succeeds.
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            answer: "done".into(),
            problem: None,
            cancels: false,
            sensitivity: Sensitivity::ReadOnly {
                target: Target::unresolved(),
            },
            diff: None,
        }
    }

    /// What it produces when it succeeds.
    pub(crate) fn answering(mut self, text: &str) -> Self {
        self.answer = text.into();
        self
    }

    /// Makes it report that it could not carry the call out.
    pub(crate) fn breaking(mut self, problem: &str) -> Self {
        self.problem = Some(problem.into());
        self
    }

    /// Makes it notice a cancellation part way through its work.
    pub(crate) fn cancelling(mut self) -> Self {
        self.cancels = true;
        self
    }

    /// How dangerous it claims to be, which is what decides whether the user
    /// is asked.
    pub(crate) fn risking(mut self, sensitivity: Sensitivity) -> Self {
        self.sensitivity = sensitivity;
        self
    }

    /// Makes it a tool that rewrote a file and has the change to show for it.
    pub(crate) fn showing(mut self, diff: Diff) -> Self {
        self.diff = Some(diff);
        self
    }
}

impl Tool for Fixed {
    fn name(&self) -> &'static str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        self.sensitivity.clone()
    }

    /// The arguments as they arrived. A real tool names one field of them;
    /// what a test needs is to see that whatever the tool said reached the
    /// other end, so this says something no other value could be mistaken for.
    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run(&self, _approved: Approved) -> Result<ToolOutput, ToolError> {
        if self.cancels {
            return Err(ToolError::Cancelled(self.name));
        }

        match &self.problem {
            Some(problem) => Err(ToolError::Arguments {
                tool: self.name,
                problem: problem.clone(),
            }),
            None => Ok(match &self.diff {
                Some(diff) => ToolOutput::ok(self.answer.clone()).showing(diff.clone()),
                None => ToolOutput::ok(self.answer.clone()),
            }),
        }
    }
}

/// A call that gets the user asked: a change to a file, which no mode waves
/// through except `fullAccess`.
///
/// The target is one nothing resolved, so no rule written about a path matches
/// it and what the tests here exercise is the loop rather than the matcher.
pub(crate) fn changing() -> Sensitivity {
    Sensitivity::MutatesFile {
        target: Target::unresolved(),
    }
}

/// A user who answers every question the same way, and counts them.
pub(crate) struct Says {
    verdict: Verdict,
    remember: Remember,
    /// How often the user was put to the question.
    pub(crate) asked: usize,
}

impl Says {
    /// Answers `verdict`, for this call only.
    pub(crate) fn new(verdict: Verdict) -> Self {
        Self {
            verdict,
            remember: Remember::Never,
            asked: 0,
        }
    }

    /// Answers the same way, and asks for it to hold until the session ends.
    pub(crate) fn for_the_session() -> Self {
        Self {
            verdict: Verdict::Allow,
            remember: Remember::Session,
            asked: 0,
        }
    }
}

impl Ask for Says {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        self.asked += 1;
        (self.verdict, self.remember)
    }
}
