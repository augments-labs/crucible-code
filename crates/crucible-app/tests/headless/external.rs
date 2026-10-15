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
use crucible_app::outside::{Installed, Relaunch};
use crucible_app::{Conversation, Settled};
use crucible_client_api::{
    Capabilities as Asking, Command, Correlation, Decision as Ruled, Lasting as Lasts, Outcome,
    Pending, PendingId, Progress, Prompt, Request, Ruling, Stop, TurnOutcome,
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

/// The route the conversation is moved onto.
fn binding() -> Result<RouteBinding, Failed> {
    Ok(RouteBinding::new("claude", "work", None)?)
}

/// Claude Code's runs launched from the peer installed in `tree` playing
/// `script`, once a connect's probe and status check have been played, and
/// where the peer keeps what it heard.
fn launching(tree: &Tree, script: &Peer) -> Result<(Launching, PathBuf), Failed> {
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
    let launching = Launching {
        sandbox,
        runtime: runtime.clone(),
        policy,
        consent,
        agent,
        row,
        launched: Arc::default(),
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

    Ok((launching, peer))
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
    /// A conversation on the route from its first turn, recording into a
    /// session started on it.
    fn new(tree: &Tree, script: &Peer) -> Result<Self, Failed> {
        let session = Arc::new(Session::start_routed(
            &tree.sessions(),
            &tree.workspace()?,
            None,
            &binding()?,
        )?);
        let conversation = super::conversation(tree, Script::new(Vec::new()), false)?;
        let (outside, _left) = Self::onto(tree, script, conversation, session, Transcript::new())?;
        Ok(outside)
    }

    /// `conversation` moved onto the route, recording into `session` with
    /// `transcript` what was seen of it, and the session it left.
    fn onto(
        tree: &Tree,
        script: &Peer,
        mut conversation: Conversation,
        session: Arc<Session>,
        transcript: Transcript,
    ) -> Result<(Self, Arc<Session>), Failed> {
        let (launching, peer) = launching(tree, script)?;
        let launched = Arc::clone(&launching.launched);
        let route = binding()?;
        let left = runtime()?.block_on(async {
            let backend = launching.launch(Mode::Ask, None).await?;
            let installed = Installed {
                backend: Some(backend),
                relaunch: Box::new(launching),
                route: route.clone(),
            };
            Ok::<_, Failed>(
                conversation
                    .external(session, transcript, installed)
                    .await?,
            )
        })?;

        Ok((
            Self {
                conversation,
                peer,
                route,
                launched,
            },
            left,
        ))
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
/// serving the conversation, nor is a model missing from it.
#[test]
fn a_turn_streams_and_is_counted_as_a_native_one_is() -> Result<(), Failed> {
    let tree = Tree::new("external-snapshot")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;

    let (ended, events) = turn(&mut outside, "hi", false)?;

    assert!(matches!(ended, Ok(Turned::Ran(_))), "{ended:?}");
    assert_eq!(outside.conversation.serving(), None);
    assert!(
        outside.conversation.missing().is_none(),
        "the turn was refused as having no model"
    );
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
        matches!(streamed.first(), Some(Progress::Started { .. })),
        "{streamed:?}"
    );
    assert!(
        streamed
            .iter()
            .any(|progress| matches!(progress, Progress::Used(_))),
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

/// On the route, a snapshot names the instance its turns run on and says the
/// agent's own account is what pays; whether any agent is offered is read
/// from the registry the conversation was built over.
#[test]
fn a_snapshot_names_the_route_s_instance_and_its_billing() -> Result<(), Failed> {
    use crucible_client_api::{Billing, Execution, Name};

    let tree = Tree::new("external-standing")?;
    let outside = Outside::new(&tree, &connected([]))?;

    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.execution, Execution::Instance(Name::new("work")?));
    assert_eq!(standing.billing, Billing::External);
    assert_eq!(standing.provider, None);
    Ok(())
}

/// Whether an agent is offered is whether any row of the registry the
/// conversation was built over ships.
#[cfg(feature = "proof")]
#[test]
fn a_snapshot_offers_agents_only_where_its_registry_ships_one() -> Result<(), Failed> {
    use crucible_app::proof;
    use crucible_client_api::Agents;

    let tree = Tree::new("external-agents")?;
    let conversation = super::conversation(&tree, Script::new(Vec::new()), false)?;
    let conversation = proof::agents(conversation, proof::registry(&proof::FAKE_ROWS));
    assert_eq!(client::snapshot(&conversation).agents, Agents::Available);

    let conversation = proof::agents(conversation, proof::registry(&[]));
    assert_eq!(client::snapshot(&conversation).agents, Agents::None);
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
    assert_eq!(outside.conversation.session().finish(), None);
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
    assert_eq!(outside.conversation.session().finish(), None);
    let log = std::fs::read_to_string(outside.conversation.session().path())?;
    assert!(log.contains("uncertain"), "{log}");
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

/// Back inside, the next turn runs on the runner the conversation was
/// assembled with, which still keeps its prompt caches where it was told to,
/// and the request it sends carries the transcript it was handed.
#[test]
fn moving_inside_continues_on_the_runner_as_it_was_assembled() -> Result<(), Failed> {
    let tree = Tree::new("external-inside")?;
    let script = Script::new(vec![saying("back")]);
    let sent = Arc::clone(&script.sent);
    let home = tree.home()?;
    let native = super::conversation(&tree, script, false)?.remembering_caches_in(home.path());
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);
    let (mut outside, _left) = Outside::onto(
        &tree,
        &connected([claude::clean()]),
        native,
        session,
        Transcript::new(),
    )?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");
    let kept = outside.settled().transcript().clone();
    let inside = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);

    let _left = runtime()?.block_on(outside.conversation.inside(inside, kept.clone()));

    assert_eq!(outside.settled().transcript().messages(), kept.messages());
    assert_eq!(outside.settled().totals().cost(), SessionCost::Unspent);
    // Records nobody can read: a runner that still keeps its caches there
    // reads them and says so, where one that forgot reads nothing.
    let caches = home.path().join("prompt-cache");
    std::fs::create_dir_all(&caches)?;
    std::fs::write(caches.join("resources-v1.json"), "not records")?;
    let listed = runtime()?.block_on(outside.conversation.prompt_cache_resources());
    assert!(listed.is_err(), "the runner forgot its caches: {listed:?}");
    let (turned, said) = super::turn(&mut outside.conversation, "two")?;
    assert!(
        matches!(&turned, Turned::Ran(result) if result.stop() == StopReason::Yielded),
        "{turned:?}"
    );
    assert_eq!(said, "back");
    let sent = sent.lock().map_err(|_| "the record")?.clone();
    assert_eq!(sent, [vec!["one".to_owned(), "two".to_owned()]]);
    Ok(())
}

/// A session picked back up shows what it held once the conversation is
/// moved onto its route.
#[test]
fn moving_outside_shows_the_transcript_reopened() -> Result<(), Failed> {
    let tree = Tree::new("external-reopened")?;
    let workspace = tree.workspace()?;
    let started = Session::start_routed(&tree.sessions(), &workspace, None, &binding()?)?;
    let id = started.id().cloned().ok_or("an id")?;
    started.append(&Message::User {
        text: "before".into(),
        attachments: Box::new([]),
    });
    drop(started);
    let (reopened, transcript) =
        Session::reopen_routed(&tree.sessions(), &workspace, &id, &binding()?)?;
    let native = super::conversation(&tree, Script::new(Vec::new()), false)?;

    let (outside, _left) = Outside::onto(
        &tree,
        &connected([]),
        native,
        Arc::new(reopened),
        transcript,
    )?;

    let shown: Vec<_> = outside
        .settled()
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(shown, ["before"]);
    Ok(())
}

/// The session a conversation left when it moved outside is let go whole:
/// it reopens in the same run, nobody holding it.
#[test]
fn the_session_left_for_the_route_reopens_at_once() -> Result<(), Failed> {
    let tree = Tree::new("external-left")?;
    let native = super::conversation(&tree, Script::new(Vec::new()), false)?;
    let id = native.session().id().cloned().ok_or("an id")?;
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);

    let (_outside, left) =
        Outside::onto(&tree, &connected([]), native, session, Transcript::new())?;
    drop(left);

    let reopened = Session::reopen(&tree.sessions(), &tree.workspace()?, &id);
    assert!(reopened.is_ok(), "{:?}", reopened.err());
    Ok(())
}

/// A conversation in a mode the agent has no launch for is not moved: it
/// stays on its own runner, in the mode it was in.
#[test]
fn a_conversation_in_full_access_is_not_moved_onto_the_route() -> Result<(), Failed> {
    let tree = Tree::new("external-full-access")?;
    let mut native = super::conversation(&tree, Script::new(Vec::new()), false)?;
    assert_eq!(native.switch(Mode::FullAccess), Mode::FullAccess);
    let (launching, _peer) = launching(&tree, &connected([claude::clean()]))?;
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);

    let moved = runtime()?.block_on(async {
        let backend = launching.launch(Mode::Ask, None).await?;
        let installed = Installed {
            backend: Some(backend),
            relaunch: Box::new(launching),
            route: binding()?,
        };
        Ok::<_, Failed>(native.external(session, Transcript::new(), installed).await)
    })?;

    assert!(moved.is_err(), "{moved:?}");
    assert_eq!(native.settled().mode(), Mode::FullAccess);
    assert!(
        native.settled().model() == "script",
        "{}",
        native.settled().model()
    );
    Ok(())
}

/// A session whose run was lost with an edit under way keeps a line saying
/// so, and the session picked back up on the route still has it.
#[test]
fn resuming_keeps_a_turn_whose_work_is_uncertain() -> Result<(), Failed> {
    let tree = Tree::new("external-uncertain")?;
    let mut outside = Outside::new(
        &tree,
        &connected([claude::crash(), claude::clean(), claude::clean()]),
    )?;
    let (ended, _) = turn(&mut outside, "go", false)?;
    assert!(ended.is_err(), "{ended:?}");
    let left = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;
    assert_eq!(outside.conversation.session().finish(), None);
    let log = std::fs::read_to_string(outside.conversation.session().path())?;
    assert!(log.contains("uncertain"), "{log}");
    let workspace = tree.workspace()?;
    let cleared = runtime()?.block_on(outside.conversation.clear(
        &tree.sessions(),
        &workspace,
        None,
    ))?;
    drop(cleared);

    let _cleared = runtime()?.block_on(outside.conversation.resume(
        &tree.sessions(),
        &workspace,
        &left,
    ))?;

    assert!(
        outside.conversation.session().uncertain_turn().is_some(),
        "the resumed session lost the uncertain turn"
    );
    Ok(())
}

/// A headless client is told of the agent's own tool use as activity, never
/// as a call it could be asked about, and of a turn whose agent did not heed
/// the stop with that work under way as uncertain rather than cancelled.
#[test]
fn the_agent_s_tool_use_is_activity_and_a_stop_under_it_is_uncertain() -> Result<(), Failed> {
    let tree = Tree::new("external-activity")?;
    let unheeding = Reply::new()
        .hear(1)
        .say(claude::init())
        .say(claude::effect())
        .hear_out();
    let mut outside = Outside::new(&tree, &connected([unheeding]))?;
    let request = Request::new(
        Asking::every(),
        Correlation::new(1),
        Command::Prompt(Prompt::new("go")?),
    );
    let mut remote = Remote {
        saying: None,
        put: Vec::new(),
    };
    let (events, reported) = mpsc::channel::<EventEnvelope>();
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let watching = Watching {
        events,
        stopping: Some(cancel.clone()),
    };
    let ended = {
        let run = outside
            .conversation
            .starting(&watching, &cancel, &steer, &aside);
        runtime()?.block_on(client::turn(
            &mut outside.conversation,
            &request,
            Box::default(),
            &mut remote,
            &run,
        ))
    };
    drop(watching);

    let streamed: Vec<_> = posted(&reported)
        .iter()
        .filter_map(|event| client::progress(Asking::every(), event, None))
        .collect();
    assert!(
        streamed.iter().any(|progress| matches!(
            progress,
            Progress::ToolActivity { tool, .. } if tool.as_str() == "Edit"
        )),
        "{streamed:?}"
    );
    assert!(
        !streamed
            .iter()
            .any(|progress| matches!(progress, Progress::ToolRequested { .. })),
        "{streamed:?}"
    );
    assert_eq!(
        ended.response(&request).outcome,
        Outcome::Turn(TurnOutcome::Ran {
            stop: Stop::Uncertain
        })
    );
    Ok(())
}

/// A client that answers an account question by the choice named, or goes
/// back where none is.
struct Choosing {
    choice: Option<&'static str>,
    put: Vec<Pending>,
}

impl Front for Choosing {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Ruled>> {
        Box::pin(async move {
            self.put.push(pending.clone());
            Some(match self.choice {
                Some(name) => Ruled::Picked {
                    id: pending.id(),
                    choice: crucible_client_api::Said::new(name).ok()?,
                },
                None => Ruled::Declined { id: pending.id() },
            })
        })
    }

    fn refused(&mut self, _refusal: crucible_client_api::Refusal) {}
}

/// The user file a disconnect is written to, naming the route's instance.
const INSTALLED: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}},
  "updates": {"check": "auto"}
}
"#;

/// Asks to disconnect the route's instance, claiming `asking`, of a client
/// that answers as `choosing` does, and answers what it was told and what
/// the user file then held.
fn disconnecting(
    tree: &Tree,
    outside: &mut Outside,
    asking: Asking,
    choosing: &mut Choosing,
) -> Result<(Outcome, String), Failed> {
    disconnecting_from(tree, outside, (INSTALLED, "work"), asking, choosing)
}

/// The same, the user file holding `installed.0` and the instance asked about
/// named `installed.1`.
fn disconnecting_from(
    tree: &Tree,
    outside: &mut Outside,
    installed: (&str, &str),
    asking: Asking,
    choosing: &mut Choosing,
) -> Result<(Outcome, String), Failed> {
    let asked = Asked {
        installed,
        command: |instance| Command::Disconnect { instance },
        asking,
        after: None,
    };
    accounting(tree, outside, &asked, choosing)
}

/// An account operation, asked of a user file holding `installed.0` about the
/// instance named `installed.1`.
struct Asked<'a> {
    installed: (&'a str, &'a str),
    command: fn(crucible_client_api::Name) -> Command,
    asking: Asking,
    /// What the user file holds once the settings were read from it, where
    /// that is something else: as though another file named the instance.
    after: Option<&'a str>,
}

/// Asks `asked` of a client that answers as `choosing` does, and answers what
/// it was told and what the user file then held.
fn accounting(
    tree: &Tree,
    outside: &mut Outside,
    asked: &Asked<'_>,
    choosing: &mut Choosing,
) -> Result<(Outcome, String), Failed> {
    use crucible_client_api::Name;

    let (installed, asking) = (asked.installed, asked.asking);
    let mut desk = super::Desk::new(tree, &[])?;
    if let Some(directory) = desk.choosing.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(&desk.choosing, installed.0)?;
    desk.settings = super::written(tree)?;
    if let Some(after) = asked.after {
        std::fs::write(&desk.choosing, after)?;
    }
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let at = client::Desk {
        switching: desk.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads: |_| false,
        environment: super::unset,
        notes: |_| Err(crucible_client_api::ErrorCode::InvalidArgument.into()),
    };
    let request = Request::new(
        asking,
        Correlation::new(1),
        (asked.command)(Name::new(installed.1)?),
    );
    let performed = runtime()?.block_on(client::account(
        &mut outside.conversation,
        &request,
        &at,
        choosing,
    ));
    let outcome = performed.response(&request).outcome;
    Ok((outcome, std::fs::read_to_string(&desk.choosing)?))
}

/// A disconnect, once chosen, writes `enabled: false` into the instance and
/// changes nothing else in the file, and the conversation's next turn on it
/// is refused without launching the agent again.
#[test]
fn a_disconnect_writes_only_that_the_instance_is_off_and_no_turn_launches_it() -> Result<(), Failed>
{
    use crucible_client_api::LogoutOutcome;

    let tree = Tree::new("external-disconnect")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let mut choosing = Choosing {
        choice: Some(client::accounts::DISCONNECT),
        put: Vec::new(),
    };

    let (outcome, file) = disconnecting(&tree, &mut outside, Asking::every(), &mut choosing)?;

    assert!(
        matches!(outcome, Outcome::Logout(LogoutOutcome::Disconnected { .. })),
        "{outcome:?}"
    );
    assert_eq!(choosing.put.len(), 1, "{:?}", choosing.put);
    assert_eq!(
        file,
        INSTALLED.replace(
            r#""executable": "claude"}"#,
            r#""executable": "claude", "enabled": false}"#
        )
    );
    let launched = outside.launched().len();
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_err(), "a disconnected agent took a turn");
    assert_eq!(
        outside.launched().len(),
        launched,
        "the agent was launched again"
    );
    Ok(())
}

/// Cancelling, going back, and a client that never said it answers account
/// questions each leave the file as it was and the agent connected.
#[test]
fn a_disconnect_not_chosen_changes_nothing() -> Result<(), Failed> {
    use crucible_client_api::{Capability as Claimed, LogoutOutcome};

    let lacking = Asking::none().with(Claimed::Permissions);
    for (case, asking, choice) in [
        ("cancel", Asking::every(), Some("Cancel")),
        ("back", Asking::every(), None),
        ("unclaimed", lacking, Some(client::accounts::DISCONNECT)),
    ] {
        let tree = Tree::new(&format!("external-kept-{case}"))?;
        let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
        let mut choosing = Choosing {
            choice,
            put: Vec::new(),
        };

        let (outcome, file) = disconnecting(&tree, &mut outside, asking, &mut choosing)?;

        assert_eq!(outcome, Outcome::Logout(LogoutOutcome::Undecided), "{case}");
        assert_eq!(file, INSTALLED, "{case}");
        assert_eq!(choosing.put.is_empty(), case == "unclaimed", "{case}");
        let (turned, _events) = turn(&mut outside, "again", false)?;
        assert!(turned.is_ok(), "{case}: {turned:?}");
    }
    Ok(())
}

/// An instance the file does not name, or names as already disabled, is
/// refused before anything is asked, and the file is left as it was.
#[test]
fn a_disconnect_of_an_instance_not_in_use_is_refused_unasked() -> Result<(), Failed> {
    use crucible_client_api::ErrorCode;

    let disabled = INSTALLED.replace(
        r#""executable": "claude"}"#,
        r#""executable": "claude", "enabled": false}"#,
    );
    for (case, file, instance) in [
        ("unknown", INSTALLED.to_owned(), "elsewhere"),
        ("disabled", disabled, "work"),
    ] {
        let tree = Tree::new(&format!("external-refused-{case}"))?;
        let mut outside = Outside::new(&tree, &connected([]))?;
        let mut choosing = Choosing {
            choice: Some(client::accounts::DISCONNECT),
            put: Vec::new(),
        };

        let (outcome, kept) = disconnecting_from(
            &tree,
            &mut outside,
            (&file, instance),
            Asking::every(),
            &mut choosing,
        )?;

        assert_eq!(
            outcome,
            Outcome::Refused(ErrorCode::InvalidArgument.into()),
            "{case}"
        );
        assert!(choosing.put.is_empty(), "{case}: {:?}", choosing.put);
        assert_eq!(kept, file, "{case}");
    }
    Ok(())
}

/// Asks to remove the instance `instance` of the user file `installed`, of a
/// client that answers as `choosing` does.
fn removing(
    tree: &Tree,
    outside: &mut Outside,
    installed: (&str, &str),
    choosing: &mut Choosing,
) -> Result<(Outcome, String), Failed> {
    let asked = Asked {
        installed,
        command: |instance| Command::RemoveIntegration { instance },
        asking: Asking::every(),
        after: None,
    };
    accounting(tree, outside, &asked, choosing)
}

/// A removal, once chosen, takes the instance's block out of the file and
/// nothing else, and the conversation's next turn on it is refused without
/// launching the agent again.
#[test]
fn a_removal_deletes_the_block_and_nothing_else() -> Result<(), Failed> {
    use crucible_client_api::LogoutOutcome;

    let tree = Tree::new("external-remove")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let mut choosing = Choosing {
        choice: Some(client::accounts::REMOVE),
        put: Vec::new(),
    };

    let (outcome, file) = removing(&tree, &mut outside, (INSTALLED, "work"), &mut choosing)?;

    assert_eq!(
        outcome,
        Outcome::Logout(LogoutOutcome::IntegrationRemoved {
            said: crucible_client_api::Text::cut(client::accounts::REMOVED)
        })
    );
    assert_eq!(choosing.put.len(), 1, "{:?}", choosing.put);
    assert_eq!(
        file,
        INSTALLED.replace(
            r#""externalAgents": {"work": {"driver": "claude", "executable": "claude"}},"#,
            r#""externalAgents": {},"#
        )
    );
    let launched = outside.launched().len();
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_err(), "a removed agent took a turn");
    assert_eq!(
        outside.launched().len(),
        launched,
        "the agent was launched again"
    );
    Ok(())
}

/// While a provider is reached through the instance, the removal is refused
/// before anything is asked, and the file is left as it was.
#[test]
fn a_removal_is_refused_while_a_provider_is_reached_through_the_instance() -> Result<(), Failed> {
    use crucible_client_api::LogoutOutcome;

    let referenced = INSTALLED.replace(
        r#""updates""#,
        r#""providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "work"}}},
  "updates""#,
    );
    let tree = Tree::new("external-remove-referenced")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let mut choosing = Choosing {
        choice: Some(client::accounts::REMOVE),
        put: Vec::new(),
    };

    let (outcome, file) = removing(&tree, &mut outside, (&referenced, "work"), &mut choosing)?;

    assert!(
        matches!(
            outcome,
            Outcome::Logout(LogoutOutcome::StillReferenced { .. })
        ),
        "{outcome:?}"
    );
    assert!(choosing.put.is_empty(), "{:?}", choosing.put);
    assert_eq!(file, referenced);
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_ok(), "{turned:?}");
    Ok(())
}

/// Cancelling or going back leaves the file byte for byte as it was, and the
/// agent connected; an instance the file does not name is refused unasked.
#[test]
fn a_removal_not_chosen_changes_nothing() -> Result<(), Failed> {
    use crucible_client_api::{ErrorCode, LogoutOutcome};

    for (case, choice, instance, expected) in [
        (
            "cancel",
            Some("Cancel"),
            "work",
            Outcome::Logout(LogoutOutcome::Undecided),
        ),
        (
            "back",
            None,
            "work",
            Outcome::Logout(LogoutOutcome::Undecided),
        ),
        (
            "unknown",
            Some(client::accounts::REMOVE),
            "elsewhere",
            Outcome::Refused(ErrorCode::InvalidArgument.into()),
        ),
    ] {
        let tree = Tree::new(&format!("external-remove-kept-{case}"))?;
        let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
        let mut choosing = Choosing {
            choice,
            put: Vec::new(),
        };

        let (outcome, file) = removing(&tree, &mut outside, (INSTALLED, instance), &mut choosing)?;

        assert_eq!(outcome, expected, "{case}");
        assert_eq!(file, INSTALLED, "{case}");
        assert_eq!(choosing.put.is_empty(), case == "unknown", "{case}");
        let (turned, _events) = turn(&mut outside, "again", false)?;
        assert!(turned.is_ok(), "{case}: {turned:?}");
    }
    Ok(())
}

/// An instance the settings name from some file other than the user file is
/// refused before anything is asked or stopped, since only the user file is
/// written.
#[test]
fn a_removal_of_an_instance_the_user_file_does_not_hold_is_refused_unasked() -> Result<(), Failed> {
    use crucible_client_api::ErrorCode;

    let elsewhere = r#"{"updates": {"check": "auto"}}
"#;
    let tree = Tree::new("external-remove-elsewhere")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let mut choosing = Choosing {
        choice: Some(client::accounts::REMOVE),
        put: Vec::new(),
    };
    let asked = Asked {
        installed: (INSTALLED, "work"),
        command: |instance| Command::RemoveIntegration { instance },
        asking: Asking::every(),
        after: Some(elsewhere),
    };

    let (outcome, file) = accounting(&tree, &mut outside, &asked, &mut choosing)?;

    assert_eq!(outcome, Outcome::Refused(ErrorCode::InvalidArgument.into()));
    assert!(choosing.put.is_empty(), "{:?}", choosing.put);
    assert_eq!(file, elsewhere);
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_ok(), "{turned:?}");
    Ok(())
}
