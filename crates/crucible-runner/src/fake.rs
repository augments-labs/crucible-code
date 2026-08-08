//! Stand-ins for the collaborators a turn needs.
//!
//! The runner names nothing concrete, so a test can hand it a provider that
//! answers from a list and tools that answer from a field. What is exercised is
//! the loop itself: what it sends, what it records, and when it stops.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crucible_core::{
    Ask, Cancel, Delta, DeltaStream, Grant, Provider, ProviderError, Request, Sensitivity, Tool,
    ToolArgs, ToolCall, ToolError, ToolOutput, Verdict,
};

/// The name a scripted provider answers to.
const SCRIPT: &str = "script";

/// Every request a provider was given, shared with the test that made it.
pub(crate) type Sent = Arc<Mutex<Vec<Request>>>;

/// A provider that answers from a script, one round per request.
pub(crate) struct Script {
    rounds: Mutex<VecDeque<Vec<Delta>>>,
    sent: Sent,
    refuses: bool,
}

impl Script {
    /// Answers each request with the next round, and with nothing once the
    /// rounds run out.
    pub(crate) fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            rounds: Mutex::new(rounds.into()),
            sent: Sent::default(),
            refuses: false,
        }
    }

    /// A provider that refuses every request.
    pub(crate) fn failing() -> Self {
        Self {
            refuses: true,
            ..Self::new(Vec::new())
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
        request: Request,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        self.sent.lock().unwrap().push(request);

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
        }))
    }
}

/// A round already in memory, handed out one delta at a time.
struct Recited {
    deltas: VecDeque<Delta>,
}

impl DeltaStream for Recited {
    fn next(&mut self) -> Option<Result<Delta, ProviderError>> {
        self.deltas.pop_front().map(Ok)
    }
}

/// A tool whose answer is decided before it runs.
pub(crate) struct Fixed {
    name: &'static str,
    answer: Box<str>,
    problem: Option<Box<str>>,
    cancels: bool,
    sensitivity: Sensitivity,
}

impl Fixed {
    /// A read-only tool that succeeds.
    pub(crate) fn new(name: &'static str) -> Self {
        Self {
            name,
            answer: "done".into(),
            problem: None,
            cancels: false,
            sensitivity: Sensitivity::ReadOnly,
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
}

impl Tool for Fixed {
    fn name(&self) -> &'static str {
        self.name
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        self.sensitivity
    }

    fn run(&self, _args: ToolArgs, _grant: Grant) -> Result<ToolOutput, ToolError> {
        if self.cancels {
            return Err(ToolError::Cancelled(self.name));
        }

        match &self.problem {
            Some(problem) => Err(ToolError::Arguments {
                tool: self.name,
                problem: problem.clone(),
            }),
            None => Ok(ToolOutput::ok(self.answer.clone())),
        }
    }
}

/// A user who answers every question the same way, and counts them.
pub(crate) struct Says {
    verdict: Verdict,
    /// How often the user was put to the question.
    pub(crate) asked: usize,
}

impl Says {
    pub(crate) fn new(verdict: Verdict) -> Self {
        Self { verdict, asked: 0 }
    }
}

impl Ask for Says {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: Sensitivity) -> Verdict {
        self.asked += 1;
        self.verdict
    }
}
