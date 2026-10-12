//! The runner behind the execution contract, driven through that contract
//! alone: a turn's events come back in the contract's words, a permission the
//! engine asks for is settled by a decision naming it, and every operation the
//! runner does not claim is refused by name.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

use crucible_agents::{AgentBuilder, Model};
use crucible_app::native::{Native, confinement};
use crucible_execution::{
    Backend, Capability, Confinement, Decision, Error, Event, Lasting, Outcome, SessionRef, Sink,
    Stale, Text, TurnRequest, conformance,
};
use crucible_models::Delta;
use crucible_runner::{Runner, Tools};
use crucible_runtime::{BoxFuture, answered};
use crucible_sandbox::SandboxCapability;
use crucible_session::Session;
use crucible_tools::{
    Approved, DescribeTool, Permission, Rules, Sensitivity, Summary, Target, Tool, ToolContext,
    ToolError, ToolOutput,
};
use crucible_types::{
    AgentId, InputTokenUsage, ProviderUsage, StopReason, ToolArgs, ToolId, TurnId,
};

use super::{Failed, Script, Tree, runtime, saying};

/// The name the counting tool is offered and called by.
const WRITE: &str = "write";

/// Changes a file it never names, and counts how often it was let.
#[derive(Debug)]
struct Counting(Arc<AtomicUsize>);

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
        Sensitivity::MutatesFile {
            target: Target::unresolved(),
        }
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
    vec![
        Delta::ToolStarted {
            id: ToolId::new("a"),
            name: WRITE.into(),
        },
        Delta::ToolArgs("{}".into()),
        Delta::Stopped(StopReason::WantsTools),
    ]
}

/// A runner over `script` that asks before anything is changed, with the
/// counting tool on offer, and the count of times it ran.
fn runner(tree: &Tree, script: Script) -> Result<(Runner, Arc<AtomicUsize>), Failed> {
    let ran = Arc::new(AtomicUsize::new(0));
    let mut tools = Tools::new();
    tools.add_builtin(Counting(Arc::clone(&ran)))?;
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
    let (mut runner, _) = runner(&tree, Script::new(Vec::new()))?;
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
    let (mut runner, _) = runner(&tree, Script::new(vec![said]))?;
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
    let (mut runner, ran) = runner(&tree, Script::new(vec![calling(), saying("done")]))?;
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
    let (mut runner, ran) = runner(&tree, Script::new(vec![calling(), saying("done")]))?;
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
