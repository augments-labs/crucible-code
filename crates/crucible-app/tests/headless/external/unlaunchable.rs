//! A prompt whose turn cannot start the agent, on a route with no yes.
//!
//! Each case holds a conversation moved onto Claude Code's route with no run
//! launched, so its next prompt would start the agent, and with the route
//! unanswered, so that prompt would put the route to the client first. A
//! prompt that carries an attachment, which no agent is handed, and one to an
//! agent whose program is nowhere, are each refused for that reason before
//! the route is put: nobody is asked to agree to a launch that cannot
//! happen, no yes is written into the user file, and nothing is started.

use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use crucible_app::access::Route;
use crucible_app::backends::Registry;
use crucible_app::client;
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::Agent;
use crucible_app::outside::Installed;
use crucible_client_api::{
    Capabilities as Asking, Command, Correlation, Outcome, Pending, Prompt, Request, TurnOutcome,
};
use crucible_config::{Driver, Serves};
use crucible_external_harnesses::fake_peer::{self, scripts::claude};
use crucible_runtime::{Aside, Cancel, Steer};
use crucible_sandbox::{SandboxPolicy, SandboxService};
use crucible_sandbox_local::LocalSandbox;
use crucible_session::Session;
use crucible_types::{Attachment, Modality, Transcript};

use super::{INSTALLED, Launched, Launching, Peer, Picking, Watching, binding, row, runtime};
use crate::{Failed, Script, Tree};

/// What an unlaunched conversation was made of, beside the conversation.
struct Unlaunched {
    conversation: crucible_app::Conversation,
    /// The agent its runs would be started as.
    agent: Agent,
    /// The yeses its sends are held to.
    consent: Consent,
    /// The user file a yes is written into.
    file: PathBuf,
    /// What each launch was asked for.
    launched: Arc<Mutex<Vec<Launched>>>,
}

/// A conversation in `tree` on Claude Code's route, no run launched and the
/// route unanswered, its runs started as the program at `program`: the fake
/// peer installed there where `program` is `None`, else a path that is not
/// there.
///
/// This is the headless [`Launching`] whose executable is missing, where
/// `program` names one.
fn unlaunched(tree: &Tree, program: Option<PathBuf>) -> Result<Unlaunched, Failed> {
    let executable = match program {
        Some(missing) => missing,
        None => fake_peer::install(&tree.0.join("peer"), "claude", &Peer::of([claude::clean()]))?,
    };
    let row = row()?;
    let consent = Consent::new(Routes::production());
    let file = tree.0.join("config.json");
    std::fs::write(&file, INSTALLED)?;
    consent.keeps_in(file.clone());
    let runtime = runtime()?;
    let route = Route::External {
        serves: Serves::Provider("anthropic"),
        instance: "work".into(),
        driver: Driver::Claude,
        executable: executable.to_string_lossy().into(),
        profile: None,
        model: None,
        effort: None,
        enabled: true,
    };
    let registry = Registry::new(std::slice::from_ref(row));
    let agent = Agent::routed(&route, &registry, |_| None).ok_or("Claude Code's route")?;
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.clone()));
    let launching = Launching {
        sandbox,
        runtime: runtime.clone(),
        policy: SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false),
        consent: consent.clone(),
        agent: agent.clone(),
        row,
        launched: Arc::default(),
    };
    let launched = Arc::clone(&launching.launched);
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);
    let mut conversation =
        crate::conversation(tree, Script::new(Vec::new()), false)?.consenting(consent.clone());
    let installed = Installed {
        backend: None,
        relaunch: Box::new(launching),
        route: binding()?,
    };
    drop(runtime.block_on(conversation.external(session, Transcript::new(), installed))?);
    Ok(Unlaunched {
        conversation,
        agent,
        consent,
        file,
        launched,
    })
}

/// Sends `prompt` with `attached` through the client's door, a client that
/// accepts whatever it is put answering; what it was put and how the turn
/// ended.
fn prompted(
    unlaunched: &mut Unlaunched,
    attached: Box<[Attachment]>,
) -> Result<(Vec<Pending>, Outcome), Failed> {
    let request = Request::new(
        Asking::every(),
        Correlation::new(1),
        Command::Prompt(Prompt::new("hello")?),
    );
    let mut picking = Picking::of([Some("Accept")]);
    let (cancel, steer, aside) = (Cancel::new(), Steer::new(), Aside::new());
    let quiet = Watching {
        events: mpsc::channel().0,
        stopping: None,
    };
    let conversation = &mut unlaunched.conversation;
    let ended = {
        let run = conversation.starting(&quiet, &cancel, &steer, &aside);
        runtime()?.block_on(client::turn(
            conversation,
            &request,
            attached,
            &mut picking,
            &run,
        ))
    };
    Ok((picking.put, ended.response(&request).outcome))
}

/// Every failure of a refused turn's case: the route put, a yes written, a
/// launch tried, or an outcome other than a failure saying `reason`.
fn refused_first(
    unlaunched: &Unlaunched,
    (put, outcome): (&[Pending], &Outcome),
    reason: &str,
) -> Result<Vec<String>, Failed> {
    let mut failures = Vec::new();
    if put
        .iter()
        .any(|pending| matches!(pending, Pending::Warning { .. }))
    {
        failures.push(format!("the route was put first: {put:?}"));
    }
    let written = std::fs::read_to_string(&unlaunched.file)?;
    if written != INSTALLED {
        failures.push(format!("a yes was written: {written}"));
    }
    if unlaunched.consent.asks("agent:claude").is_none() {
        failures.push("the route was taken as answered".to_owned());
    }
    let launched = unlaunched
        .launched
        .lock()
        .map(|launched| launched.clone())
        .unwrap_or_default();
    if !launched.is_empty() {
        failures.push(format!("a launch was tried: {launched:?}"));
    }
    match outcome {
        Outcome::Turn(TurnOutcome::Failed(problem))
            if problem.message.as_str().contains(reason) => {}
        other => failures.push(format!(
            "the turn did not fail saying {reason:?}: {other:?}"
        )),
    }
    Ok(failures)
}

/// A prompt carrying an attachment, on an unanswered route whose turn would
/// start the agent, is refused for the attachment before the route is put.
/// Before, the route was put, the yes written, and only then was the turn
/// refused.
#[test]
fn an_attachment_on_an_unanswered_agent_route_is_refused_before_the_route_is_asked()
-> Result<(), Failed> {
    let tree = Tree::new("external-unanswered-attachment")?;
    let mut unlaunched = unlaunched(&tree, None)?;
    let attached: Box<[Attachment]> = Box::new([Attachment {
        path: tree
            .0
            .join("work")
            .join("shot.png")
            .to_string_lossy()
            .into(),
        modality: Modality::Image,
        media_type: "image/png".into(),
        hash: [7; 32],
    }]);

    let (put, outcome) = prompted(&mut unlaunched, attached)?;

    let failures = refused_first(
        &unlaunched,
        (&put, &outcome),
        "an attachment to an installed agent's turn",
    )?;
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}

/// A prompt to an agent whose program is not there, on an unanswered route,
/// is refused with what the launch would say before the route is put.
/// Before, the route was put, the yes written, and only then did the launch
/// fail.
#[test]
fn a_missing_program_on_an_unanswered_agent_route_is_refused_before_the_route_is_asked()
-> Result<(), Failed> {
    let tree = Tree::new("external-unanswered-missing")?;
    let mut unlaunched = unlaunched(&tree, Some(tree.0.join("no-such-claude-program")))?;
    let reason = unlaunched
        .agent
        .program(&unlaunched.consent)
        .err()
        .ok_or("the program was found")?
        .to_string();

    let (put, outcome) = prompted(&mut unlaunched, Box::default())?;

    let failures = refused_first(&unlaunched, (&put, &outcome), &reason)?;
    assert!(failures.is_empty(), "{failures:#?}");
    Ok(())
}
