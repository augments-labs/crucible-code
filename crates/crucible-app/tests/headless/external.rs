//! The same conversation, on an installed agent.
//!
//! Each case here moves a conversation onto Claude Code's route, played by
//! the fake peer, and drives it through the calls a front end makes: a turn,
//! a stop, `/clear`, `/resume`, a change of mode, a decision on what the agent
//! asked. What is read back is what a front end reads, the turn's answer and
//! the events it posted, beside what only the agent saw: which runs were
//! launched, and what each heard.

use std::future::{Future, poll_fn};
use std::path::PathBuf;
use std::pin::pin;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::task::Poll;

use crucible_app::access::Route;
use crucible_app::backends::{REGISTRY, Registry, Row};
use crucible_app::client::{self, Front, Shown};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::external::External;
use crucible_app::launching::{Agent, Launcher};
use crucible_app::outside::Relaunch;
use crucible_app::{Conversation, Settled};
use crucible_client_api::{
    Capabilities as Asking, Command, Correlation, Decision as Ruled, Lasting as Lasts, Pending,
    PendingId, Progress, Prompt, Request, Ruling,
};
use crucible_config::Driver;
use crucible_execution::{
    Backend, Capabilities, Capability, Decision, Failure, Lasting, SessionRef,
};
use crucible_external_harnesses::fake_peer::{self, Reply, Script as Peer, scripts::claude};
use crucible_external_harnesses::{Account, Control};
use crucible_runner::{Event, EventEnvelope, Post, SessionCost, Totals, TurnError, Turned};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_sandbox::{SandboxPolicy, SandboxService};
use crucible_sandbox_local::LocalSandbox;
use crucible_session::{ExternalRef, RouteBinding, Session};
use crucible_tools::Mode;
use crucible_types::{CostAmount, Message, PricingCurrency, PricingUnit, StopReason, Transcript};

use super::{Failed, Nobody, Script, Tree, saying};

/// The runtime an agent's turns run on: its pipes are read on it, so it
/// drives input and output as the application's own does.
fn runtime() -> Result<tokio::runtime::Handle, Failed> {
    static RUNTIME: std::sync::OnceLock<std::io::Result<tokio::runtime::Runtime>> =
        std::sync::OnceLock::new();
    match RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
    }) {
        Ok(runtime) => Ok(runtime.handle().clone()),
        Err(problem) => Err(format!("no runtime to take a turn on: {problem}").into()),
    }
}

/// What the agent's runs are taken to claim.
fn claims() -> Capabilities {
    Capabilities::unknown()
        .declared(Capability::Cancel, true)
        .declared(Capability::PermissionOnce, true)
        .declared(Capability::PermissionSession, true)
        .declared(Capability::Resume, true)
}

/// Claude Code's own row, reaching no host: the peer reaches none, and a
/// sandbox that cannot hold a run to its hosts refuses to start one.
fn row() -> Result<&'static Row, Failed> {
    let shipped = REGISTRY
        .row(Driver::Claude)
        .ok_or("Claude Code has no row")?;
    Ok(Box::leak(Box::new(Row {
        reaches: &[],
        ..*shipped
    })))
}

/// What a launch was asked for: the mode, and the agent's own session it
/// resumed.
type Launched = (Mode, Option<String>);

/// Launches Claude Code's runs from the peer installed in a tree, and keeps
/// what each launch was asked for.
struct Launching {
    sandbox: Arc<dyn SandboxService>,
    runtime: tokio::runtime::Handle,
    policy: SandboxPolicy,
    consent: Consent,
    agent: Agent,
    row: &'static Row,
    launched: Arc<Mutex<Vec<Launched>>>,
}

impl Launching {
    /// The launcher this one starts each run through.
    fn launcher(&self) -> Launcher<'_> {
        Launcher::new(
            Arc::clone(&self.sandbox),
            self.runtime.clone(),
            &self.policy,
            &self.consent,
        )
    }
}

impl Relaunch for Launching {
    fn admits(&self, mode: Mode) -> Result<(), Failure> {
        (self.row.dialect)(mode).map(drop)
    }

    fn launch<'a>(
        &'a self,
        mode: Mode,
        resume: Option<&'a ExternalRef>,
    ) -> BoxFuture<'a, Result<Box<dyn Backend>, Failure>> {
        Box::pin(async move {
            if let Ok(mut launched) = self.launched.lock() {
                launched.push((mode, resume.map(|named| named.reference().to_owned())));
            }
            let mut dialect = (self.row.dialect)(mode)?;
            let control = Control::new().ok_or_else(|| Failure::new("no control"))?;
            let host = self
                .launcher()
                .launch(
                    &self.agent,
                    &control,
                    dialect.as_mut(),
                    resume.map(ExternalRef::reference),
                )
                .await
                .map_err(|refused| Failure::new(&refused.to_string()))?;
            let session = SessionRef::new("one").ok_or_else(|| Failure::new("a name"))?;
            let backend: Box<dyn Backend> =
                Box::new(External::new(host, dialect, session, "work", claims()));
            Ok(backend)
        })
    }
}

/// A conversation on Claude Code's route, the peer playing its script from a
/// connect's probe and status check on, and where the peer keeps what it
/// heard.
struct Outside {
    conversation: Conversation,
    peer: PathBuf,
    route: RouteBinding,
    launched: Arc<Mutex<Vec<Launched>>>,
}

impl Outside {
    fn new(tree: &Tree, script: &Peer) -> Result<Self, Failed> {
        let peer = tree.0.join("peer");
        let program = fake_peer::install(&peer, "claude", script)?;
        let row = row()?;
        let consent = Consent::new(Routes::production());
        consent.give(row.route);
        let policy = SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false);
        let runtime = runtime()?;
        let sandbox: Arc<dyn SandboxService> =
            Arc::new(LocalSandbox::new().watching_on(runtime.clone()));
        let route = Route::External {
            provider: "anthropic",
            instance: "work".into(),
            driver: Driver::Claude,
            executable: program.to_string_lossy().into(),
            profile: None,
            enabled: true,
        };
        let registry = Registry::new(std::slice::from_ref(row));
        let agent = Agent::routed(&route, &registry, |_| None).ok_or("Claude Code's route")?;
        let launched = Arc::default();
        let launching = Launching {
            sandbox,
            runtime: runtime.clone(),
            policy,
            consent,
            agent,
            row,
            launched: Arc::clone(&launched),
        };

        // A connect: the version, then whether the profile is signed in.
        runtime.block_on(async {
            let starting = launching.launcher();
            for step in [Account::Version, Account::Status] {
                let control = Control::new().ok_or("no control")?;
                let mut dialect = (row.dialect)(Mode::Ask)?;
                starting
                    .account(&launching.agent, &control, dialect.as_mut(), step, &|_| {})
                    .await?;
            }
            Ok::<_, Failed>(())
        })?;

        let binding = RouteBinding::new("claude", "work", None)?;
        let session = Arc::new(Session::start_routed(
            &tree.sessions(),
            &tree.workspace()?,
            None,
            &binding,
        )?);
        let mut conversation = super::conversation(tree, Script::new(Vec::new()), false)?;
        runtime.block_on(async {
            let backend = launching.launch(Mode::Ask, None).await?;
            conversation
                .external(
                    session,
                    Transcript::new(),
                    backend,
                    Box::new(launching),
                    binding.clone(),
                )
                .await;
            Ok::<_, Failed>(())
        })?;

        Ok(Self {
            conversation,
            peer,
            route: binding,
            launched,
        })
    }

    /// What each launch was asked for, in order.
    fn launched(&self) -> Vec<Launched> {
        self.launched
            .lock()
            .map(|launched| launched.clone())
            .unwrap_or_default()
    }

    /// Every line the peer heard, across its runs.
    fn heard(&self) -> Vec<String> {
        fake_peer::heard(&self.peer)
    }

    fn settled(&self) -> Settled<'_> {
        self.conversation.settled()
    }
}

/// Where a turn's events go: to the test, and, for a turn to be stopped, to
/// its stop as soon as it has begun.
struct Watching {
    events: Sender<EventEnvelope>,
    stopping: Option<Cancel>,
}

impl Post for Watching {
    fn post(&self, reported: EventEnvelope) {
        if let (Some(cancel), Event::TurnStarted { .. }) = (&self.stopping, reported.event()) {
            cancel.request();
        }
        drop(self.events.send(reported));
    }
}

/// Takes one turn of `prompt`, stopped as soon as it begins where `stop`
/// says, and answers how it ended beside every event it posted.
fn turn(
    outside: &mut Outside,
    prompt: &str,
    stop: bool,
) -> Result<(Result<Turned, TurnError>, Vec<Event>), Failed> {
    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let watching = Watching {
        events,
        stopping: stop.then(|| cancel.clone()),
    };
    let turned = {
        let run = outside
            .conversation
            .starting(&watching, &cancel, &steer, &aside);
        runtime()?.block_on(
            outside
                .conversation
                .turn(prompt, Box::default(), &mut Nobody, &run),
        )
    };
    drop(watching);

    Ok((turned, posted(&reported)))
}

fn posted(reported: &Receiver<EventEnvelope>) -> Vec<Event> {
    reported.try_iter().map(EventEnvelope::into_event).collect()
}

/// `millionths` of a dollar, as a session's figures hold it.
fn dollars(millionths: u64) -> CostAmount {
    CostAmount::new(
        PricingCurrency::new("USD"),
        PricingUnit::Mixed,
        u128::from(millionths) * 1_000_000_000,
    )
}

/// The figures the last `Used` in `events` carried.
fn last_used(events: &[Event]) -> Option<Totals> {
    events.iter().rev().find_map(|event| match event {
        Event::Used { totals } => Some(*totals),
        _ => None,
    })
}

/// A connect, then `runs`.
fn connected(runs: impl IntoIterator<Item = Reply>) -> Peer {
    Peer::of(
        [claude::probe(), claude::signed_in()]
            .into_iter()
            .chain(runs),
    )
}

/// Each sequence of turns, over the agent's own stream, leaves the session's
/// cost where the native path would: a turn with no cost said makes it not
/// priced, and a stopped one makes what was summed a floor.
#[test]
fn the_seven_cost_sequences_read_over_the_agent_s_stream() -> Result<(), Failed> {
    let price = claude::PRICE;
    let cases: [(&str, Peer, &[bool], SessionCost); 7] = [
        (
            "priced",
            connected([claude::priced()]),
            &[false],
            SessionCost::Priced(dollars(price)),
        ),
        (
            "unpriced",
            connected([claude::unpriced()]),
            &[false],
            SessionCost::NotPriced,
        ),
        (
            "priced then unreported",
            claude::priced_then_unreported(),
            &[false, false],
            SessionCost::NotPriced,
        ),
        (
            "two priced",
            claude::two_priced(),
            &[false, false],
            SessionCost::Priced(dollars(2 * price)),
        ),
        (
            "priced then stopped",
            claude::priced_then_interrupt(),
            &[false, true],
            SessionCost::AtLeast(dollars(price)),
        ),
        (
            "stopped then priced",
            connected([claude::interrupt()
                .hear(1)
                .say(claude::init())
                .say(claude::said("Done."))
                .say(claude::answered(Some(price)))]),
            &[true, false],
            SessionCost::NotPriced,
        ),
        (
            "priced, stopped, priced",
            claude::priced_interrupt_priced(),
            &[false, true, false],
            SessionCost::AtLeast(dollars(2 * price)),
        ),
    ];
    for (name, script, stops, expected) in cases {
        let tree = Tree::new(&format!("external-cost-{}", name.replace([' ', ','], "-")))?;
        let mut outside = Outside::new(&tree, &script)?;
        let mut last = None;
        for (turned, stop) in stops.iter().enumerate() {
            let (ended, events) = turn(&mut outside, &format!("turn {turned}"), *stop)?;
            assert!(ended.is_ok(), "{name}: {ended:?}");
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, Event::PlanLimits { .. })),
                "{name}: an agent's turn posted a plan limit"
            );
            last = last_used(&events).or(last);
        }

        assert_eq!(outside.settled().totals().cost(), expected, "{name}");
        if let Some(last) = last {
            assert_eq!(last.cost(), expected, "{name}: the last Used");
        }
    }
    Ok(())
}

/// While a turn is under way, what is read between turns and the figures
/// last posted are the same: those of the turn before it, priced.
#[test]
fn mid_turn_the_settled_figures_and_the_last_used_agree() -> Result<(), Failed> {
    let tree = Tree::new("external-mid-turn")?;
    let mut outside = Outside::new(&tree, &claude::priced_then_held())?;
    let (first, mut seen) = turn(&mut outside, "first", false)?;
    assert!(first.is_ok(), "{first:?}");

    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let watching = Watching {
        events,
        stopping: None,
    };
    let mut nobody = Nobody;
    {
        let run = outside
            .conversation
            .starting(&watching, &cancel, &steer, &aside);
        let mut held = pin!(
            outside
                .conversation
                .turn("second", Box::default(), &mut nobody, &run,)
        );
        runtime()?.block_on(poll_fn(|cx| {
            if let Poll::Ready(ended) = held.as_mut().poll(cx) {
                return Poll::Ready(Err::<(), Failed>(format!("{ended:?}").into()));
            }
            seen.extend(reported.try_iter().map(EventEnvelope::into_event));
            let working = seen
                .iter()
                .any(|event| matches!(event, Event::Delta { text } if &**text == "Working."));
            if working {
                return Poll::Ready(Ok(()));
            }
            cx.waker().wake_by_ref();
            Poll::Pending
        }))?;
    }

    let held = outside.settled().totals();
    let posted = last_used(&seen).ok_or("no Used was posted")?;
    assert_eq!(held.cost(), SessionCost::Priced(dollars(claude::PRICE)));
    assert_eq!(posted.cost(), held.cost());
    assert_eq!(posted.input(), held.input());
    Ok(())
}

/// A turn's answer streams as a headless client would read it, the snapshot
/// after it counts one turn and its messages, and no provider is named as
/// serving the conversation.
#[test]
fn a_turn_streams_and_is_counted_as_a_native_one_is() -> Result<(), Failed> {
    let tree = Tree::new("external-snapshot")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;

    let (ended, events) = turn(&mut outside, "hi", false)?;

    assert!(matches!(ended, Ok(Turned::Ran(_))), "{ended:?}");
    assert_eq!(outside.conversation.serving(), None);
    let streamed: Vec<_> = events
        .iter()
        .filter_map(|event| client::progress(Asking::every(), event, None))
        .collect();
    assert!(
        streamed.iter().any(
            |progress| matches!(progress, Progress::Delta { text } if text.as_str() == "Done.")
        ),
        "{streamed:?}"
    );
    assert!(
        matches!(streamed.last(), Some(Progress::Finished { .. })),
        "{streamed:?}"
    );
    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.turns, 1);
    assert_eq!(standing.messages, 2);
    Ok(())
}

/// `/clear` starts a new session, the next turn launches a new run that
/// starts the agent's own session afresh, and the name the agent gives it is
/// kept in the new session's log; the session left behind reopens on its
/// route.
#[test]
fn clearing_launches_a_new_run_and_the_session_left_reopens_on_its_route() -> Result<(), Failed> {
    let tree = Tree::new("external-clear")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");
    let left = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;

    let workspace = tree.workspace()?;
    let cleared = runtime()?.block_on(outside.conversation.clear(
        &tree.sessions(),
        &workspace,
        None,
    ))?;
    let (second, _) = turn(&mut outside, "two", false)?;

    assert!(second.is_ok(), "{second:?}");
    assert_eq!(cleared.id(), Some(&left));
    assert_eq!(
        outside.launched(),
        [(Mode::Ask, None), (Mode::Ask, None)],
        "a cleared conversation resumed the agent's old session"
    );
    let log = std::fs::read_to_string(outside.conversation.session().path())?;
    assert!(log.contains(claude::SESSION), "{log}");
    drop(cleared);
    let reopened = Session::reopen_routed(&tree.sessions(), &workspace, &left, &outside.route);
    assert!(reopened.is_ok(), "{:?}", reopened.err());
    Ok(())
}

/// `/resume` on the route continues a session of it through a new run that
/// resumes the agent's own session by the name the log kept.
#[test]
fn resuming_continues_the_agent_s_own_session() -> Result<(), Failed> {
    let tree = Tree::new("external-resume")?;
    let mut outside = Outside::new(
        &tree,
        &connected([claude::clean(), claude::clean(), claude::clean()]),
    )?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");
    let left = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;
    let workspace = tree.workspace()?;
    let cleared = runtime()?.block_on(outside.conversation.clear(
        &tree.sessions(),
        &workspace,
        None,
    ))?;
    drop(cleared);
    let (second, _) = turn(&mut outside, "two", false)?;
    assert!(second.is_ok(), "{second:?}");

    let _left = runtime()?.block_on(outside.conversation.resume(
        &tree.sessions(),
        &workspace,
        &left,
    ))?;
    let (third, _) = turn(&mut outside, "three", false)?;

    assert!(third.is_ok(), "{third:?}");
    assert_eq!(
        outside.launched().last(),
        Some(&(Mode::Ask, Some(claude::SESSION.to_owned())))
    );
    let said: Vec<_> = outside
        .settled()
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(said, ["one", "three"]);
    Ok(())
}

/// A change of mode is taken at the next turn's launch, never by the run
/// under way, and full access is never one a run is launched in.
#[test]
fn a_mode_change_takes_effect_at_the_next_launch() -> Result<(), Failed> {
    let tree = Tree::new("external-mode")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");

    assert_eq!(outside.conversation.switch(Mode::FullAccess), Mode::Ask);
    assert_eq!(
        outside.conversation.switch(Mode::AllowEdits),
        Mode::AllowEdits
    );
    assert_eq!(outside.launched().len(), 1, "a mode change launched a run");
    let (second, _) = turn(&mut outside, "two", false)?;

    assert!(second.is_ok(), "{second:?}");
    assert_eq!(
        outside.launched(),
        [
            (Mode::Ask, None),
            (Mode::AllowEdits, Some(claude::SESSION.to_owned()))
        ]
    );
    Ok(())
}

/// A run lost with an effect under way answers a failure saying the work
/// may or may not have happened, and the session keeps a line saying so.
#[test]
fn a_run_lost_mid_effect_says_its_work_is_uncertain() -> Result<(), Failed> {
    let tree = Tree::new("external-lost")?;
    let mut outside = Outside::new(&tree, &connected([claude::crash()]))?;

    let (ended, events) = turn(&mut outside, "go", false)?;

    let Err(TurnError::Provider(lost)) = ended else {
        return Err(format!("{ended:?}").into());
    };
    assert!(
        lost.to_string()
            .ends_with("What the agent was doing may or may not have happened."),
        "{lost}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::Failed { .. })),
        "the turn posted its own failure"
    );
    let log = std::fs::read_to_string(outside.conversation.session().path())?;
    assert!(log.contains("uncertain"), "{log}");
    Ok(())
}

/// After moving back inside, the next turn runs on the runner the
/// conversation kept, over the transcript it was handed.
#[test]
fn moving_inside_continues_on_the_kept_runner() -> Result<(), Failed> {
    let tree = Tree::new("external-inside")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");
    let kept = outside.settled().transcript().clone();
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);

    let _left = runtime()?.block_on(outside.conversation.inside(session, kept.clone()));

    assert_eq!(outside.settled().transcript().messages(), kept.messages());
    assert_eq!(
        outside.conversation.settled().totals().cost(),
        SessionCost::Unspent
    );
    Ok(())
}

/// A client on the far side of a wire, answering each action put to it as
/// it was told to.
struct Remote {
    saying: Option<Saying>,
    put: Vec<Pending>,
}

#[derive(Debug, Clone, Copy)]
enum Saying {
    /// A ruling on the action that is pending.
    Ruling(Ruling),
    /// A yes naming some other action.
    Elsewhere,
}

impl Front for Remote {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Ruled>> {
        Box::pin(async move {
            self.put.push(pending.clone());
            let (id, ruling) = match self.saying? {
                Saying::Ruling(ruling) => (pending.id(), ruling),
                Saying::Elsewhere => (PendingId::new(pending.id().number() + 40), Ruling::Allow),
            };
            Some(Ruled::Ruled {
                id,
                ruling,
                lasting: Lasts::Once,
            })
        })
    }

    fn refused(&mut self, _refusal: crucible_client_api::Refusal) {}
}

/// An action the agent asks leave for reaches the client as one pending
/// action; an allow reaches the agent as an allow, and a deny, a yes naming
/// another action and a client that says nothing each reach it as a deny.
#[test]
fn the_agent_s_question_reaches_the_client_and_its_answer_the_agent() -> Result<(), Failed> {
    let allowed = claude::answer("q", &Decision::Allow(Lasting::Once));
    let denied = claude::answer("q", &Decision::Deny);
    for (saying, heard) in [
        (Some(Saying::Ruling(Ruling::Allow)), &allowed),
        (Some(Saying::Ruling(Ruling::Deny)), &denied),
        (Some(Saying::Elsewhere), &denied),
        (None, &denied),
    ] {
        let tree = Tree::new(&format!("external-asked-{saying:?}").replace(['(', ')'], ""))?;
        let mut outside = Outside::new(&tree, &connected([claude::permission()]))?;
        let mut remote = Remote {
            saying,
            put: Vec::new(),
        };
        let request = Request::new(
            Asking::every(),
            Correlation::new(1),
            Command::Prompt(Prompt::new("list")?),
        );
        let (events, _reported) = mpsc::channel::<EventEnvelope>();
        let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
        let ended = {
            let run = outside
                .conversation
                .starting(&events, &cancel, &steer, &aside);
            runtime()?.block_on(client::turn(
                &mut outside.conversation,
                &request,
                Box::default(),
                &mut remote,
                &run,
            ))
        };

        assert!(
            matches!(
                ended.response(&request).outcome,
                crucible_client_api::Outcome::Turn(_)
            ),
            "{saying:?}"
        );
        // A yes naming another action is not an answer, so the same action
        // is put again until the client gives up.
        let first = remote.put.first().map(Pending::id);
        assert!(first.is_some(), "{saying:?}: nothing was put");
        assert!(
            remote.put.iter().all(|put| Some(put.id()) == first),
            "{saying:?}: {:?}",
            remote.put
        );
        if !matches!(saying, Some(Saying::Elsewhere)) {
            assert_eq!(remote.put.len(), 1, "{saying:?}: {:?}", remote.put);
        }
        assert!(
            outside.heard().contains(heard),
            "{saying:?}: {:?}",
            outside.heard()
        );
    }
    Ok(())
}

/// The runner a conversation moved back inside answers the next turn.
#[test]
fn the_kept_runner_answers_after_moving_inside() -> Result<(), Failed> {
    let tree = Tree::new("external-inside-turn")?;
    let mut conversation = super::conversation(&tree, Script::new(vec![saying("hello")]), false)?;
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let _left = runtime()?.block_on(conversation.inside(session, Transcript::new()));

    let (turned, said) = super::turn(&mut conversation, "hi")?;

    assert!(
        matches!(&turned, Turned::Ran(result) if result.stop() == StopReason::Yielded),
        "{turned:?}"
    );
    assert_eq!(said, "hello");
    Ok(())
}
