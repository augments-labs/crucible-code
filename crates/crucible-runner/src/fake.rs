//! Stand-ins for the collaborators a turn needs.
//!
//! The runner names nothing concrete, so a test can hand it a provider that
//! answers from a list and tools that answer from a field. What is exercised is
//! the loop itself: what it sends, what it records, and when it stops.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash as _, Hasher as _};
use std::sync::{Arc, Mutex};

use crucible_core::{
    Approved, Ask, Cancel, Delta, DeltaStream, DescribeTool, Diff, Effort, Message, Modalities,
    Modality, Provider, ProviderError, Remember, Request, Sensitivity, Steer, Summary, Target,
    Tool, ToolArgs, ToolCall, ToolContext, ToolError, ToolOutput, Verdict, Wrote,
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
    pub(crate) tools: Vec<SentToolSchema>,
    pub(crate) max_tokens: u32,
    pub(crate) effort: Option<Effort>,
    /// Whether this request carried a system prompt at all.
    ///
    /// The text is not kept because no test here reads it. What is worth
    /// recording is the yes or no: one request a turn deliberately sends none,
    /// and nothing else could tell that request apart from the ordinary ones.
    pub(crate) had_system: bool,
}

/// Owned evidence of one borrowed provider projection.
#[derive(Debug)]
pub(crate) struct SentToolSchema {
    pub(crate) name: Box<str>,
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
    refuses: Option<u16>,
    breaks: bool,
    /// How many more requests go away before they have said anything.
    drops: Mutex<usize>,
    /// A line typed into this queue as the first request goes out.
    types: Mutex<Option<(Steer, Box<str>)>>,

    /// Whether every request is refused for not fitting the window.
    ///
    /// Separate from `refuses`, which carries a status: this refusal has none
    /// to carry. It is the one a provider gives before it has read anything,
    /// and the only refusal the loop answers by making room rather than by
    /// handing back.
    over_window: bool,
}

impl Script {
    /// Answers each request with the next round, and with nothing once the
    /// rounds run out.
    pub(crate) fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            rounds: Mutex::new(rounds.into()),
            sent: Sent::default(),
            refuses: None,
            breaks: false,
            drops: Mutex::new(0),
            types: Mutex::new(None),
            over_window: false,
        }
    }

    /// A provider that refuses every request for not fitting the window.
    ///
    /// The refusal that arrives instead of an answer, rather than the stop
    /// reason that arrives inside one. They are two different rails through
    /// the loop and only this one is a [`ProviderError`].
    pub(crate) fn over_window() -> Self {
        Self {
            over_window: true,
            ..Self::new(Vec::new())
        }
    }

    /// A provider that types `line` into `steer` as its first request goes
    /// out, and answers from the script.
    ///
    /// The moment a reader actually types: while an answer is arriving, which
    /// is after the pass drained the queue and before the tools it asks for
    /// run. [`Typing`] covers the other one — typed while a call is out — and
    /// between them they are the two places a line can appear inside a pass.
    pub(crate) fn typing(steer: Steer, line: &str, rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            types: Mutex::new(Some((steer, line.into()))),
            ..Self::new(rounds)
        }
    }

    /// A provider that refuses every request, with a status nothing recovers
    /// from.
    pub(crate) fn failing() -> Self {
        Self::refusing(401)
    }

    /// A provider that refuses every request with `status`.
    pub(crate) fn refusing(status: u16) -> Self {
        Self {
            refuses: Some(status),
            ..Self::new(Vec::new())
        }
    }

    /// A provider whose first `drops` requests go away before they have said
    /// anything, and which answers from the script after that.
    ///
    /// The connection a provider closed while the tools ran: the request is
    /// accepted and the stream produces nothing at all, which is the one shape
    /// the loop may ask for again.
    pub(crate) fn dropping(drops: usize, rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            drops: Mutex::new(drops),
            ..Self::new(rounds)
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

    /// A stand-in spells what every real provider here spells today.
    ///
    /// It is not a wire protocol, so it has nothing of its own to declare; what
    /// it must not do is claim more, or a test would be exercising a capability
    /// no provider has. Pictures are in because all three wires write one; a
    /// PDF is not, because only two of them do.
    fn spells(&self) -> Modalities {
        Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
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
                    Message::Context(_) | Message::User { .. } | Message::ToolResults(_) => None,
                })
                .collect(),
            tools: request
                .tools
                .iter()
                .map(|tool| SentToolSchema {
                    name: tool.name.into(),
                })
                .collect(),
            max_tokens: request.max_tokens,
            effort: request.effort,
            had_system: request.system.is_some(),
        });

        // Before anything is answered: the line is meant to arrive while the
        // request is out, not once it has been read.
        if let Some((steer, line)) = self.types.lock().unwrap().take() {
            steer.say(line.into());
        }

        if self.over_window {
            return Err(ProviderError::WindowExceeded { provider: SCRIPT });
        }

        if let Some(status) = self.refuses {
            return Err(ProviderError::Refused {
                provider: SCRIPT,
                status,
                message: "no".into(),
            });
        }

        // Before a round is taken, because a response that went away said
        // nothing and cost the script nothing: the answer it was going to give
        // is still the next one.
        let mut drops = self.drops.lock().unwrap();
        if *drops > 0 {
            *drops -= 1;
            return Ok(Box::new(Recited {
                deltas: VecDeque::new(),
                breaks: true,
            }));
        }
        drop(drops);

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
    writes: Vec<Box<str>>,
    backgroundable: bool,
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
            writes: Vec::new(),
            backgroundable: false,
        }
    }

    /// What it prints, one piece at a time, before it answers.
    pub(crate) fn writing(mut self, pieces: &[&str]) -> Self {
        self.writes = pieces.iter().map(|piece| (*piece).into()).collect();
        self
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

    /// Makes it a tool whose calls can be left running.
    pub(crate) fn detachable(mut self) -> Self {
        self.backgroundable = true;
        self
    }

    /// Makes it a tool that rewrote a file and has the change to show for it.
    pub(crate) fn showing(mut self, diff: Diff) -> Self {
        self.diff = Some(diff);
        self
    }
}

impl DescribeTool for Fixed {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Fixed {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
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

    fn backgroundable(&self, _args: &ToolArgs) -> bool {
        self.backgroundable
    }

    fn run(&self, _approved: Approved, context: &ToolContext<'_>) -> Result<ToolOutput, ToolError> {
        for piece in &self.writes {
            context.wrote(Wrote::new(piece.clone()));
        }

        if self.cancels {
            return Err(ToolError::Cancelled(self.name.into()));
        }

        match &self.problem {
            Some(problem) => Err(ToolError::Arguments {
                tool: self.name.into(),
                problem: problem.clone(),
            }),
            None => Ok(match &self.diff {
                Some(diff) => ToolOutput::ok(self.answer.clone()).showing(diff.clone()),
                None => ToolOutput::ok(self.answer.clone()),
            }),
        }
    }
}

/// A tool that types a line into the reader's queue while it runs.
///
/// The one moment a steered line can arrive between a call and its answer is
/// while that call is out, and nothing else here can reach it: the queue is
/// pushed to from the thread that reads the keyboard, and a test has only the
/// thread the turn runs on.
pub(crate) struct Typing {
    name: &'static str,
    steer: Steer,
    line: Box<str>,
}

impl Typing {
    /// A read-only tool that says `line` as the reader would, then answers.
    pub(crate) fn new(name: &'static str, steer: Steer, line: &str) -> Self {
        Self {
            name,
            steer,
            line: line.into(),
        }
    }
}

impl DescribeTool for Typing {
    fn name(&self) -> &str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Typing {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        self.steer.say(self.line.to_string());
        Ok(ToolOutput::ok("done"))
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
