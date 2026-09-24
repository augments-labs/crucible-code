//! A tool, a toolset and a result's acceptance, implemented outside this crate.
//!
//! This file is its own crate, so it reaches only what this crate makes
//! public: what compiles here is what an implementer written elsewhere can
//! write. Each implementation is used as a trait object, and each future it
//! hands back is asked once, where it answers, because every body here is
//! finished the first time it is polled.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crucible_runtime::{BoxFuture, Cancel, answered};
use crucible_sandbox::SandboxError;
use crucible_tools::{
    Approved, Ask, CallResultAcceptance, CallResultReceipt, Fetch, Host, InvocationId, Page,
    Permission, Remember, Search, SearchResponse, Sensitivity, Settled, SourceError, Summary,
    Target, Tool, ToolContext, ToolError, ToolOutput, ToolSnapshot, Toolset, ToolsetContext,
    ToolsetError, Unwatched, Verdict,
};
use crucible_types::{Ancestry, ToolArgs, ToolCall, ToolId};

/// A tool that answers with the call it was lent, and hands its result's
/// acceptance to whoever finalizes it.
struct Echo {
    accepted: Arc<Mutex<Option<CallResultReceipt>>>,
}

impl Tool for Echo {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new("echo")
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            // The context is read inside the future, which is what the borrow
            // it was lent for is: the future holds it and nothing else does.
            let heard = context.call().as_str().to_owned();
            context
                .defer_call_result(Box::new(Accepting {
                    into: Arc::clone(&self.accepted),
                }))
                .map_err(|_| ToolError::Cancelled("echo".into()))?;
            Ok(ToolOutput::ok(heard))
        })
    }
}

/// The executor half of a result, which keeps the receipt it is bound to.
struct Accepting {
    into: Arc<Mutex<Option<CallResultReceipt>>>,
}

impl CallResultAcceptance for Accepting {
    fn accept<'a>(
        self: Box<Self>,
        receipt: CallResultReceipt,
    ) -> BoxFuture<'a, Result<(), SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            *self.into.lock().unwrap_or_else(PoisonError::into_inner) = Some(receipt);
            Ok(())
        })
    }
}

/// A source that counts each step of its lifecycle.
#[derive(Default)]
struct Counted {
    prepared: AtomicUsize,
    snapshots: AtomicUsize,
    disposed: AtomicUsize,
}

impl Toolset for Counted {
    fn prepare<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            self.prepared.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }

    fn snapshot<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(async move {
            self.snapshots.fetch_add(1, Ordering::Relaxed);
            Ok(ToolSnapshot::empty())
        })
    }

    fn refresh<'a>(
        &'a self,
        context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        // A step may be another step of the same contract, awaited.
        Box::pin(async move { self.snapshot(context).await })
    }

    fn dispose<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move {
            self.disposed.fetch_add(1, Ordering::Relaxed);
            Ok(())
        })
    }
}

/// Nobody to ask: a read is settled without a question.
struct Unasked;

impl Ask for Unasked {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Deny, Remember::Never)
    }
}

/// A verdict the permission engine reached about `call`.
#[allow(clippy::panic)] // A read the engine will not settle is a test failure.
fn approved(tool: &dyn Tool, call: &ToolCall) -> Approved {
    match Permission::new().decide(call, &tool.sensitivity(&call.args), &mut Unasked) {
        Settled::Approved(approved) => approved,
        Settled::Forbidden | Settled::Refused => panic!("a read is settled without a question"),
    }
}

#[test]
fn an_external_tool_runs_as_a_trait_object_and_hands_its_result_on() {
    let accepted = Arc::new(Mutex::new(None));
    let tool: Arc<dyn Tool> = Arc::new(Echo {
        accepted: Arc::clone(&accepted),
    });
    let call = ToolCall {
        id: ToolId::new("call-1"),
        name: "echo".into(),
        args: ToolArgs::new("{}"),
    };
    let approved = approved(&*tool, &call);
    let parent = Cancel::new();
    let context = ToolContext::new(Ancestry::new(), call.id.clone(), &parent, None, &Unwatched)
        .with_invocation(InvocationId::new());

    let output = answered!(tool.run(approved, &context)).expect("the echo answers");

    assert_eq!(
        output.text(),
        "call-1",
        "the run did not read the context it was lent"
    );
    let pending = context
        .take_call_result()
        .expect("the slot is readable")
        .expect("the run handed its acceptance on");
    answered!(pending.accept(CallResultReceipt::from_digest([7; 32])))
        .expect("the acceptance closes");
    assert_eq!(
        accepted
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .map(CallResultReceipt::bytes),
        Some([7; 32]),
        "the acceptance was not bound to the receipt it was handed"
    );
}

/// `Send` is what lets the future be polled on a worker other than the one
/// that asked for it, even while it borrows a context from the asking frame.
#[test]
fn a_run_borrowing_its_context_is_polled_on_another_thread() {
    let tool: Arc<dyn Tool> = Arc::new(Echo {
        accepted: Arc::default(),
    });
    let call = ToolCall {
        id: ToolId::new("call-2"),
        name: "echo".into(),
        args: ToolArgs::new("{}"),
    };
    let approved = approved(&*tool, &call);
    let parent = Cancel::new();
    let context = ToolContext::new(Ancestry::new(), call.id.clone(), &parent, None, &Unwatched)
        .with_invocation(InvocationId::new());
    let running = tool.run(approved, &context);

    let output = std::thread::scope(|scope| {
        scope
            .spawn(move || answered!(running))
            .join()
            .expect("the worker answered")
    })
    .expect("the echo answers");

    assert_eq!(output.text(), "call-2");
}

#[test]
fn an_external_toolset_is_driven_through_its_lifecycle_as_a_trait_object() {
    let counted = Arc::new(Counted::default());
    let toolset: Arc<dyn Toolset> = counted.clone();
    let context = ToolsetContext::new(Ancestry::new(), Cancel::new(), None);

    answered!(toolset.prepare(&context)).expect("prepared");
    let first = answered!(toolset.snapshot(&context)).expect("a snapshot");
    let refreshed = answered!(toolset.refresh(&context)).expect("a refreshed snapshot");
    answered!(toolset.dispose(&context)).expect("disposed");
    answered!(toolset.dispose(&context)).expect("disposed again");

    assert!(first.find("echo").is_none() && refreshed.find("echo").is_none());
    assert_eq!(counted.prepared.load(Ordering::Relaxed), 1);
    assert_eq!(
        counted.snapshots.load(Ordering::Relaxed),
        2,
        "refresh did not snapshot"
    );
    assert_eq!(counted.disposed.load(Ordering::Relaxed), 2);
    assert!(toolset.registered("echo").is_none());
}

/// A source that answers from memory, over both `Search` and `Fetch`.
struct Remembered;

impl Search for Remembered {
    fn name(&self) -> &'static str {
        "remembered"
    }

    fn reaches(&self) -> Host {
        Host::Named {
            sent: "https://search.example/".into(),
            host: "search.example".into(),
        }
    }

    fn search<'a>(
        &'a self,
        query: &'a str,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<SearchResponse, SourceError>> {
        Box::pin(async move { Ok(SearchResponse::grounded(query.to_owned(), Vec::new(), "")) })
    }
}

impl Fetch for Remembered {
    fn name(&self) -> &'static str {
        "remembered"
    }

    fn reaches(&self, url: &str) -> Host {
        Host::Named {
            sent: url.into(),
            host: "example.com".into(),
        }
    }

    fn fetch<'a>(
        &'a self,
        url: &'a str,
        _cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Page, SourceError>> {
        Box::pin(async move {
            Ok(Page {
                url: url.into(),
                title: None,
                text: "remembered".into(),
            })
        })
    }
}

#[test]
fn an_external_search_source_runs_as_a_trait_object_and_answers_at_its_first_poll() {
    let source: Arc<dyn Search> = Arc::new(Remembered);
    let cancel = Cancel::new();

    let response = answered!(source.search("crucible", &cancel)).expect("the search answers");

    assert_eq!(response.answer.as_deref(), Some("crucible"));
}

#[test]
fn an_external_fetch_source_runs_as_a_trait_object_and_answers_at_its_first_poll() {
    let source: Arc<dyn Fetch> = Arc::new(Remembered);
    let cancel = Cancel::new();

    let page =
        answered!(source.fetch("https://example.com/page", &cancel)).expect("the fetch answers");

    assert_eq!(page.url.as_ref(), "https://example.com/page");
}
