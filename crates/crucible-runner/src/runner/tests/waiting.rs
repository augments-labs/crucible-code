//! A turn over services that answer only after waiting.
//!
//! Each stand-in here says it is not ready the first time it is asked, wakes
//! whoever asked, and answers the next time: the smallest wait there is, and
//! one no step of a turn can mistake for an answer. A turn awaits every such
//! step it takes — the provider's stream and each of its reads, a tool's run,
//! the toolset's preparation and disposal, and each line written to the
//! session — and ends as it would have had every step answered at once.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crucible_core::{
    Calibration, CallResultKey, CallResultReceipt, CallResultStoreError, Compacted, ContextError,
    ContextPatch, ContextSnapshot, PromptCacheCapabilities, PromptCacheRoute, SessionId,
    SessionOwner,
};

use super::aiming::left_behind;
use super::pick_up::restrictions;
use super::unanswered::{Answers, shape, stopped_by};
use super::*;

/// `future`, after saying once that it is not ready.
///
/// It wakes whoever asked before it says so, as a future that has something
/// to wait for does once that thing arrives, so a caller that waits asks again
/// and a caller that cannot wait is left holding a step that never answered.
pub(super) struct Later<F> {
    asked: bool,
    future: Pin<Box<F>>,
}

impl<F> Later<F> {
    pub(super) fn new(future: F) -> Self {
        Self {
            asked: false,
            future: Box::pin(future),
        }
    }
}

impl<F: Future> Future for Later<F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        if !self.asked {
            self.asked = true;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        self.future.as_mut().poll(cx)
    }
}

/// Waits until `cancel` is raised, raising it itself the first time it
/// waits, as a reader pressing the key while a step had not answered would,
/// and then answers `answer`.
pub(super) struct UntilStopped<T> {
    pub(super) cancel: Cancel,
    pub(super) answer: Option<T>,
}

impl<T: Unpin> Future for UntilStopped<T> {
    type Output = T;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        if self.cancel.requested()
            && let Some(answer) = self.answer.take()
        {
            return Poll::Ready(answer);
        }
        self.cancel.request();
        cx.waker().wake_by_ref();
        Poll::Pending
    }
}

/// The lines a [`Slow`] store waits before taking.
#[derive(Debug, Clone, Copy)]
pub(super) enum Waits {
    /// Every line.
    Always,
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

/// A store that keeps what its [`Recording`] keeps, waiting once before it
/// takes a line of the kind it [`Waits`] on, as a log does while it has a
/// line to write.
#[derive(Debug)]
pub(super) struct Slow {
    pub(super) recording: Arc<Recording>,
    pub(super) waits: Waits,
}

impl Slow {
    /// `write`, after waiting once where this store waits on `kind`: a line of
    /// none of the kinds named is written as [`Waits::Always`], which only a
    /// store that waits on every line waits on.
    fn taking<'a, T: 'a>(&self, kind: Waits, write: BoxFuture<'a, T>) -> BoxFuture<'a, T> {
        let waits = matches!(
            (self.waits, kind),
            (Waits::Always, _)
                | (Waits::Restricted, Waits::Restricted)
                | (Waits::Pruned, Waits::Pruned)
                | (Waits::Answer, Waits::Answer)
                | (Waits::Results, Waits::Results)
        );
        if waits {
            Box::pin(Later::new(write))
        } else {
            write
        }
    }
}

impl SessionStore for Slow {
    fn session_id(&self) -> Option<SessionId> {
        self.recording.session_id()
    }

    fn owner(&self) -> Option<SessionOwner> {
        self.recording.owner()
    }

    fn append_message<'a>(&'a self, message: &'a Message) -> BoxFuture<'a, ()> {
        let kind = match message {
            Message::Agent { calls, .. } if calls.is_empty() => Waits::Answer,
            Message::ToolResults(_) => Waits::Results,
            _ => Waits::Always,
        };
        self.taking(kind, self.recording.append_message(message))
    }

    fn context_snapshot(&self) -> Option<ContextSnapshot> {
        self.recording.context_snapshot()
    }

    fn contextual<'a>(
        &'a self,
        patch: &'a ContextPatch,
    ) -> BoxFuture<'a, Result<(), ContextError>> {
        self.taking(Waits::Always, self.recording.contextual(patch))
    }

    fn compacted<'a>(&'a self, replaced: usize, recap: &'a str) -> BoxFuture<'a, ()> {
        self.taking(Waits::Always, self.recording.compacted(replaced, recap))
    }

    fn display_compacted(&self, compacted: Compacted, pruned: bool) -> BoxFuture<'_, ()> {
        self.taking(
            Waits::Always,
            self.recording.display_compacted(compacted, pruned),
        )
    }

    fn pruned<'a>(&'a self, freed: usize, results: &'a [ToolId]) -> BoxFuture<'a, ()> {
        self.taking(Waits::Pruned, self.recording.pruned(freed, results))
    }

    fn restricted<'a>(
        &'a self,
        freed: usize,
        results: &'a [ToolId],
        notice: &'a str,
    ) -> BoxFuture<'a, ()> {
        self.taking(
            Waits::Restricted,
            self.recording.restricted(freed, results, notice),
        )
    }

    fn measured<'a>(&'a self, calibration: &'a Calibration) -> BoxFuture<'a, ()> {
        self.taking(Waits::Always, self.recording.measured(calibration))
    }

    fn calibrated(&self) -> Option<Calibration> {
        self.recording.calibrated()
    }
}

impl JournalStore for Slow {
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

/// A provider that answers from its script, waiting once before it opens
/// each stream and once before each read of one.
pub(super) struct Unhurried(pub(super) Script);

impl Provider for Unhurried {
    fn name(&self) -> &'static str {
        self.0.name()
    }

    fn spells(&self) -> Modalities {
        self.0.spells()
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        self.0.prompt_cache_capabilities(model)
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        self.0.prompt_cache_route()
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> PromptCacheEncoding {
        self.0.prompt_cache_encoding(request)
    }

    fn stream<'a>(
        &'a self,
        request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            let opened = Later::new(self.0.stream(request, cancel)).await?;
            Ok(Box::new(UnhurriedStream(opened)) as Box<dyn DeltaStream>)
        })
    }
}

/// A stream that waits once before each read.
struct UnhurriedStream(Box<dyn DeltaStream>);

impl DeltaStream for UnhurriedStream {
    fn next(&mut self) -> BoxFuture<'_, Option<Result<Delta, ProviderError>>> {
        Box::pin(Later::new(self.0.next()))
    }
}

/// A tool that waits once before it answers `answer`.
struct Deliberate {
    answer: &'static str,
}

impl DescribeTool for Deliberate {
    fn name(&self) -> &'static str {
        "deliberate"
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Deliberate {
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
        Box::pin(Later::new(async move { Ok(ToolOutput::ok(self.answer)) }))
    }
}

#[test]
fn a_response_that_waits_before_it_answers_is_awaited() {
    let mut scripted = Scripted::new(Script::new(Vec::new()), Tools::new(), Verdict::Allow);
    scripted.runner.provider = Box::new(Unhurried(Script::new(vec![saying("worth the wait")])));

    let turned = scripted.turn("go");

    assert_eq!(turned.unwrap(), StopReason::Yielded);
    assert_eq!(scripted.said(), "worth the wait");
}

#[test]
fn a_run_that_waits_before_it_answers_is_awaited() {
    let mut offered = Tools::new();
    offered
        .add_builtin(Deliberate { answer: "found it" })
        .unwrap();
    let mut scripted = Scripted::new(
        Script::new(vec![calling("a", "deliberate", "{}"), saying("done")]),
        offered,
        Verdict::Allow,
    );

    let turned = scripted.turn("go");

    assert_eq!(turned.unwrap(), StopReason::Yielded);
    assert_eq!(only_result(&scripted).output.text(), "found it");
}

#[test]
fn a_recap_that_waits_before_it_answers_is_awaited() {
    let mut scripted = Scripted::new(
        Script::new(vec![saying("first"), saying("second")]),
        Tools::new(),
        Verdict::Allow,
    );
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    scripted.runner.provider = Box::new(Unhurried(Script::new(vec![recap("notes to self")])));

    let compacted = scripted.compacting();

    assert!(matches!(compacted, Ok(Room::Made(_))), "{compacted:?}");
    assert!(
        conversation(scripted.runner.transcript()).iter().any(
            |message| matches!(message, Message::User { text, .. } if text.contains("notes to self"))
        ),
        "the recap is not standing in the transcript"
    );
}

/// A session that waits once before it takes each line, recording to what
/// `recording` keeps.
fn slow(recording: &Arc<Recording>) -> Arc<Slow> {
    Arc::new(Slow {
        recording: Arc::clone(recording),
        waits: Waits::Always,
    })
}

#[test]
fn a_turn_over_a_session_that_waits_before_each_line_records_every_one() {
    let recording = Recording::started("a slow disk");
    let mut scripted = Scripted::recording(
        Script::new(vec![calling("a", "read", "{}"), saying("done")]),
        tools([Fixed::new("read").answering("found it")]),
        Verdict::Allow,
        Arc::clone(&recording),
    );
    scripted.runner.store = slow(&recording);

    let turned = scripted.turn("go");

    assert_eq!(turned.unwrap(), StopReason::Yielded);
    let (_, replayed) = recording.reopened();
    assert_eq!(
        replayed.messages(),
        scripted.runner.transcript().messages(),
        "the log replays a different turn from the one the runner holds"
    );
}

#[test]
fn a_compaction_over_a_session_that_waits_before_each_line_records_every_one() {
    let recording = Recording::started("a slow disk");
    let mut scripted = Scripted::recording(
        Script::new(vec![
            saying("first"),
            saying("second"),
            recap("notes to self"),
        ]),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&recording),
    );
    scripted.runner.policy.compaction = Compaction {
        keep_tokens: 1,
        ..Compaction::default()
    };
    scripted.turn("first").expect("a turn to compact from");
    scripted.turn("second").expect("a middle to replace");
    scripted.runner.store = slow(&recording);

    let compacted = scripted.compacting();

    assert!(matches!(compacted, Ok(Room::Made(_))), "{compacted:?}");
    let kept = recording.kept();
    assert!(
        kept.iter().any(
            |one| matches!(one, Kept::Compacted { recap, .. } if recap.contains("notes to self"))
        ),
        "the recap was not recorded"
    );
    assert!(
        matches!(kept.last(), Some(Kept::Shown { .. })),
        "the notice a reader was shown was not the last line: {kept:?}"
    );
}

/// What the session behind `store` holds of the conversation, as a later run
/// would be asked with it.
fn logged(store: &Recording) -> Vec<String> {
    shape(&conversation(&store.reopened().1))
}

#[test]
fn an_answer_the_connection_broke_off_is_in_the_log_before_the_failure_leaves() {
    let recording = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::breaking(vec![vec![Delta::Text("let me look".into())]]),
        Tools::new(),
        Verdict::Allow,
        Arc::clone(&recording),
    );
    scripted.runner.store = Arc::new(Slow {
        recording: Arc::clone(&recording),
        waits: Waits::Answer,
    });

    let problem = scripted.turn("go").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::Provider(ProviderError::Transport { .. })
        ),
        "{problem:?}"
    );
    assert_eq!(logged(&recording), ["user go", "answer"]);
}

#[test]
fn a_refused_call_ends_the_turn_once_the_session_has_taken_its_results() {
    let recording = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![calling("a", "write", "{}")]),
        tools([Fixed::new("write").risking(changing())]),
        Verdict::Deny,
        Arc::clone(&recording),
    );
    scripted.runner.store = Arc::new(Slow {
        recording: Arc::clone(&recording),
        waits: Waits::Results,
    });

    let problem = scripted.turn("write it").unwrap_err();

    assert!(
        matches!(&problem, TurnError::Refused(name) if &**name == "write"),
        "{problem:?}"
    );
    assert_eq!(
        logged(&recording),
        ["user write it", "calls a", "results a"]
    );
}

#[test]
fn results_past_the_boundary_end_the_turn_once_the_session_has_taken_them() {
    let recording = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![calling("a", "read", "{}")]),
        tools([Fixed::new("read").answering("ninebytes")]),
        Verdict::Allow,
        Arc::clone(&recording),
    );
    scripted.runner.store = Arc::new(Slow {
        recording: Arc::clone(&recording),
        waits: Waits::Results,
    });

    let problem = scripted.turning_under("go", holding(8)).unwrap_err();

    assert!(
        matches!(problem, TurnError::ToolOutputBytes { maximum: 8 }),
        "{problem:?}"
    );
    assert_eq!(logged(&recording), ["user go", "calls a", "results a"]);
}

#[test]
fn a_results_line_the_session_waits_to_take_while_stopping_ends_the_turn_stopped() {
    let recording = Recording::nowhere();
    let mut scripted = stopped_by(
        Answers::AtOnce,
        vec![calling("a", "stop", "{}")],
        Arc::clone(&recording),
    );
    scripted.runner.store = Arc::new(Slow {
        recording: Arc::clone(&recording),
        waits: Waits::Results,
    });

    let turned = scripted.turn("go");

    assert_eq!(turned.unwrap(), StopReason::Cancelled);
    assert_eq!(logged(&recording), ["user go", "calls a", "results a"]);
}

#[test]
fn results_the_session_waits_to_take_are_settled_once_it_has_them() {
    let recording = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(vec![calling("a", "read", "{}"), saying("done")]),
        tools([Fixed::new("read")]),
        Verdict::Allow,
        Arc::clone(&recording),
    );
    scripted.runner.store = Arc::new(Slow {
        recording: Arc::clone(&recording),
        waits: Waits::Results,
    });

    scripted
        .turn("go")
        .expect("a turn whose results the session took");

    let kept = recording.kept();
    let results = kept
        .iter()
        .position(|one| matches!(one, Kept::Said(Message::ToolResults(_))));
    let settled = kept.iter().position(|one| matches!(one, Kept::Settled));
    assert!(
        results.is_some() && settled > results,
        "the results were not settled after their line: {kept:?}"
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
fn the_request_after_results_the_session_waited_to_take_sends_the_calls_answered() {
    let kept = Arc::new(Mutex::new(Vec::new()));
    let recording = Recording::nowhere();
    let mut scripted = Scripted::recording(
        Script::new(Vec::new()),
        tools([Fixed::new("read")]),
        Verdict::Allow,
        Arc::clone(&recording),
    );
    scripted.runner.provider = Box::new(Keeping {
        script: Script::new(vec![calling("a", "read", "{}"), saying("after")]),
        kept: Arc::clone(&kept),
    });
    scripted.runner.store = Arc::new(Slow {
        recording,
        waits: Waits::Results,
    });

    scripted
        .turn("go")
        .expect("a turn whose results the session took");

    let kept = kept.lock().unwrap();
    let [_, after] = kept.as_slice() else {
        panic!("{} requests went out, not two", kept.len());
    };
    assert_eq!(
        shape(after),
        ["user go", "calls a", "results a"],
        "the next request did not hold the calls and then their results"
    );
}

#[test]
fn a_restricted_result_is_cleared_once_the_session_has_taken_its_results_line() {
    let recording = Recording::nowhere();
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
        Arc::clone(&recording),
    );
    scripted.runner.store = Arc::new(Slow {
        recording: Arc::clone(&recording),
        waits: Waits::Always,
    });

    scripted
        .turn("search for rust")
        .expect("a turn whose results the session took");

    let sent = scripted.sent.lock().unwrap();
    let after = sent.last().expect("the request after the search");
    assert!(
        !after.carried_result("restricted search results canary"),
        "a restricted result went out after the session took its line"
    );
    assert!(
        after.carried_result(RESTRICTED),
        "the request did not carry the sentence left in the result's place"
    );
    assert_eq!(
        restrictions(&recording).len(),
        1,
        "the clearing's own line was not written"
    );
}
