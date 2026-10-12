//! The runner behind the execution contract, driven through that contract
//! alone: a turn's events come back in the contract's words, a permission the
//! engine asks for is settled by a decision naming it, and every operation the
//! runner does not claim is refused by name.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crucible_agents::{AgentBuilder, Model};
use crucible_app::native::{Native, confinement};
use crucible_execution::{
    Backend, Capability, Confinement, Decision, Detail, Error, Event, Lasting, Outcome, Pending,
    PendingRef, SessionRef, Sink, Stale, TEXT_BYTES, Text, TurnRequest, Unsupported, conformance,
};
use crucible_models::Delta;
use crucible_runner::{Runner, Tools};
use crucible_runtime::{BoxFuture, answered};
use crucible_sandbox::SandboxCapability;
use crucible_session::Session;
use crucible_tools::{
    Approved, Command, DescribeTool, Host, Permission, Rules, Sensitivity, Summary, Target, Tool,
    ToolContext, ToolError, ToolOutput,
};
use crucible_types::{
    AgentId, InputTokenUsage, ProviderUsage, StopReason, ToolArgs, ToolId, TurnId,
};

use super::{Failed, Script, Tree, runtime, saying};

/// The name the counting tool is offered and called by.
const WRITE: &str = "write";

/// Does what `sensitivity` says it does, and counts how often it was let.
#[derive(Debug)]
struct Counting(Arc<AtomicUsize>, Sensitivity);

impl DescribeTool for Counting {
    fn name(&self) -> &str {
        WRITE
    }

    fn schema(&self) -> &'static str {
        r#"{"type":"object","properties":{}}"#
    }
}

impl Tool for Counting {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        self.1.clone()
    }

    fn summary(&self, args: &ToolArgs) -> Summary {
        Summary::new(args.as_str())
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(ToolOutput::ok("done"))
        })
    }
}

/// One round in which the model reaches for the counting tool.
fn calling() -> Vec<Delta> {
    calling_with("{}")
}

/// One round in which the model reaches for the counting tool with `args`.
fn calling_with(args: &str) -> Vec<Delta> {
    vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: WRITE.into(),
        },
        Delta::ToolArgs(args.into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// A change to a file the call never names.
fn writing() -> Sensitivity {
    Sensitivity::MutatesFile {
        target: Target::unresolved(),
    }
}

/// A search for `query`, bound for a host a rule could name.
fn searching(query: &str) -> Sensitivity {
    Sensitivity::ReachesNetwork {
        host: Host::Named {
            sent: query.into(),
            host: "search.example".into(),
        },
    }
}

/// A runner over `script` that asks before anything is changed, with the
/// counting tool on offer doing what `sensitivity` says, and the count of
/// times it ran.
fn runner(
    tree: &Tree,
    script: Script,
    sensitivity: Sensitivity,
) -> Result<(Runner, Arc<AtomicUsize>), Failed> {
    let ran = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools.add_builtin(Counting(Arc::clone(&ran), sensitivity))?;
    let session = Arc::new(Session::start(&tree.sessions(), &tree.workspace()?, None)?);
    let agent = AgentBuilder::new(
        AgentId::new("test"),
        Model {
            name: "script".into(),
            max_tokens: 64,
            window: None,
            accepts: None,
            effort: None,
        },
    );
    let runner = Runner::new(
        Box::new(script),
        tools,
        agent.build(),
        crucible_context::ContextInputs::new(tree.0.join("work")),
        session,
    )
    .permitting(Permission::with(crucible_tools::Mode::Ask, Rules::new()));
    Ok((runner, ran))
}

/// The session the runner records into, as the contract names it.
fn named() -> Result<SessionRef, Failed> {
    SessionRef::new("native").ok_or_else(|| "no session name".into())
}

/// Keeps every event, and settles each permission it is asked with
/// `answer`, through the backend that asked.
struct Deciding<'n, 'r> {
    native: &'n Native<'r>,
    answer: Option<Decision>,
    kept: Mutex<Vec<Event>>,
    settled: Mutex<Vec<Result<(), Error>>>,
}

impl<'n, 'r> Deciding<'n, 'r> {
    fn new(native: &'n Native<'r>, answer: Option<Decision>) -> Self {
        Self {
            native,
            answer,
            kept: Mutex::default(),
            settled: Mutex::default(),
        }
    }

    fn kept(self) -> (Vec<Event>, Vec<Result<(), Error>>) {
        (
            self.kept
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
            self.settled
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }
}

impl Sink for Deciding<'_, '_> {
    fn post(&self, event: Event) {
        if let (Event::Pending(pending, _), Some(decision)) = (&event, &self.answer) {
            let settled = self.native.decide(pending, decision.clone());
            self.settled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(settled);
        }
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

/// The runner refuses, by name, everything it does not claim, and claims
/// confinement only as the sandbox does.
#[test]
fn the_native_backend_refuses_what_it_does_not_claim() -> Result<(), Failed> {
    let tree = Tree::new("native-conformance")?;
    let (mut runner, _) = runner(&tree, Script::new(Vec::new()), writing())?;
    let native = Native::new(&mut runner, named()?, Some(SandboxCapability::Observed))
        .ok_or("no instance number")?;

    assert_eq!(answered!(conformance(&native)), Ok(()));
    let described = native.describe();
    let claims = described.capabilities();
    assert!(claims.supports(Capability::Cancel));
    assert!(!claims.supports(Capability::Questions));
    assert_eq!(
        claims
            .confinement()
            .map(crucible_execution::Confined::level),
        Some(Confinement::Observed)
    );
    assert_eq!(
        confinement(SandboxCapability::Enforced),
        Confinement::HardEnforced
    );
    Ok(())
}

/// A turn comes back as the contract's events, with the turn's own usage and
/// one ending.
#[test]
fn a_native_turn_reports_through_the_contract() -> Result<(), Failed> {
    let tree = Tree::new("native-turn")?;
    let usage = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(100), Some(20))?,
        Some(10),
        None,
        None,
        &[],
    )?;
    let mut said = saying("hello");
    said.insert(1, Delta::Usage(usage));
    let (mut runner, _) = runner(&tree, Script::new(vec![said]), writing())?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, None);

    let outcome = runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "hi"), &sink));

    assert_eq!(outcome, Ok(Outcome::Answered));
    let (events, _) = sink.kept();
    assert!(
        matches!(events.first(), Some(Event::Started { .. })),
        "{events:?}"
    );
    assert!(
        events.contains(&Event::Delta(Text::new("hello"))),
        "{events:?}"
    );
    let used = events.iter().find_map(|event| match event {
        Event::Usage(observed) => Some(*observed.value()),
        _ => None,
    });
    let used = used.ok_or("no usage was reported")?;
    assert_eq!(used.input_tokens(), Some(100));
    assert_eq!(used.output_tokens(), Some(10));
    assert_eq!(used.cache_read_tokens(), Some(20));
    assert_eq!(
        used.cache_write_tokens(),
        None,
        "the provider said nothing of writing to its cache"
    );
    assert_eq!(events.last(), Some(&Event::Finished(Outcome::Answered)));
    let endings = events
        .iter()
        .filter(|event| matches!(event, Event::Finished(_) | Event::Failed(_)))
        .count();
    assert_eq!(endings, 1, "{events:?}");
    Ok(())
}

/// A permission the engine asks for is raised as a pending action, and a
/// decision naming it lets the call run; the same decision again settles
/// nothing.
#[test]
fn a_permission_the_native_turn_asks_is_settled_by_the_decision_naming_it() -> Result<(), Failed> {
    let tree = Tree::new("native-allowed")?;
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), saying("done")]),
        writing(),
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, Some(Decision::Allow(Lasting::Once)));

    let outcome =
        runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "write"), &sink));

    assert_eq!(outcome, Ok(Outcome::Answered));
    assert_eq!(ran.load(Ordering::Relaxed), 1);
    let (events, settled) = sink.kept();
    assert_eq!(settled, vec![Ok(())]);
    let pending = events
        .iter()
        .find_map(|event| match event {
            Event::Pending(pending, _) => Some(pending.clone()),
            _ => None,
        })
        .ok_or("no permission was asked")?;
    assert_eq!(
        native.decide(&pending, Decision::Allow(Lasting::Once)),
        Err(Error::Stale(Stale::Turn))
    );
    Ok(())
}

/// A denial reaches the engine as the no a person would have given, and the
/// call does not run.
#[test]
fn a_permission_denied_through_the_contract_runs_nothing() -> Result<(), Failed> {
    let tree = Tree::new("native-denied")?;
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), saying("done")]),
        writing(),
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, Some(Decision::Deny));

    let outcome =
        runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "write"), &sink));

    // A refused call ends the turn as the runner says it does: as a failure,
    // posted once, last.
    assert_eq!(ran.load(Ordering::Relaxed), 0);
    let (events, settled) = sink.kept();
    assert_eq!(settled, vec![Ok(())]);
    assert!(matches!(outcome, Err(Error::Failed(_))), "{outcome:?}");
    assert!(
        matches!(events.last(), Some(Event::Failed(_))),
        "{events:?}"
    );
    Ok(())
}

/// Acts on each pending action it is posted, through the backend that raised
/// it, and keeps every event and what each act came to.
struct Acting<'n, 'r> {
    native: &'n Native<'r>,
    act: fn(&Native<'r>, &PendingRef) -> Vec<Result<(), Error>>,
    kept: Mutex<Vec<Event>>,
    acted: Mutex<Vec<Result<(), Error>>>,
}

impl<'n, 'r> Acting<'n, 'r> {
    fn new(
        native: &'n Native<'r>,
        act: fn(&Native<'r>, &PendingRef) -> Vec<Result<(), Error>>,
    ) -> Self {
        Self {
            native,
            act,
            kept: Mutex::default(),
            acted: Mutex::default(),
        }
    }

    fn kept(self) -> (Vec<Event>, Vec<Result<(), Error>>) {
        (
            self.kept
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
            self.acted
                .into_inner()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }
}

impl Sink for Acting<'_, '_> {
    fn post(&self, event: Event) {
        if let Event::Pending(pending, _) = &event {
            let acted = (self.act)(self.native, pending);
            self.acted
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(acted);
        }
        self.kept
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
    }
}

/// Every permission question a turn posted.
fn asked(events: &[Event]) -> Vec<Pending> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Pending(_, pending) => Some(pending.clone()),
            _ => None,
        })
        .collect()
}

/// A question shows what the call sends, not only the host a rule is about:
/// a yes to "reach search.example" is not a yes to the query that leaves.
#[test]
fn a_native_question_shows_what_the_call_sends() -> Result<(), Failed> {
    let tree = Tree::new("native-sent")?;
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), saying("done")]),
        searching("what the secret is"),
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, Some(Decision::Deny));

    drop(runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "search"), &sink)));

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    let (events, _) = sink.kept();
    let asked = asked(&events);
    assert!(
        matches!(
            asked.as_slice(),
            [Pending::Permission { detail: Detail::Network { sent }, .. }]
                if sent.as_str() == "what the secret is"
        ),
        "{asked:?}"
    );
    Ok(())
}

/// A line is shown with the operators between its commands, and a command
/// that will outlive the turn says so: a yes to `make` is not a yes to
/// `make && ./serve`, nor to a server left running after the answer.
#[test]
fn a_native_question_shows_a_line_whole_and_what_it_leaves_running() -> Result<(), Failed> {
    let tree = Tree::new("native-line")?;
    let line = "make && ./serve";
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling_with(r#"{"background":true}"#), saying("done")]),
        Sensitivity::SpawnsProcess {
            command: Command::Understood {
                sent: line.into(),
                parts: Box::from([Box::from("make"), Box::from("./serve")]),
            },
        },
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, Some(Decision::Deny));

    drop(runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "serve"), &sink)));

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    let (events, _) = sink.kept();
    let asked = asked(&events);
    assert!(
        matches!(
            asked.as_slice(),
            [Pending::Permission {
                detail: Detail::Command { sent, left_running: true },
                ..
            }] if sent.as_str() == line
        ),
        "{asked:?}"
    );
    Ok(())
}

/// A question whose summary had to be cut is not put either.
#[test]
fn a_native_question_with_its_summary_cut_is_not_asked() -> Result<(), Failed> {
    let tree = Tree::new("native-summary")?;
    let long = "x".repeat(TEXT_BYTES + 1);
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), saying("done")]),
        Sensitivity::SpawnsProcess {
            command: Command::Understood {
                sent: "x".into(),
                parts: Box::from([Box::from(long.as_str())]),
            },
        },
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, Some(Decision::Allow(Lasting::Once)));

    drop(runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "run"), &sink)));

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    let (events, settled) = sink.kept();
    assert_eq!(asked(&events), Vec::new());
    assert_eq!(settled, Vec::new());
    Ok(())
}

/// A question whose sent text had to be cut is not put: the call is refused
/// as it is where nobody answers.
#[test]
fn a_native_question_cut_short_is_not_asked() -> Result<(), Failed> {
    let tree = Tree::new("native-cut")?;
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), saying("done")]),
        searching(&"q".repeat(TEXT_BYTES + 1)),
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, Some(Decision::Allow(Lasting::Once)));

    drop(runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "search"), &sink)));

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    let (events, settled) = sink.kept();
    assert_eq!(asked(&events), Vec::new());
    assert_eq!(settled, Vec::new());
    Ok(())
}

/// A yes for the session is remembered for the session: the same call again
/// runs without a second question.
#[test]
fn a_native_yes_for_the_session_is_not_asked_again() -> Result<(), Failed> {
    let tree = Tree::new("native-session")?;
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), calling(), saying("done")]),
        searching("the same query"),
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Deciding::new(&native, Some(Decision::Allow(Lasting::Session)));

    let outcome =
        runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "search"), &sink));

    assert_eq!(outcome, Ok(Outcome::Answered));
    assert_eq!(ran.load(Ordering::Relaxed), 2);
    let (events, settled) = sink.kept();
    assert_eq!(asked(&events).len(), 1, "{events:?}");
    assert_eq!(settled, vec![Ok(())]);
    Ok(())
}

/// An answer of the wrong kind settles nothing and leaves the question open
/// for one of the right kind.
#[test]
fn a_native_answer_of_the_wrong_kind_leaves_the_question_open() -> Result<(), Failed> {
    let tree = Tree::new("native-kind")?;
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), saying("done")]),
        writing(),
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Acting::new(&native, |native, pending| {
        vec![
            native.decide(pending, Decision::Answer(Text::new("yes"))),
            native.decide(pending, Decision::Allow(Lasting::Once)),
        ]
    });

    let outcome =
        runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "write"), &sink));

    assert_eq!(outcome, Ok(Outcome::Answered));
    assert_eq!(ran.load(Ordering::Relaxed), 1);
    let (_, acted) = sink.kept();
    assert_eq!(
        acted,
        vec![Err(Unsupported(Capability::Questions).into()), Ok(())]
    );
    Ok(())
}

/// An interrupt answers an open question no: the call does not run, and a
/// decision for it afterwards settles nothing.
#[test]
fn an_interrupt_answers_an_open_native_question_no() -> Result<(), Failed> {
    let tree = Tree::new("native-interrupt")?;
    let (mut runner, ran) = runner(
        &tree,
        Script::new(vec![calling(), saying("done")]),
        writing(),
    )?;
    let native = Native::new(&mut runner, named()?, None).ok_or("no instance number")?;
    let sink = Acting::new(&native, |native, pending| {
        vec![
            native.interrupt(),
            native.decide(pending, Decision::Allow(Lasting::Once)),
        ]
    });

    drop(runtime()?.block_on(native.prompt(TurnRequest::new(TurnId::FIRST, "write"), &sink)));

    assert_eq!(ran.load(Ordering::Relaxed), 0);
    let (_, acted) = sink.kept();
    assert_eq!(acted, vec![Ok(()), Err(Error::Stale(Stale::Settled))]);
    Ok(())
}
