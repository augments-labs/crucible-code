//! A whole asynchronous turn, taken the way the application takes one, with
//! the faults a turn has to survive put into it on purpose.
//!
//! Each turn is a task on the application's own runtime, as is everything it
//! starts, and talks to a loopback vendor over the shared HTTP client, to
//! built-in tools, to an MCP server hosted through the sandbox seam and to a
//! real session log. Into that go a stalled answer, a stop at each point a
//! call is recorded, a call past its deadline, a tool that panics, a process
//! that dies mid-turn and a server whose cleanup fails; and two agents take
//! their turns at once, each on its own wire, with its own tools, check,
//! session and spend.
//!
//! What every case holds the turn to is the same: each call the model asked
//! for is answered exactly once and none is run twice, nothing either agent
//! has reaches the other, the runtime's workers are free whenever the vendor
//! looks, and everything the turn started has stopped once the application's
//! services shut down.

// Test-only helpers fail the owning case when its controlled fixture is invalid.
#![allow(clippy::expect_used, clippy::panic)]

#[path = "async_trial/harness.rs"]
mod harness;
#[path = "async_trial/server.rs"]
mod server;
#[path = "async_trial/vendor.rs"]
mod vendor;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crucible_app::services::serving;
use crucible_context::ContextInputs;
use crucible_mcp::Hosting;
use crucible_models::ProviderError;
use crucible_runner::{AgentBuilder, Bounds, Model, RunPolicy, Runner, Tools, TurnError, Turned};
use crucible_runtime::{Aside, Cancel, Steer};
use crucible_sandbox::SandboxService;
use crucible_storage::JournalStore;
use crucible_tools::ToolsetError;
use crucible_types::{AgentId, Message, StopReason, Transcript};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

use harness::{
    Falls, Guard, LATE, Late, Permit, Scratch, Seen, Stage, Stamp, Trip, Tripwire, described,
};
use server::{Docs, FOUND, TOOL, chosen};
use vendor::{Call, Gate, Probe, Reply, Vendor, WAIT, Wire, advertised};

/// What the first agent is asked.
const ASKED_ALPHA: &str = "alpha: stamp it, wait, fall over and look it up";

/// What the second agent is asked.
const ASKED_BETA: &str = "beta: stamp it twice";

/// The definition of an agent called `id`, aimed at `wire`'s model.
fn agent(id: &str, wire: Wire) -> AgentBuilder {
    AgentBuilder::new(
        AgentId::new(id),
        Model {
            name: wire.model().into(),
            max_tokens: 4096,
            window: Some(200_000),
            accepts: None,
            effort: None,
        },
    )
}

/// Every result the transcript holds, by the call it answers.
fn answers(transcript: &Transcript) -> BTreeMap<String, Vec<String>> {
    let mut answers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for message in transcript.messages() {
        if let Message::ToolResults(results) = message {
            for result in results {
                answers
                    .entry(result.id.as_str().to_owned())
                    .or_default()
                    .push(result.output.text().to_owned());
            }
        }
    }
    answers
}

/// The id of every call the transcript holds, in the order they were asked.
fn calls(transcript: &Transcript) -> Vec<String> {
    transcript
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::Agent { calls, .. } => Some(calls),
            _ => None,
        })
        .flatten()
        .map(|call| call.id.as_str().to_owned())
        .collect()
}

/// Everything the transcript says in words, from either side.
fn words(transcript: &Transcript) -> String {
    let mut words = String::new();
    for message in transcript.messages() {
        match message {
            Message::User { text, .. } | Message::Agent { text, .. } => words.push_str(text),
            Message::ToolResults(results) => {
                for result in results {
                    words.push_str(result.output.text());
                }
            }
            Message::Context(_) => {}
        }
        words.push('\n');
    }
    words
}

/// Asserts that every call the transcript holds was answered exactly once,
/// and that nothing was answered that was not asked.
fn answered_once(transcript: &Transcript) -> BTreeMap<String, String> {
    let mut asked = calls(transcript);
    asked.sort();
    let answers = answers(transcript);
    assert_eq!(
        asked,
        answers.keys().cloned().collect::<Vec<_>>(),
        "every call answered, and nothing answered that was not asked: {answers:?}"
    );
    answers
        .into_iter()
        .map(|(id, mut said)| {
            assert_eq!(said.len(), 1, "{id} answered more than once: {said:?}");
            (id, said.remove(0))
        })
        .collect()
}

/// What a turn's task hands back once the turn is over.
struct Taken {
    /// The runner, holding the transcript the turn left.
    runner: Runner,
    /// What the turn came to.
    turned: Result<Turned, TurnError>,
    /// Every call the person was asked about.
    permit: Permit,
}

/// One turn of `runner`, taken as a task on `runtime`, posting to `seen` and
/// stopped by `cancel`, as the key a person presses stops one.
fn taking(
    runtime: &Handle,
    mut runner: Runner,
    asked: &'static str,
    seen: Seen,
    cancel: Cancel,
) -> JoinHandle<Taken> {
    runtime.spawn(async move {
        let (steer, aside) = (Steer::new(), Aside::new());
        let mut permit = Permit::default();
        let run = runner.starting(&seen, &cancel, &steer, &aside);
        let turned = runner.turn(asked, Box::new([]), &mut permit, &run).await;
        drop(run);
        Taken {
            runner,
            turned,
            permit,
        }
    })
}

/// One turn of `runner`, stopped by `cancel`, with the task that took it
/// waited for to its end.
fn turn(runtime: &Handle, runner: Runner, asked: &'static str, cancel: &Cancel) -> Taken {
    let task = taking(runtime, runner, asked, Seen::default(), cancel.clone());
    runtime.block_on(task).expect("the turn's task ended")
}

/// Two agents take their turns at once on the application's runtime, and
/// neither one's tools, check, events, session or spend reaches the other.
///
/// The two are made to overlap. The second agent's vendor holds its first
/// answer back until the first agent's stamp has run, so both turns are in
/// flight together; and the first agent's vendor holds its last answer back
/// until the second agent's turn is over, a stalled stream the first turn
/// simply waits on. The first agent's one pass asks for four calls: a stamp, a call that outlives its deadline, a
/// tool that panics and the hosted server's search. The second agent stamps
/// once per pass under a spend ceiling its passes cross on the second, and is
/// stopped at the third before it asks for anything, while the first agent
/// spends ten times as much under no ceiling and is never stopped.
#[test]
fn two_agents_take_their_turns_at_once_without_sharing_anything() {
    let (alpha_scratch, beta_scratch) = (Scratch::new(), Scratch::new());
    let ((), shutdown) = serving(|services| {
        let runtime = services
            .runtime()
            .handle()
            .expect("the application's runtime");
        let http = services.http().clone();
        let probe = Probe::of(runtime.clone());
        let (alpha_stamped, beta_done) = (Gate::default(), Gate::default());

        let (stamp, alpha_stamps) = Stamp::opening("stamp_alpha", &alpha_stamped);
        let mut tools = Tools::new();
        tools
            .add(described("stamp_alpha"), stamp)
            .expect("a new name");
        tools
            .add(
                described("late")
                    .timing_out_after(LATE)
                    .expect("a deadline"),
                Arc::new(Late),
            )
            .expect("a new name");
        tools
            .add(described("falls"), Arc::new(Falls))
            .expect("a new name");
        let (docs, served) = Docs::willing();
        let hosting = Hosting::new(
            Arc::new(tools),
            docs as Arc<dyn SandboxService>,
            vec![chosen("docs")],
            runtime.clone(),
        );
        let alpha_vendor = Vendor::new(
            Wire::Claude,
            vec![
                Reply::with(|request| {
                    let search = advertised(request)
                        .into_iter()
                        .find(|name| name.contains(TOOL))
                        .expect("the hosted server's tool was offered");
                    Wire::Claude.answer(
                        "alpha looks",
                        &[
                            Call::to("alpha-1", "stamp_alpha"),
                            Call::to("alpha-2", "late"),
                            Call::to("alpha-3", "falls"),
                            Call::to("alpha-4", &search),
                        ],
                        1000,
                    )
                }),
                Reply::saying(Wire::Claude.answer("alpha is done", &[], 10))
                    .after(&beta_done)
                    .probing(&probe),
            ],
        );
        let (alpha_guard, alpha_heard) = Guard::new("alpha-check");
        let alpha = Runner::with_toolset(
            Wire::Claude.provider(alpha_vendor.endpoint.clone(), http.clone()),
            hosting,
            agent("alpha", Wire::Claude)
                .checking_input(alpha_guard)
                .expect("one check")
                .build(),
            ContextInputs::new(alpha_scratch.path()),
            Arc::new(alpha_scratch.session()),
        );

        let (stamp, beta_stamps) = Stamp::new("stamp_beta");
        let mut tools = Tools::new();
        tools
            .add(described("stamp_beta"), stamp)
            .expect("a new name");
        let beta_vendor = Vendor::new(
            Wire::OpenAi,
            vec![
                Reply::saying(Wire::OpenAi.answer(
                    "beta stamps",
                    &[Call::to("beta-1", "stamp_beta")],
                    60,
                ))
                .after(&alpha_stamped),
                Reply::saying(Wire::OpenAi.answer(
                    "beta stamps again",
                    &[Call::to("beta-2", "stamp_beta")],
                    60,
                ))
                .probing(&probe),
            ],
        );
        let (beta_guard, beta_heard) = Guard::new("beta-check");
        let beta = Runner::new(
            Wire::OpenAi.provider(beta_vendor.endpoint.clone(), http),
            tools,
            agent("beta", Wire::OpenAi)
                .checking_input(beta_guard)
                .expect("one check")
                .build(),
            ContextInputs::new(beta_scratch.path()),
            Arc::new(beta_scratch.session()),
        )
        .under(RunPolicy {
            bounds: Bounds {
                spend: Some(100),
                ..Bounds::default()
            },
            ..RunPolicy::default()
        });

        let (alpha_seen, beta_seen) = (Seen::default(), Seen::default());
        let alpha_task = taking(
            &runtime,
            alpha,
            ASKED_ALPHA,
            alpha_seen.clone(),
            Cancel::new(),
        );
        let beta_task = taking(&runtime, beta, ASKED_BETA, beta_seen.clone(), Cancel::new());
        let beta_took = runtime
            .block_on(beta_task)
            .expect("the second agent's task ended");
        beta_done.open();
        let alpha_took = runtime
            .block_on(alpha_task)
            .expect("the first agent's task ended");
        let Taken {
            runner: alpha,
            turned: alpha_turned,
            permit: alpha_permit,
        } = alpha_took;
        let Taken {
            runner: beta,
            turned: beta_turned,
            permit: beta_permit,
        } = beta_took;

        // Each agent ended its own way: the one under a ceiling at it, the
        // other, which spent ten times as much, where its model yielded.
        let alpha_turned = alpha_turned.expect("the first agent's turn");
        assert!(
            matches!(alpha_turned.stop(), Some(StopReason::Yielded)),
            "{:?}",
            alpha_turned.stop()
        );
        assert!(
            matches!(beta_turned, Err(TurnError::Spent { ceiling: 100 })),
            "{beta_turned:?}"
        );
        assert_eq!(
            beta_vendor.requests().len(),
            2,
            "the second agent was charged for its own passes and no one else's"
        );

        // Every call ran at most once and was answered exactly once.
        assert_eq!(alpha_stamps.load(Ordering::SeqCst), 1);
        assert_eq!(beta_stamps.load(Ordering::SeqCst), 2);
        assert_eq!(served.calls(), 1, "the hosted search was asked once");
        let alpha_answers = answered_once(alpha.transcript());
        let beta_answers = answered_once(beta.transcript());
        let answer = |answers: &BTreeMap<String, String>, id: &str| {
            answers.get(id).cloned().unwrap_or_default()
        };
        assert_eq!(answer(&alpha_answers, "alpha-1"), "stamp_alpha #1");
        assert!(answer(&alpha_answers, "alpha-2").contains("timed out"));
        assert!(answer(&alpha_answers, "alpha-3").contains("panicked"));
        assert!(answer(&alpha_answers, "alpha-4").contains(FOUND));
        assert_eq!(answer(&beta_answers, "beta-1"), "stamp_beta #1");
        assert_eq!(answer(&beta_answers, "beta-2"), "stamp_beta #2");

        // Neither vendor was offered the other agent's tools.
        let offered = |vendor: &Vendor| -> Vec<String> {
            vendor.requests().iter().flat_map(advertised).collect()
        };
        let (alpha_offered, beta_offered) = (offered(&alpha_vendor), offered(&beta_vendor));
        assert!(alpha_offered.iter().any(|name| name == "stamp_alpha"));
        assert!(alpha_offered.iter().any(|name| name.contains(TOOL)));
        assert!(!beta_offered.is_empty());
        assert!(beta_offered.iter().all(|name| name == "stamp_beta"));
        assert!(!alpha_offered.iter().any(|name| name == "stamp_beta"));

        // Each check heard its own agent, once, about its own words.
        assert_eq!(
            *alpha_heard.lock().expect("what the check heard"),
            [("alpha".to_owned(), ASKED_ALPHA.to_owned())]
        );
        assert_eq!(
            *beta_heard.lock().expect("what the check heard"),
            [("beta".to_owned(), ASKED_BETA.to_owned())]
        );

        // Each agent's events, permissions and transcript are its own.
        let alpha_run = only_run(&alpha_seen, "alpha");
        let beta_run = only_run(&beta_seen, "beta");
        assert_ne!(alpha_run, beta_run, "two turns are two runs");
        // Only the hosted search needed asking about, and only its own
        // agent's person was asked.
        assert_eq!(alpha_permit.0, ["alpha-4"]);
        assert!(beta_permit.0.is_empty(), "{:?}", beta_permit.0);
        let (alpha_words, beta_words) = (words(alpha.transcript()), words(beta.transcript()));
        assert!(!alpha_words.contains("beta"), "{alpha_words}");
        assert!(!beta_words.contains("alpha"), "{beta_words}");

        // The runtime had every worker free each time a vendor looked, and
        // the hosted server was started once and let go.
        assert_eq!(probe.seen(), [true, true]);
        assert_eq!(served.launches(), 1);
        assert!(served.all_ended(), "the hosted server was let go");
        drop((alpha, beta));
    });
    assert_eq!(
        shutdown,
        Ok(()),
        "every task either turn started was over by the end"
    );
}

/// The one run every event `seen` holds was posted under, having checked that
/// every call it names is `agent`'s.
fn only_run(seen: &Seen, agent: &str) -> crucible_types::RunId {
    let posted = seen.posted();
    let run = posted.first().expect("the turn posted something").run;
    for event in posted.iter() {
        assert_eq!(event.run, run, "one turn, one run: {}", event.said);
        if let Some(call) = &event.call {
            assert!(
                call.starts_with(&format!("{agent}-")),
                "{agent} was told about {call}"
            );
        }
    }
    run
}

/// A runner of one stamp over `store`, asking `vendor`.
fn stamping(
    scratch: &Scratch,
    stamp: &Arc<Stamp>,
    vendor: &Vendor,
    http: &crucible_provider::HttpTurns,
    store: Arc<dyn JournalStore>,
) -> Runner {
    let mut tools = Tools::new();
    tools
        .add(
            described("stamp"),
            Arc::clone(stamp) as Arc<dyn crucible_tools::Tool>,
        )
        .expect("a new name");
    Runner::new(
        Wire::Claude.provider(vendor.endpoint.clone(), http.clone()),
        tools,
        agent("solo", Wire::Claude).build(),
        ContextInputs::new(scratch.path()),
        store,
    )
}

/// The vendor's answer asking for the one stamp.
fn asking_for_the_stamp() -> Reply {
    Reply::saying(Wire::Claude.answer("stamping", &[Call::to("call-1", "stamp")], 5))
}

/// Picks the session up again and takes one more turn, which the vendor
/// answers in words. What the turn was sent holds every call it names
/// exactly once with exactly one result.
fn carried_on(
    scratch: &Scratch,
    stamp: &Arc<Stamp>,
    runtime: &Handle,
    http: &crucible_provider::HttpTurns,
) {
    let (session, transcript) = scratch.resumed();
    answered_once(&transcript);
    let vendor = Vendor::new(
        Wire::Claude,
        vec![Reply::saying(Wire::Claude.answer("carried on", &[], 5))],
    );
    let runner = stamping(scratch, stamp, &vendor, http, Arc::new(session)).resuming(transcript);
    let Taken { runner, turned, .. } = turn(runtime, runner, "carry on", &Cancel::new());
    let turned = turned.expect("the resumed turn");
    assert!(matches!(turned.stop(), Some(StopReason::Yielded)));
    answered_once(runner.transcript());
    let sent = vendor
        .requests()
        .first()
        .map(ToString::to_string)
        .expect("the resumed turn asked");
    for asked in ["\"tool_use\"", "\"tool_result\""] {
        assert!(
            sent.matches(asked).count() <= 1,
            "the resumed request repeats a call: {sent}"
        );
    }
    assert_eq!(
        sent.matches("\"tool_use\"").count(),
        sent.matches("\"tool_result\"").count(),
        "the resumed request holds a call without its result: {sent}"
    );
}

/// A stop requested at each point a call is recorded ends the turn with the
/// call accounted for once, and the session picked up afterwards neither runs
/// it again nor sends it unanswered.
///
/// Stopped once the call is prepared, it never ran; once it is started, it
/// may have; once it is finished, it did. In every case the stamp's count
/// after the session is picked up and a further turn taken is what it was
/// when the stop landed.
#[test]
fn a_stop_at_each_record_of_a_call_accounts_for_it_once() {
    for stage in [Stage::Prepared, Stage::Started, Stage::Finished] {
        let scratch = Scratch::new();
        let ((), shutdown) = serving(|services| {
            let runtime = services
                .runtime()
                .handle()
                .expect("the application's runtime");
            let http = services.http().clone();
            let (stamp, stamps) = Stamp::new("stamp");
            let cancel = Cancel::new();
            let vendor = Vendor::new(Wire::Claude, vec![asking_for_the_stamp()]);
            let store = Tripwire::new(scratch.session(), stage, Trip::Cancel(cancel.clone()));
            let runner = stamping(&scratch, &stamp, &vendor, &http, Arc::new(store));
            let Taken { runner, turned, .. } = turn(&runtime, runner, "stamp it", &cancel);
            let ran = stamps.load(Ordering::SeqCst);
            match stage {
                Stage::Prepared => assert_eq!(ran, 0, "a prepared call has not run"),
                Stage::Started => assert!(ran <= 1, "a started call ran at most once"),
                Stage::Finished => assert_eq!(ran, 1, "a finished call ran"),
            }
            // Stopped among the calls, the turn ends stopped. Stopped once they
            // are all answered, it ends at the next request, which the provider
            // declines to send for a turn already abandoned.
            match stage {
                Stage::Prepared | Stage::Started => assert!(
                    matches!(
                        turned.as_ref().map(Turned::stop),
                        Ok(Some(StopReason::Cancelled))
                    ),
                    "{stage:?}: {turned:?}"
                ),
                Stage::Finished => assert!(
                    matches!(
                        turned,
                        Err(TurnError::Provider(ProviderError::Cancelled(_)))
                    ),
                    "{stage:?}: {turned:?}"
                ),
            }
            assert_eq!(
                vendor.requests().len(),
                1,
                "{stage:?}: the model was asked again"
            );
            let answered = answered_once(runner.transcript());
            assert_eq!(answered.len(), 1, "{stage:?}: {answered:?}");
            drop(runner);

            carried_on(&scratch, &stamp, &runtime, &http);
            assert_eq!(
                stamps.load(Ordering::SeqCst),
                ran,
                "{stage:?}: picking the session up ran the call again"
            );
        });
        assert_eq!(
            shutdown,
            Ok(()),
            "{stage:?}: the stopped turn left work behind"
        );
    }
}

/// A turn whose process dies after a call's result is recorded, and before
/// the result reaches the transcript, is picked up with no call left hanging
/// and without running the call again.
#[test]
fn a_turn_that_dies_after_its_call_is_picked_up_without_running_it_again() {
    let scratch = Scratch::new();
    let ((), shutdown) = serving(|services| {
        let runtime = services
            .runtime()
            .handle()
            .expect("the application's runtime");
        let http = services.http().clone();
        let (stamp, stamps) = Stamp::new("stamp");
        let dead = Gate::default();
        let vendor = Vendor::new(Wire::Claude, vec![asking_for_the_stamp()]);
        let store = Tripwire::new(scratch.session(), Stage::Finished, Trip::Park(dead.clone()));
        let runner = stamping(&scratch, &stamp, &vendor, &http, Arc::new(store));
        let task = taking(&runtime, runner, "stamp it", Seen::default(), Cancel::new());
        let recorded = dead.opened(WAIT);
        task.abort();
        let died = runtime.block_on(task);
        assert!(recorded, "the call's result was recorded");
        assert!(
            died.as_ref()
                .is_err_and(tokio::task::JoinError::is_cancelled),
            "the turn was cut off where it stood: {:?}",
            died.map(|taken| taken.turned)
        );
        drop(vendor);
        assert_eq!(stamps.load(Ordering::SeqCst), 1);

        carried_on(&scratch, &stamp, &runtime, &http);
        assert_eq!(
            stamps.load(Ordering::SeqCst),
            1,
            "picking the session up ran the call again"
        );
    });
    assert_eq!(shutdown, Ok(()), "the dead turn left work behind");
}

/// A hosted server that will not stop fails the turn that started it, after
/// its one call was answered once, and refuses the next turn before anything
/// is asked of the vendor.
#[test]
fn a_server_that_will_not_stop_fails_its_turn_and_refuses_the_next() {
    let scratch = Scratch::new();
    let ((), shutdown) = serving(|services| {
        let runtime = services
            .runtime()
            .handle()
            .expect("the application's runtime");
        let (docs, served) = Docs::stubborn();
        let hosting = Hosting::new(
            Arc::new(Tools::new()),
            docs as Arc<dyn SandboxService>,
            vec![chosen("docs")],
            runtime.clone(),
        );
        let vendor = Vendor::new(
            Wire::Claude,
            vec![
                Reply::with(|request| {
                    let search = advertised(request)
                        .into_iter()
                        .find(|name| name.contains(TOOL))
                        .expect("the hosted server's tool was offered");
                    Wire::Claude.answer("searching", &[Call::to("call-1", &search)], 5)
                }),
                Reply::saying(Wire::Claude.answer("found it", &[], 5)),
            ],
        );
        let runner = Runner::with_toolset(
            Wire::Claude.provider(vendor.endpoint.clone(), services.http().clone()),
            hosting,
            agent("solo", Wire::Claude).build(),
            ContextInputs::new(scratch.path()),
            Arc::new(scratch.session()),
        );

        let Taken {
            runner,
            turned: first,
            ..
        } = turn(&runtime, runner, "look it up", &Cancel::new());
        assert!(
            matches!(first, Err(TurnError::Toolset(ToolsetError::Source { .. }))),
            "{first:?}"
        );
        assert_eq!(served.calls(), 1);
        let answered = answered_once(runner.transcript());
        assert!(
            answered.values().all(|said| said.contains(FOUND)),
            "{answered:?}"
        );

        let Taken {
            runner,
            turned: second,
            ..
        } = turn(&runtime, runner, "again", &Cancel::new());
        // Refused on the way in, and refused again by the cleanup that
        // follows every preparation, for the one server still unconfirmed.
        let unconfirmed = |problem: &ToolsetError| matches!(problem, ToolsetError::Source { problem, .. } if problem.contains("unconfirmed"));
        match &second {
            Err(TurnError::ToolsetCleanup { primary, cleanup }) => {
                assert!(
                    matches!(&**primary, TurnError::Toolset(problem) if unconfirmed(problem)),
                    "{second:?}"
                );
                assert!(unconfirmed(cleanup), "{second:?}");
            }
            other => panic!("the second turn was not refused: {other:?}"),
        }
        assert_eq!(vendor.requests().len(), 2, "the refused turn asked nothing");
        assert_eq!(served.launches(), 1, "nothing was started over it");
        assert_eq!(served.calls(), 1);
        drop(runner);
    });
    assert_eq!(
        shutdown,
        Ok(()),
        "the stubborn server's streams were let go with the turn"
    );
}
