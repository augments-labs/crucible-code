//! A provider that answers from a script, and a tool that answers from a field.
//!
//! The wiring's own tests need a whole turn to run — that is the only way the
//! thread, the two channels and the drain are exercised together — but they
//! must not need a network or a machine to run things on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crucible_core::{
    Approved, Cancel, Command, Delta, DeltaStream, Provider, ProviderError, Request, Sensitivity,
    Target, Tool, ToolArgs, ToolError, ToolOutput,
};

/// How many requests a script has been given, readable after it has moved into
/// a runner.
pub(crate) type Asked = Arc<AtomicUsize>;

/// What each request was asked under, in the order the requests arrived and
/// readable after the script has moved into a runner.
///
/// The system prompt is the one part of a request nothing on screen shows, and
/// half of what it says is about the session rather than about the harness — so
/// a session that changed model and went on saying it was the old one would
/// look right from every other angle.
pub(crate) type Under = Arc<Mutex<Vec<String>>>;

/// Answers each request with the next batch of deltas it was given.
#[derive(Debug)]
pub(crate) struct Script {
    rounds: Mutex<std::vec::IntoIter<Vec<Delta>>>,
    asked: Asked,
    under: Under,
    /// Refuses every request, for the path where the provider itself fails
    /// rather than the transcript going wrong inside a response.
    refusing: bool,
}

impl Script {
    pub(crate) fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            rounds: Mutex::new(rounds.into_iter()),
            asked: Asked::default(),
            under: Under::default(),
            refusing: false,
        }
    }

    /// A provider that will not answer at all.
    pub(crate) fn refusing() -> Self {
        Self {
            refusing: true,
            ..Self::new(Vec::new())
        }
    }

    /// A handle on the request count, taken before the script is handed over.
    pub(crate) fn asked(&self) -> Asked {
        Arc::clone(&self.asked)
    }

    /// A handle on what the requests were asked under, taken the same way and
    /// for the same reason: the script itself is inside the runner by the time
    /// there is anything to read.
    pub(crate) fn under(&self) -> Under {
        Arc::clone(&self.under)
    }
}

impl Provider for Script {
    fn name(&self) -> &'static str {
        "script"
    }

    fn stream(
        &self,
        request: Request,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        self.asked.fetch_add(1, Ordering::Relaxed);

        // A poisoned lock is a panic in another test's thread, which this one
        // cannot report better than by having nothing to assert on.
        if let Ok(mut under) = self.under.lock() {
            under.push(request.system.unwrap_or_default().into());
        }

        if self.refusing {
            return Err(ProviderError::Refused {
                provider: "script",
                status: 401,
                message: "no".into(),
            });
        }

        let round = self
            .rounds
            .lock()
            .map_err(|_| ProviderError::Transport {
                provider: "script",
                problem: "poisoned".into(),
            })?
            .next()
            .unwrap_or_default();

        Ok(Box::new(Reading(round.into_iter())))
    }
}

/// The deltas of one round, handed over one at a time.
struct Reading(std::vec::IntoIter<Delta>);

impl DeltaStream for Reading {
    fn next(&mut self) -> Option<Result<Delta, ProviderError>> {
        self.0.next().map(Ok)
    }
}

/// A tool that always produces the same thing, at a sensitivity it was told.
#[derive(Debug)]
pub(crate) struct Fixed {
    name: &'static str,
    answer: &'static str,
    sensitivity: Sensitivity,
}

impl Fixed {
    pub(crate) fn new(name: &'static str, sensitivity: Sensitivity) -> Self {
        Self {
            name,
            answer: "done",
            sensitivity,
        }
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

    fn run(&self, _approved: Approved) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok(self.answer))
    }
}

/// A change to a file: what every mode but `fullAccess` puts to the user.
///
/// The target is one nothing resolved, so no rule written about a path matches
/// it and what these tests exercise is the loop rather than the matcher.
pub(crate) fn changing() -> Sensitivity {
    Sensitivity::MutatesFile {
        target: Target::unresolved(),
    }
}

/// One program, run with nothing after it: the shape a rule can be minted from.
pub(crate) fn running(command: &str) -> Sensitivity {
    Sensitivity::SpawnsProcess {
        command: Command::Understood {
            parts: Box::from([Box::from(command)]),
        },
    }
}
