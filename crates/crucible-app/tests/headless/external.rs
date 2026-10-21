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
use crucible_app::content_use::{Consent, LAUNCHING, Routes};
use crucible_app::external::External;
use crucible_app::launching::{Agent, Launcher, Steps};
use crucible_app::outside::{Installed, Relaunch};
use crucible_app::{Conversation, Settled};
use crucible_client_api::{
    Capabilities as Asking, Command, Correlation, Decision as Ruled, Lasting as Lasts, Outcome,
    Pending, PendingId, Progress, Prompt, Request, ResumeOutcome, Ruling, Stop, SummaryKind,
    TurnOutcome,
};
use crucible_config::Driver;
use crucible_execution::{
    Backend, Capabilities, Capability, Decision, Failure, Lasting, SessionRef,
};
use crucible_external_harnesses::fake_peer::{self, Reply, Script as Peer, scripts::claude};
use crucible_external_harnesses::{Account, Control};
use crucible_models::Effort;
use crucible_runner::{Event, EventEnvelope, Post, SessionCost, Totals, TurnError, Turned};
use crucible_runtime::{Aside, BoxFuture, Cancel, Steer};
use crucible_sandbox::{SandboxEnablement, SandboxPolicy, SandboxService};
use crucible_sandbox_local::LocalSandbox;
use crucible_session::{ExternalRef, RouteBinding, Session};
use crucible_tools::Mode;
use crucible_types::{
    CostAmount, Message, PricingCurrency, PricingUnit, SessionId, StopReason, Transcript,
};

use super::{Failed, Nobody, Script, Tree, saying};

#[path = "external/routing.rs"]
mod routing;

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

    fn model(&self) -> Option<&str> {
        self.agent.model()
    }

    fn effort(&self) -> Option<Effort> {
        self.agent.effort()
    }

    fn launch<'a>(
        &'a self,
        mode: Mode,
        model: Option<&'a str>,
        effort: Option<Effort>,
        resume: Option<&'a ExternalRef>,
    ) -> BoxFuture<'a, Result<Box<dyn Backend>, Failure>> {
        Box::pin(async move {
            if let Ok(mut launched) = self.launched.lock() {
                launched.push((mode, resume.map(|named| named.reference().to_owned())));
            }
            let mut dialect = (self.row.dialect)(mode)?;
            let control = Control::new().ok_or_else(|| Failure::new("no control"))?;
            let agent = self.agent.on(model, effort);
            let host = self
                .launcher()
                .launch(
                    &agent,
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
    launching_on(tree, script, None)
}

/// [`launching`], with each run started on `model` where one is named.
fn launching_on(
    tree: &Tree,
    script: &Peer,
    model: Option<&str>,
) -> Result<(Launching, PathBuf), Failed> {
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
        model: model.map(Into::into),
        effort: None,
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

/// What takes Claude Code's account steps, and the runs of a session it ran
/// picked up from elsewhere, as `launching` starts them: any configured
/// instance finds the peer installed in `peer` on the `PATH`, as a bare
/// `claude` would be found.
fn steps(launching: &Launching, peer: &std::path::Path) -> Steps {
    Steps::new(
        launching.launcher(),
        Arc::new(SandboxEnablement::new(false, false)),
        Box::leak(Box::new(Registry::new(std::slice::from_ref(launching.row)))),
        Some(peer.to_path_buf().into_os_string()),
    )
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
        Self::on(tree, script, None)
    }

    /// [`Outside::new`], with each run started on `model` where one is named.
    fn on(tree: &Tree, script: &Peer, model: Option<&str>) -> Result<Self, Failed> {
        let session = Arc::new(Session::start_routed(
            &tree.sessions(),
            &tree.workspace()?,
            None,
            &binding()?,
        )?);
        let conversation = super::conversation(tree, Script::new(Vec::new()), false)?;
        let (outside, _left) = Self::onto_on(
            tree,
            (script, model),
            conversation,
            session,
            Transcript::new(),
        )?;
        Ok(outside)
    }

    /// `conversation` moved onto the route, recording into `session` with
    /// `transcript` what was seen of it, and the session it left.
    fn onto(
        tree: &Tree,
        script: &Peer,
        conversation: Conversation,
        session: Arc<Session>,
        transcript: Transcript,
    ) -> Result<(Self, Arc<Session>), Failed> {
        Self::onto_on(tree, (script, None), conversation, session, transcript)
    }

    /// [`Outside::onto`], with the agent playing `script` and each run
    /// started on `model` where one is named.
    fn onto_on(
        tree: &Tree,
        (script, model): (&Peer, Option<&str>),
        mut conversation: Conversation,
        session: Arc<Session>,
        transcript: Transcript,
    ) -> Result<(Self, Arc<Session>), Failed> {
        let (launching, peer) = launching_on(tree, script, model)?;
        // The conversation holds its sends to the yeses the runs are launched
        // under, as one started by the application does.
        conversation = conversation.consenting(launching.consent.clone());
        conversation = conversation.stepping(steps(&launching, &peer));
        let launched = Arc::clone(&launching.launched);
        let route = binding()?;
        let left = runtime()?.block_on(async {
            let backend = launching
                .launch(
                    Mode::Ask,
                    launching.agent.model(),
                    launching.agent.effort(),
                    None,
                )
                .await?;
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

    /// Takes back the yes the agent's route was given, as though it had
    /// never been answered.
    fn unanswer(&self) {
        if let Some(consent) = self.conversation.consent() {
            consent.withdraw("agent:claude");
        }
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
    assert_eq!(
        standing.model.as_ref().map(|model| model.text().as_str()),
        Some("claude-opus-5-5"),
        "an agent's route names the model it is started on"
    );
    Ok(())
}

/// On the route, the model the agent's entry names is the snapshot's model,
/// and the one the agent's program is started with.
#[test]
fn a_snapshot_on_the_route_names_the_model_the_agent_is_started_with() -> Result<(), Failed> {
    let tree = Tree::new("external-model")?;
    let mut outside = Outside::on(&tree, &connected([claude::clean()]), Some("opus"))?;

    let standing = client::snapshot(&outside.conversation);
    let (turned, _) = super::turn(&mut outside.conversation, "hello")?;

    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert_eq!(
        standing.model.as_ref().map(|model| model.text().as_str()),
        Some("opus")
    );
    let started = fake_peer::argued(&outside.peer);
    let run = started.last().ok_or("the run was never started")?;
    assert!(
        run.windows(2).any(|pair| pair == ["--model", "opus"]),
        "{started:?}"
    );
    Ok(())
}

/// What `command` is answered with on the route.
fn answered(tree: &Tree, outside: &mut Outside, command: Command) -> Result<Outcome, Failed> {
    answered_by(&super::Desk::new(tree, &[])?, tree, outside, command)
}

/// [`answered`], by `desk`.
fn answered_by(
    desk: &super::Desk,
    tree: &Tree,
    outside: &mut Outside,
    command: Command,
) -> Result<Outcome, Failed> {
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let at = client::Desk {
        switching: desk.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads: |_| false,
        environment: super::unset,
        notes: |_| Err(crucible_client_api::ErrorCode::InvalidArgument.into()),
    };
    let request = Request::new(Asking::every(), Correlation::new(1), command);
    let performed = runtime()?.block_on(client::perform(&mut outside.conversation, &request, &at));
    Ok(performed.response(&request).outcome)
}

/// The agent's entry written into the user's file of `tree`, as a connect
/// leaves it, and where that file is.
fn entered(tree: &Tree) -> Result<PathBuf, Failed> {
    let file = crucible_config::user(&tree.home()?);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(
        &file,
        "{\"externalAgents\": {\"work\": {\"driver\": \"claude\", \"executable\": \"claude\"}}}\n",
    )?;
    Ok(file)
}

/// Whether the last run the peer in `outside` was started as carries `flag`
/// followed by `value`.
fn started_with(outside: &Outside, flag: &str, value: &str) -> Result<bool, Failed> {
    let started = fake_peer::argued(&outside.peer);
    let run = started.last().ok_or("the run was never started")?;
    Ok(run.windows(2).any(|pair| pair == [flag, value]))
}

/// On a route whose entry names no model, the agent is started on its
/// provider's default model at that model's default rung, and the snapshot
/// names them.
#[test]
fn a_route_with_no_model_chosen_starts_on_the_provider_s_default_and_its_rung() -> Result<(), Failed>
{
    use crucible_client_api::Rung;

    let tree = Tree::new("external-model-default")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;

    let standing = client::snapshot(&outside.conversation);
    let (turned, _) = super::turn(&mut outside.conversation, "hello")?;

    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert_eq!(
        standing.model.as_ref().map(|model| model.text().as_str()),
        Some("claude-opus-5-5")
    );
    assert_eq!(standing.effort, Some(Rung::Medium));
    assert!(started_with(&outside, "--model", "claude-opus-5-5")?);
    assert!(started_with(&outside, "--effort", "medium")?);
    Ok(())
}

/// On the route, a model off the agent's provider's list is taken with its
/// rung for the agent's next run, which continues the agent's own session,
/// and both are written into its entry so the next start is on them too.
#[test]
fn a_model_off_the_provider_s_list_starts_the_agent_s_next_run_at_its_rung() -> Result<(), Failed> {
    use crucible_client_api::{ModelOutcome, Name, Rung};

    let tree = Tree::new("external-model-listed")?;
    let file = entered(&tree)?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");

    let chosen = answered(
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-sonnet-5-5")?,
            effort: Some(Rung::High),
        },
    )?;
    assert!(
        matches!(
            chosen,
            Outcome::Model(ModelOutcome::Taken {
                unwritten: None,
                ..
            })
        ),
        "{chosen:?}"
    );
    assert_eq!(outside.launched().len(), 1, "a choice launched a run");
    let standing = client::snapshot(&outside.conversation);
    assert_eq!(
        standing.model.as_ref().map(|model| model.text().as_str()),
        Some("claude-sonnet-5-5")
    );
    assert_eq!(standing.effort, Some(Rung::High));
    let (second, _) = turn(&mut outside, "two", false)?;

    assert!(second.is_ok(), "{second:?}");
    assert_eq!(
        outside.launched().last(),
        Some(&(Mode::Ask, Some(claude::SESSION.to_owned())))
    );
    assert!(started_with(&outside, "--model", "claude-sonnet-5-5")?);
    assert!(started_with(&outside, "--effort", "high")?);
    let written = std::fs::read_to_string(&file)?;
    assert!(
        written.contains("\"model\": \"claude-sonnet-5-5\""),
        "{written}"
    );
    assert!(written.contains("\"effort\": \"high\""), "{written}");
    Ok(())
}

/// On the route, `/effort` takes a rung the model in force serves for the
/// agent's next run, and writes it into the agent's entry.
#[test]
fn an_effort_on_the_route_starts_the_agent_s_next_run_at_it() -> Result<(), Failed> {
    use crucible_client_api::{EffortOutcome, Rung};

    let tree = Tree::new("external-effort")?;
    let file = entered(&tree)?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");

    let taken = answered(&tree, &mut outside, Command::SetEffort(Rung::Max))?;
    assert_eq!(
        taken,
        Outcome::Effort(EffortOutcome::Taken { unwritten: None })
    );
    let (second, _) = turn(&mut outside, "two", false)?;

    assert!(second.is_ok(), "{second:?}");
    assert!(started_with(&outside, "--effort", "max")?);
    assert!(std::fs::read_to_string(&file)?.contains("\"effort\": \"max\""));
    Ok(())
}

/// On the route, a rung the model in force does not serve is refused, with a
/// model or alone, since the agent would refuse every run started at it; and
/// nothing is taken or written.
#[test]
fn a_rung_the_route_s_model_does_not_serve_is_refused() -> Result<(), Failed> {
    use crucible_client_api::{EffortOutcome, ModelOutcome, Name, Rung};

    let tree = Tree::new("external-effort-unserved")?;
    let file = entered(&tree)?;
    let before = std::fs::read_to_string(&file)?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;

    // A model with no rung at all.
    let refused = answered(
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-haiku-4-5")?,
            effort: Some(Rung::Max),
        },
    )?;
    assert!(
        matches!(refused, Outcome::Model(ModelOutcome::Unsupported)),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&file)?, before);

    let taken = answered(
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-haiku-4-5")?,
            effort: None,
        },
    )?;
    assert!(
        matches!(taken, Outcome::Model(ModelOutcome::Taken { .. })),
        "{taken:?}"
    );
    let written = std::fs::read_to_string(&file)?;
    let refused = answered(&tree, &mut outside, Command::SetEffort(Rung::Max))?;
    assert_eq!(refused, Outcome::Effort(EffortOutcome::Unsupported));
    assert_eq!(std::fs::read_to_string(&file)?, written);
    Ok(())
}

/// On the route, a model named as the agent's program could not be started
/// on, here as an option, is refused, and nothing is taken or written: the
/// agent's next run is started on the model in force.
#[test]
fn a_model_the_route_s_agent_cannot_be_started_on_is_refused() -> Result<(), Failed> {
    use crucible_client_api::{ModelOutcome, Name};

    let tree = Tree::new("external-model-unnamed")?;
    let file = entered(&tree)?;
    let before = std::fs::read_to_string(&file)?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;

    let refused = answered(
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("-p")?,
            effort: None,
        },
    )?;

    let Outcome::Model(ModelOutcome::Unreachable(problem)) = &refused else {
        return Err(format!("{refused:?}").into());
    };
    assert_eq!(
        problem.message.as_str(),
        "the agent work cannot be started on \"-p\": a model is named with letters, digits and \
         . _ - : @ / [ ] only, at most 128 of them, and never starting with -, since it is \
         handed to the agent's program as an argument"
    );
    assert_eq!(std::fs::read_to_string(&file)?, before);
    let (turned, _) = turn(&mut outside, "after", false)?;
    assert!(turned.is_ok(), "{turned:?}");
    assert!(started_with(&outside, "--model", "claude-opus-5-5")?);
    Ok(())
}

/// On the route, a model of a provider the agent does not reach, with nothing
/// held to reach that provider with, is refused, and nothing is taken or
/// written.
#[test]
fn a_model_of_another_provider_is_refused_on_the_route() -> Result<(), Failed> {
    use crucible_client_api::{ModelOutcome, Name};

    let tree = Tree::new("external-model-foreign")?;
    let file = entered(&tree)?;
    let before = std::fs::read_to_string(&file)?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;

    let refused = answered(
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new("openai")?,
            model: Name::new("gpt-5.6-sol")?,
            effort: None,
        },
    )?;

    assert!(
        matches!(refused, Outcome::Model(ModelOutcome::Unreachable(_))),
        "{refused:?}"
    );
    assert_eq!(std::fs::read_to_string(&file)?, before);
    assert_eq!(
        client::snapshot(&outside.conversation)
            .model
            .as_ref()
            .map(|model| model.text().as_str()),
        Some("claude-opus-5-5")
    );
    Ok(())
}

/// On the route, a model of a provider the agent does not reach, with a key
/// held for that provider, moves the conversation off the agent onto it and
/// that model, in a new session, and writes both down; the session left can be
/// resumed.
#[test]
fn a_model_of_another_provider_with_a_key_moves_off_the_route() -> Result<(), Failed> {
    use crucible_client_api::{Execution, ModelOutcome, Name, Retained, Rung, Text};

    let tree = Tree::new("external-model-moved-off")?;
    let file = entered(&tree)?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    let left = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;
    let route = outside
        .conversation
        .session()
        .route()
        .cloned()
        .ok_or("a route")?;
    let desk = super::Desk::stored(&tree)?;
    desk.logins.keep("openai", "a-key-no-vendor-issued")?;

    let moved = answered_by(
        &desk,
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new("openai")?,
            model: Name::new("gpt-5.6-sol")?,
            effort: Some(Rung::High),
        },
    )?;

    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.execution, Execution::Native, "{moved:?}");
    assert_eq!(standing.provider, Some(Name::new("openai")?));
    assert_eq!(
        standing.model,
        crucible_client_api::Model::new("gpt-5.6-sol")
    );
    assert_eq!(
        moved,
        Outcome::Model(ModelOutcome::MovedOff {
            provider: Text::cut("openai"),
            retained: Retained::default(),
            unwritten: None,
        })
    );
    let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file)?)?;
    assert_eq!(
        written.pointer("/provider"),
        Some(&serde_json::json!("openai"))
    );
    assert_eq!(
        written.pointer("/providers/openai/model"),
        Some(&serde_json::json!("gpt-5.6-sol"))
    );
    assert_eq!(
        written.pointer("/providers/openai/effort"),
        Some(&serde_json::json!("high"))
    );
    let moved = outside.conversation.session();
    assert!(moved.route().is_none(), "{:?}", moved.route());
    assert!(
        moved.id().is_some_and(|moved| *moved != left),
        "no new session"
    );
    Session::reopen_routed(&tree.sessions(), &tree.workspace()?, &left, &route)?;
    Ok(())
}

/// On the route, a model of another provider at a rung it does not serve is
/// refused before anything moves: the conversation stays on the agent, in the
/// session it was in, and nothing is written.
#[test]
fn a_model_of_another_provider_at_a_rung_it_does_not_serve_stays_on_the_route() -> Result<(), Failed>
{
    use crucible_client_api::{Execution, ModelOutcome, Name, Rung};

    let tree = Tree::new("external-model-unserved-elsewhere")?;
    let file = entered(&tree)?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    let left = outside.conversation.session().id().cloned();
    let desk = super::Desk::stored(&tree)?;
    desk.logins.keep("google", "a-key-no-vendor-issued")?;
    let before = std::fs::read_to_string(&file)?;

    let refused = answered_by(
        &desk,
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new("google")?,
            model: Name::new("gemini-3.1-pro-preview")?,
            effort: Some(Rung::Max),
        },
    )?;

    assert!(
        matches!(refused, Outcome::Model(ModelOutcome::Unsupported)),
        "{refused:?}"
    );
    let standing = client::snapshot(&outside.conversation);
    assert!(
        matches!(standing.execution, Execution::Instance(_)),
        "{:?}",
        standing.execution
    );
    assert_eq!(outside.conversation.session().id().cloned(), left);
    assert_eq!(std::fs::read_to_string(&file)?, before);
    Ok(())
}

/// The user file of a run that reaches anthropic through the installed
/// agent `work` and asks openai.
const ROUTED_ELSEWHERE: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}},
  "providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "work"}}},
  "provider": "openai"
}
"#;

/// What `command` is answered with, the user file holding `file` and this
/// run's settings read from it.
fn answered_off(
    tree: &Tree,
    conversation: &mut Conversation,
    file: &str,
    command: Command,
) -> Result<Outcome, Failed> {
    let mut desk = super::Desk::new(tree, &[])?;
    if let Some(directory) = desk.choosing.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(&desk.choosing, file)?;
    desk.settings = super::written(tree)?;
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let at = client::Desk {
        switching: desk.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads: |_| false,
        environment: super::unset,
        notes: |_| Err(crucible_client_api::ErrorCode::InvalidArgument.into()),
    };
    let request = Request::new(Asking::every(), Correlation::new(1), command);
    let performed = runtime()?.block_on(client::perform(conversation, &request, &at));
    Ok(performed.response(&request).outcome)
}

/// Off the route, a model of a provider this run reaches through an installed
/// agent moves the conversation onto that agent, on that model, in a new
/// session, and writes the model into the agent's entry; the agent's first
/// run is started on it, and the session left can be resumed.
#[test]
fn a_model_of_a_provider_reached_through_an_agent_moves_onto_it() -> Result<(), Failed> {
    use crucible_client_api::{Execution, ModelOutcome, Name, Text};

    let tree = Tree::new("external-model-moved-onto")?;
    let native = super::conversing(
        &tree,
        Script::new(Vec::new()),
        &super::Guard::Nothing,
        Some("openai"),
    )?;
    let left = native.session().id().cloned().ok_or("an id")?;
    let (mut conversation, peer) = stepped(&tree, native, &connected([claude::clean()]))?;

    let moved = answered_off(
        &tree,
        &mut conversation,
        ROUTED_ELSEWHERE,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-sonnet-5-5")?,
            effort: None,
        },
    )?;

    assert_eq!(
        moved,
        Outcome::Model(ModelOutcome::MovedOnto {
            onto: Text::cut("work"),
            unwritten: None,
        })
    );
    let standing = client::snapshot(&conversation);
    assert_eq!(standing.execution, Execution::Instance(Name::new("work")?));
    assert_eq!(
        standing.model.as_ref().map(|model| model.text().as_str()),
        Some("claude-sonnet-5-5")
    );
    let route = conversation.session().route().map(RouteBinding::instance);
    assert_eq!(route, Some("work"));
    assert!(
        conversation
            .session()
            .id()
            .is_some_and(|moved| *moved != left),
        "no new session"
    );
    let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        crucible_config::user(&tree.home()?),
    )?)?;
    assert_eq!(
        written.pointer("/externalAgents/work/model"),
        Some(&serde_json::json!("claude-sonnet-5-5"))
    );
    let (turned, _) = super::turn(&mut conversation, "after")?;
    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    let started = fake_peer::argued(&peer);
    let run = started.last().ok_or("the run was never started")?;
    assert!(
        run.windows(2)
            .any(|pair| pair == ["--model", "claude-sonnet-5-5"]),
        "{started:?}"
    );
    Session::reopen(&tree.sessions(), &tree.workspace()?, &left)?;
    Ok(())
}

/// Off the route, a model of a provider reached through an agent keeps the
/// rung in force where the model serves it: the agent's entry is written at
/// it and the agent's first run is started at it.
#[test]
fn a_model_moved_onto_an_agent_keeps_the_rung_in_force_it_serves() -> Result<(), Failed> {
    use crucible_client_api::{EffortOutcome, ModelOutcome, Name, Rung, Text};

    let tree = Tree::new("external-model-onto-rung")?;
    let native = super::conversing(
        &tree,
        Script::new(Vec::new()),
        &super::Guard::Nothing,
        Some("openai"),
    )?;
    let (mut conversation, peer) = stepped(&tree, native, &connected([claude::clean()]))?;
    let set = answered_off(
        &tree,
        &mut conversation,
        ROUTED_ELSEWHERE,
        Command::SetEffort(Rung::Xhigh),
    )?;
    assert!(
        matches!(set, Outcome::Effort(EffortOutcome::Taken { .. })),
        "{set:?}"
    );

    let moved = answered_off(
        &tree,
        &mut conversation,
        ROUTED_ELSEWHERE,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-sonnet-5-5")?,
            effort: None,
        },
    )?;

    assert_eq!(
        moved,
        Outcome::Model(ModelOutcome::MovedOnto {
            onto: Text::cut("work"),
            unwritten: None,
        })
    );
    let written: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(
        crucible_config::user(&tree.home()?),
    )?)?;
    assert_eq!(
        written.pointer("/externalAgents/work/effort"),
        Some(&serde_json::json!("xhigh"))
    );
    let (turned, _) = super::turn(&mut conversation, "after")?;
    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    let started = fake_peer::argued(&peer);
    let run = started.last().ok_or("the run was never started")?;
    assert!(
        run.windows(2).any(|pair| pair == ["--effort", "xhigh"]),
        "{started:?}"
    );
    Ok(())
}

/// The user file of a run that reaches anthropic through the installed
/// agent `work`, which is disconnected, and asks openai.
const ROUTED_DISCONNECTED: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude", "enabled": false}},
  "providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "work"}}},
  "provider": "openai"
}
"#;

/// Off the route, a model of a provider reached through a disconnected agent
/// is refused with the two ways back, and nothing moves or is written.
#[test]
fn a_model_of_a_provider_reached_through_a_disconnected_agent_moves_nothing() -> Result<(), Failed>
{
    use crucible_client_api::{Execution, ModelOutcome, Name};

    let tree = Tree::new("external-model-onto-disconnected")?;
    let native = super::conversing(
        &tree,
        Script::new(Vec::new()),
        &super::Guard::Nothing,
        Some("openai"),
    )?;
    let left = native.session().id().cloned();
    let (mut conversation, _peer) = stepped(&tree, native, &connected([]))?;

    let refused = answered_off(
        &tree,
        &mut conversation,
        ROUTED_DISCONNECTED,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-sonnet-5-5")?,
            effort: None,
        },
    )?;

    let Outcome::Model(ModelOutcome::Unreachable(problem)) = &refused else {
        return Err(format!("{refused:?}").into());
    };
    assert_eq!(
        problem.message.as_str(),
        "the agent work cannot be started: it is disconnected: set \
         externalAgents.work.enabled to true in the user configuration file, or remove \
         providers.anthropic.access there to reach anthropic directly"
    );
    assert_eq!(client::snapshot(&conversation).execution, Execution::Native);
    assert_eq!(conversation.session().id().cloned(), left);
    assert_eq!(
        std::fs::read_to_string(crucible_config::user(&tree.home()?))?,
        ROUTED_DISCONNECTED
    );
    Ok(())
}

/// Off the route, a model of a provider reached through an agent at a rung
/// the model does not serve is refused before anything moves or is written.
#[test]
fn a_model_reached_through_an_agent_at_a_rung_it_does_not_serve_moves_nothing() -> Result<(), Failed>
{
    use crucible_client_api::{Execution, ModelOutcome, Name, Rung};

    let tree = Tree::new("external-model-onto-unserved")?;
    let native = super::conversing(
        &tree,
        Script::new(Vec::new()),
        &super::Guard::Nothing,
        Some("openai"),
    )?;
    let left = native.session().id().cloned();
    let (mut conversation, _peer) = stepped(&tree, native, &connected([]))?;

    let refused = answered_off(
        &tree,
        &mut conversation,
        ROUTED_ELSEWHERE,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude-haiku-4-5")?,
            effort: Some(Rung::Max),
        },
    )?;

    assert!(
        matches!(refused, Outcome::Model(ModelOutcome::Unsupported)),
        "{refused:?}"
    );
    assert_eq!(client::snapshot(&conversation).execution, Execution::Native);
    assert_eq!(conversation.session().id().cloned(), left);
    assert_eq!(
        std::fs::read_to_string(crucible_config::user(&tree.home()?))?,
        ROUTED_ELSEWHERE
    );
    Ok(())
}

/// Off the route, a model of a provider reached through an agent, named as
/// the agent's program could not be started on, is refused before anything
/// moves or is written.
#[test]
fn a_model_reached_through_an_agent_it_cannot_be_started_on_moves_nothing() -> Result<(), Failed> {
    use crucible_client_api::{Execution, ModelOutcome, Name};

    let tree = Tree::new("external-model-onto-unnamed")?;
    let native = super::conversing(
        &tree,
        Script::new(Vec::new()),
        &super::Guard::Nothing,
        Some("openai"),
    )?;
    let left = native.session().id().cloned();
    let (mut conversation, _peer) = stepped(&tree, native, &connected([]))?;

    let refused = answered_off(
        &tree,
        &mut conversation,
        ROUTED_ELSEWHERE,
        Command::SelectModel {
            provider: Name::new("anthropic")?,
            model: Name::new("claude sonnet 5")?,
            effort: None,
        },
    )?;

    let Outcome::Model(ModelOutcome::Unreachable(problem)) = &refused else {
        return Err(format!("{refused:?}").into());
    };
    assert_eq!(
        problem.message.as_str(),
        "the agent work cannot be started on \"claude sonnet 5\": a model is named with \
         letters, digits and . _ - : @ / [ ] only, at most 128 of them, and never starting with \
         -, since it is handed to the agent's program as an argument"
    );
    assert_eq!(client::snapshot(&conversation).execution, Execution::Native);
    assert_eq!(conversation.session().id().cloned(), left);
    assert_eq!(
        std::fs::read_to_string(crucible_config::user(&tree.home()?))?,
        ROUTED_ELSEWHERE
    );
    Ok(())
}

/// The user file of a run on the installed agent `work` that reaches google
/// through `space`, an agent that chooses its own model, with its profile
/// under `home`: a profile is named by a rooted path, which on Windows starts
/// with a drive.
fn routed_to_one_of_its_own(home: &std::path::Path) -> String {
    serde_json::json!({
        "externalAgents": {
            "work": {"driver": "claude", "executable": "claude"},
            "space": {"driver": "antigravity", "executable": "agy", "profileDir": home.join("agy")}
        },
        "providers": {
            "anthropic": {"access": {"type": "externalAgent", "instance": "work"}},
            "google": {"access": {"type": "externalAgent", "instance": "space"}}
        }
    })
    .to_string()
}

/// On a route, a model of a provider reached through another agent, one that
/// chooses its own model, is refused with what that agent is, and the
/// conversation stays on its route in the session it was in.
#[test]
fn a_model_of_a_provider_reached_through_an_agent_choosing_its_own_is_refused() -> Result<(), Failed>
{
    use crucible_client_api::{Execution, ModelOutcome, Name};

    let tree = Tree::new("external-model-onto-its-own")?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    let left = outside.conversation.session().id().cloned();

    let refused = answered_off(
        &tree,
        &mut outside.conversation,
        &routed_to_one_of_its_own(tree.home()?.path()),
        Command::SelectModel {
            provider: Name::new("google")?,
            model: Name::new("gemini-3.8-flash")?,
            effort: None,
        },
    )?;

    let Outcome::Model(ModelOutcome::Unreachable(problem)) = &refused else {
        return Err(format!("{refused:?}").into());
    };
    assert_eq!(
        problem.message.as_str(),
        "providers.google.access names the agent space, which runs the conversation in \
         place of google's models; remove access to reach google directly"
    );
    assert_eq!(
        client::snapshot(&outside.conversation).execution,
        Execution::Instance(Name::new("work")?)
    );
    assert_eq!(outside.conversation.session().id().cloned(), left);
    Ok(())
}

/// What `/context` answers on the route.
fn context(tree: &Tree, outside: &mut Outside) -> Result<crucible_client_api::Context, Failed> {
    match answered(tree, outside, Command::Context)? {
        Outcome::Context(context) => Ok(context),
        other => Err(format!("/context was answered with {other:?}").into()),
    }
}

/// On the route, `/context` counts nothing Crucible cannot see and carries
/// what the agent's context holds against its model's window: nothing before
/// it said anything, what its last request carried once it has, and what is
/// left once it made room. `/usage` carries the same figures.
#[test]
fn context_on_the_route_holds_what_the_agent_carried_against_its_window() -> Result<(), Failed> {
    let tree = Tree::new("external-context")?;
    let mut outside = Outside::new(&tree, &claude::carried_then_compacted())?;

    let before = context(&tree, &mut outside)?;
    let window = before.window.ok_or("the route's model has no window")?;
    assert_eq!(before.external, Some(0), "{before:?}");
    assert_eq!(before.free, window, "{before:?}");
    let (first, _) = turn(&mut outside, "first", false)?;
    assert!(matches!(first, Ok(Turned::Ran(_))), "{first:?}");
    let carried = context(&tree, &mut outside)?;
    assert_eq!(carried.external, Some(claude::CARRIED), "{carried:?}");
    assert_eq!(carried.free, window - claude::CARRIED, "{carried:?}");
    let left = (window - claude::CARRIED) * 100 / window;
    assert_eq!(
        carried.left.map(|left| u64::from(left.get())),
        Some(left),
        "{carried:?}"
    );

    let (second, events) = turn(&mut outside, "second", false)?;
    assert!(matches!(second, Ok(Turned::Ran(_))), "{second:?}");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::AgentCompacted { .. })),
        "{events:?}"
    );
    let after = context(&tree, &mut outside)?;
    assert_eq!(after.external, Some(claude::COMPACTED), "{after:?}");
    let Outcome::Usage(usage) = answered(&tree, &mut outside, Command::Usage)? else {
        return Err("/usage was not answered with a usage".into());
    };
    assert_eq!(usage.context, after);
    let counted = after.system + after.instructions + after.tools + after.mcp + after.messages;
    assert_eq!(counted + after.reserve, 0, "{after:?}");
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
        &[],
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
/// under way.
#[test]
fn a_mode_change_takes_effect_at_the_next_launch() -> Result<(), Failed> {
    let tree = Tree::new("external-mode")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");

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

/// Full access is a mode an agent's run is switched to, and the next turn's
/// run is launched in it, continuing the session.
#[test]
fn full_access_is_taken_at_the_next_launch_on_an_agent() -> Result<(), Failed> {
    let tree = Tree::new("external-full-access")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;
    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");

    assert_eq!(
        outside.conversation.switch(Mode::FullAccess),
        Mode::FullAccess
    );
    let (second, _) = turn(&mut outside, "two", false)?;

    assert!(second.is_ok(), "{second:?}");
    assert_eq!(
        outside.launched(),
        [
            (Mode::Ask, None),
            (Mode::FullAccess, Some(claude::SESSION.to_owned()))
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

/// A conversation in full access is moved onto the route and stays in full
/// access there.
#[test]
fn a_conversation_in_full_access_is_moved_onto_the_route_in_it() -> Result<(), Failed> {
    let tree = Tree::new("external-full-access-moved")?;
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
        let backend = launching.launch(Mode::Ask, None, None, None).await?;
        let installed = Installed {
            backend: Some(backend),
            relaunch: Box::new(launching),
            route: binding()?,
        };
        Ok::<_, Failed>(native.external(session, Transcript::new(), installed).await)
    })?;

    assert!(moved.is_ok(), "{moved:?}");
    assert_eq!(native.settled().mode(), Mode::FullAccess);
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
        &[],
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

/// A headless client is told of each of the agent's own calls under the
/// agent's id for it, with what kind of thing it names, and of each one
/// ending under the same id, a failure as a failure: what the terminal draws
/// as a call row and its result.
#[test]
fn the_agent_s_calls_and_their_ends_reach_a_client_under_the_call_s_id() -> Result<(), Failed> {
    let tree = Tree::new("external-calls")?;
    let mut outside = Outside::new(&tree, &connected([claude::tooled()]))?;
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
        stopping: None,
    };
    let answered = {
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
    assert_eq!(
        answered.response(&request).outcome,
        Outcome::Turn(TurnOutcome::Ran {
            stop: Stop::Yielded
        })
    );

    let streamed: Vec<_> = posted(&reported)
        .iter()
        .filter_map(|event| client::progress(Asking::every(), event, None))
        .collect();
    let activity: Vec<_> = streamed
        .iter()
        .filter_map(|progress| match progress {
            Progress::ToolActivity {
                call,
                tool,
                summary,
                summary_kind,
            } => Some((
                call.as_str(),
                tool.as_str(),
                summary.as_str(),
                *summary_kind,
            )),
            _ => None,
        })
        .collect();
    let ended: Vec<_> = streamed
        .iter()
        .filter_map(|progress| match progress {
            Progress::ToolFinished { call, failed } => Some((call.as_str(), *failed)),
            _ => None,
        })
        .collect();
    assert_eq!(
        activity,
        [
            ("toolu_ls", "Bash", claude::LISTED, SummaryKind::Command),
            ("toolu_read", "Read", claude::MISSING, SummaryKind::Path),
        ],
        "{streamed:?}"
    );
    assert_eq!(
        ended,
        [("toolu_ls", false), ("toolu_read", true)],
        "{streamed:?}"
    );
    assert!(
        !streamed
            .iter()
            .any(|progress| matches!(progress, Progress::ToolRequested { .. })),
        "nothing the agent ran is a call this program was asked to run: {streamed:?}"
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
        command: &|instance| Command::Disconnect { instance },
        asking,
        after: None,
    };
    accounting(tree, outside, &asked, choosing)
}

/// An account operation, asked of a user file holding `installed.0` about the
/// instance named `installed.1`.
struct Asked<'a> {
    installed: (&'a str, &'a str),
    command: &'a dyn Fn(crucible_client_api::Name) -> Command,
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
    choosing: &mut dyn Front,
) -> Result<(Outcome, String), Failed> {
    accounting_on(tree, &mut outside.conversation, asked, choosing)
}

/// [`accounting`], of `conversation` wherever it is.
fn accounting_on(
    tree: &Tree,
    conversation: &mut Conversation,
    asked: &Asked<'_>,
    choosing: &mut dyn Front,
) -> Result<(Outcome, String), Failed> {
    accounting_at(
        super::Desk::new(tree, &[])?,
        tree,
        conversation,
        asked,
        choosing,
    )
}

/// What [`accounting_on`] does, at `desk`.
fn accounting_at(
    mut desk: super::Desk,
    tree: &Tree,
    conversation: &mut Conversation,
    asked: &Asked<'_>,
    choosing: &mut dyn Front,
) -> Result<(Outcome, String), Failed> {
    use crucible_client_api::Name;

    let (installed, asking) = (asked.installed, asked.asking);
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
    let performed = runtime()?.block_on(client::account(conversation, &request, &at, choosing));
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
        command: &|instance| Command::RemoveIntegration { instance },
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
        command: &|instance| Command::RemoveIntegration { instance },
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

/// A client that chooses `REMOVE`, and as it does leaves the user file holding
/// `unusable`, as though another program had been writing it between the
/// check and the write.
struct Spoiling {
    file: PathBuf,
    unusable: &'static str,
    put: usize,
}

impl Front for Spoiling {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Ruled>> {
        Box::pin(async move {
            self.put += 1;
            std::fs::write(&self.file, self.unusable).ok()?;
            Some(Ruled::Picked {
                id: pending.id(),
                choice: crucible_client_api::Said::new(client::accounts::REMOVE).ok()?,
            })
        })
    }

    fn refused(&mut self, _refusal: crucible_client_api::Refusal) {}
}

/// A removal whose write fails once the agent's run was stopped tells the
/// client the instance is stopped for this run only, since the settings are
/// still in the file and the next run starts it again; it is not told only
/// that the removal failed.
#[test]
fn a_removal_whose_write_fails_says_the_agent_was_stopped_and_its_settings_remain()
-> Result<(), Failed> {
    use crucible_client_api::LogoutOutcome;

    // Still names the instance, and is not a configuration that can be edited.
    let unusable = r#"{"externalAgents": {"work": {"driver": "claude""#;
    let tree = Tree::new("external-remove-unwritten")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let desk = super::Desk::new(&tree, &[])?;
    let mut spoiling = Spoiling {
        file: desk.choosing.clone(),
        unusable,
        put: 0,
    };
    let asked = Asked {
        installed: (INSTALLED, "work"),
        command: &|instance| Command::RemoveIntegration { instance },
        asking: Asking::every(),
        after: None,
    };

    let (outcome, file) = accounting_at(
        desk,
        &tree,
        &mut outside.conversation,
        &asked,
        &mut spoiling,
    )?;

    assert_eq!(spoiling.put, 1, "the removal was asked about once");
    assert!(
        matches!(
            outcome,
            Outcome::Logout(LogoutOutcome::StoppedForThisRunOnly { .. })
        ),
        "{outcome:?}"
    );
    assert_eq!(file, unusable, "nothing was written");
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_err(), "a stopped agent took a turn");
    Ok(())
}

/// A removal of an instance the conversation is not on stops nothing, so where
/// its write fails the client is told only that it failed, not that anything
/// was stopped for this run.
#[test]
fn a_removal_whose_write_fails_off_the_route_says_only_that_it_failed() -> Result<(), Failed> {
    let two = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}, "spare": {"driver": "claude", "executable": "claude"}},
  "updates": {"check": "auto"}
}
"#;
    // Still names the instance, and is not a configuration that can be edited.
    let unusable = r#"{"externalAgents": {"spare": {"driver": "claude""#;
    let tree = Tree::new("external-remove-unwritten-off-route")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let desk = super::Desk::new(&tree, &[])?;
    let mut spoiling = Spoiling {
        file: desk.choosing.clone(),
        unusable,
        put: 0,
    };
    let asked = Asked {
        installed: (two, "spare"),
        command: &|instance| Command::RemoveIntegration { instance },
        asking: Asking::every(),
        after: None,
    };

    let (outcome, file) = accounting_at(
        desk,
        &tree,
        &mut outside.conversation,
        &asked,
        &mut spoiling,
    )?;

    assert_eq!(spoiling.put, 1, "the removal was asked about once");
    assert_eq!(
        outcome,
        Outcome::Refused(crucible_client_api::ErrorCode::Failed.into())
    );
    assert_eq!(file, unusable, "nothing was written");
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_ok(), "the agent in use was stopped: {turned:?}");
    Ok(())
}

/// Two instances on Claude Code's default profile, and one on a profile of
/// its own, written as an absolute path on each platform.
#[cfg(not(windows))]
const SHARED: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}, "spare": {"driver": "claude", "executable": "claude"}, "apart": {"driver": "claude", "executable": "claude", "configDir": "/nowhere/apart"}},
  "updates": {"check": "auto"}
}
"#;
#[cfg(windows)]
const SHARED: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}, "spare": {"driver": "claude", "executable": "claude"}, "apart": {"driver": "claude", "executable": "claude", "configDir": "C:/nowhere/apart"}},
  "updates": {"check": "auto"}
}
"#;

/// Asks to sign the instance `work` of the user file `installed` out, of a
/// client that answers `choice`, the file holding `after` once the settings
/// were read, where that is something else; and answers what the client was
/// told, what the file then held, and how many times the agent's program was
/// started for it.
fn signing_out(
    tree: &Tree,
    outside: &mut Outside,
    installed: &str,
    after: Option<&str>,
    choice: Option<&'static str>,
) -> Result<(Outcome, String, usize), Failed> {
    let asked = Asked {
        installed: (installed, "work"),
        command: &|instance| Command::SignOut { instance },
        asking: Asking::every(),
        after,
    };
    let mut choosing = Choosing {
        choice,
        put: Vec::new(),
    };
    // The conversation's run is started as it opens, and is counted once the
    // peer has read its script: after the probe, the status check and it.
    for _ in 0..500 {
        if fake_peer::played(&outside.peer) >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let before = fake_peer::played(&outside.peer);
    let (outcome, file) = accounting(tree, outside, &asked, &mut choosing)?;
    Ok((outcome, file, fake_peer::played(&outside.peer) - before))
}

/// A sign-out writes `enabled: false` for every instance on the profile, and
/// for nothing else, before the agent's own logout and a status step after it
/// run; with both reading signed out, it is said to be signed out.
#[test]
fn a_sign_out_disables_every_instance_on_the_profile_then_signs_the_agent_out() -> Result<(), Failed>
{
    use crucible_client_api::LogoutOutcome;

    let tree = Tree::new("external-sign-out")?;
    let script = connected([claude::clean(), Reply::new(), claude::signed_out()]);
    let mut outside = Outside::new(&tree, &script)?;

    let (outcome, file, started) = signing_out(
        &tree,
        &mut outside,
        SHARED,
        None,
        Some(client::accounts::SIGN_OUT),
    )?;

    assert_eq!(
        outcome,
        Outcome::Logout(LogoutOutcome::AgentSignedOut {
            said: crucible_client_api::Text::cut("Signed out of this agent profile"),
        })
    );
    assert_eq!(
        file,
        SHARED.replace(
            r#""executable": "claude"},"#,
            r#""executable": "claude", "enabled": false},"#
        )
    );
    assert_eq!(started, 2, "a logout and a status step");
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_err(), "a signed-out agent took a turn");
    Ok(())
}

/// Where the agent does not read signed out afterwards, every instance on the
/// profile is still written off, and the sign-out is said to be unverified.
#[test]
fn a_sign_out_the_agent_does_not_confirm_is_unverified() -> Result<(), Failed> {
    use crucible_client_api::LogoutOutcome;

    let tree = Tree::new("external-sign-out-unverified")?;
    let script = connected([claude::clean(), Reply::new(), claude::signed_in()]);
    let mut outside = Outside::new(&tree, &script)?;

    let (outcome, file, started) = signing_out(
        &tree,
        &mut outside,
        SHARED,
        None,
        Some(client::accounts::SIGN_OUT),
    )?;

    assert_eq!(
        outcome,
        Outcome::Logout(LogoutOutcome::SignOutUnverified {
            said: crucible_client_api::Text::cut("Disconnected; external sign-out unverified"),
        })
    );
    assert_eq!(
        file,
        SHARED.replace(
            r#""executable": "claude"},"#,
            r#""executable": "claude", "enabled": false},"#
        )
    );
    assert_eq!(started, 2, "a logout and a status step");
    Ok(())
}

/// Where `enabled: false` cannot be written, the agent's program is not
/// started at all, and the client is told the stop holds for this run only.
#[test]
fn a_sign_out_whose_write_fails_starts_no_agent_program() -> Result<(), Failed> {
    use crucible_client_api::LogoutOutcome;

    let elsewhere = r#"{"updates": {"check": "auto"}}
"#;
    let tree = Tree::new("external-sign-out-unwritten")?;
    let script = connected([claude::clean(), Reply::new(), claude::signed_out()]);
    let mut outside = Outside::new(&tree, &script)?;

    let (outcome, file, started) = signing_out(
        &tree,
        &mut outside,
        SHARED,
        Some(elsewhere),
        Some(client::accounts::SIGN_OUT),
    )?;

    assert!(
        matches!(
            outcome,
            Outcome::Logout(LogoutOutcome::StoppedForThisRunOnly { .. })
        ),
        "{outcome:?}"
    );
    assert_eq!(file, elsewhere);
    assert_eq!(started, 0, "the agent's program was started");
    Ok(())
}

/// Cancelling or going back leaves the file byte for byte as it was and
/// starts nothing; choosing to disconnect instead writes off that instance
/// alone and starts nothing either.
#[test]
fn a_sign_out_not_chosen_signs_nothing_out() -> Result<(), Failed> {
    use crucible_client_api::LogoutOutcome;

    let disconnected = SHARED.replacen(
        r#""executable": "claude"},"#,
        r#""executable": "claude", "enabled": false},"#,
        1,
    );
    for (case, choice, expected, kept) in [
        (
            "cancel",
            Some("Cancel"),
            Outcome::Logout(LogoutOutcome::Undecided),
            SHARED,
        ),
        (
            "back",
            None,
            Outcome::Logout(LogoutOutcome::Undecided),
            SHARED,
        ),
        (
            "instead",
            Some(client::accounts::DISCONNECT_INSTEAD),
            Outcome::Logout(LogoutOutcome::Disconnected {
                said: crucible_client_api::Text::cut(client::accounts::DISCONNECTED),
            }),
            disconnected.as_str(),
        ),
    ] {
        let tree = Tree::new(&format!("external-sign-out-kept-{case}"))?;
        let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;

        let (outcome, file, started) = signing_out(&tree, &mut outside, SHARED, None, choice)?;

        assert_eq!(outcome, expected, "{case}");
        assert_eq!(file, kept, "{case}");
        assert_eq!(started, 0, "{case}: the agent's program was started");
    }
    Ok(())
}

/// A client that answers each account question with the next choice named,
/// accepts a warning where any is named, and goes back where none is, or once
/// the choices run out. Where it is `reading` a file, it reads it as each
/// question is put.
struct Picking {
    choices: std::collections::VecDeque<Option<&'static str>>,
    put: Vec<Pending>,
    reading: Option<std::path::PathBuf>,
    read: Vec<String>,
}

impl Picking {
    fn of(choices: impl IntoIterator<Item = Option<&'static str>>) -> Self {
        Self {
            choices: choices.into_iter().collect(),
            put: Vec::new(),
            reading: None,
            read: Vec::new(),
        }
    }
}

impl Front for Picking {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Ruled>> {
        Box::pin(async move {
            self.put.push(pending.clone());
            if let Some(file) = &self.reading {
                self.read
                    .push(std::fs::read_to_string(file).unwrap_or_default());
            }
            Some(match self.choices.pop_front().flatten() {
                Some(_) if matches!(pending, Pending::Warning { .. }) => {
                    Ruled::Accepted { id: pending.id() }
                }
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

/// The user file a connect is written to: the instance installed and off.
const UNCONNECTED: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude", "enabled": false}},
  "updates": {"check": "auto"}
}
"#;

/// What [`UNCONNECTED`] holds once the provider is connected through `work`.
const CONNECTED_FILE: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude", "enabled": true}},
  "updates": {"check": "auto"},
  "providers": {
    "anthropic": {
      "access": {"instance":"work","type":"externalAgent"}
    }
  },
  "provider": "anthropic"
}
"#;

/// Asks to connect through `instance`, or through none, of a
/// client that answers as `picking` does, once the conversation's own run has
/// started; and answers what the client was told and what the user file then
/// held.
fn connecting(
    tree: &Tree,
    outside: &mut Outside,
    instance: Option<&str>,
    picking: &mut Picking,
) -> Result<(Outcome, String), Failed> {
    Ok(connecting_counted(tree, outside, UNCONNECTED, instance, picking)?.0)
}

/// The same, the user file holding `file` first, answering also how many
/// times the agent's program was started for the connect.
fn connecting_counted(
    tree: &Tree,
    outside: &mut Outside,
    file: &str,
    instance: Option<&str>,
    picking: &mut Picking,
) -> Result<((Outcome, String), usize), Failed> {
    let named = instance.is_some();
    let command = move |instance| Command::Connect {
        instance: named.then_some(instance),
    };
    let asked = Asked {
        installed: (file, instance.unwrap_or("work")),
        command: &command,
        asking: Asking::every(),
        after: None,
    };
    for _ in 0..500 {
        if fake_peer::played(&outside.peer) >= 3 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let before = fake_peer::played(&outside.peer);
    let asked = accounting(tree, outside, &asked, picking)?;
    Ok((asked, fake_peer::played(&outside.peer) - before))
}

/// Signed in, a connect asks nothing more: a conversation on another
/// provider's model moves onto the agent, and the user file reaches and asks
/// the provider the agent serves, leaving the other provider as it was.
#[test]
fn a_connect_signed_in_asks_nothing_more_and_moves_a_conversation_off_another_provider()
-> Result<(), Failed> {
    use crucible_client_api::{LoginOutcome, Name, Text};

    let tree = Tree::new("external-connect-elsewhere")?;
    let native = super::conversing(
        &tree,
        Script::new(Vec::new()),
        &super::Guard::Nothing,
        Some("openai"),
    )?;
    let script = connected([claude::signed_in(), claude::clean()]);
    let (mut conversation, _peer) = stepped(&tree, native, &script)?;
    let mut picking = Picking::of([]);
    let command = |instance| Command::Connect {
        instance: Some(instance),
    };
    let asked = Asked {
        installed: (UNCONNECTED, "work"),
        command: &command,
        asking: Asking::every(),
        after: None,
    };

    let (outcome, file) = accounting_on(&tree, &mut conversation, &asked, &mut picking)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentConnected {
            said: Text::cut(&client::accounts::moved_onto("work")),
            onto: Some(Name::new("work")?),
        })
    );
    assert!(picking.put.is_empty(), "{:?}", picking.put);
    let route = conversation.session().route().map(RouteBinding::instance);
    assert_eq!(route, Some("work"));
    let written: serde_json::Value = serde_json::from_str(&file)?;
    let at = |pointer: &str| written.pointer(pointer).and_then(serde_json::Value::as_str);
    assert_eq!(
        at("/providers/anthropic/access/instance"),
        Some("work"),
        "{file}"
    );
    assert_eq!(at("/provider"), Some("anthropic"), "{file}");
    assert!(written.pointer("/providers/openai").is_none(), "{file}");
    Ok(())
}

/// Signed in, a connect asks nothing, writes the provider's access to the
/// instance, asks the provider and enables the instance, and changes nothing
/// else in the file.
#[test]
fn a_connect_writes_the_route_and_enables_the_instance() -> Result<(), Failed> {
    use crucible_client_api::LoginOutcome;

    let tree = Tree::new("external-connect")?;
    let script = connected([claude::clean(), claude::signed_in()]);
    let mut outside = Outside::new(&tree, &script)?;
    let mut picking = Picking::of([]);

    let (outcome, file) = connecting(&tree, &mut outside, Some("work"), &mut picking)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentConnected {
            said: crucible_client_api::Text::cut(client::accounts::CONNECTED),
            onto: None,
        })
    );
    assert!(picking.put.is_empty(), "{:?}", picking.put);
    assert_eq!(file, CONNECTED_FILE);
    Ok(())
}

/// Connecting the agent a conversation was disconnected from, while it is
/// still on that agent, lets its next turn start the agent again.
#[test]
fn a_connect_after_a_disconnect_lets_the_next_turn_start_the_agent() -> Result<(), Failed> {
    use crucible_client_api::{LoginOutcome, LogoutOutcome, Text};

    let tree = Tree::new("external-reconnect")?;
    let script = connected([claude::clean(), claude::signed_in(), claude::clean()]);
    let mut outside = Outside::new(&tree, &script)?;
    let mut choosing = Choosing {
        choice: Some(client::accounts::DISCONNECT),
        put: Vec::new(),
    };
    let (outcome, disconnected) =
        disconnecting(&tree, &mut outside, Asking::every(), &mut choosing)?;
    assert!(
        matches!(outcome, Outcome::Logout(LogoutOutcome::Disconnected { .. })),
        "{outcome:?}"
    );
    let mut picking = Picking::of([]);

    let ((outcome, file), _) = connecting_counted(
        &tree,
        &mut outside,
        &disconnected,
        Some("work"),
        &mut picking,
    )?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentConnected {
            said: Text::cut(client::accounts::CONNECTED),
            onto: None,
        })
    );
    assert_eq!(file, CONNECTED_FILE);
    let launched = outside.launched().len();
    let (turned, _events) = turn(&mut outside, "again", false)?;
    assert!(turned.is_ok(), "{turned:?}");
    assert_eq!(
        outside.launched().len(),
        launched + 1,
        "the agent was not started again"
    );
    Ok(())
}

/// The user file of a conversation on `work` with a second instance beside it.
const TWO_ROUTED: &str = r#"{
  "externalAgents": {
    "work": {"driver": "claude", "executable": "claude"},
    "home": {"driver": "claude", "executable": "claude"}
  },
  "providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "work"}}}
}
"#;

/// A connect, on a route, of the provider it reaches through another instance
/// moves the conversation onto that instance, in a new session bound to it;
/// the session it left can be resumed.
#[test]
fn a_connect_through_another_agent_moves_the_route_onto_it() -> Result<(), Failed> {
    use crucible_client_api::{LoginOutcome, Name, Text};

    let tree = Tree::new("external-connect-another")?;
    let script = connected([claude::clean(), claude::signed_in()]);
    let mut outside = Outside::new(&tree, &script)?;
    let left = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;
    let route = outside
        .conversation
        .session()
        .route()
        .cloned()
        .ok_or("a route")?;
    let mut picking = Picking::of([]);

    let ((outcome, _), _) =
        connecting_counted(&tree, &mut outside, TWO_ROUTED, Some("home"), &mut picking)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentConnected {
            said: Text::cut(&client::accounts::moved_onto("home")),
            onto: Some(Name::new("home")?),
        })
    );
    let moved = outside.conversation.session();
    assert_eq!(moved.route().map(RouteBinding::instance), Some("home"));
    assert!(
        moved.id().is_some_and(|moved| *moved != left),
        "no new session"
    );
    Session::reopen_routed(&tree.sessions(), &tree.workspace()?, &left, &route)?;
    Ok(())
}

/// A connect writes nothing while it asks: the file read as the signed-out
/// agent is said to be so holds what it held before, and the route and the
/// instance's enabling are there once the agent reads signed in.
#[test]
fn a_connect_writes_nothing_while_it_asks() -> Result<(), Failed> {
    let tree = Tree::new("external-connect-asking")?;
    let script = connected([claude::clean(), claude::signed_out(), claude::signed_in()]);
    let mut outside = Outside::new(&tree, &script)?;
    let mut picking = Picking::of([Some(client::accounts::CHECK_AGAIN)]);
    picking.reading = Some(crucible_config::user(&tree.home()?));

    let (_, file) = connecting(&tree, &mut outside, Some("work"), &mut picking)?;

    assert_eq!(picking.read, [UNCONNECTED]);
    assert_eq!(file, CONNECTED_FILE);
    Ok(())
}

/// A connect of the provider a conversation is asking moves it onto the
/// agent, in a new session bound to the route with nothing carried over, and
/// the session it left can be resumed with what it held.
#[test]
fn a_connect_moves_a_conversation_on_the_provider_s_model_onto_the_route() -> Result<(), Failed> {
    use crucible_client_api::LoginOutcome;

    let tree = Tree::new("external-connect-moves")?;
    let mut native = started_native(&tree, Script::new(vec![saying("first")]))?;
    let (first, _) = super::turn(&mut native, "one")?;
    let left = native.session().id().cloned().ok_or("an id")?;
    let script = connected([claude::signed_in(), claude::clean()]);
    let (mut conversation, peer) = stepped(&tree, native, &script)?;
    let mut picking = Picking::of([]);
    let command = |instance| Command::Connect {
        instance: Some(instance),
    };
    let asked = Asked {
        installed: (UNCONNECTED, "work"),
        command: &command,
        asking: Asking::every(),
        after: None,
    };

    let (outcome, file) = accounting_on(&tree, &mut conversation, &asked, &mut picking)?;
    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::AgentConnected {
            said: crucible_client_api::Text::cut(&client::accounts::moved_onto("work")),
            onto: Some(crucible_client_api::Name::new("work")?),
        })
    );
    let moved = conversation.session().id().cloned();
    let carried = users(conversation.settled());
    let (turned, _) = super::turn(&mut conversation, "after")?;

    assert!(matches!(first, Turned::Ran(_)), "{first:?}");
    assert_eq!(file, CONNECTED_FILE);
    let route = conversation.session().route().map(RouteBinding::instance);
    assert_eq!(route, Some("work"));
    assert!(moved.is_some_and(|moved| moved != left), "no new session");
    assert!(carried.is_empty(), "{carried:?}");
    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert_eq!(users(conversation.settled()), ["after"]);
    assert!(
        fake_peer::argued(&peer).len() > 2,
        "the agent never ran a turn"
    );
    let (_, kept) = Session::reopen(&tree.sessions(), &tree.workspace()?, &left)?;
    let kept: Vec<_> = kept
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect();
    assert_eq!(kept, ["one"]);
    Ok(())
}

/// A key logged in, after `/resume` moved a conversation on a provider's model
/// onto an agent that does not reach that provider, moves it back onto the
/// provider as any provider the route does not reach: how the provider is
/// reached is not rewritten.
#[test]
fn a_key_for_the_provider_a_resume_moved_off_moves_back_onto_it() -> Result<(), Failed> {
    use crucible_client_api::{LoginOutcome, Retained, Text};

    let tree = Tree::new("external-resume-then-key")?;
    let id = recorded_on(&tree, "work", "before")?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let (mut conversation, _peer) = stepped(&tree, native, &connected([claude::clean()]))?;
    let resumed = resuming_at(&tree, &mut conversation, INSTALLED, id)?;
    assert!(
        matches!(resumed, Outcome::Resumed(ResumeOutcome::Picked { .. })),
        "{resumed:?}"
    );
    super::Desk::new(&tree, &[])?
        .logins
        .keep("anthropic", "a-key-no-vendor-issued")?;

    let (outcome, file) = credited_on(&tree, &mut conversation, INSTALLED, "anthropic", login)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::MovedOff {
            provider: Text::cut("anthropic"),
            retained: Retained::default(),
            unwritten: None,
        })
    );
    let file: serde_json::Value = serde_json::from_str(&file)?;
    assert_eq!(file.pointer("/providers"), None);
    assert_eq!(
        file.pointer("/provider"),
        Some(&serde_json::json!("anthropic"))
    );
    assert!(conversation.session().route().is_none());
    Ok(())
}

/// Signed out at first, the client is told the command that signs the agent
/// in, and checking again once it reads signed in goes on to connect.
#[test]
fn a_connect_checks_again_until_the_agent_reads_signed_in() -> Result<(), Failed> {
    use crucible_client_api::LoginOutcome;

    let tree = Tree::new("external-connect-again")?;
    let script = connected([claude::clean(), claude::signed_out(), claude::signed_in()]);
    let mut outside = Outside::new(&tree, &script)?;
    let mut picking = Picking::of([Some(client::accounts::CHECK_AGAIN)]);

    let (outcome, file) = connecting(&tree, &mut outside, Some("work"), &mut picking)?;

    assert!(
        matches!(outcome, Outcome::Login(LoginOutcome::AgentConnected { .. })),
        "{outcome:?}"
    );
    assert_eq!(picking.put.len(), 1, "{:?}", picking.put);
    let told = match picking.put.first() {
        Some(Pending::Account { facts, .. }) => facts
            .iter()
            .any(|fact| fact.as_str().contains("claude auth login")),
        _ => false,
    };
    assert!(told, "{:?}", picking.put.first());
    assert_eq!(file, CONNECTED_FILE);
    Ok(())
}

/// A connect that writes nothing: its name, what the agent's status reads
/// say, what the client picks, and the first choice of each question put.
type Kept = (
    &'static str,
    Vec<Reply>,
    Vec<Option<&'static str>>,
    &'static [&'static str],
);

/// Cancelled signed out or unverified; checked again while it still cannot be
/// read; and a client that answers nothing: each is asked what its sign-in
/// reads as, and leaves the file byte for byte as it was.
#[test]
fn a_connect_not_carried_through_writes_nothing() -> Result<(), Failed> {
    use client::accounts::CHECK_AGAIN;
    use crucible_client_api::LoginOutcome;

    let cancel = Some("Cancel");
    let cases: [Kept; 4] = [
        (
            "signed-out",
            vec![claude::signed_out()],
            vec![cancel],
            &[CHECK_AGAIN],
        ),
        (
            "unverified",
            vec![Reply::new()],
            vec![cancel],
            &[CHECK_AGAIN],
        ),
        (
            "unverified-again",
            vec![Reply::new(), Reply::new()],
            vec![Some(CHECK_AGAIN), cancel],
            &[CHECK_AGAIN, CHECK_AGAIN],
        ),
        ("nobody", vec![claude::signed_out()], vec![], &[CHECK_AGAIN]),
    ];
    for (case, replies, choices, offered) in cases {
        let tree = Tree::new(&format!("external-connect-kept-{case}"))?;
        let script = connected(std::iter::once(claude::clean()).chain(replies));
        let mut outside = Outside::new(&tree, &script)?;
        let mut picking = Picking::of(choices);

        let (outcome, file) = connecting(&tree, &mut outside, Some("work"), &mut picking)?;

        assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided), "{case}");
        let first: Vec<&str> = picking
            .put
            .iter()
            .filter_map(|put| match put {
                Pending::Account { choices, .. } => {
                    choices.first().map(|choice| choice.name.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(first, offered, "{case}");
        assert_eq!(file, UNCONNECTED, "{case}");
    }
    Ok(())
}

/// A connect naming an instance the file does not name is refused before
/// anything is asked.
#[test]
fn a_connect_naming_no_known_instance_is_refused_unasked() -> Result<(), Failed> {
    use crucible_client_api::ErrorCode;

    let tree = Tree::new("external-connect-refused")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let mut picking = Picking::of([Some(client::accounts::CHECK_AGAIN)]);

    let (outcome, file) = connecting(&tree, &mut outside, Some("elsewhere"), &mut picking)?;

    assert_eq!(outcome, Outcome::Refused(ErrorCode::InvalidArgument.into()));
    assert!(picking.put.is_empty(), "{:?}", picking.put);
    assert_eq!(file, UNCONNECTED);
    Ok(())
}

/// With the agent's route not yet answered, a connect asks it first: declined,
/// the agent's program is not started and nothing is written; accepted where
/// the yes cannot be written down, the client is told so, and still nothing
/// is started or written.
#[test]
fn a_connect_asks_the_agent_s_route_before_starting_the_agent() -> Result<(), Failed> {
    use crucible_client_api::LoginOutcome;

    let unkept = crucible_client_api::Text::cut(client::accounts::HOLD_UNKEPT);
    for (case, choices, said) in [
        ("declined", vec![None], None),
        ("unkept", vec![Some("Accept")], Some(unkept)),
    ] {
        let tree = Tree::new(&format!("external-connect-held-{case}"))?;
        let script = connected([claude::clean(), claude::signed_in()]);
        let mut outside = Outside::new(&tree, &script)?;
        outside.unanswer();
        let mut picking = Picking::of(choices);

        let ((outcome, file), started) =
            connecting_counted(&tree, &mut outside, UNCONNECTED, Some("work"), &mut picking)?;

        match (&outcome, &said) {
            (Outcome::Login(LoginOutcome::Undecided), None) => {}
            (Outcome::Login(LoginOutcome::ConnectUnsaved { said: told, .. }), Some(said)) => {
                assert_eq!(told, said, "{case}");
            }
            _ => return Err(format!("{case}: {outcome:?}").into()),
        }
        assert!(
            matches!(picking.put.as_slice(), [Pending::Warning { .. }]),
            "{case}: {:?}",
            picking.put
        );
        assert_eq!(started, 0, "{case}: the agent's program was started");
        assert_eq!(file, UNCONNECTED, "{case}");
    }
    Ok(())
}

/// The user file holding no agent at all.
const UNINSTALLED: &str = r#"{
  "updates": {"check": "auto"}
}
"#;

/// The user file holding two instances of the one agent.
const TWICE: &str = r#"{
  "externalAgents": {
    "work": {"driver": "claude", "executable": "claude", "enabled": false},
    "home": {"driver": "claude", "executable": "claude", "enabled": false}
  }
}
"#;

/// The names of the choices each account question put offered, in order.
fn offered(put: &[Pending]) -> Vec<Vec<&str>> {
    put.iter()
        .filter_map(|put| match put {
            Pending::Account { choices, .. } => {
                Some(choices.iter().map(|choice| choice.name.as_str()).collect())
            }
            _ => None,
        })
        .collect()
}

/// A connect naming no instance: its name, what the user file holds first,
/// what the client picks, the instance it then goes through, and how many
/// questions were put.
type Picked = (
    &'static str,
    &'static str,
    Vec<Option<&'static str>>,
    &'static str,
    usize,
);

/// Naming no instance, a connect asks which agent first; then goes through
/// the one instance of it configured, the one picked of several, or, where
/// none is, one named after the agent, written with the route.
#[test]
fn a_connect_naming_no_instance_asks_the_agent_and_then_its_instance() -> Result<(), Failed> {
    use crucible_client_api::LoginOutcome;
    use crucible_config::Access;

    let agent = Some("Claude Code");
    let agents = vec!["Claude Code", "Antigravity", "Cancel"];
    let cases: [Picked; 3] = [
        ("one", UNCONNECTED, vec![agent], "work", 1),
        ("several", TWICE, vec![agent, Some("home")], "home", 2),
        ("none", UNINSTALLED, vec![agent], "claude", 1),
    ];
    for (case, before, choices, through, asked) in cases {
        let tree = Tree::new(&format!("external-connect-pick-{case}"))?;
        let script = connected([claude::clean(), claude::signed_in()]);
        let mut outside = Outside::new(&tree, &script)?;
        let mut picking = Picking::of(choices);

        let ((outcome, file), _) =
            connecting_counted(&tree, &mut outside, before, None, &mut picking)?;

        assert!(
            matches!(outcome, Outcome::Login(LoginOutcome::AgentConnected { .. })),
            "{case}: {outcome:?}"
        );
        let put = offered(&picking.put);
        assert_eq!(put.len(), asked, "{case}: {put:?}");
        assert_eq!(put.first(), Some(&agents), "{case}");
        let settings = super::written(&tree)?;
        assert_eq!(
            settings.access("anthropic"),
            Some(Access::ExternalAgent {
                instance: through.into()
            }),
            "{case}: {file}"
        );
        let enabled: Vec<(String, bool)> = settings
            .external_agents()
            .iter()
            .map(|agent| (agent.name().to_owned(), agent.enabled()))
            .collect();
        let others = enabled
            .iter()
            .filter(|(name, on)| (name == through) != *on)
            .count();
        assert_eq!(others, 0, "{case}: {enabled:?}");
    }
    Ok(())
}

/// An agent whose program is not installed is said to be so, with the
/// command that installs the release crucible runs it as; looked for again it is still not there; going back offers the
/// agents again; and cancelling starts nothing and writes nothing.
#[test]
fn a_connect_to_an_agent_not_installed_says_so_and_writes_nothing() -> Result<(), Failed> {
    use client::accounts::CHECK_AGAIN;
    use crucible_client_api::LoginOutcome;

    let tree = Tree::new("external-connect-missing")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let mut picking = Picking::of([
        Some("Antigravity"),
        Some(CHECK_AGAIN),
        Some("Go back"),
        Some("Cancel"),
    ]);

    let ((outcome, file), started) =
        connecting_counted(&tree, &mut outside, UNINSTALLED, None, &mut picking)?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    let put = offered(&picking.put);
    let missing = vec![CHECK_AGAIN, "Go back"];
    let agents = vec!["Claude Code", "Antigravity", "Cancel"];
    assert_eq!(put, [agents.clone(), missing.clone(), missing, agents]);
    let told = picking.put.get(1).is_some_and(|put| match put {
        Pending::Account { asks, facts, .. } => {
            asks.as_str() == "Antigravity cannot be started."
                && facts
                    .first()
                    .is_some_and(|fact| fact.as_str().starts_with("Not installed"))
                && facts.iter().any(|fact| {
                    fact.as_str()
                        == "Run `crucible agents install antigravity` in a terminal, \
                            then choose Check again."
                })
                && facts.iter().any(|fact| {
                    let sep = std::path::MAIN_SEPARATOR;
                    fact.as_str().starts_with("Agent: antigravity, run as ")
                        && fact
                            .as_str()
                            .contains(&format!("{sep}programs{sep}antigravity{sep}1.3.0{sep}"))
                })
        }
        _ => false,
    });
    assert!(told, "{:?}", picking.put.get(1));
    assert_eq!(started, 0, "the agent's program was started");
    assert_eq!(file, UNINSTALLED);
    Ok(())
}

/// An agent installed by hand, found on the `PATH`, is run from there and not
/// from the release crucible would install: a connect naming no instance does
/// not say it cannot be started, and cancelling writes nothing.
#[cfg(unix)]
#[test]
fn a_connect_to_an_agent_found_on_path_does_not_ask_for_an_install() -> Result<(), Failed> {
    use crucible_client_api::LoginOutcome;
    use std::os::unix::fs::PermissionsExt as _;

    let tree = Tree::new("external-connect-on-path")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let program = outside.peer.join("antigravity");
    std::fs::write(&program, "")?;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))?;
    let mut picking = Picking::of([Some("Antigravity"), None]);

    let ((outcome, file), started) =
        connecting_counted(&tree, &mut outside, UNINSTALLED, None, &mut picking)?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    let refused = picking.put.get(1).is_some_and(|put| match put {
        Pending::Account { asks, .. } => asks.as_str() == "Antigravity cannot be started.",
        _ => false,
    });
    assert!(!refused, "{:?}", picking.put.get(1));
    assert!(picking.put.get(1).is_some(), "{:?}", picking.put);
    assert_eq!(started, 0, "the agent's program was started");
    assert_eq!(file, UNINSTALLED);
    Ok(())
}

/// A connect to an agent whose program is there but cannot be run says so,
/// with what to do, may look again or go back, and starts and writes nothing.
#[cfg(unix)]
#[test]
fn a_connect_to_an_agent_whose_program_cannot_run_says_so_and_writes_nothing() -> Result<(), Failed>
{
    use client::accounts::CHECK_AGAIN;
    use crucible_client_api::LoginOutcome;
    use std::os::unix::fs::PermissionsExt as _;

    let tree = Tree::new("external-connect-not-executable")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    let program = tree.0.join("claude");
    std::fs::write(&program, "")?;
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o644))?;
    let installed = format!(
        r#"{{"externalAgents": {{"work": {{"driver": "claude", "executable": {}}}}}}}"#,
        serde_json::Value::from(program.to_string_lossy()),
    );
    let mut picking = Picking::of([
        Some("Claude Code"),
        Some(CHECK_AGAIN),
        Some("Go back"),
        Some("Cancel"),
    ]);

    let ((outcome, file), started) =
        connecting_counted(&tree, &mut outside, &installed, None, &mut picking)?;

    assert_eq!(outcome, Outcome::Login(LoginOutcome::Undecided));
    let put = offered(&picking.put);
    let refused = vec![CHECK_AGAIN, "Go back"];
    let agents = vec!["Claude Code", "Antigravity", "Cancel"];
    assert_eq!(put, [agents.clone(), refused.clone(), refused, agents]);
    let said = format!("Found {} but it is not executable.", program.display());
    let told = picking.put.get(1).is_some_and(|put| match put {
        Pending::Account { asks, facts, .. } => {
            asks.as_str() == "Claude Code cannot be started."
                && facts.first().is_some_and(|fact| fact.as_str() == said)
                && facts
                    .iter()
                    .any(|fact| fact.as_str() == "Make it executable, then choose Check again.")
        }
        _ => false,
    });
    assert!(told, "{:?}", picking.put.get(1));
    assert_eq!(started, 0, "the agent's program was started");
    assert_eq!(file, installed);
    Ok(())
}

/// Naming no instance where no agent this build knows ships is refused
/// before anything is asked.
#[cfg(feature = "proof")]
#[test]
fn a_connect_naming_no_instance_with_no_agent_shipping_is_refused_unasked() -> Result<(), Failed> {
    use crucible_app::proof;
    use crucible_client_api::ErrorCode;

    let tree = Tree::new("external-connect-none-shipping")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean()]))?;
    outside.conversation = proof::agents(outside.conversation, proof::registry(&[]));
    let mut picking = Picking::of([Some("Claude Code")]);

    let ((outcome, file), _) =
        connecting_counted(&tree, &mut outside, UNINSTALLED, None, &mut picking)?;

    assert_eq!(outcome, Outcome::Refused(ErrorCode::InvalidArgument.into()));
    assert!(picking.put.is_empty(), "{:?}", picking.put);
    assert_eq!(file, UNINSTALLED);
    Ok(())
}

/// The user file of a conversation on `work`: the provider reached through it.
const ROUTED: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}},
  "providers": {"anthropic": {"access": {"type": "externalAgent", "instance": "work"}}}
}
"#;

/// What [`ROUTED`] holds once a key for the provider is logged in on the route:
/// the way it is reached, and its default model and rung.
const KEYED: &str = r#"{
  "externalAgents": {"work": {"driver": "claude", "executable": "claude"}},
  "providers": {"anthropic": {"access": {"type":"apiKey"}, "model": "claude-opus-5-5", "effort": "medium"}},
  "provider": "anthropic"
}
"#;

/// Stores a key for `provider` and asks `command` of the conversation on the
/// route, the user file holding `file` first; and answers what the client was
/// told and what the user file then held.
fn crediting(
    tree: &Tree,
    outside: &mut Outside,
    file: &str,
    provider: &str,
    command: fn(crucible_client_api::Name) -> Command,
) -> Result<(Outcome, String), Failed> {
    super::Desk::new(tree, &[])?
        .logins
        .keep(provider, "a-key-no-vendor-issued")?;
    credited(tree, outside, file, provider, command)
}

/// [`crediting`], with whatever is already stored for `provider`.
fn credited(
    tree: &Tree,
    outside: &mut Outside,
    file: &str,
    provider: &str,
    command: fn(crucible_client_api::Name) -> Command,
) -> Result<(Outcome, String), Failed> {
    credited_on(tree, &mut outside.conversation, file, provider, command)
}

/// What [`credited`] does, of any conversation.
fn credited_on(
    tree: &Tree,
    conversation: &mut Conversation,
    file: &str,
    provider: &str,
    command: fn(crucible_client_api::Name) -> Command,
) -> Result<(Outcome, String), Failed> {
    let desk = super::Desk::stored(tree)?;
    credited_at(desk, tree, conversation, (file, provider), command)
}

/// What [`credited_on`] does, at `desk`.
fn credited_at(
    mut desk: super::Desk,
    tree: &Tree,
    conversation: &mut Conversation,
    (file, provider): (&str, &str),
    command: fn(crucible_client_api::Name) -> Command,
) -> Result<(Outcome, String), Failed> {
    if let Some(directory) = desk.choosing.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(&desk.choosing, file)?;
    desk.settings = super::written(tree)?;
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
        Asking::every(),
        Correlation::new(1),
        command(crucible_client_api::Name::new(provider)?),
    );
    let performed = runtime()?.block_on(client::perform(conversation, &request, &at));
    let outcome = performed.response(&request).outcome;
    Ok((outcome, std::fs::read_to_string(&desk.choosing)?))
}

fn login(provider: crucible_client_api::Name) -> Command {
    Command::Login { provider }
}

fn logout(provider: crucible_client_api::Name) -> Command {
    Command::Logout { provider }
}

/// A key logged in for the provider the route reaches is written down as how
/// it is reached, and the conversation moves off the route onto it at once,
/// in a new session, as a login for a provider the route does not reach does.
#[test]
fn a_login_for_the_route_s_provider_moves_onto_it_directly() -> Result<(), Failed> {
    use crucible_client_api::{Execution, LoginOutcome, Name, Retained, Text};

    let tree = Tree::new("external-login-routed")?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    let left = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;

    let (outcome, file) = crediting(&tree, &mut outside, ROUTED, "anthropic", login)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::MovedOff {
            provider: Text::cut("anthropic"),
            retained: Retained::default(),
            unwritten: None,
        })
    );
    assert_eq!(file, KEYED);
    assert!(
        super::Desk::new(&tree, &[])?
            .logins
            .read()
            .held("anthropic")
            .is_some(),
        "the key logged in is no longer stored"
    );
    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.execution, Execution::Native);
    assert_eq!(standing.provider, Some(Name::new("anthropic")?));
    assert_eq!(
        standing.model,
        crucible_client_api::Model::new("claude-opus-5-5")
    );
    let moved = outside.conversation.session();
    assert!(moved.route().is_none(), "{:?}", moved.route());
    assert!(
        moved.id().is_some_and(|moved| *moved != left),
        "no new session"
    );
    Ok(())
}

/// A connect after a login moved the conversation off the route is how the
/// provider is read for the rest of the run: the access the login rewrote no
/// longer stands in for the route the connect wrote back.
#[test]
fn a_connect_after_a_login_moved_off_the_route_stands_over_the_login() -> Result<(), Failed> {
    use crucible_app::access::Rewritten;
    use crucible_client_api::LoginOutcome;
    use crucible_config::Access;

    let tree = Tree::new("external-login-then-connect")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::signed_in()]))?;
    let rewritten = Rewritten::new();
    let shared = |desk: super::Desk| super::Desk {
        rewritten: rewritten.clone(),
        ..desk
    };
    let home = tree.home()?;
    std::fs::create_dir_all(home.path())?;
    std::fs::write(crucible_config::user(&home), ROUTED)?;
    let launch = super::written(&tree)?;
    let desk = shared(super::Desk::stored(&tree)?);
    desk.logins.keep("anthropic", "a-key-no-vendor-issued")?;

    let (logged, _) = credited_at(
        desk,
        &tree,
        &mut outside.conversation,
        (ROUTED, "anthropic"),
        login,
    )?;
    assert!(
        matches!(logged, Outcome::Login(LoginOutcome::MovedOff { .. })),
        "{logged:?}"
    );
    assert_eq!(
        rewritten.over(&launch).access("anthropic"),
        Some(Access::ApiKey)
    );

    let command = |instance| Command::Connect {
        instance: Some(instance),
    };
    let asked = Asked {
        installed: (KEYED, "work"),
        command: &command,
        asking: Asking::every(),
        after: None,
    };
    let (connected, file) = accounting_at(
        shared(super::Desk::new(&tree, &[])?),
        &tree,
        &mut outside.conversation,
        &asked,
        &mut Picking::of([]),
    )?;
    assert!(
        matches!(
            connected,
            Outcome::Login(LoginOutcome::AgentConnected { .. })
        ),
        "{connected:?}"
    );
    let file: serde_json::Value = serde_json::from_str(&file)?;
    assert_eq!(
        file.pointer("/providers/anthropic/access/type"),
        Some(&serde_json::json!("externalAgent"))
    );
    assert_eq!(
        rewritten.over(&launch).access("anthropic"),
        Some(Access::ExternalAgent {
            instance: "work".into()
        })
    );
    Ok(())
}

/// An account signed in for the provider the route reaches is written down as
/// reached by its subscription, and the conversation moves off the route onto
/// it at once. OpenAI's, since its accounts are ones this program can sign in.
#[test]
fn a_subscription_for_the_route_s_provider_moves_onto_it_directly() -> Result<(), Failed> {
    use crucible_client_api::{Execution, LoginOutcome, Name, Retained, Text};

    let tree = Tree::new("external-login-subscribed")?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    let home = tree.home()?;
    std::fs::create_dir_all(home.path())?;
    std::fs::write(
        home.path().join("auth.json"),
        concat!(
            r#"{"version":2,"keys":{},"subscriptions":{"openai":{"#,
            r#""access_token":"test-access","refresh_token":"test-refresh","#,
            r#""details":{"account_id":"test-account"},"#,
            r#""expires_at":18446744073709551615,"refreshed_at":1}},"identities":{}}"#,
        ),
    )?;

    let routed = ROUTED.replace("anthropic", "openai");

    let (outcome, file) = credited(&tree, &mut outside, &routed, "openai", login)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::MovedOff {
            provider: Text::cut("openai"),
            retained: Retained::default(),
            unwritten: None,
        })
    );
    let subscribed = KEYED
        .replace("anthropic", "openai")
        .replace("claude-opus-5-5", "gpt-6.1-sol")
        .replace(r#""apiKey""#, r#""subscription""#);
    assert_eq!(file, subscribed);
    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.execution, Execution::Native);
    assert_eq!(standing.provider, Some(Name::new("openai")?));
    Ok(())
}

/// Where the user file cannot say so, it is left byte for byte as it was, the
/// client is told why, and the conversation stays on the route.
#[test]
fn a_login_whose_write_fails_leaves_the_route_written_as_it_was() -> Result<(), Failed> {
    use crucible_client_api::{Execution, LoginOutcome, Name};

    // The providers written twice, so the block the file means is not one a
    // scan of the text can be sure of.
    let spelled = ROUTED.replace(r#""providers": {"#, r#""providers": {}, "providers": {"#);
    let tree = Tree::new("external-login-unwritten")?;
    let mut outside = Outside::new(&tree, &connected([]))?;

    let (outcome, file) = crediting(&tree, &mut outside, &spelled, "anthropic", login)?;

    assert!(
        matches!(
            outcome,
            Outcome::Login(LoginOutcome::NextLaunch {
                unwritten: Some(_),
                ..
            })
        ),
        "{outcome:?}"
    );
    assert_eq!(file, spelled);
    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.execution, Execution::Instance(Name::new("work")?));
    Ok(())
}

/// A key logged in on the route for a provider the route does not reach moves
/// the conversation off the agent onto that provider, in a new session, and
/// writes it down as the provider asked; the session left can be resumed.
#[test]
fn a_login_for_another_provider_on_the_route_moves_onto_it() -> Result<(), Failed> {
    use crucible_client_api::{Execution, LoginOutcome, Name, Retained, Text};

    let tree = Tree::new("external-login-moved-off")?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    let left = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;
    let route = outside
        .conversation
        .session()
        .route()
        .cloned()
        .ok_or("a route")?;

    let (outcome, file) = crediting(&tree, &mut outside, ROUTED, "openai", login)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::MovedOff {
            provider: Text::cut("openai"),
            retained: Retained::default(),
            unwritten: None,
        })
    );
    assert_eq!(
        file,
        ROUTED.replace(
            "}}}\n}",
            "}}, \"openai\": {\"model\": \"gpt-6.1-sol\", \"effort\": \"medium\"}},\n  \"provider\": \"openai\"\n}"
        )
    );
    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.execution, Execution::Native);
    assert_eq!(standing.provider, Some(Name::new("openai")?));
    assert_eq!(
        standing.model,
        crucible_client_api::Model::new("gpt-6.1-sol")
    );
    assert_eq!(
        outside.conversation.settled().effort(),
        Some(crucible_models::Effort::Medium)
    );
    let moved = outside.conversation.session();
    assert!(moved.route().is_none(), "{:?}", moved.route());
    assert!(
        moved.id().is_some_and(|moved| *moved != left),
        "no new session"
    );
    Session::reopen_routed(&tree.sessions(), &tree.workspace()?, &left, &route)?;
    Ok(())
}

/// The user file of a conversation on `work` with a second provider reached
/// through it, and a third through another instance.
const BESIDE: &str = r#"{
  "externalAgents": {
    "work": {"driver": "claude", "executable": "claude"},
    "home": {"driver": "claude", "executable": "claude"}
  },
  "providers": {
    "anthropic": {"access": {"type": "externalAgent", "instance": "work"}},
    "openai": {"access": {"type": "externalAgent", "instance": "work"}},
    "deepseek": {"access": {"type": "externalAgent", "instance": "home"}}
  }
}
"#;

/// A key for a second provider the route reaches is written down as the way
/// it is reached, and moves the conversation onto it, as the first one's does.
#[test]
fn a_login_for_a_second_provider_the_route_reaches_moves_onto_it() -> Result<(), Failed> {
    use crucible_client_api::{LoginOutcome, Retained, Text};

    let tree = Tree::new("external-login-second-routed")?;
    let mut outside = Outside::new(&tree, &connected([]))?;

    let (outcome, file) = crediting(&tree, &mut outside, BESIDE, "openai", login)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::MovedOff {
            provider: Text::cut("openai"),
            retained: Retained::default(),
            unwritten: None,
        })
    );
    let file: serde_json::Value = serde_json::from_str(&file)?;
    assert_eq!(
        file.pointer("/providers/openai/access/type"),
        Some(&serde_json::json!("apiKey"))
    );
    assert!(outside.conversation.session().route().is_none());
    Ok(())
}

/// A key for a provider another instance reaches is only stored: this run's
/// settings route it through that agent, so the conversation does not move.
#[test]
fn a_login_for_a_provider_another_agent_reaches_is_only_stored() -> Result<(), Failed> {
    use crucible_client_api::{LoginOutcome, Text};

    let tree = Tree::new("external-login-other-routed")?;
    let mut outside = Outside::new(&tree, &connected([]))?;

    let (outcome, file) = crediting(&tree, &mut outside, BESIDE, "deepseek", login)?;

    assert_eq!(
        outcome,
        Outcome::Login(LoginOutcome::Stored {
            provider: Text::cut("deepseek"),
        })
    );
    assert_eq!(file, BESIDE);
    assert!(outside.conversation.session().route().is_some());
    Ok(())
}

/// The permission mode the conversation was in on the route is the one it is
/// in once a login moves it off.
#[test]
fn a_move_off_the_route_keeps_the_conversation_s_mode() -> Result<(), Failed> {
    let tree = Tree::new("external-login-moved-off-mode")?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    assert_eq!(
        outside.conversation.switch(Mode::FullAccess),
        Mode::FullAccess
    );

    let _ = crediting(&tree, &mut outside, ROUTED, "openai", login)?;

    assert_eq!(outside.conversation.settled().mode(), Mode::FullAccess);
    assert!(outside.conversation.session().route().is_none());
    Ok(())
}

/// A key for a provider the route does not reach, which cannot be set up, is
/// said to be unusable, and the conversation stays on the route.
#[test]
fn an_unusable_login_for_another_provider_leaves_the_route() -> Result<(), Failed> {
    use crucible_client_api::{Execution, LoginOutcome, Name};

    let tree = Tree::new("external-login-unusable-off")?;
    let mut outside = Outside::new(&tree, &connected([]))?;

    // Nothing stored for it, so the desk cannot set it up.
    let (outcome, file) = credited(&tree, &mut outside, ROUTED, "openai", login)?;

    assert!(
        matches!(outcome, Outcome::Login(LoginOutcome::Unusable(_))),
        "{outcome:?}"
    );
    assert_eq!(file, ROUTED);
    let standing = client::snapshot(&outside.conversation);
    assert_eq!(standing.execution, Execution::Instance(Name::new("work")?));
    Ok(())
}

/// The stored key of the provider the route reaches is forgotten, and the
/// client is told the installed agent still reaches it.
#[test]
fn a_logout_of_the_route_s_provider_says_the_agent_still_reaches_it() -> Result<(), Failed> {
    use crucible_client_api::{LogoutOutcome, Retained, Standing, Text};

    let tree = Tree::new("external-logout-routed")?;
    let mut outside = Outside::new(&tree, &connected([]))?;

    let (outcome, file) = crediting(&tree, &mut outside, ROUTED, "anthropic", logout)?;

    assert_eq!(
        outcome,
        Outcome::Logout(LogoutOutcome::StillServed {
            retained: Retained::default(),
            standing: Standing::External {
                instance: Text::cut("work"),
            },
        })
    );
    assert_eq!(file, ROUTED);
    assert_eq!(
        super::Desk::new(&tree, &[])?
            .logins
            .read()
            .held("anthropic"),
        None
    );
    Ok(())
}

/// The user file of a run whose sessions an agent ran under two entries.
const TWO_ENTRIES: &str = r#"{
  "externalAgents": {
    "work": {"driver": "claude", "executable": "claude"},
    "home": {"driver": "claude", "executable": "claude"}
  }
}
"#;

/// What the agent called each session [`recorded_on`] keeps.
const EARLIER: &str = "earlier";

/// A session Claude Code ran under the entry `instance`, holding `said`, and
/// what the agent called it, [`EARLIER`].
fn recorded_on(tree: &Tree, instance: &str, said: &str) -> Result<SessionId, Failed> {
    let route = RouteBinding::new("claude", instance, None)?;
    let session = Session::start_routed(&tree.sessions(), &tree.workspace()?, None, &route)?;
    let id = session.id().cloned().ok_or("an id")?;
    session.append(&Message::User {
        text: said.into(),
        attachments: Box::new([]),
    });
    assert!(session.external(&ExternalRef::new(instance, EARLIER)?));
    drop(session);
    Ok(id)
}

/// `conversation`, able to start Claude Code's runs as the application's is,
/// from the peer playing `script` once a connect's probe and status check have
/// been played; and where the peer keeps what it heard.
fn stepped(
    tree: &Tree,
    conversation: Conversation,
    script: &Peer,
) -> Result<(Conversation, PathBuf), Failed> {
    let (launching, peer) = launching(tree, script)?;
    let conversation = conversation
        .consenting(launching.consent.clone())
        .stepping(steps(&launching, &peer));
    Ok((conversation, peer))
}

/// A conversation that started on a provider's model, asking `script`.
fn started_native(tree: &Tree, script: Script) -> Result<Conversation, Failed> {
    super::conversing(tree, script, &super::Guard::Nothing, Some("anthropic"))
}

/// Asks `/resume id` of `conversation`, the user file holding `file`, and
/// answers what the client was told.
fn resuming_at(
    tree: &Tree,
    conversation: &mut Conversation,
    file: &str,
    id: SessionId,
) -> Result<Outcome, Failed> {
    let mut desk = super::Desk::new(tree, &[])?;
    if let Some(directory) = desk.choosing.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(&desk.choosing, file)?;
    desk.settings = super::written(tree)?;
    let (workspace, sessions) = (tree.workspace()?, tree.sessions());
    let at = client::Desk {
        switching: desk.with(),
        sessions: &sessions,
        workspace: &workspace,
        reads: |_| false,
        environment: super::unset,
        notes: |_| Err(crucible_client_api::ErrorCode::InvalidArgument.into()),
    };
    let request = Request::new(Asking::every(), Correlation::new(1), Command::Resume(id));
    let performed = runtime()?.block_on(client::perform(conversation, &request, &at));
    Ok(performed.response(&request).outcome)
}

/// What the user said in `settled`'s transcript, in order.
fn users(settled: Settled<'_>) -> Vec<String> {
    settled
        .transcript()
        .messages()
        .iter()
        .filter_map(|message| match message {
            Message::User { text, .. } => Some(text.to_string()),
            _ => None,
        })
        .collect()
}

/// Whether the last start of the peer kept in `peer` resumed the agent's
/// session called `reference`.
fn resumed_as(peer: &std::path::Path, reference: &str) -> bool {
    fake_peer::argued(peer)
        .last()
        .is_some_and(|start| start.windows(2).any(|pair| pair == ["--resume", reference]))
}

/// `/resume` of a session an agent ran, from a conversation on a provider's
/// model, goes on on that agent: its history shown, and the agent asked for
/// its own session by the name the log kept.
#[test]
fn a_session_an_agent_ran_resumes_on_its_route_from_a_provider_s_model() -> Result<(), Failed> {
    let tree = Tree::new("external-resume-from-native")?;
    let id = recorded_on(&tree, "work", "before")?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let (mut conversation, peer) = stepped(&tree, native, &connected([claude::clean()]))?;

    let outcome = resuming_at(&tree, &mut conversation, INSTALLED, id.clone())?;
    let (turned, _) = super::turn(&mut conversation, "after")?;

    assert!(
        matches!(outcome, Outcome::Resumed(ResumeOutcome::Picked { .. })),
        "{outcome:?}"
    );
    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert_eq!(conversation.session().id(), Some(&id));
    assert!(resumed_as(&peer, EARLIER), "{:?}", fake_peer::argued(&peer));
    assert_eq!(users(conversation.settled()), ["before", "after"]);
    // Claimed once, by the conversation now recording into it.
    let held = Session::reopen_routed(&tree.sessions(), &tree.workspace()?, &id, &binding()?);
    assert!(held.is_err(), "the session was left unclaimed");
    Ok(())
}

/// A prompt that would start an agent whose route has no yes puts the route
/// to the client before the agent is started: a yes is written down and the
/// turn goes on the agent; a no starts nothing and sends nothing.
#[test]
fn a_prompt_that_would_start_an_unanswered_agent_asks_its_route_first() -> Result<(), Failed> {
    for (case, choice) in [("accepted", Some("Accept")), ("declined", None)] {
        let tree = Tree::new(&format!("external-launch-asked-{case}"))?;
        let id = recorded_on(&tree, "work", "before")?;
        let native = started_native(&tree, Script::new(Vec::new()))?;
        let (mut conversation, peer) = stepped(&tree, native, &connected([claude::clean()]))?;
        let consent = conversation.consent().ok_or("a consent")?.clone();
        let choosing = super::Desk::new(&tree, &[])?.choosing;
        consent.keeps_in(choosing.clone());
        consent.withdraw("agent:claude");
        let outcome = resuming_at(&tree, &mut conversation, INSTALLED, id)?;
        assert!(
            matches!(outcome, Outcome::Resumed(ResumeOutcome::Picked { .. })),
            "{case}: {outcome:?}"
        );
        let started = fake_peer::argued(&peer).len();

        let request = Request::new(
            Asking::every(),
            Correlation::new(1),
            Command::Prompt(Prompt::new("after")?),
        );
        let mut picking = Picking::of([choice]);
        let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
        let quiet = Watching {
            events: mpsc::channel().0,
            stopping: None,
        };
        let ended = {
            let run = conversation.starting(&quiet, &cancel, &steer, &aside);
            runtime()?.block_on(client::turn(
                &mut conversation,
                &request,
                Box::default(),
                &mut picking,
                &run,
            ))
        };

        assert!(
            matches!(
                picking.put.as_slice(),
                [Pending::Warning { route, sentence, .. }]
                    if route.as_str() == "agent:claude"
                        && sentence.as_str().ends_with(LAUNCHING)
            ),
            "{case}: the client is not told what starting the agent runs: {:?}",
            picking.put
        );
        let written = std::fs::read_to_string(&choosing)?;
        match (choice, ended.response(&request).outcome) {
            (Some(_), Outcome::Turn(TurnOutcome::Ran { .. })) => {
                assert!(written.contains("agent:claude"), "{written}");
                assert!(resumed_as(&peer, EARLIER), "{:?}", fake_peer::argued(&peer));
                assert_eq!(users(conversation.settled()), ["before", "after"]);
            }
            (None, Outcome::Turn(TurnOutcome::Warned { route, .. })) => {
                assert_eq!(route.as_str(), "agent:claude");
                assert!(!written.contains("agent:claude"), "{written}");
                assert_eq!(
                    fake_peer::argued(&peer).len(),
                    started,
                    "the agent was started"
                );
                assert_eq!(users(conversation.settled()), ["before"]);
            }
            other => return Err(format!("{case}: {other:?}").into()),
        }
    }
    Ok(())
}

/// `/resume` of a session an agent ran under another entry moves the
/// conversation off the route it is on and onto that one.
#[test]
fn a_session_an_agent_ran_resumes_on_its_route_from_another_route() -> Result<(), Failed> {
    let tree = Tree::new("external-resume-from-route")?;
    let id = recorded_on(&tree, "home", "before")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;

    let outcome = resuming_at(&tree, &mut outside.conversation, TWO_ENTRIES, id.clone())?;
    let (turned, _) = turn(&mut outside, "after", false)?;

    assert!(
        matches!(outcome, Outcome::Resumed(ResumeOutcome::Picked { .. })),
        "{outcome:?}"
    );
    assert!(turned.is_ok(), "{turned:?}");
    let session = outside.conversation.session();
    assert_eq!(session.id(), Some(&id));
    assert_eq!(session.route().map(RouteBinding::instance), Some("home"));
    assert!(
        resumed_as(&outside.peer, EARLIER),
        "{:?}",
        fake_peer::argued(&outside.peer)
    );
    assert_eq!(users(outside.settled()), ["before", "after"]);
    let home = RouteBinding::new("claude", "home", None)?;
    let held = Session::reopen_routed(&tree.sessions(), &tree.workspace()?, &id, &home);
    assert!(held.is_err(), "the session was left unclaimed");
    Ok(())
}

/// `/resume` of another session the same agent ran, from a conversation on
/// its route, goes on on that route: its history shown, the agent asked for
/// its own session by the name the log kept, and the log claimed once.
#[test]
fn a_session_an_agent_ran_resumes_on_its_route_from_that_route() -> Result<(), Failed> {
    let tree = Tree::new("external-resume-same-route")?;
    let id = recorded_on(&tree, "work", "before")?;
    let mut outside = Outside::new(&tree, &connected([claude::clean(), claude::clean()]))?;
    let (first, _) = turn(&mut outside, "elsewhere", false)?;
    assert!(first.is_ok(), "{first:?}");

    let outcome = resuming_at(&tree, &mut outside.conversation, INSTALLED, id.clone())?;
    let (turned, _) = turn(&mut outside, "after", false)?;

    assert!(
        matches!(outcome, Outcome::Resumed(ResumeOutcome::Picked { .. })),
        "{outcome:?}"
    );
    assert!(turned.is_ok(), "{turned:?}");
    assert_eq!(outside.conversation.session().id(), Some(&id));
    assert!(
        resumed_as(&outside.peer, EARLIER),
        "{:?}",
        fake_peer::argued(&outside.peer)
    );
    assert_eq!(users(outside.settled()), ["before", "after"]);
    let held = Session::reopen_routed(&tree.sessions(), &tree.workspace()?, &id, &binding()?);
    assert!(held.is_err(), "the session was left unclaimed");
    Ok(())
}

/// `/resume` of a session a provider's model ran, on a route the run moved
/// onto from a provider's model, goes back to the runner it started on, which
/// still keeps its prompt caches where it was told to, shows the session's
/// history, and is sent its earlier turns.
#[test]
fn a_session_a_provider_ran_resumes_on_the_kept_runner_of_a_run_that_started_native()
-> Result<(), Failed> {
    let tree = Tree::new("external-resume-inside")?;
    let script = Script::new(vec![saying("first"), saying("back")]);
    let sent = Arc::clone(&script.sent);
    let home = tree.home()?;
    let mut native = started_native(&tree, script)?.remembering_caches_in(home.path());
    let id = native.session().id().cloned().ok_or("an id")?;
    let (first, _) = super::turn(&mut native, "one")?;
    assert!(matches!(first, Turned::Ran(_)), "{first:?}");
    let routed = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);
    let (mut outside, left) =
        Outside::onto(&tree, &connected([]), native, routed, Transcript::new())?;
    drop(left);

    let outcome = resuming_at(&tree, &mut outside.conversation, INSTALLED, id.clone())?;
    assert_eq!(users(outside.settled()), ["one"]);
    // Records nobody can read: a runner that still keeps its caches there
    // reads them and says so, where one that forgot reads nothing.
    let caches = home.path().join("prompt-cache");
    std::fs::create_dir_all(&caches)?;
    std::fs::write(caches.join("resources-v1.json"), "not records")?;
    let listed = runtime()?.block_on(outside.conversation.prompt_cache_resources());
    let (turned, said) = super::turn(&mut outside.conversation, "two")?;

    assert!(
        matches!(outcome, Outcome::Resumed(ResumeOutcome::Picked { .. })),
        "{outcome:?}"
    );
    assert!(listed.is_err(), "the runner forgot its caches: {listed:?}");
    assert!(matches!(turned, Turned::Ran(_)), "{turned:?}");
    assert_eq!(said, "back");
    assert_eq!(outside.conversation.session().id(), Some(&id));
    let sent = sent.lock().map_err(|_| "the record")?.clone();
    assert_eq!(sent.last(), Some(&vec!["one".to_owned(), "two".to_owned()]));
    Ok(())
}

/// `/resume` of a session a provider's model ran, on a run that started on an
/// agent's route, says it goes on at the next launch, and opens nothing.
#[test]
fn a_session_a_provider_ran_waits_for_the_next_launch_on_a_run_that_started_on_a_route()
-> Result<(), Failed> {
    let tree = Tree::new("external-resume-next-launch")?;
    let workspace = tree.workspace()?;
    let native = Session::start(&tree.sessions(), &workspace, None)?;
    let id = native.id().cloned().ok_or("an id")?;
    native.append(&Message::User {
        text: "before".into(),
        attachments: Box::new([]),
    });
    let path = native.path().to_owned();
    drop(native);
    let before = std::fs::read(&path)?;
    let mut outside = Outside::new(&tree, &connected([]))?;
    let current = outside.conversation.session().id().cloned();

    let outcome = resuming_at(&tree, &mut outside.conversation, INSTALLED, id.clone())?;

    assert_eq!(outcome, Outcome::Resumed(ResumeOutcome::NextLaunch));
    assert_eq!(outside.conversation.session().id(), current.as_ref());
    assert_eq!(std::fs::read(&path)?, before);
    let reopened = Session::reopen(&tree.sessions(), &workspace, &id);
    assert!(reopened.is_ok(), "{:?}", reopened.err());
    Ok(())
}

/// `/resume` of a session an agent ran under an entry the configuration no
/// longer has, or has disconnected, is refused saying so, and nothing is
/// started or opened.
#[test]
fn a_session_whose_agent_is_gone_or_disconnected_is_refused_and_nothing_starts()
-> Result<(), Failed> {
    const DISCONNECTED: &str = r#"{
  "externalAgents": {"home": {"driver": "claude", "executable": "claude", "enabled": false}}
}
"#;
    let tree = Tree::new("external-resume-gone")?;
    let id = recorded_on(&tree, "home", "before")?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let current = native.session().id().cloned();
    let (mut conversation, peer) = stepped(&tree, native, &connected([]))?;
    let started = fake_peer::played(&peer);

    for (file, says) in [
        (INSTALLED, "run by the installed agent home"),
        (DISCONNECTED, "home cannot be started"),
    ] {
        let outcome = resuming_at(&tree, &mut conversation, file, id.clone())?;
        let told = match &outcome {
            Outcome::Resumed(ResumeOutcome::Failed(problem)) => problem.message.as_str().to_owned(),
            other => format!("{other:?}"),
        };
        assert!(told.contains(says), "{told}");
    }

    assert_eq!(conversation.session().id(), current.as_ref());
    assert_eq!(
        fake_peer::played(&peer),
        started,
        "the agent's program was started"
    );
    let reopened = Session::reopen_routed(
        &tree.sessions(),
        &tree.workspace()?,
        &id,
        &RouteBinding::new("claude", "home", None)?,
    );
    assert!(reopened.is_ok(), "{:?}", reopened.err());
    Ok(())
}

/// A client that answers what it is put as a step of the proof table says,
/// and keeps what it was put.
#[cfg(feature = "proof")]
struct Following {
    answers: std::collections::VecDeque<crucible_app::proof::accounts::Put>,
    put: Vec<Pending>,
}

#[cfg(feature = "proof")]
impl Following {
    fn of(step: &crucible_app::proof::accounts::Expected) -> Self {
        Self {
            answers: step.put.iter().copied().collect(),
            put: Vec::new(),
        }
    }
}

#[cfg(feature = "proof")]
impl Front for Following {
    fn put<'a>(
        &'a mut self,
        pending: &'a Pending,
        _shown: Shown<'a>,
    ) -> BoxFuture<'a, Option<Ruled>> {
        use crucible_app::proof::accounts::Put;

        Box::pin(async move {
            self.put.push(pending.clone());
            let id = pending.id();
            Some(match self.answers.pop_front()? {
                Put::Permission => Ruled::Ruled {
                    id,
                    ruling: Ruling::Allow,
                    lasting: Lasts::Once,
                },
                Put::Choose(name) | Put::Missing { answer: name, .. } => Ruled::Picked {
                    id,
                    choice: crucible_client_api::Said::new(name).ok()?,
                },
            })
        })
    }

    fn refused(&mut self, _refusal: crucible_client_api::Refusal) {}
}

/// Whether `pending` is what `put` says the client is asked.
#[cfg(feature = "proof")]
fn is_put(pending: &Pending, put: crucible_app::proof::accounts::Put) -> bool {
    use crucible_app::proof::accounts::Put;

    match (put, pending) {
        (Put::Permission, Pending::Permission { .. }) => true,
        (Put::Choose(name), Pending::Account { choices, .. }) => {
            choices.iter().any(|choice| choice.name.as_str() == name)
        }
        (
            Put::Missing {
                asks: question,
                why,
                install,
                answer,
            },
            Pending::Account {
                asks,
                facts,
                choices,
                ..
            },
        ) => {
            asks.as_str() == question
                && facts
                    .first()
                    .is_some_and(|fact| fact.as_str().starts_with(why))
                && facts.iter().any(|fact| fact.as_str() == install)
                && choices.iter().any(|choice| choice.name.as_str() == answer)
        }
        _ => false,
    }
}

/// Where a turn's stop goes once it has begun: through the client's own
/// cancel, whose answer is kept.
#[cfg(feature = "proof")]
struct Interrupting {
    cancel: Cancel,
    told: Mutex<Option<Outcome>>,
}

#[cfg(feature = "proof")]
impl Post for Interrupting {
    fn post(&self, reported: EventEnvelope) {
        if matches!(reported.event(), Event::TurnStarted { .. }) {
            let request = Request::new(Asking::every(), Correlation::new(2), Command::Cancel);
            let told = client::interrupt(&request, &self.cancel);
            if let Ok(mut kept) = self.told.lock() {
                kept.get_or_insert(told);
            }
        }
    }
}

/// Takes `step` of the proof table on `conversation`, answering as it says,
/// and answers what the client was put and what it was told.
#[cfg(feature = "proof")]
fn taking(
    tree: &Tree,
    conversation: &mut Conversation,
    step: &crucible_app::proof::accounts::Expected,
) -> Result<(Vec<Pending>, Vec<Outcome>), Failed> {
    use crucible_app::proof::accounts::{INSTANCE, Step};
    use crucible_client_api::Name;

    let mut following = Following::of(step);
    let file = std::fs::read_to_string(super::Desk::new(tree, &[])?.choosing)?;
    let told = match step.step {
        Step::Connect | Step::ConnectMissing | Step::Disconnect => {
            let named = step.step != Step::ConnectMissing;
            let connect = move |instance| Command::Connect {
                instance: named.then_some(instance),
            };
            let disconnect = |instance| Command::Disconnect { instance };
            let command: &dyn Fn(Name) -> Command = if step.step == Step::Disconnect {
                &disconnect
            } else {
                &connect
            };
            let asked = Asked {
                installed: (&file, INSTANCE),
                command,
                asking: Asking::every(),
                after: None,
            };
            vec![accounting_on(tree, conversation, &asked, &mut following)?.0]
        }
        Step::Prompt | Step::Interrupt => {
            let request = Request::new(
                Asking::every(),
                Correlation::new(1),
                Command::Prompt(Prompt::new("go")?),
            );
            let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
            let interrupting = Interrupting {
                cancel: cancel.clone(),
                told: Mutex::new(None),
            };
            let quiet = Watching {
                events: mpsc::channel().0,
                stopping: None,
            };
            let post: &dyn Post = if step.step == Step::Interrupt {
                &interrupting
            } else {
                &quiet
            };
            let ended = {
                let run = conversation.starting(post, &cancel, &steer, &aside);
                runtime()?.block_on(client::turn(
                    conversation,
                    &request,
                    Box::default(),
                    &mut following,
                    &run,
                ))
            };
            let cancelling = interrupting.told.into_inner().ok().flatten();
            cancelling
                .into_iter()
                .chain([ended.response(&request).outcome])
                .collect()
        }
    };
    Ok((following.put, told))
}

/// A client driving an installed agent from a conversation on the provider's
/// model is asked and told, at every step, what the proof table says: the
/// connect moves it onto the agent, a prompt asks one permission and runs to
/// its end, an interrupt is heard and stops the turn, a disconnect takes the
/// instance out of use, and a connect to an agent not installed says what to
/// install.
#[cfg(feature = "proof")]
#[test]
fn a_client_driving_an_agent_is_asked_and_told_what_the_proof_table_says() -> Result<(), Failed> {
    use crucible_app::proof::ACCOUNTS;

    let tree = Tree::new("external-proof-accounts")?;
    let native = started_native(&tree, Script::new(Vec::new()))?;
    let run = Reply::new()
        .hear(1)
        .say(claude::init())
        .say(claude::asked("q"))
        .hear(1)
        .say(claude::said("Done."))
        .say(claude::answered(Some(claude::PRICE)))
        .hear(1)
        .say(claude::init())
        .say(claude::said("Working."))
        .hear(1)
        .say(claude::stopped())
        .say(claude::interrupted());
    let script = connected([claude::signed_in(), run]);
    let (mut conversation, _) = stepped(&tree, native, &script)?;
    let choosing = super::Desk::new(&tree, &[])?.choosing;
    if let Some(directory) = choosing.parent() {
        std::fs::create_dir_all(directory)?;
    }
    std::fs::write(&choosing, UNCONNECTED)?;

    for step in &ACCOUNTS {
        let (put, told) = taking(&tree, &mut conversation, step)?;

        assert_eq!(told, (step.told)(), "{:?}", step.step);
        assert_eq!(put.len(), step.put.len(), "{:?}: {put:?}", step.step);
        for (pending, expected) in put.iter().zip(step.put) {
            assert!(is_put(pending, *expected), "{:?}: {pending:?}", step.step);
        }
    }
    Ok(())
}
