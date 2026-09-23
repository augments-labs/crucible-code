//! A turn over a service that never answers one step.
//!
//! A turn awaits the provider, a call that runs alone and the toolset's
//! preparation and disposal, and crosses to the rest through a bridge that
//! asks once. A step crossed to that would have waited is dropped before it
//! answers, and whatever it began is unconfirmed. The stand-ins here each
//! leave exactly one such step unanswered, so a refusal can only have come
//! from that step. A provider that has not answered is awaited instead, and
//! what ends the turn then is what the provider answers once the turn is
//! stopped. The recap a compaction asks for is a request like any other.
//!
//! Three things decide what the refusal is reported as. A refusal that ends
//! the turn or a compaction is reported as the refusal, named for the crossing
//! it could not wait at, even while the turn or the compaction is being
//! stopped, never as a clean stop. A refused write of what a turn had reached
//! when it ended is carried beside the failure that ended it, never in its
//! place. And a tool's own run never ends the turn on a refusal: the call is
//! answered with a failed result that says what the run began is unconfirmed,
//! and where the turn is being stopped the pass then ends on the stop at that
//! call; where reporting the call's sandbox facts fails after the run, that
//! failure is the call's result instead, and the pass does not end on the
//! stop at that call.

use crucible_core::{
    Calibration, CallResultKey, CallResultReceipt, CallResultStoreError, Compacted, ContextError,
    ContextPatch, ContextSnapshot, PromptCacheCapabilities, PromptCacheRoute, SessionId,
    SessionOwner, ToolDescriptor, ToolExecutionMode, ToolOutcome, ToolProvenance, ToolSourceKind,
};
use crucible_runtime::{BoxFuture, Bridge};

use super::aiming::left_behind;
use super::pick_up::restrictions;
use super::waiting::UntilStopped;
use super::*;

/// The one kind of line a [`Withholding`] store never answers the write of.
#[derive(Debug, Clone, Copy)]
pub(super) enum Withheld {
    /// What clearing results another vendor may not be sent writes.
    Restricted,
    /// What a pruning writes.
    Pruned,
    /// An answer that asks for no tools, which is also what a failed request
    /// leaves of the answer it was reading.
    Answer,
    /// The results of a pass of calls.
    Results,
}

/// A store that keeps what its [`Recording`] keeps, except one kind of line.
///
/// Writing that line never answers, so a runner that cannot wait drops the
/// write and holds a refusal instead: whether the line was kept is not known.
#[derive(Debug)]
pub(super) struct Withholding {
    pub(super) recording: Arc<Recording>,
    pub(super) withheld: Withheld,
}

impl SessionStore for Withholding {
    fn session_id(&self) -> Option<SessionId> {
        self.recording.session_id()
    }

    fn owner(&self) -> Option<SessionOwner> {
        self.recording.owner()
    }

    fn append_message<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, ()> {
        let withheld = match (self.withheld, message) {
            (Withheld::Answer, Message::Agent { calls, .. }) => calls.is_empty(),
            (Withheld::Results, Message::ToolResults(_)) => true,
            _ => false,
        };
        if withheld {
            Box::pin(std::future::pending())
        } else {
            self.recording.append_message(message)
        }
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        self.recording.context_snapshot()
    }

    fn contextual<'a>(
        &'a self,
        patch: &'a ContextPatch,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        self.recording.contextual(patch)
    }

    fn compacted<'a>(&'a self, replaced: usize, recap: &'a str) -> BoxFuture<'a, ()> {
        self.recording.compacted(replaced, recap)
    }

    fn display_compacted(&self, compacted: Compacted, pruned: bool) -> BoxFuture<'_, ()> {
        self.recording.display_compacted(compacted, pruned)
    }

    fn pruned<'a>(&'a self, freed: usize, results: &'a [ToolId]) -> BoxFuture<'a, ()> {
        match self.withheld {
            Withheld::Pruned => Box::pin(std::future::pending()),
            Withheld::Restricted | Withheld::Answer | Withheld::Results => {
                self.recording.pruned(freed, results)
            }
        }
    }

    fn restricted<'a>(
        &'a self,
        freed: usize,
        results: &'a [ToolId],
        notice: &'a str,
    ) -> BoxFuture<'a, ()> {
        match self.withheld {
            Withheld::Restricted => Box::pin(std::future::pending()),
            Withheld::Pruned | Withheld::Answer | Withheld::Results => {
                self.recording.restricted(freed, results, notice)
            }
        }
    }

    fn measured<'a>(&'a self, calibration: &'a Calibration) -> BoxFuture<'a, ()> {
        self.recording.measured(calibration)
    }

    fn calibrated(&self) -> Option<Calibration> {
        self.recording.calibrated()
    }
}

impl JournalStore for Withholding {
    fn append_run_item(&self, item: &RunItem) {
        self.recording.append_run_item(item);
    }

    fn put_call_result(
        &self,
        key: CallResultKey,
        result: &ToolResult,
    ) -> Result<CallResultReceipt, CallResultStoreError> {
        self.recording.put_call_result(key, result)
    }

    fn settle_call_results(&self) {
        self.recording.settle_call_results();
    }
}

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

/// Asks the one failure a turn ended on out of the error that also carries
/// the refused write of what it had reached, checking that the reader is told
/// both, in exactly the words of `told`.
fn beside_refused_write(problem: TurnError, told: &str) -> TurnError {
    let shown = problem.to_string();
    match problem {
        TurnError::RecordUnready { primary, record } => {
            assert_eq!(record.bridge(), Bridge::TurnSession, "{shown}");
            assert_eq!(
                shown, told,
                "the reader is told the failure, then which write would have had to wait"
            );
            *primary
        }
        other => panic!("the failure and the refused write together, not {other:?}"),
    }
}

#[test]
fn a_failed_request_whose_answer_is_never_written_ends_on_both() {
    // The provider's refusal is the reason the turn ended, and the reader has
    // to be told it: a session write dropped while recording what the answer
    // had reached must not stand in its place. Automatic compaction is on, and
    // two turns answered in full leave a middle a recap could replace, so a
    // compaction that started would ask for one. A refused write is not a
    // request that did not fit, so nothing is made room for on top of a record
    // that cannot say what it holds.
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
    assert!(
        scripted.runner.policy.compaction.automatic,
        "the point of this"
    );
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    assert!(
        scripted.runner.transcript().turns() >= 2,
        "a middle a recap could replace"
    );
    let over = Script::over_window();
    let sent = over.sent();
    scripted.runner.provider = Box::new(over);
    scripted.runner.store = Arc::new(Withholding {
        recording: store,
        withheld: Withheld::Answer,
    });

    let problem = beside_refused_write(
        scripted.turn("go").unwrap_err(),
        "script: the request did not fit the model's window; then a session write of what the \
         turn had reached: writing to the session would have had to wait, and the caller \
         cannot; the waiting step was dropped before it answered, so whatever that step began \
         is unconfirmed",
    );

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::WindowExceeded { .. })
        ),
        "{problem:?}"
    );
    assert_eq!(sent.lock().unwrap().len(), 1, "a recap was asked for");
}

#[test]
fn a_refused_call_whose_results_are_never_written_ends_on_both() {
    let store = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![calling("a", "write", "{}")]),
        tools([Fixed::new("write").risking(changing())]),
        Verdict::Deny,
        Arc::clone(&store),
    );
    scripted.runner.store = Arc::new(Withholding {
        recording: store,
        withheld: Withheld::Results,
    });

    let problem = beside_refused_write(
        scripted.turn("write it").unwrap_err(),
        "write was not allowed; then a session write of what the turn had reached: writing to \
         the session would have had to wait, and the caller cannot; the waiting step was \
         dropped before it answered, so whatever that step began is unconfirmed",
    );

    assert!(
        matches!(&problem, TurnError::Refused(name) if &**name == "write"),
        "{problem:?}"
    );
}

#[test]
fn results_past_the_boundary_whose_line_is_never_written_end_on_both() {
    let store = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![calling("a", "read", "{}")]),
        tools([Fixed::new("read").answering("ninebytes")]),
        Verdict::Allow,
        Arc::clone(&store),
    );
    scripted.runner.store = Arc::new(Withholding {
        recording: store,
        withheld: Withheld::Results,
    });

    let problem = beside_refused_write(
        scripted.turning_under("go", holding(8)).unwrap_err(),
        "tool results exceeded the 8-byte per-turn limit; then a session write of what the \
         turn had reached: writing to the session would have had to wait, and the caller \
         cannot; the waiting step was dropped before it answered, so whatever that step began \
         is unconfirmed",
    );

    assert!(
        matches!(problem, TurnError::ToolOutputBytes { maximum: 8 }),
        "{problem:?}"
    );
}

/// A tool that raises the turn's stop as it runs, then answers, never does,
/// or answers only once it sees the stop it raised.
///
/// It raises the run's own stop rather than the one its context hands it: a
/// tool's run is handed a child of the run's, and a child's stop does not
/// reach the run it came from.
struct Stopping {
    stop: Cancel,
    answers: Answers,
}

/// When a [`Stopping`] tool answers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answers {
    /// At once.
    AtOnce,
    /// Never.
    Never,
    /// Once it has waited and seen the stop.
    OnceStopped,
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
                Answers::Never => {
                    self.stop.request();
                    std::future::pending().await
                }
                Answers::OnceStopped => {
                    UntilStopped {
                        cancel: self.stop.clone(),
                        answer: Some(Ok(ToolOutput::ok("stopped"))),
                    }
                    .await
                }
            }
        })
    }
}

/// A turn whose one tool raises its stop while it runs, recorded to `store`.
fn stopped_by(answers: Answers, rounds: Vec<Vec<Delta>>, store: Arc<Recording>) -> Scripted {
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

#[test]
fn a_results_line_refused_while_stopping_ends_the_turn_refused() {
    // The stop is not a clean one while the record of what the pass answered
    // is in doubt: the refusal is what the turn ends on.
    let store = Recording::nowhere();
    let mut scripted = stopped_by(
        Answers::AtOnce,
        vec![calling("a", "stop", "{}")],
        Arc::clone(&store),
    );
    scripted.runner.store = Arc::new(Withholding {
        recording: store,
        withheld: Withheld::Results,
    });

    let turned = scripted.turn("go");

    assert!(
        matches!(
            &turned,
            Err(TurnError::Unready(unready)) if unready.bridge() == Bridge::TurnSession
        ),
        "{turned:?}"
    );
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
fn results_whose_line_is_never_written_still_answer_their_calls() {
    // The calls ran and were answered; only the log line is in doubt. A
    // transcript left on calls with nothing after them is one a provider
    // replaying the history natively refuses to build a request from, turn
    // after turn, since it checks that every call was answered. A recap, and a
    // history an older wire describes rather than replays, carry every message
    // as text and are not checked.
    let store = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![calling("a", "read", "{}")]),
        tools([Fixed::new("read")]),
        Verdict::Allow,
        Arc::clone(&store),
    );
    scripted.runner.store = Arc::new(Withholding {
        recording: Arc::clone(&store),
        withheld: Withheld::Results,
    });

    let turned = scripted.turn("go");

    assert!(
        matches!(
            &turned,
            Err(TurnError::Unready(unready)) if unready.bridge() == Bridge::TurnSession
        ),
        "{turned:?}"
    );
    assert_eq!(
        shape(&conversation(scripted.runner.transcript())),
        ["user go", "calls a", "results a"],
        "the transcript ends on calls nothing answered"
    );
    assert!(
        !store
            .kept()
            .iter()
            .any(|kept| matches!(kept, Kept::Settled)),
        "results not known to be in the log were settled"
    );
}

/// A provider that answers from its script and keeps the conversation each
/// request carried.
struct Keeping {
    script: Script,
    kept: Arc<Mutex<Vec<Vec<Message>>>>,
}

impl Provider for Keeping {
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
        self.kept
            .lock()
            .unwrap()
            .push(conversation(request.transcript));
        self.script.stream(request, cancel)
    }
}

#[test]
fn the_turn_after_an_unwritten_results_line_sends_the_calls_answered() {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let store = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(Vec::new()),
        tools([Fixed::new("read")]),
        Verdict::Allow,
        Arc::clone(&store),
    );
    scripted.runner.provider = Box::new(Keeping {
        script: Script::new(vec![calling("a", "read", "{}"), saying("after")]),
        kept: Arc::clone(&kept),
    });
    scripted.runner.store = Arc::new(Withholding {
        recording: store,
        withheld: Withheld::Results,
    });

    scripted
        .turn("go")
        .expect_err("a results line nobody confirmed");
    scripted.turn("again").expect("the turn after it");

    let kept = kept.lock().unwrap();
    let [_, again] = kept.as_slice() else {
        panic!("{} requests went out, not two", kept.len());
    };
    assert_eq!(
        shape(again),
        ["user go", "calls a", "results a", "user again"],
        "the next request did not hold the calls and then their results"
    );
}

#[test]
fn a_restricted_result_in_an_unwritten_results_line_is_still_cleared() {
    // Clearing is what keeps output another vendor may not be sent out of
    // every later request. A results line the session never confirmed is no
    // reason to send what it held.
    let store = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![
            calling("call_search", "web_search", r#"{"query":"rust"}"#),
            saying("an answer from elsewhere"),
        ])
        .with_name("elsewhere"),
        tools([Fixed::new("web_search")
            .answering("restricted search results canary")
            .answered_by(left_behind())]),
        Verdict::Allow,
        Arc::clone(&store),
    );
    scripted.runner.store = Arc::new(Withholding {
        recording: Arc::clone(&store),
        withheld: Withheld::Results,
    });

    scripted
        .turn("search for rust")
        .expect_err("a results line nobody confirmed");
    scripted.turn("summarize").expect("the turn after it");

    let sent = scripted.sent.lock().unwrap();
    let after = sent.last().expect("the request after the search");
    assert!(
        !after.carried_result("restricted search results canary"),
        "a restricted result went out because its line was not confirmed"
    );
    assert!(
        after.carried_result(RESTRICTED),
        "the request did not carry the sentence left in the result's place"
    );
    assert_eq!(
        restrictions(&store).len(),
        1,
        "the clearing's own line was not written"
    );
}

#[test]
fn a_run_in_a_parallel_wave_refused_while_stopping_is_answered_as_unconfirmed() {
    // The calls of a parallel wave each ask their run once on a thread of
    // their own. A run there began and was dropped before it answered. "Not
    // run" would be false, and whatever the run began would go unmentioned.
    // The stop still ends the turn after that pass: the model is not asked
    // again.
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
                answers: Answers::Never,
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
    assert_eq!(
        only_result(&scripted).output.text(),
        "stop: its run would have had to wait, so the run was dropped before it answered; \
         whatever the run began is unconfirmed"
    );
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
    assert_eq!(outcomes, [ToolOutcome::Failed, ToolOutcome::Failed]);
}

#[test]
fn a_run_that_waits_across_the_stop_ends_the_pass_stopped() {
    // A call that runs alone is awaited: its run waits, sees the stop, and
    // answers, and the call is answered as the stop cut it short.
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
    assert_eq!(
        only_result(&scripted).output.text(),
        "not run: the turn ended first"
    );
}
