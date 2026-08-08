//! A provider that answers from a script, and a tool that answers from a field.
//!
//! The wiring's own tests need a whole turn to run — that is the only way the
//! thread, the two channels and the drain are exercised together — but they
//! must not need a network or a machine to run things on.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crucible_core::{
    Cancel, Delta, DeltaStream, Grant, Provider, ProviderError, Request, Sensitivity, Tool,
    ToolArgs, ToolError, ToolOutput,
};

/// How many requests a script has been given, readable after it has moved into
/// a runner.
pub(crate) type Asked = Arc<AtomicUsize>;

/// Answers each request with the next batch of deltas it was given.
#[derive(Debug)]
pub(crate) struct Script {
    rounds: Mutex<std::vec::IntoIter<Vec<Delta>>>,
    asked: Asked,
    /// Refuses every request, for the path where the provider itself fails
    /// rather than the transcript going wrong inside a response.
    refusing: bool,
}

impl Script {
    pub(crate) fn new(rounds: Vec<Vec<Delta>>) -> Self {
        Self {
            rounds: Mutex::new(rounds.into_iter()),
            asked: Asked::default(),
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
}

impl Provider for Script {
    fn name(&self) -> &'static str {
        "script"
    }

    fn stream(
        &self,
        _request: Request,
        _cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        self.asked.fetch_add(1, Ordering::Relaxed);

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

    fn run(&self, _args: ToolArgs, _grant: Grant) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok(self.answer))
    }
}
