use std::path::Path;
use std::sync::{Arc, Mutex, PoisonError};

use crucible_execution::{
    Capabilities, Capability, Confinement, Decision, Error, Event, Outcome, PendingRef, SessionRef,
    Sink, Stale, TurnRequest,
};
use crucible_external_harnesses::fake_peer::{self, Plain, Reply, Script};
use crucible_external_harnesses::{Control, Discovery, Held, Host, Launch};
use crucible_sandbox::{
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxPolicy, SandboxResourceLimits,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::TurnId;
use tokio::runtime::Handle;

use super::*;
use crate::sample::Sample;

/// Keeps every event it is posted.
#[derive(Default)]
struct Kept(Mutex<Vec<Event>>);

impl Sink for Kept {
    fn post(&self, event: Event) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

impl Kept {
    fn events(&self) -> Vec<Event> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn asked(&self) -> Option<PendingRef> {
        self.events().into_iter().find_map(|event| match event {
            Event::Pending(reference, _) => Some(reference),
            _ => None,
        })
    }
}

/// What the plain protocol can do.
fn claims() -> Capabilities {
    Capabilities::unknown()
        .declared(Capability::Cancel, true)
        .declared(Capability::PermissionOnce, true)
}

/// `here` as the only root, unconfined.
fn unconfined(here: &Path) -> SandboxPolicy {
    let root = SandboxFilesystemRule::new(
        here,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .expect("an absolute root");
    SandboxPolicy::new(
        true,
        [root],
        here,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::confining(),
    )
    .expect("a valid policy")
    .with_enabled(false)
}

/// One run of the peer at `peer` on `control`, behind the backend.
async fn started(peer: &Path, control: &Control) -> External {
    let Discovery::Found(program) = Discovery::of(Some(peer)) else {
        panic!("the installed peer is found");
    };
    let here = peer.parent().expect("a directory");
    // The launch's own gates are `launching`'s to prove; this is the run
    // behind the contract.
    let launch = Launch::new(program, unconfined(here), Held::granted("agent:claude"))
        .asking(Plain.request(crucible_external_harnesses::Shape::default()));
    let host = Host::launch(
        Arc::new(LocalSandbox::new().watching_on(Handle::current())),
        Handle::current(),
        control,
        launch,
    )
    .await
    .expect("started");
    External::new(
        host,
        Box::new(Plain),
        SessionRef::new("one").expect("a name"),
        "work",
        claims(),
    )
}

fn turn(prompt: &str) -> TurnRequest {
    TurnRequest::new(TurnId::FIRST, prompt)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clean_turn_is_complete() {
    let sample = Sample::new("external-clean");
    let peer = fake_peer::install(
        &sample.root(),
        "plain",
        &Script::of([Reply::new().hear(1).say("say hello").say("end")]),
    )
    .expect("the fake peer is built");
    let control = Control::new().expect("a control");
    let backend = started(&peer, &control).await;
    let kept = Kept::default();

    let ended = backend.prompt(turn("go"), &kept).await;
    let closed = backend.close().await;

    assert!(matches!(ended, Ok(Outcome::Answered)), "{ended:?}");
    assert!(closed.is_ok());
    let events = kept.events();
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, Event::Uncertain(_))),
        "{events:?}"
    );
    assert!(matches!(
        events.last(),
        Some(Event::Finished(Outcome::Answered))
    ));
    let described = backend.describe();
    assert_eq!(described.driver().as_str(), "plain");
    assert_eq!(
        described
            .capabilities()
            .confinement()
            .map(crucible_execution::Confined::level),
        Some(Confinement::Unsupported)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_crash_after_an_effect_is_incomplete() {
    let sample = Sample::new("external-crash");
    let peer = fake_peer::install(
        &sample.root(),
        "plain",
        &Script::of([Reply::new().hear(1).say("file src/lib.rs").exit(9)]),
    )
    .expect("the fake peer is built");
    let control = Control::new().expect("a control");
    let backend = started(&peer, &control).await;
    let kept = Kept::default();

    let ended = backend.prompt(turn("go"), &kept).await;

    assert!(matches!(ended, Err(Error::Failed(_))), "{ended:?}");
    let events = kept.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::Uncertain(_))),
        "{events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Finished(_))),
        "{events:?}"
    );
    assert!(
        matches!(events.last(), Some(Event::Failed(_))),
        "{events:?}"
    );
}

/// Closing waits until the program a turn was running has stopped, and a
/// closed backend runs no further turn.
#[tokio::test(flavor = "multi_thread")]
async fn closing_during_a_turn_returns_once_the_program_has_stopped() {
    let sample = Sample::new("external-close");
    let root = sample.root();
    let peer = fake_peer::install(
        &root,
        "plain",
        &Script::of([Reply::new()
            .hear(1)
            .linger()
            .pause(std::time::Duration::from_mins(1))]),
    )
    .expect("the fake peer is built");
    let control = Control::new().expect("a control");
    let backend = started(&peer, &control).await;
    let kept = Kept::default();
    let prompting = backend.prompt(turn("go"), &kept);
    tokio::pin!(prompting);
    let mut ended = None;
    while fake_peer::lingering(&root).is_none() {
        tokio::select! {
            outcome = &mut prompting => panic!("the turn ended first: {outcome:?}"),
            () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
        }
    }

    let closing = backend.close();
    tokio::pin!(closing);
    let closed = tokio::time::timeout(std::time::Duration::from_mins(1), async {
        loop {
            tokio::select! {
                closed = &mut closing => break closed,
                outcome = &mut prompting, if ended.is_none() => ended = Some(outcome),
            }
        }
    })
    .await
    .expect("closed in time");

    assert!(closed.is_ok(), "{closed:?}");
    let first = fake_peer::lingering(&root);
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(fake_peer::lingering(&root), first, "it still runs");
    let again = backend.prompt(turn("again"), &kept).await;
    assert!(again.is_err(), "{again:?}");
}

/// A turn whose caller stopped waiting may have left the program mid-answer,
/// so its run takes no further turn: the next one never reads the old
/// answer as its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_given_up_on_leaves_no_answer_to_the_next() {
    let sample = Sample::new("external-abandoned");
    let peer = fake_peer::install(
        &sample.root(),
        "plain",
        &Script::of([
            Reply::new()
                .hear(1)
                .say("say partial")
                .pause(std::time::Duration::from_secs(2))
                .say("say stale")
                .say("end"),
            Reply::new().hear(1).say("say fresh").say("end"),
        ]),
    )
    .expect("the fake peer is built");
    let control = Control::new().expect("a control");
    let backend = started(&peer, &control).await;
    let first = Kept::default();
    {
        let prompting = backend.prompt(turn("go"), &first);
        tokio::pin!(prompting);
        while !first
            .events()
            .iter()
            .any(|event| matches!(event, Event::Delta(_)))
        {
            tokio::select! {
                outcome = &mut prompting => panic!("the turn ended first: {outcome:?}"),
                () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {}
            }
        }
    }

    let second = Kept::default();
    let again = backend.prompt(turn("again"), &second).await;
    backend.close().await.expect("closed");

    let said = second
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::Delta(text) => Some(text.as_str().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(!said.iter().any(|text| text.contains("stale")), "{said:?}");
    assert!(again.is_err(), "{again:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_for_a_run_that_has_restarted_is_refused() {
    let sample = Sample::new("external-generation");
    let peer = fake_peer::install(
        &sample.root(),
        "plain",
        &Script::of([
            Reply::new().hear(1).say("ask q read the notes").hear_out(),
            Reply::new().hear(1).say("end"),
        ]),
    )
    .expect("the fake peer is built");
    let control = Control::new().expect("a control");

    let first = started(&peer, &control).await;
    let kept = Kept::default();
    let asking = first.prompt(turn("go"), &kept);
    let reference = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        tokio::pin!(asking);
        loop {
            tokio::select! {
                _ = &mut asking => break None,
                () = tokio::time::sleep(std::time::Duration::from_millis(20)) => {
                    if let Some(reference) = kept.asked() {
                        break Some(reference);
                    }
                }
            }
        }
    })
    .await
    .expect("asked in time")
    .expect("the question was raised");
    first.close().await.expect("closed");
    drop(first);
    let second = started(&peer, &control).await;

    let refused = second.decide(&reference, Decision::Deny);

    assert!(
        matches!(refused, Err(Error::Stale(Stale::Generation))),
        "{refused:?}"
    );
    second.close().await.expect("closed");
}
