//! A request whose credential is still being renewed stops waiting for it as
//! soon as whoever asked for the request stops.
//!
//! The four sources an account credential is applied to — the `ChatGPT` and
//! Kimi Code turns, and their web sources — each await `authorize` before
//! anything is sent. A renewal can take the store's lock for up to 5 s and a
//! request for up to 30 s, and it is the renewal owner's work rather than the
//! caller's, so the caller's cancel has to end the wait itself. Each test
//! stands a credential whose renewal never answers in front of one source,
//! raises the cancel shortly after the call begins, and says how long the call
//! took to come back.

use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{Fetch, Search, SourceError};
use crucible_credentials::{Authorization, Credential, Outgoing};
use crucible_models::{Provider, ProviderError, Request, RequestPurpose};
use crucible_runtime::Cancel;

use crucible_types::{CredentialScopeId, Message, Transcript};

use crate::transport::Replay;
use crate::{Moonshot, MoonshotWeb, OpenAi, OpenAiWeb};

/// How long after a call begins its cancel is raised.
const STOPPED_AFTER: Duration = Duration::from_millis(100);

/// How long a stopped call may take to come back: its cancel's notice, with
/// room for a busy machine, and a small part of the least a renewal can take.
const BOUND: Duration = Duration::from_secs(1);

/// How long a test waits for a call that is not coming back before it says
/// so, rather than hanging.
const PATIENCE: Duration = Duration::from_secs(5);

/// An account credential whose renewal is in flight and has not answered.
#[derive(Debug)]
struct Renewing;

impl Credential for Renewing {
    fn scope(&self) -> CredentialScopeId {
        CredentialScopeId::new()
    }

    fn authorize<'a>(&'a self, _request: &'a mut Outgoing) -> Authorization<'a> {
        Box::pin(std::future::pending())
    }
}

/// A transport nothing may reach: the request is never sent.
fn unreached() -> Box<dyn crate::Transport> {
    Box::new(Replay::new(500, ""))
}

fn asking() -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript
        .push(Message::said("hello"))
        .expect("valid fixture transcript");
    Request {
        purpose: RequestPurpose::Turn,
        model: "model-test",
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        attached: &[],
        max_tokens: 1024,
        system: None,
        effort: None,
        prompt_cache: None,
    }
}

/// Runs `call` on a thread and a runtime of its own, raises the cancel it is
/// handed [`STOPPED_AFTER`] after it began, and hands back what it answered
/// and how long it took — or `None` where it had not answered within
/// [`PATIENCE`].
fn stopped_while_renewing<T: Send + 'static>(
    call: impl FnOnce(tokio::runtime::Runtime, Cancel) -> T + Send + 'static,
) -> Option<(T, Duration)> {
    let cancel = Cancel::new();
    let (answered, heard) = mpsc::channel();
    let stopping = cancel.clone();
    let begun = Instant::now();
    thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a test runtime");
        let _ = answered.send(call(runtime, stopping));
    });
    thread::sleep(STOPPED_AFTER);
    cancel.request();
    heard
        .recv_timeout(PATIENCE)
        .ok()
        .map(|answer| (answer, begun.elapsed()))
}

#[track_caller]
fn came_back<T>(answer: Option<(T, Duration)>) -> T {
    let Some((answer, took)) = answer else {
        panic!("a call stopped while its credential renewed had not come back after {PATIENCE:?}");
    };
    assert!(
        took < BOUND,
        "a call stopped while its credential renewed came back {took:?} after it began, not \
         within {BOUND:?}"
    );
    answer
}

#[test]
fn a_chatgpt_turn_stopped_while_its_credential_renews_returns_within_its_bound() {
    let provider = OpenAi::at(OpenAi::SUBSCRIPTION, Box::new(Renewing), unreached());

    let answer = came_back(stopped_while_renewing(move |runtime, cancel| {
        runtime
            .block_on(provider.stream(asking(), &cancel))
            .map(|_| ())
    }));

    assert!(
        matches!(answer, Err(ProviderError::Cancelled(_))),
        "{answer:?}"
    );
}

#[test]
fn a_kimi_code_turn_stopped_while_its_credential_renews_returns_within_its_bound() {
    let provider = Moonshot::at(Moonshot::CODING, Box::new(Renewing), unreached());

    let answer = came_back(stopped_while_renewing(move |runtime, cancel| {
        runtime
            .block_on(provider.stream(asking(), &cancel))
            .map(|_| ())
    }));

    assert!(
        matches!(answer, Err(ProviderError::Cancelled(_))),
        "{answer:?}"
    );
}

#[test]
fn a_chatgpt_web_search_stopped_while_its_credential_renews_returns_within_its_bound() {
    let source = OpenAiWeb::new(
        OpenAi::SUBSCRIPTION,
        Box::new(Renewing),
        unreached(),
        "gpt-5.6",
    );

    let answer = came_back(stopped_while_renewing(move |runtime, cancel| {
        runtime
            .block_on(Search::search(&source, "renewal", &cancel))
            .map(|_| ())
    }));

    assert!(
        matches!(answer, Err(SourceError::Cancelled(_))),
        "{answer:?}"
    );
}

#[test]
fn a_kimi_code_web_fetch_stopped_while_its_credential_renews_returns_within_its_bound() {
    let source = MoonshotWeb::new(Box::new(Renewing), unreached());

    let answer = came_back(stopped_while_renewing(move |runtime, cancel| {
        runtime
            .block_on(Fetch::fetch(&source, "https://example.com/", &cancel))
            .map(|_| ())
    }));

    assert!(
        matches!(answer, Err(SourceError::Cancelled(_))),
        "{answer:?}"
    );
}
