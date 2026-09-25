//! A turn over steps that do not answer at once.
//!
//! A turn awaits the provider, the session, every call's run, a background
//! result's acceptance and the toolset's preparation, listing, refreshing
//! and disposal, and crosses to the prompt cache through a bridge that asks
//! once. The stand-ins here each leave exactly one such step unanswered at
//! first, so what ends the turn is what that step answers once the turn is
//! stopped. A provider that has not answered is awaited instead, and what
//! ends the turn then is what the provider answers once the turn is stopped.
//! The recap a compaction asks for is a request like any other.
//!
//! Two things decide what the ending is reported as. Awaiting across the stop
//! still ends the turn stopped: the model is not asked again. And each call
//! is answered with what its run answered, even where the stop came first —
//! a run that waited across the stop and then answered is committed as what
//! it answered.

use super::waiting::UntilStopped;
use super::*;
use crucible_core::{
    PromptCacheCapabilities, PromptCacheRoute, ToolDescriptor, ToolExecutionMode, ToolOutcome,
    ToolProvenance, ToolSourceKind,
};
use crucible_runtime::BoxFuture;

/// The part of a request a [`Stalling`] provider answers only once stopped.
#[derive(Debug, Clone, Copy)]
enum Stalls {
    /// Sending the request.
    Request,
    /// Reading the first delta of the response it opened.
    Response,
}

/// A provider that is sent a request and answers one part of it only once
/// the turn has been stopped, as a real one answers a stop.
///
/// It stops the turn itself the first time it has nothing to say, as a
/// reader pressing the key while it waited would. Everything a request is
/// built from is its script's, and so is the record of what it was sent.
struct Stalling {
    script: Script,
    at: Stalls,
}

impl Provider for Stalling {
    fn name(&self) -> &'static str {
        self.script.name()
    }

    fn spells(&self) -> Modalities {
        self.script.spells()
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        self.script.prompt_cache_capabilities(model)
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        self.script.prompt_cache_route()
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> PromptCacheEncoding {
        self.script.prompt_cache_encoding(request)
    }

    fn stream<'a>(
        &'a self,
        request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            // Kept in the script's record, so a test can count what went out.
            drop(self.script.stream(request, cancel).await);
            match self.at {
                Stalls::Request => {
                    UntilStopped {
                        cancel: cancel.clone(),
                        answer: Some(Err(ProviderError::Cancelled(self.script.name()))),
                    }
                    .await
                }
                Stalls::Response => Ok(Box::new(Silent {
                    cancel: cancel.clone(),
                    said: false,
                }) as Box<dyn DeltaStream>),
            }
        })
    }
}

/// A response that says nothing until the turn is stopped, then says once
/// that it was, and ends.
struct Silent {
    cancel: Cancel,
    said: bool,
}

impl DeltaStream for Silent {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        if std::mem::replace(&mut self.said, true) {
            return Box::pin(async { None });
        }
        Box::pin(UntilStopped {
            cancel: self.cancel.clone(),
            answer: Some(Some(Ok(Delta::Stopped(StopReason::Cancelled)))),
        })
    }
}

/// What is said of the one request `sent` recorded: the disposition the runner
/// holds for its attempt, and each one reported for that attempt, as posted
/// while it happened and as the session was asked to keep it.
fn dispositions(
    scripted: &Scripted,
    store: &Recording,
    sent: &Sent,
) -> (
    Option<PromptCacheRequestDisposition>,
    Vec<PromptCacheRequestDisposition>,
    Vec<PromptCacheRequestDisposition>,
) {
    let attempt = {
        let sent = sent.lock().unwrap();
        let [request] = sent.as_slice() else {
            panic!("{} requests went out, not one", sent.len());
        };
        request
            .cache_attempt
            .expect("a request goes out under a cache attempt")
    };
    let held = scripted
        .runner
        .prompt_cache_attempt()
        .filter(|held| held.id == attempt)
        .map(|held| held.disposition);
    let posted = scripted
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::PromptCache {
                fact: PromptCacheFact::RequestEncoded(fact),
            } if fact.attempt == attempt => Some(fact.disposition),
            _ => None,
        })
        .collect();
    let kept = store
        .journaled()
        .into_iter()
        .filter_map(|item| match item {
            RunItem::ProviderAttempt {
                fact: PromptCacheFact::RequestEncoded(fact),
                ..
            } if fact.attempt == attempt => Some(fact.disposition),
            _ => None,
        })
        .collect();
    (held, posted, kept)
}

/// Runs a turn whose provider answers `at` only once the turn is stopped.
///
/// The provider's step is awaited across the stop, not dropped, so what ends
/// the turn is what the provider answered: a request it had not sent is its
/// own cancellation, and a response it had opened ends stopped. It is not a
/// reason to send the request again either. The attempt is held and
/// reported as `disposition`: a send the provider says it cancelled was not
/// sent, and a request whose response was opened was accepted.
fn stalled(
    at: Stalls,
    disposition: PromptCacheRequestDisposition,
) -> Result<StopReason, TurnError> {
    let script = Script::new(vec![saying("never read")]);
    let sent = script.sent();
    let store = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(Vec::new()),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&store),
    );
    scripted.runner.provider = Box::new(Stalling { script, at });

    let turned = scripted.turn("go");

    assert_eq!(
        sent.lock().unwrap().len(),
        1,
        "{at:?}: the request went out again"
    );
    assert_eq!(
        dispositions(&scripted, &store, &sent),
        (Some(disposition), vec![disposition], vec![disposition]),
        "{at:?}: the attempt as held, as posted and as kept"
    );
    turned
}

#[test]
fn a_request_stopped_while_the_provider_had_not_answered_ends_as_the_provider_says() {
    let turned = stalled(Stalls::Request, PromptCacheRequestDisposition::NotSent);

    assert!(
        matches!(
            &turned,
            Err(TurnError::Provider(ProviderError::Cancelled(_)))
        ),
        "{turned:?}"
    );
}

#[test]
fn a_response_stopped_before_it_said_anything_ends_the_turn_stopped() {
    let turned = stalled(Stalls::Response, PromptCacheRequestDisposition::Accepted);

    assert_eq!(turned.unwrap(), StopReason::Cancelled);
}

/// Asks for a recap, after two turns answered in full, from a provider that
/// answers `at` only once the compaction is stopped.
///
/// The compaction ends stopped and replaces nothing, and the request for the
/// recap is not left in the transcript. The recap's attempt is held and
/// reported as `disposition`, for the reason [`stalled`] gives.
fn stalled_recap(at: Stalls, disposition: PromptCacheRequestDisposition) {
    let store = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![saying("first"), saying("second")]),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&store),
    );
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    let script = Script::new(vec![saying("never read")]);
    let sent = script.sent();
    scripted.runner.provider = Box::new(Stalling { script, at });
    let before = scripted.runner.state.transcript().messages().to_vec();

    let compacted = scripted.compacting();

    assert!(
        matches!(compacted, Ok(Room::Stopped)),
        "{at:?}: {compacted:?}"
    );
    assert_eq!(
        scripted.runner.state.transcript().messages(),
        before.as_slice(),
        "{at:?}: the transcript moved"
    );
    assert_eq!(
        dispositions(&scripted, &store, &sent),
        (Some(disposition), vec![disposition], vec![disposition]),
        "{at:?}: the recap's attempt as held, as posted and as kept"
    );
}

#[test]
fn a_recap_request_stopped_while_the_provider_had_not_answered_ends_the_compaction_stopped() {
    stalled_recap(Stalls::Request, PromptCacheRequestDisposition::NotSent);
}

#[test]
fn a_recap_stopped_before_it_said_anything_ends_the_compaction_stopped() {
    stalled_recap(Stalls::Response, PromptCacheRequestDisposition::Accepted);
}

/// A tool that raises the turn's stop as it runs answers at once, answers
/// only after it has waited and seen the stop, or raises the stop without
/// answering.
///
/// It raises the run's own stop rather than the one its context hands it: a
/// tool's run is handed a child of the run's, and a child's stop does not
/// reach the run it came from.
pub(super) struct Stopping {
    stop: Cancel,
    answers: Answers,
}

/// When a [`Stopping`] tool answers.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Answers {
    /// At once.
    AtOnce,
    /// Once it has waited and seen the stop.
    OnceStopped,
    /// After raising the stop, before it can answer.
    PanicAfterStop,
}

impl DescribeTool for Stopping {
    fn name(&self) -> &'static str {
        "stop"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Stopping {
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

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            match self.answers {
                Answers::AtOnce => {
                    self.stop.request();
                    Ok(ToolOutput::ok("stopped"))
                }
                Answers::OnceStopped => {
                    UntilStopped {
                        cancel: self.stop.clone(),
                        answer: Some(Ok(ToolOutput::ok("stopped"))),
                    }
                    .await
                }
                Answers::PanicAfterStop => {
                    self.stop.request();
                    panic!("the run came apart after the stop")
                }
            }
        })
    }
}

/// A turn whose one tool raises its stop while it runs, recorded to `store`.
pub(super) fn stopped_by(
    answers: Answers,
    rounds: Vec<Vec<Delta>>,
    store: Arc<Recording>,
) -> Scripted {
    let stop = Cancel::new();
    let mut offered = Tools::new();
    offered
        .add_builtin(Stopping {
            stop: stop.clone(),
            answers,
        })
        .unwrap();
    let mut scripted = Scripted::recording(Script::new(rounds), offered, Verdict::Allow, store);
    scripted.cancel = stop;
    scripted
}

/// What each message is, in order, by the calls and results it pairs.
pub(super) fn shape(messages: &[Message]) -> Vec<String> {
    messages
        .iter()
        .map(|message| match message {
            Message::Context(_) => "context".to_owned(),
            Message::User { text, .. } => format!("user {text}"),
            Message::Agent { calls, .. } if calls.is_empty() => "answer".to_owned(),
            Message::Agent { calls, .. } => {
                let ids: Vec<&str> = calls.iter().map(|call| call.id.as_str()).collect();
                format!("calls {}", ids.join(" "))
            }
            Message::ToolResults(results) => {
                let ids: Vec<&str> = results.iter().map(|result| result.id.as_str()).collect();
                format!("results {}", ids.join(" "))
            }
        })
        .collect()
}

#[test]
fn a_run_in_a_parallel_wave_that_waits_across_the_stop_ends_the_pass_stopped() {
    // The calls of a parallel wave are awaited as a call that runs alone is:
    // each run waits, sees the stop and answers, and each call is answered
    // with what its run answered, since that is what it did. The stop ends
    // the turn after that pass: the model is not asked again.
    let stop = Cancel::new();
    let provenance = ToolProvenance::new(
        ToolSourceKind::User,
        "test:stop",
        "a tool that stops the turn",
    )
    .unwrap();
    let descriptor =
        ToolDescriptor::new("stop", r#"{"type":"object","properties":{}}"#, provenance)
            .unwrap()
            .executing(ToolExecutionMode::Parallel);
    let mut offered = Tools::new();
    offered
        .add(
            descriptor,
            Arc::new(Stopping {
                stop: stop.clone(),
                answers: Answers::OnceStopped,
            }),
        )
        .unwrap();
    let mut scripted = Scripted::recording(
        Script::new(vec![vec![
            Delta::ToolStarted {
                id: ToolId::new("a"),
                name: "stop".into(),
            },
            Delta::ToolArgs("{}".into()),
            Delta::ToolStarted {
                id: ToolId::new("b"),
                name: "stop".into(),
            },
            Delta::ToolArgs("{}".into()),
            Delta::Stopped(StopReason::WantsTools),
        ]]),
        offered,
        Verdict::Allow,
        Recording::nowhere(),
    );
    scripted.cancel = stop;
    scripted.runner.policy.tools = crate::ToolScheduling::bounded(2).unwrap();

    let turned = scripted.turn("go");

    assert_eq!(turned.unwrap(), StopReason::Cancelled);
    assert_eq!(
        scripted.sent.lock().unwrap().len(),
        1,
        "the model was asked again after the stop"
    );
    assert_eq!(only_result(&scripted).output.text(), "stopped");
    let outcomes: Vec<ToolOutcome> = scripted
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ToolFinished {
                receipt: Some(receipt),
                ..
            } => Some(receipt.outcome()),
            _ => None,
        })
        .collect();
    assert_eq!(outcomes, [ToolOutcome::Succeeded, ToolOutcome::Succeeded]);
}

#[test]
fn a_run_that_waits_across_the_stop_ends_the_pass_stopped() {
    // A call that runs alone is awaited: its run waits, sees the stop, and
    // answers, and the call is answered with what its run answered; the stop
    // still ends the turn.
    let mut scripted = stopped_by(
        Answers::OnceStopped,
        vec![calling("a", "stop", "{}")],
        Recording::nowhere(),
    );

    let turned = scripted.turn("go");

    assert_eq!(turned.unwrap(), StopReason::Cancelled);
    assert_eq!(
        scripted.sent.lock().unwrap().len(),
        1,
        "the model was asked again after the stop"
    );
    assert_eq!(only_result(&scripted).output.text(), "stopped");
}

#[test]
fn a_run_that_raises_the_stop_and_then_comes_apart_ends_the_pass_stopped() {
    // A panic after the stop still leaves the stop as the reason the pass ends;
    // the contained answer does not make the loop ask the model again.
    let mut scripted = stopped_by(
        Answers::PanicAfterStop,
        vec![calling("a", "stop", "{}"), saying("asked again")],
        Recording::nowhere(),
    );

    let turned = scripted.turn("go");

    assert_eq!(
        scripted.sent.lock().unwrap().len(),
        1,
        "the model was asked again after the stop"
    );
    assert_eq!(turned.unwrap(), StopReason::Cancelled);
}
