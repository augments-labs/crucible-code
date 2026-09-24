//! A turn over services that answer only after waiting.
//!
//! Each stand-in here says it is not ready the first time it is asked, wakes
//! whoever asked, and answers the next time: the smallest wait there is, and
//! one no step of a turn can mistake for an answer. A turn awaits every such
//! step it takes — the provider's stream and each of its reads, a tool's run,
//! and the toolset's preparation and disposal — and ends as it would have had
//! every step answered at once.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use crucible_core::{PromptCacheCapabilities, PromptCacheRoute};

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
