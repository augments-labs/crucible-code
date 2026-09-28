//! What a step given up on while it waits leaves behind.
//!
//! A turn gives a waiting step up by dropping it: a call that runs alone is
//! raced against its deadline, and whatever drives a turn can stop polling it.
//! Whatever such a step had started is still somebody's to stop, so each test
//! here gives a step up part way and then asks the one thing that matters:
//! whether every process it reached is stopped, once, by what comes after.

use super::*;

/// How long a test lets a step run before giving it up.
const GIVEN: Duration = Duration::from_millis(200);

/// How long a server that is being given up on thinks before it answers.
///
/// Far past [`GIVEN`], so a step still running when it answers is one that
/// could not be given up on.
const DAWDLING: Duration = Duration::from_secs(3);

/// Awaits `work` on the tests' runtime, giving it up once [`GIVEN`] has
/// passed, the way a turn races a call against its deadline.
///
/// `None` is the work given up on, and how long the giving up took.
fn given_up<F: Future>(work: F) -> (Option<F::Output>, Duration) {
    let giving_up = Cancel::new().child_until(Instant::now().checked_add(GIVEN));
    let began = Instant::now();
    let ran = crate::testing::runtime().block_on(giving_up.race(work));
    (ran, began.elapsed())
}

/// One selected server that is given long enough to greet that only giving
/// it up can end a greeting it is thinking about.
fn unhurried(name: &str) -> Chosen {
    Chosen::new(name, PROGRAM, [], policy())
        .given(SandboxEnvironment::new([]).expect("an empty environment"))
        .waiting(DAWDLING * 3, PATIENCE, GRACE)
        .required(true)
}

/// Whether `watched` was ever sent a call.
fn called(watched: &Watched) -> bool {
    watched
        .sent()
        .iter()
        .any(|frame| frame.get("method").and_then(Value::as_str) == Some("tools/call"))
}

#[test]
fn a_call_given_up_on_while_its_server_restarts_leaves_the_replacement_for_disposal() {
    let mut first = opening("docs", &json!([offers("search")]));
    first.push(produced("never said", false));
    let mut second = opening("docs", &json!([offers("search")]));
    second.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(first), Answers::Slowly(second, 0, DAWDLING)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![unhurried("docs").restarting(1)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    // The frame never leaves, so the server is started again, and its
    // replacement thinks about its greeting for far longer than the call is
    // waited for.
    sandbox.server(0).departs();
    let cancel = Cancel::new();
    let (ran, took) = given_up(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", "{}"),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &cancel,
            None,
            &Nothing,
        ),
    ));

    assert!(
        ran.is_none(),
        "a call is given up on while its server is being started again, not once the \
         replacement has answered: it ran for {took:?}"
    );
    assert!(took < DAWDLING / 2, "giving it up took {took:?}");
    assert_eq!(sandbox.started(), 2, "it was being started again");
    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "the server it replaces was stopped before its replacement started"
    );

    // What the replacement was asked is unknown, so it is asked nothing more.
    let after = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("a server whose restart was given up on is finished with");
    assert!(
        matches!(&after, ToolError::StaleGeneration { tool } if tool.as_ref() == "mcp:docs/search"),
        "{after}"
    );
    assert!(
        !called(&sandbox.server(1)),
        "the replacement was sent no call"
    );
    assert_eq!(sandbox.started(), 2, "and nothing is started a third time");

    awaited(hosting.dispose(&context)).expect("nothing is left unconfirmed");
    assert_eq!(
        sandbox.server(1).stops(),
        1,
        "a replacement nothing reaps is a server left running"
    );
}

#[test]
fn a_call_given_up_on_after_its_frame_went_leaves_its_server_asked_nothing_more() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("two pages", false));
    frames.push(produced("never asked", false));
    let sandbox = Pretend::new([Answers::Slowly(frames, 2, DAWDLING)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![patient("docs", DAWDLING * 3).restarting(3)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let cancel = Cancel::new();
    let (ran, took) = given_up(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", "{}"),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &cancel,
            None,
            &Nothing,
        ),
    ));

    assert!(
        ran.is_none(),
        "a call is given up on while its server thinks, not once it answers: it ran for \
         {took:?}"
    );
    assert!(took < DAWDLING / 2, "giving it up took {took:?}");

    // The call went, so the server may be doing it, and its answer would be
    // read as the reply to the next question: the server is finished with,
    // and no restart is bought for a call whose fate is unknown.
    let after = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("a server with a call given up on is finished with");
    assert!(
        matches!(&after, ToolError::StaleGeneration { tool } if tool.as_ref() == "mcp:docs/search"),
        "{after}"
    );
    let asked = sandbox
        .server(0)
        .sent()
        .iter()
        .filter(|frame| frame.get("method").and_then(Value::as_str) == Some("tools/call"))
        .count();
    assert_eq!(asked, 1, "the server was asked one call and no more");
    assert_eq!(sandbox.started(), 1, "and it was not started again");
    assert_eq!(sandbox.server(0).stops(), 1, "it was stopped once");

    awaited(hosting.dispose(&context)).expect("nothing is left to stop");
    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "and disposal stops it no further"
    );
}

#[test]
fn a_preparation_given_up_on_leaves_what_it_started_for_the_next_one_to_stop() {
    let sandbox = Pretend::new([
        Answers::Says(opening("docs", &json!([offers("search")]))),
        Answers::Slowly(opening("notes", &json!([offers("find")])), 0, DAWDLING),
        Answers::Says(opening("docs", &json!([offers("search")]))),
        Answers::Says(opening("notes", &json!([offers("find")]))),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![unhurried("docs"), unhurried("notes")],
        crate::testing::runtime(),
    );
    let context = lifecycle();

    let (prepared, took) = given_up(hosting.prepare(&context));

    assert!(
        prepared.is_none(),
        "a preparation is given up on while a server greets, not once it has: it ran for \
         {took:?}"
    );
    assert!(took < DAWDLING / 2, "giving it up took {took:?}");
    assert_eq!(sandbox.started(), 2, "both servers had been reached");
    assert!(
        awaited(hosting.snapshot(&context))
            .expect("the built-in generation")
            .entries()
            .is_empty(),
        "nothing a preparation given up on read is offered"
    );

    // The next preparation stops what the last one left before starting its
    // own, so no server of the one given up on outlives it.
    awaited(hosting.prepare(&context)).expect("the servers started again");
    assert_eq!(sandbox.server(0).stops(), 1, "the first was stopped");
    assert_eq!(sandbox.server(1).stops(), 1, "the one still greeting too");
    assert_eq!(sandbox.started(), 4);
    let named: Vec<_> = awaited(hosting.snapshot(&context))
        .expect("one generation")
        .entries()
        .iter()
        .map(|entry| entry.descriptor().name().to_owned())
        .collect();
    assert_eq!(named, ["mcp:docs/search", "mcp:notes/find"]);

    awaited(hosting.dispose(&context)).expect("both stopped");
    for n in 0..4 {
        assert_eq!(sandbox.server(n).stops(), 1, "server {n} is stopped once");
    }
}

#[test]
fn a_preparation_given_up_on_while_its_sandbox_waits_holds_the_server_unconfirmed() {
    let sandbox = Pretend::new([Answers::Hangs]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![optional("docs")],
        crate::testing::runtime(),
    );
    let context = lifecycle();

    let (prepared, took) = given_up(hosting.prepare(&context));

    assert!(
        prepared.is_none(),
        "the preparation was given up on while its sandbox was being prepared: it ran for \
         {took:?}"
    );
    // Nothing confirms what the step given up on had begun, so the server is
    // held, optional as it is, and nothing is started after it.
    for _ in 0..2 {
        let disposed = awaited(hosting.dispose(&context))
            .expect_err("disposal reports the cleanup nobody confirmed");
        assert!(disposed.to_string().contains("docs"), "{disposed}");
        assert!(disposed.to_string().contains("unconfirmed"), "{disposed}");
    }
    assert!(awaited(hosting.prepare(&context)).is_err());
}

#[test]
fn a_call_given_up_on_while_its_replacement_is_launched_holds_the_server_unconfirmed() {
    let mut first = opening("docs", &json!([offers("search")]));
    first.push(produced("never said", false));
    let sandbox = Pretend::new([Answers::Says(first), Answers::Hangs]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    sandbox.server(0).departs();
    let cancel = Cancel::new();
    let (ran, took) = given_up(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", "{}"),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &cancel,
            None,
            &Nothing,
        ),
    ));

    assert!(
        ran.is_none(),
        "the call was given up on while its replacement was being launched: it ran for \
         {took:?}"
    );
    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "the one it replaces was stopped"
    );
    let after = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("a server whose restart was given up on is finished with");
    assert!(
        matches!(&after, ToolError::StaleGeneration { .. }),
        "{after}"
    );
    let disposed = awaited(hosting.dispose(&context))
        .expect_err("what the launch given up on began is unconfirmed");
    assert!(disposed.to_string().contains("unconfirmed"), "{disposed}");
}

/// Awaits `work` on the tests' runtime for at most [`OUTLASTED`], so a step
/// that is never given up on fails its test rather than hanging it.
///
/// `None` is work that was still waiting then, and how long it ran.
fn outlasted<F: Future>(work: F) -> (Option<F::Output>, Duration) {
    let guard = Cancel::new().child_until(Instant::now().checked_add(OUTLASTED));
    let began = Instant::now();
    let ran = crate::testing::runtime().block_on(guard.race(work));
    (ran, began.elapsed())
}

/// How long a test waits for a step it expects to end on its own.
const OUTLASTED: Duration = Duration::from_secs(5);

#[test]
fn a_replacement_whose_sandbox_never_answers_is_given_up_on_at_the_handshake_patience() {
    // Nobody cancels anything: the call waits as long as the restart does,
    // and the restart's sandbox step is what has to end on its own.
    let mut first = opening("docs", &json!([offers("search")]));
    first.push(produced("never said", false));
    let sandbox = Pretend::new([Answers::Says(first), Answers::Hangs]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    sandbox.server(0).departs();
    let cancel = Cancel::new();
    let (ran, took) = outlasted(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", "{}"),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &cancel,
            None,
            &Nothing,
        ),
    ));

    let Some(Err(refused)) = ran else {
        panic!(
            "a restart whose sandbox never answers ends at the handshake patience with no cancel \
             raised; after {took:?} it had not"
        );
    };
    assert!(took < OUTLASTED / 2, "it took {took:?}");
    assert!(
        refused.to_string().contains("after 500ms"),
        "the refusal says how long was waited: {refused}"
    );
    let disposed = awaited(hosting.dispose(&context))
        .expect_err("what the step given up on began is unconfirmed");
    assert!(disposed.to_string().contains("unconfirmed"), "{disposed}");
}

#[test]
fn a_disposal_given_up_on_part_way_leaves_the_next_preparation_to_start_again() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("two pages", false));
    let sandbox = Pretend::new([
        Answers::Slowly(frames, 2, Duration::from_secs(1)),
        Answers::Says(opening("docs", &json!([offers("search")]))),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![patient("docs", DAWDLING)],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    // A call holds the server while it thinks, so the disposal waits behind
    // it, and is given up on there.
    let disposed = std::thread::scope(|scope| {
        let calling = scope.spawn(|| calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new()));
        std::thread::sleep(Duration::from_millis(150));
        let (disposed, _) = given_up(hosting.dispose(&context));
        calling
            .join()
            .expect("the call ran")
            .expect("the call was answered");
        disposed
    });
    assert!(
        disposed.is_none(),
        "the disposal was given up on while it waited"
    );

    awaited(hosting.prepare(&context)).expect("the servers started again");
    assert_eq!(
        sandbox.started(),
        2,
        "a disposal given up on part way leaves the next preparation to start again, not to \
         stand on a lifecycle whose tools were already withdrawn"
    );
    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "and it stopped the first first"
    );
    assert!(
        awaited(hosting.snapshot(&context))
            .expect("one generation")
            .find("mcp:docs/search")
            .is_some()
    );
    awaited(hosting.dispose(&context)).expect("the server stopped");
}

#[test]
fn a_preparation_that_left_every_optional_server_out_is_tried_again() {
    let sandbox = Pretend::new([
        Answers::Refuses,
        Answers::Says(opening("docs", &json!([offers("search")]))),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![optional("docs")],
        crate::testing::runtime(),
    );
    let context = lifecycle();

    awaited(hosting.prepare(&context)).expect("an optional server cannot fail the turn");
    awaited(hosting.prepare(&context)).expect("and neither can its second chance");

    assert!(
        awaited(hosting.snapshot(&context))
            .expect("one generation")
            .find("mcp:docs/search")
            .is_some(),
        "a preparation that started nothing tries again"
    );
    awaited(hosting.dispose(&context)).expect("the server stopped");
}

#[test]
fn a_call_waiting_behind_one_whose_answer_is_unsettled_asks_the_server_nothing() {
    // The first call's server goes quiet past the request patience, so its
    // question is still with the server when the lock is handed on; the call
    // queued behind it must not ask a second one.
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("late", false));
    let sandbox = Pretend::new([Answers::Slowly(frames, 2, DAWDLING)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![patient("docs", Duration::from_millis(300))],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let (first, second) = std::thread::scope(|scope| {
        let first = scope.spawn(|| calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new()));
        std::thread::sleep(Duration::from_millis(50));
        let second = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new());
        (first.join().expect("the first call ran"), second)
    });

    first.expect_err("the first call's server went quiet");
    second.expect_err("the second call found the server finished with");
    let asked = sandbox
        .server(0)
        .sent()
        .iter()
        .filter(|frame| frame.get("method").and_then(Value::as_str) == Some("tools/call"))
        .count();
    assert_eq!(
        asked, 1,
        "one question was outstanding, and no second was asked"
    );
    awaited(hosting.dispose(&context)).expect("nothing left to stop");
}
