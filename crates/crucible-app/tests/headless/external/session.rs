//! What a turn's failure leaves of the session an installed agent was asked to
//! resume.
//!
//! Each case here holds a conversation on Antigravity's route, played by the
//! fake peer, whose second run is refused the session the first one opened.
//! An agent that answers it has no session by that name lets the name go, so
//! the next run starts a new one; any other refusal keeps the name, and the
//! next run asks for it again.

use std::sync::Arc;

use crucible_app::access::Route;
use crucible_app::backends::{REGISTRY, Registry, Row};
use crucible_app::content_use::{Consent, Routes};
use crucible_app::launching::Agent;
use crucible_app::outside::{Installed, Relaunch};
use crucible_config::{Driver, Serves};
use crucible_external_harnesses::fake_peer::{self, Reply, Script as Peer};
use crucible_runner::{TurnError, Turned};
use crucible_sandbox::{SandboxPolicy, SandboxService};
use crucible_sandbox_local::LocalSandbox;
use crucible_session::{RouteBinding, Session};
use crucible_tools::Mode;
use crucible_types::Transcript;

use super::{Launching, Outside, answered, runtime, turn};
use crate::{Failed, Script, Tree};

/// Antigravity's own row, reaching no host, as [`row`] is Claude Code's.
fn antigravity_row() -> Result<&'static Row, Failed> {
    let shipped = REGISTRY
        .built(Driver::Antigravity)
        .ok_or("Antigravity has no row")?;
    Ok(Box::leak(Box::new(Row {
        reaches: &[],
        ..*shipped
    })))
}

/// A conversation on Antigravity's route from its first turn, the peer
/// playing `script` from the run launched for it, recording into a session
/// started on the route.
fn on_antigravity(tree: &Tree, script: &Peer) -> Result<Outside, Failed> {
    let peer = tree.0.join("peer");
    let program = fake_peer::install(&peer, "antigravity", script)?;
    let row = antigravity_row()?;
    let consent = Consent::new(Routes::production());
    consent.give(row.route);
    let runtime = runtime()?;
    let sandbox: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.clone()));
    let route = Route::External {
        serves: Serves::Provider("google"),
        instance: "space".into(),
        driver: Driver::Antigravity,
        executable: program.to_string_lossy().into(),
        profile: None,
        model: None,
        effort: None,
        enabled: true,
    };
    let registry = Registry::new(std::slice::from_ref(row));
    let agent = Agent::routed(&route, &registry, |_| None).ok_or("Antigravity's route")?;
    let launching = Launching {
        sandbox,
        runtime: runtime.clone(),
        policy: SandboxPolicy::standard(&tree.workspace()?)?.with_enabled(false),
        consent,
        agent,
        row,
        launched: Arc::default(),
    };
    let binding = RouteBinding::new("antigravity", "space", None)?;
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding,
    )?);
    let mut conversation = crate::conversation(tree, Script::new(Vec::new()), false)?
        .consenting(launching.consent.clone());
    let launched = Arc::clone(&launching.launched);
    runtime.block_on(async {
        let backend = launching.launch(Mode::Ask, None, None, None).await?;
        let installed = Installed {
            backend: Some(backend),
            relaunch: Box::new(launching),
            route: binding.clone(),
        };
        let _left = conversation
            .external(session, Transcript::new(), installed)
            .await?;
        Ok::<_, Failed>(())
    })?;
    Ok(Outside {
        conversation,
        peer,
        route: binding,
        launched,
    })
}

/// An agent that says the session a run was launched to resume is not there
/// fails that turn, and the next turn starts the agent on a new session
/// rather than asking it again for the one it said was gone. The session log
/// keeps that too: reopened after such a turn, it names no agent session, so
/// the first turn of the session picked back up opens a new one.
#[test]
fn a_session_the_agent_says_is_gone_is_opened_anew_by_the_next_turn() -> Result<(), Failed> {
    use crucible_external_harnesses::acp::SignIn;
    use crucible_external_harnesses::antigravity::Manifest;
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let reviewed = acp::initialized(
        Manifest::REVIEWED.name,
        Manifest::REVIEWED
            .versions
            .first()
            .copied()
            .unwrap_or_default(),
        Manifest::REVIEWED.sign_in.first().map_or("", SignIn::id),
    );
    let answering = |text: &str| {
        acp::opening(&reviewed)
            .say(acp::said(text))
            .say(acp::stopped(2, "end_turn"))
            .hear_out()
    };
    let gone = Reply::new()
        .hear(1)
        .say(reviewed.as_str())
        .hear(1)
        .say(acp::session_gone(1))
        .hear_out();
    let tree = Tree::new("external-session-gone")?;
    let script = Peer::of([
        answering("one"),
        gone.clone(),
        answering("three"),
        gone,
        answering("five"),
    ]);
    let mut outside = on_antigravity(&tree, &script)?;

    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");
    assert_eq!(
        outside.conversation.switch(Mode::AllowEdits),
        Mode::AllowEdits
    );
    let (second, _) = turn(&mut outside, "two", false)?;
    let Err(TurnError::Provider(refused)) = second else {
        return Err(format!("{second:?}").into());
    };
    assert!(
        refused
            .to_string()
            .contains("The agent refused: Resource not found"),
        "{refused}"
    );
    let (third, _) = turn(&mut outside, "three", false)?;

    assert_eq!(
        outside.launched(),
        [
            (Mode::Ask, None),
            (Mode::AllowEdits, Some(acp::SESSION.to_owned())),
            (Mode::AllowEdits, None),
        ],
        "the next turn asked the agent again for the session it said was gone"
    );
    assert!(matches!(third, Ok(Turned::Ran(_))), "{third:?}");

    // The agent names its new session, and says that one is gone too when
    // the next run asks for it; the session is then picked back up from its
    // log.
    assert_eq!(outside.conversation.switch(Mode::Ask), Mode::Ask);
    let (fourth, _) = turn(&mut outside, "four", false)?;
    assert!(matches!(fourth, Err(TurnError::Provider(_))), "{fourth:?}");
    let id = outside
        .conversation
        .session()
        .id()
        .cloned()
        .ok_or("an id")?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);
    let left = runtime()?.block_on(outside.conversation.clear(&sessions, &workspace, None))?;
    drop(left);
    let (reopened, _) = Session::reopen_routed(&sessions, &workspace, &id, &outside.route)?;
    assert_eq!(
        reopened.external_ref(),
        None,
        "the log still names the session the agent said was gone"
    );
    drop(reopened);
    let _cleared =
        runtime()?.block_on(outside.conversation.resume(&sessions, &workspace, &id, &[]))?;
    let (fifth, _) = turn(&mut outside, "five", false)?;

    assert!(matches!(fifth, Ok(Turned::Ran(_))), "{fifth:?}");
    assert_eq!(
        outside.launched().get(3..),
        Some(
            &[
                (Mode::Ask, Some(acp::SESSION.to_owned())),
                (Mode::Ask, None)
            ][..]
        ),
        "the session picked back up asked the agent for the session it said was gone"
    );
    Ok(())
}

/// An agent that refuses to resume its session for any other reason, here
/// for want of a sign-in, leaves the session bound: the next turn asks for it
/// again.
#[test]
fn a_session_refused_for_another_reason_is_resumed_by_the_next_turn() -> Result<(), Failed> {
    use crucible_external_harnesses::acp::SignIn;
    use crucible_external_harnesses::antigravity::Manifest;
    use crucible_external_harnesses::fake_peer::scripts::acp;

    let reviewed = acp::initialized(
        Manifest::REVIEWED.name,
        Manifest::REVIEWED
            .versions
            .first()
            .copied()
            .unwrap_or_default(),
        Manifest::REVIEWED.sign_in.first().map_or("", SignIn::id),
    );
    let answering = |text: &str| {
        acp::opening(&reviewed)
            .say(acp::said(text))
            .say(acp::stopped(2, "end_turn"))
            .hear_out()
    };
    let signed_out = Reply::new()
        .hear(1)
        .say(reviewed.as_str())
        .hear(1)
        .say(acp::sign_in_required(1))
        .hear_out();
    let tree = Tree::new("external-session-signed-out")?;
    let script = Peer::of([answering("one"), signed_out, answering("three")]);
    let mut outside = on_antigravity(&tree, &script)?;

    let (first, _) = turn(&mut outside, "one", false)?;
    assert!(first.is_ok(), "{first:?}");
    let _mode = outside.conversation.switch(Mode::AllowEdits);
    let (second, _) = turn(&mut outside, "two", false)?;
    let Err(TurnError::Provider(refused)) = second else {
        return Err(format!("{second:?}").into());
    };
    assert!(
        refused.to_string().contains("The agent is not signed in."),
        "{refused}"
    );
    let (third, _) = turn(&mut outside, "three", false)?;

    let kept = Some(acp::SESSION.to_owned());
    assert_eq!(
        outside.launched(),
        [
            (Mode::Ask, None),
            (Mode::AllowEdits, kept.clone()),
            (Mode::AllowEdits, kept),
        ],
        "a refusal that says nothing of the session let it go"
    );
    assert!(matches!(third, Ok(Turned::Ran(_))), "{third:?}");
    Ok(())
}

/// On a route whose agent chooses its own model, a model of the provider that
/// agent itself reaches is still refused: the agent is not started with a
/// model from here, so no provider is reachable by choosing one. The refusal
/// says the model is not chosen from here, and the conversation stays on its
/// route in the session it was in, with nothing written.
#[test]
fn a_model_of_the_agent_s_own_provider_is_refused_where_it_chooses_its_own() -> Result<(), Failed> {
    use crucible_client_api::{Command, Execution, ModelOutcome, Name, Outcome};

    let tree = Tree::new("external-model-own-provider")?;
    let mut outside = on_antigravity(&tree, &Peer::of([]))?;
    let left = outside.conversation.session().id().cloned();
    let standing = crucible_app::client::snapshot(&outside.conversation).execution;
    assert!(matches!(standing, Execution::Instance(_)), "{standing:?}");

    let refused = answered(
        &tree,
        &mut outside,
        Command::SelectModel {
            provider: Name::new(
                &Driver::Antigravity
                    .serves(None)
                    .map(|serves| serves.to_string())
                    .ok_or("Antigravity serves a provider")?,
            )?,
            model: Name::new("gemini-3.8-flash")?,
            effort: None,
        },
    )?;

    let Outcome::Model(ModelOutcome::Unreachable(problem)) = &refused else {
        return Err(format!("{refused:?}").into());
    };
    assert_eq!(
        problem.message.as_str(),
        "this conversation is on antigravity, whose model is not chosen from here"
    );
    assert_eq!(
        crucible_app::client::snapshot(&outside.conversation).execution,
        standing
    );
    assert_eq!(outside.conversation.session().id().cloned(), left);
    assert!(!crucible_config::user(&tree.home()?).exists());
    Ok(())
}
