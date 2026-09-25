//! A refused startup still owns its process cleanup outcome.

use super::*;

fn dialogue(name: &str) -> Vec<Value> {
    opening(name, &json!([offers("search")]))
}

fn bad_greeting() -> Vec<Value> {
    vec![json!({"jsonrpc":"2.0", "id":1, "result":{"protocolVersion":"unsupported"}})]
}

fn failed_preparation(script: Answers, required: bool, expected_cause: &str) {
    let sandbox = Pretend::new([
        Answers::Says(dialogue("docs")),
        script,
        Answers::Says(dialogue("later")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![
            chosen("docs"),
            chosen("broken").required(required),
            chosen("later"),
        ],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    let error =
        awaited(hosting.prepare(&context)).expect_err("uncertain cleanup is never optional");
    let message = error.to_string();
    assert!(message.contains("broken"), "{message}");
    assert!(message.contains(expected_cause), "{message}");
    assert!(message.contains("unconfirmed cleanup"), "{message}");
    assert_eq!(sandbox.started(), 2, "no later server can start");
    assert_eq!(sandbox.server(0).stops(), 1, "earlier peers are stopped");
    assert_eq!(sandbox.server(1).stop_attempts.load(Ordering::Relaxed), 1);
    assert!(
        awaited(hosting.snapshot(&context))
            .unwrap()
            .entries()
            .is_empty()
    );
    assert!(awaited(hosting.dispose(&context)).is_err());
    assert!(awaited(hosting.dispose(&context)).is_err());
    assert!(awaited(hosting.prepare(&context)).is_err());
    assert_eq!(sandbox.started(), 2);
}

#[test]
fn optional_bad_greeting_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(bad_greeting()),
        false,
        "MCP version unsupported",
    );
}

#[test]
fn required_bad_greeting_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(bad_greeting()),
        true,
        "MCP version unsupported",
    );
}

#[test]
fn optional_bad_catalogue_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(opening("broken", &json!(false))),
        false,
        "without tools",
    );
}

#[test]
fn required_bad_catalogue_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(opening("broken", &json!(false))),
        true,
        "without tools",
    );
}

#[test]
fn failed_restart_handshake_retains_unconfirmed_cleanup() {
    let sandbox = Pretend::new([
        Answers::Says(dialogue("docs")),
        Answers::Unreapable(bad_greeting()),
        Answers::Says(dialogue("docs")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(3)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).unwrap();
    let snapshot = awaited(hosting.snapshot(&context)).unwrap();
    let entry = snapshot.find("mcp:docs/search").unwrap();
    sandbox.server(0).departs();
    let error = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new()).unwrap_err();
    assert!(error.to_string().contains("unconfirmed cleanup"), "{error}");
    assert!(
        error.to_string().contains("MCP version unsupported"),
        "{error}"
    );
    assert_eq!(sandbox.started(), 2);
    assert_eq!(sandbox.server(1).stop_attempts.load(Ordering::Relaxed), 1);
    assert!(awaited(hosting.dispose(&context)).is_err());
    assert!(awaited(hosting.dispose(&context)).is_err());
    assert!(awaited(hosting.prepare(&context)).is_err());
    assert_eq!(sandbox.started(), 2);
}

#[test]
fn optional_missing_input_retains_unconfirmed_cleanup() {
    failed_preparation(Answers::MissingInput, false, "input");
}

#[test]
fn optional_missing_output_retains_unconfirmed_cleanup() {
    failed_preparation(Answers::MissingOutput, false, "output");
}

/// What a refused preparation says of a server that answered without tools and
/// did not finish when its input closed, stopped the way `stop` says.
///
/// The server is one the run can do without, so a refusal alone would be passed
/// over and preparation would succeed: only the unconfirmed stop keeps it.
fn unfinished_after_a_bad_catalogue(stop: Stop) -> (String, Arc<Watched>) {
    let sandbox = Pretend::new([Answers::Unfinished(opening("broken", &json!(false)), stop)]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![optional("broken")],
        crate::testing::runtime(),
    );
    let error = awaited(hosting.prepare(&lifecycle()))
        .expect_err("a server whose stop is not confirmed is never passed over");
    (error.to_string(), sandbox.server(0))
}

#[test]
fn a_stop_that_never_answers_is_said_to_be_unconfirmed_once() {
    let (message, server) = unfinished_after_a_bad_catalogue(Stop::Unanswered);
    // The timeout's own words say that what the stop began is unconfirmed, so
    // the message does not lead them in by saying it first.
    assert_eq!(
        message,
        "tool source broken: the server answered without tools, which every \
         tools/list answer has to carry; cleanup: stopping a hosted program did \
         not answer within 10s, so whatever it began is unconfirmed"
    );
    assert_eq!(server.stop_attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn a_stop_that_never_answers_after_the_ceiling_is_said_to_be_unconfirmed_once() {
    let (message, server) = unfinished_after_a_bad_catalogue(Stop::UnansweredAfterEnding);
    // What the ceiling cost it, and then the timeout, whose words say the rest.
    assert_eq!(
        message,
        "tool source broken: the server answered without tools, which every \
         tools/list answer has to carry; cleanup: its publication did not finish \
         in time, and stopping a hosted program did not answer within 10s, so \
         whatever it began is unconfirmed"
    );
    assert_eq!(server.stop_attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn a_publication_completed_during_a_stop_is_not_reported_as_unpublished() {
    let (message, server) = unfinished_after_a_bad_catalogue(Stop::PublishedAfterEnding);
    assert!(
        !message.contains("its publication did not finish"),
        "{message}"
    );
    assert!(server.published.load(Ordering::Acquire));
}

#[test]
fn a_stop_that_failed_still_leaves_cleanup_said_to_be_unconfirmed() {
    let (message, server) = unfinished_after_a_bad_catalogue(Stop::Fails);
    // A failed stop's words say only what went wrong, so the message says that
    // cleanup is unconfirmed.
    assert_eq!(
        message,
        "tool source broken: the server answered without tools, which every \
         tools/list answer has to carry; unconfirmed cleanup: the scope could not \
         be reaped"
    );
    assert_eq!(server.stop_attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn optional_bad_greeting_with_confirmed_cleanup_still_allows_later_server() {
    let sandbox = Pretend::new([
        Answers::Says(bad_greeting()),
        Answers::Says(dialogue("later")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![optional("broken"), chosen("later")],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).unwrap();
    assert_eq!(sandbox.started(), 2);
    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "cleanup confirmed before continuing"
    );
    assert!(
        awaited(hosting.snapshot(&context))
            .unwrap()
            .find("mcp:later/search")
            .is_some()
    );
    awaited(hosting.dispose(&context)).unwrap();
    awaited(hosting.dispose(&context)).unwrap();
}

/// The one step of a start that is never answered.
#[derive(Debug, Clone, Copy)]
enum Waits {
    Preparing,
    Materializing,
    Starting,
}

/// A sandbox whose start never gets past one step: that step's future is
/// pending forever, so giving it up is all a caller can do.
struct Waiting {
    at: Waits,
    /// How many preparations it was asked for.
    asked: AtomicUsize,
}

impl SandboxService for Waiting {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(std::future::pending())
    }

    fn prepare(
        &self,
        _request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        match self.at {
            Waits::Preparing => Box::pin(std::future::pending()),
            Waits::Materializing | Waits::Starting => {
                let session: Box<dyn SandboxSession> = Box::new(Unanswered {
                    at: self.at,
                    inspection: inspection(),
                });
                Box::pin(std::future::ready(Ok(session)))
            }
        }
    }
}

/// A prepared session that answers every step before the one it waits at.
struct Unanswered {
    at: Waits,
    inspection: SandboxInspection,
}

impl SandboxSession for Unanswered {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        match self.at {
            Waits::Materializing => Box::pin(std::future::pending()),
            Waits::Preparing | Waits::Starting => Box::pin(std::future::ready(Ok(()))),
        }
    }

    fn stage<'a>(
        self: Box<Self>,
        _command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(std::future::pending())
    }
}

/// Prepares `server` over a sandbox that never answers step `at`, and gives the
/// preparation up once it has waited a while.
///
/// Nothing but the lifecycle's cancel ends a step that never answers, and the
/// step is dropped there. Whatever the dropped step had begun is not known to
/// be undone, since no sandbox contract says what dropping a step leaves: so
/// the server is held as unconfirmed cleanup, required or not, and the refusal
/// says which step was given up on.
fn unanswered(at: Waits, server: Chosen) {
    let name = server.name.clone();
    let sandbox = Arc::new(Waiting {
        at,
        asked: AtomicUsize::new(0),
    });
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![server],
        crate::testing::runtime(),
    );
    let cancel = Cancel::new();
    let context = ToolsetContext::new(Ancestry::new(), cancel.clone(), None);
    let raising = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        cancel.request();
    });
    let began = Instant::now();
    let Err(error) = awaited(hosting.prepare(&context)) else {
        panic!("{at:?}: a dropped step confirms no cleanup, so its server cannot be left out");
    };
    let waited = began.elapsed();
    raising.join().expect("the cancel was raised");

    assert!(
        waited < Duration::from_secs(5),
        "{at:?}: a step that never answers is given up on when the cancel is raised: {waited:?}"
    );
    let said = error.to_string();
    assert!(said.contains(&*name), "{at:?}: {said}");
    assert!(
        said.contains("crucible stopped waiting while") && said.contains("unconfirmed"),
        "{at:?}: the refusal says the step was given up on and what that leaves: {said}"
    );
    for _ in 0..2 {
        let disposed = awaited(hosting.dispose(&context))
            .expect_err("disposal reports the cleanup nobody confirmed");
        assert!(disposed.to_string().contains("unconfirmed"), "{disposed}");
    }
    assert!(awaited(hosting.prepare(&context)).is_err());
    assert_eq!(
        sandbox.asked.load(Ordering::Relaxed),
        1,
        "{at:?}: nothing is started after it"
    );
}

#[test]
fn optional_server_whose_preparation_never_answers_is_held_unconfirmed() {
    unanswered(Waits::Preparing, optional("notes"));
}

#[test]
fn optional_server_whose_materialization_never_answers_is_held_unconfirmed() {
    unanswered(Waits::Materializing, optional("notes"));
}

#[test]
fn optional_server_whose_start_never_answers_is_held_unconfirmed() {
    unanswered(Waits::Starting, optional("notes"));
}

#[test]
fn required_server_whose_preparation_never_answers_is_held_unconfirmed() {
    unanswered(Waits::Preparing, chosen("docs"));
}

#[test]
fn required_server_whose_materialization_never_answers_is_held_unconfirmed() {
    unanswered(Waits::Materializing, chosen("docs"));
}

#[test]
fn required_server_whose_start_never_answers_is_held_unconfirmed() {
    unanswered(Waits::Starting, chosen("docs"));
}

/// Prepares `server` over a sandbox that never answers step `at`, with nobody
/// raising the lifecycle's cancel.
///
/// The step is given up on at the server's handshake patience, which is a
/// step given up on all the same: the server is held as unconfirmed cleanup,
/// and the refusal says how long was waited.
fn unanswered_with_nobody_cancelling(at: Waits, server: Chosen) {
    let name = server.name.clone();
    let patience = server.handshake();
    let sandbox = Arc::new(Waiting {
        at,
        asked: AtomicUsize::new(0),
    });
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![server],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    let guard = Cancel::new().child_until(Instant::now().checked_add(Duration::from_secs(5)));
    let began = Instant::now();
    let prepared = crate::testing::runtime().block_on(guard.race(hosting.prepare(&context)));
    let waited = began.elapsed();

    let Some(Err(error)) = prepared else {
        panic!(
            "{at:?}: a step that never answers is given up on at the handshake patience with no \
             cancel raised; after {waited:?} it came to {prepared:?}"
        );
    };
    assert!(
        waited >= patience && waited < Duration::from_secs(3),
        "{at:?}: given up on after {waited:?}"
    );
    let said = error.to_string();
    assert!(said.contains(&*name), "{at:?}: {said}");
    assert!(
        said.contains(&format!(
            "crucible stopped waiting after {patience:?} while"
        )) && said.contains("unconfirmed"),
        "{at:?}: the refusal says how long was waited and what that leaves: {said}"
    );
    for _ in 0..2 {
        let disposed = awaited(hosting.dispose(&context))
            .expect_err("disposal reports the cleanup nobody confirmed");
        assert!(disposed.to_string().contains("unconfirmed"), "{disposed}");
    }
    assert!(awaited(hosting.prepare(&context)).is_err());
    assert_eq!(
        sandbox.asked.load(Ordering::Relaxed),
        1,
        "{at:?}: nothing after it"
    );
}

#[test]
fn a_preparation_that_never_answers_is_given_up_on_at_the_handshake_patience() {
    unanswered_with_nobody_cancelling(Waits::Preparing, optional("notes"));
}

#[test]
fn a_materialization_that_never_answers_is_given_up_on_at_the_handshake_patience() {
    unanswered_with_nobody_cancelling(Waits::Materializing, chosen("docs"));
}

#[test]
fn a_start_that_never_answers_is_given_up_on_at_the_handshake_patience() {
    unanswered_with_nobody_cancelling(Waits::Starting, optional("notes"));
}
