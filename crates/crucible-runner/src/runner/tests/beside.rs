//! What two runs of two definitions can reach of one another while both are
//! under way: what each may spend, which run each event names, a question
//! still waiting for its answer, what a tool handed back, and the cache
//! resources each one owns.
//!
//! The definitions, the checks and the roster are the guardrail module's to
//! prove. Everything here is about what a run *accumulates*, which is where a
//! leak would be a line somebody wrote rather than a field somebody shared.

use crucible_agents::Availability;
use crucible_core::{CredentialScopeId, Remember, ToolCall};

use super::guardrails::{Gate, agent, answering, offered};
use super::*;

/// A destination that keeps the attribution, so a test can read whose run an
/// event was filed under.
struct Filed(Mutex<Sender<EventEnvelope>>);

impl Post for Filed {
    fn post(&self, reported: EventEnvelope) {
        drop(self.0.lock().unwrap().send(reported));
    }
}

/// One pass that asks for `name` and reports having cost `spend` tokens.
fn costing(id: &str, name: &str, spend: u64) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new(id),
            name: name.into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Spent(Spend::new(spend)),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// A reader who says so when asked, and answers only once the test lets them.
///
/// The one way to hold a question open for as long as a test needs it to be:
/// the run that asked is stopped inside its own permission prompt, with the
/// call neither allowed nor refused.
struct Deliberating {
    /// Told each time a question arrives.
    put: Sender<()>,
    /// Waited on before the answer is given.
    go: Receiver<()>,
    /// How often the reader was put to the question.
    asked: usize,
}

impl Ask for Deliberating {
    fn ask<'a>(
        &'a mut self,
        _call: &'a ToolCall,
        _sensitivity: &'a Sensitivity,
    ) -> crucible_runtime::BoxFuture<'a, (Verdict, Remember)> {
        // A real rendezvous rather than a future that answers `Pending` and
        // wakes: this fixture holds one run against another on the threads
        // the test itself put them on, so it is those threads, and not a
        // poll, that must wait here.
        self.asked += 1;
        self.put.send(()).expect("the test to be waiting");
        self.go.recv().expect("the test to let the reader answer");
        Box::pin(async { (Verdict::Allow, Remember::Session) })
    }
}

#[test]
fn two_runs_at_once_spend_against_their_own_ceilings_and_file_under_their_own_runs() {
    // One run is held inside a tool call having already spent past its own
    // small ceiling. The other, allowed ten times as much, spends the same
    // while the first is held and has to finish: a tally the two shared would
    // stop it, and a ceiling the two shared would stop one of them at the
    // other's figure. Every event either posts names its own run, and neither
    // run descends from the other.
    let (started, mid_turn) = channel();
    let (release, go) = channel();
    let mut held = Tools::new();
    held.add_builtin(Gate::new(started, go)).unwrap();

    let mut one = Scripted::under(
        Script::new(vec![costing("c1", "gate", 90), answering("never asked")]),
        held,
        agent("one")
            .telling("You are one.")
            .offering(Availability::Named(Box::new(["gate".into()])))
            .build(),
    );
    one.runner.policy.bounds.spend = Some(50);
    let mut two = Scripted::under(
        Script::new(vec![costing("c2", "read", 90), answering("from two")]),
        tools([Fixed::new("read")]),
        agent("two").telling("You are two.").build(),
    );
    two.runner.policy.bounds.spend = Some(500);

    let (to_one, from_one) = channel();
    let (to_two, from_two) = channel();
    let (to_one, to_two) = (Filed(Mutex::new(to_one)), Filed(Mutex::new(to_two)));

    // Nothing is asserted while the first run is held: a failure there would
    // leave its thread waiting on a test that had already given up.
    let (stopped, finished) = thread::scope(|scope| {
        let filing = &to_one;
        let running = scope.spawn(move || {
            let run = one
                .runner
                .starting(filing, &one.cancel, &one.steer, &one.aside);
            one.runner
                .turn("go", Box::new([]), &mut one.says, &run)
                .awaited()
        });

        mid_turn
            .recv()
            .expect("the first run to reach its tool call");
        let run = two
            .runner
            .starting(&to_two, &two.cancel, &two.steer, &two.aside);
        let finished = two
            .runner
            .turn("go", Box::new([]), &mut two.says, &run)
            .awaited()
            .map(ran);

        release.send(()).expect("the first run to still be waiting");
        (running.join().expect("the first run's thread"), finished)
    });

    assert_eq!(
        finished.expect("the second run to finish under its own ceiling"),
        StopReason::Yielded,
        "a run was stopped for what the run beside it had spent"
    );
    let problem = stopped.expect_err("the first run is over its own ceiling");
    assert!(
        matches!(problem, TurnError::Spent { ceiling } if ceiling == 50),
        "a run was held to a ceiling that was not its own: {problem:?}"
    );

    drop((to_one, to_two));
    let ones: Vec<_> = from_one.into_iter().collect();
    let twos: Vec<_> = from_two.into_iter().collect();
    let first = ones.first().expect("the first run reported something");
    let second = twos.first().expect("the second run reported something");
    assert_ne!(first.run(), second.run(), "two runs answered to one name");
    assert_ne!(
        first.ancestry().root(),
        second.ancestry().root(),
        "two runs nothing started share a root"
    );
    for (reported, own) in [(&ones, first), (&twos, second)] {
        for one in reported {
            assert_eq!(
                one.ancestry(),
                own.ancestry(),
                "{:?} was filed under a run other than the one that posted it",
                one.event()
            );
            assert_eq!(one.ancestry().parent(), None);
        }
    }
}

#[test]
fn a_question_one_run_is_waiting_on_is_neither_put_to_nor_settled_for_the_other() {
    // The first run asks to write and its reader has not answered. The second
    // asks for exactly the same call meanwhile: it is put to the second run's
    // reader and nobody else's. Then the first reader says yes for the whole
    // session, which holds for the first run's next call and not for the
    // second run's.
    let writing = || tools([Fixed::new("write").risking(changing())]);
    let (put, asked) = channel();
    let (answer, go) = channel();
    let mut reader = Deliberating { put, go, asked: 0 };

    let mut one = Scripted::under(
        Script::new(vec![
            calling("a", "write", "{}"),
            calling("b", "write", "{}"),
            answering("one wrote twice"),
        ]),
        writing(),
        agent("one").build(),
    );
    let mut two = Scripted::under(
        Script::new(vec![
            calling("a", "write", "{}"),
            answering("two wrote"),
            calling("b", "write", "{}"),
            answering("two wrote again"),
        ]),
        writing(),
        agent("two").build(),
    );

    // Read while the first run's question is open, asserted once it is not.
    let (beside, put_to_two) = thread::scope(|scope| {
        let reader = &mut reader;
        let running = scope.spawn(move || {
            let run = one
                .runner
                .starting(&one.events, &one.cancel, &one.steer, &one.aside);
            one.runner
                .turn("go", Box::new([]), reader, &run)
                .awaited()
                .map(ran)
                .expect("the first run to finish once it is answered")
        });

        asked.recv().expect("the first run to ask");
        let beside = two.turn("go");
        let put_to_two = two.says.asked;

        answer.send(()).expect("the first reader to be waiting");
        assert_eq!(running.join().unwrap(), StopReason::Yielded);
        (beside, put_to_two)
    });

    assert_eq!(
        beside.expect("the second run's turn"),
        StopReason::Yielded,
        "a run waited on a question put to the run beside it"
    );
    assert_eq!(
        put_to_two, 1,
        "the second run's call was not put to the second run's reader"
    );
    assert_eq!(
        reader.asked, 1,
        "a yes for the session did not hold for the run it was given to"
    );
    assert_eq!(two.turn("again").unwrap(), StopReason::Yielded);
    assert_eq!(
        two.says.asked, 2,
        "a yes one run's reader gave for the session settled the other run's call"
    );
}

/// Every tool result a run's transcript holds, in order.
fn handed(scripted: &Scripted) -> Vec<&str> {
    scripted
        .runner
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::ToolResults(results) => Some(results.iter()),
            _ => None,
        })
        .flatten()
        .map(|result| result.output.text())
        .collect()
}

#[test]
fn what_a_tool_handed_one_run_is_never_sent_or_shown_by_the_other() {
    // The first run reads something it should not pass on and is then held
    // open with it in hand. The second run takes a whole turn beside it, over
    // a roster that names the same tool: no request it sends carries the
    // words, nothing it reports shows them, and its transcript never held
    // them. The first run's own next request does carry them, so the test
    // is looking for something that is really there to be found.
    const SECRET: &str = "sk-one-only-0123456789";
    let (started, mid_turn) = channel();
    let (release, go) = channel();
    let mut held = Tools::new();
    held.add_builtin(Gate::new(started, go)).unwrap();
    held.add_builtin(Fixed::new("vault").answering(SECRET))
        .unwrap();

    let mut one = Scripted::under(
        Script::new(vec![
            calling("c1", "vault", "{}"),
            calling("c2", "gate", "{}"),
            answering("one is done"),
        ]),
        held,
        agent("one").telling("You are one.").build(),
    );
    let mut two = Scripted::under(
        Script::new(vec![calling("c1", "vault", "{}"), answering("from two")]),
        tools([Fixed::new("vault").answering("nothing to see")]),
        agent("two").telling("You are two.").build(),
    );
    let (first, second) = (one.sent.clone(), two.sent.clone());

    let (one, beside) = thread::scope(|scope| {
        let running = scope.spawn(move || {
            one.turn("go").expect("the first run's turn");
            one
        });

        mid_turn
            .recv()
            .expect("the first run to be holding what it read");
        let beside = two.turn("go");

        release.send(()).expect("the first run to still be waiting");
        (running.join().expect("the first run's thread"), beside)
    });
    assert_eq!(beside.unwrap(), StopReason::Yielded);

    assert!(
        first
            .lock()
            .unwrap()
            .iter()
            .any(|request| request.carried_result(SECRET)),
        "the run that read the words never sent them, so nothing here looked for a leak"
    );
    let second = second.lock().unwrap();
    let [opening, _] = second.as_slice() else {
        panic!(
            "the second run sends two requests, and sent {}",
            second.len()
        );
    };
    assert!(
        !second.iter().any(|request| request.carried_result(SECRET)),
        "one run's tool result went out in the other's request"
    );
    assert_eq!(offered(opening), vec!["vault".to_owned()]);
    let shown: Vec<_> = two
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ToolFinished { output, .. } => Some(output.text().to_owned()),
            _ => None,
        })
        .collect();
    assert_eq!(
        shown,
        ["nothing to see"],
        "the second run reported something other than what its own tool handed it"
    );
    assert_eq!(
        handed(&two),
        ["nothing to see"],
        "one run's tool result is in the other's transcript"
    );
    assert!(
        handed(&one).contains(&SECRET),
        "the run that read the words does not hold them"
    );
}

#[test]
fn two_runs_keeping_cache_resources_in_one_place_own_and_retire_only_their_own() {
    // Everything that could tell the two apart for the wrong reason is made
    // equal: one credential, one model, one set of instructions, one store.
    // What is left is that they are two runs, and that alone has to keep one
    // from being handed the resource the other made, and from deleting it.
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let credential = CredentialScopeId::new();
    let caching = |id: &str| {
        let mut scripted = Scripted::under(
            Script::new(vec![answering("done")])
                .persistent()
                .with_credential_scope(credential),
            Tools::new(),
            agent(id).telling("stable fixture instructions").build(),
        )
        .storing(store.clone());
        scripted.runner.policy.prompt_cache = scripted
            .runner
            .policy
            .prompt_cache
            .with_persistent_resources(PromptCachePersistentMode::Create);
        scripted
    };
    let (mut one, mut two) = (caching("one"), caching("two"));

    let (mut one, mut two) = thread::scope(|scope| {
        let first = scope.spawn(move || {
            one.turn("go").expect("the first run's turn");
            one
        });
        let second = scope.spawn(move || {
            two.turn("go").expect("the second run's turn");
            two
        });
        (first.join().unwrap(), second.join().unwrap())
    });

    for scripted in [&one, &two] {
        let sent = scripted.sent.lock().unwrap();
        assert!(
            sent.first().expect("the run's request").cache_resource,
            "a run sent no resource, so nothing here is about owning one"
        );
    }
    let owners: Vec<_> = records
        .lock()
        .unwrap()
        .iter()
        .map(|record| record.binding().owner_scope())
        .collect();
    let [first, second] = owners.as_slice() else {
        panic!("each run keeps a resource of its own, and the store holds {owners:?}");
    };
    assert_ne!(first, second, "two runs own their resources as one");

    let retired = one
        .runner
        .retire_prompt_cache(&Cancel::new())
        .expect("bounded retirement");
    assert_eq!(retired.deleted, 1);
    assert_eq!(
        records.lock().unwrap().len(),
        1,
        "retiring one run's resources reached the other's"
    );
    let retired = two
        .runner
        .retire_prompt_cache(&Cancel::new())
        .expect("bounded retirement");
    assert_eq!(
        retired.deleted, 1,
        "the second run's resource was already gone"
    );
    assert!(records.lock().unwrap().is_empty());
}
