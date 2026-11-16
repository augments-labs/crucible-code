//! Questions held for later, and a conversation on an installed agent.
//!
//! The agent asks through its own tools, not the runner's, so nothing waits
//! on the route and nothing there delivers an answer. Each move onto, along
//! and off the route moves the questions to the session it records into: what
//! was shown before the move is refused as stale, and no run of the agent is
//! launched for it.

use std::sync::Arc;

use crucible_app::client::Questions;
use crucible_client_api::{Era, ErrorCode, Outcome};
use crucible_session::Session;
use crucible_types::Transcript;

use super::super::Script;
use super::super::deferring::{deferring, emptied, held, oldest, prompt, rust, stops, turned};
use super::{Failed, Outside, Tree, binding, connected, runtime, saying};

/// The era of the session `outside` records into.
fn recording(outside: &Outside) -> Result<Era, Failed> {
    Ok(Era::Session(
        outside
            .conversation
            .session()
            .id()
            .cloned()
            .ok_or("an id")?,
    ))
}

/// An answer shown on a provider's model is refused once the conversation
/// has moved onto the agent's route, and no run is launched to deliver it.
#[test]
fn on_an_agent_s_route_an_answer_is_refused_stale_and_starts_no_turn() -> Result<(), Failed> {
    let tree = Tree::new("external-deferring")?;
    let questions = Questions::new();
    let script = Script::new(vec![deferring("c1", "Language"), saying("asked")]);
    let mut conversation = held(&tree, script, &questions)?;
    turned(&mut conversation, &prompt("plan it")?, &questions)?;
    let (shown, id) = oldest(&questions)?;
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);
    let (mut outside, _left) = Outside::onto(
        &tree,
        &connected([]),
        conversation,
        session,
        Transcript::new(),
    )?;
    assert_eq!(emptied(&questions)?, recording(&outside)?);
    let (launched, heard) = (outside.launched().len(), outside.heard().len());

    let (response, streamed) = turned(&mut outside.conversation, &rust(&shown, id)?, &questions)?;

    assert_eq!(
        response.outcome,
        Outcome::Refused(ErrorCode::StaleQuestions.into())
    );
    assert!(stops(&streamed).is_empty(), "a turn ran: {streamed:?}");
    assert_eq!(outside.launched().len(), launched, "a run was launched");
    assert_eq!(outside.heard().len(), heard, "the agent heard of it");
    Ok(())
}

/// `/clear` and picking another session up on the route, and moving back
/// onto this program's own models, each move the questions to the session
/// recorded into next: an answer named for the session left is refused, and
/// launches nothing.
#[test]
fn on_an_agent_s_route_each_session_change_moves_the_questions() -> Result<(), Failed> {
    let tree = Tree::new("external-deferring-leaving")?;
    let questions = Questions::new();
    let conversation = held(&tree, Script::new(Vec::new()), &questions)?;
    let session = Arc::new(Session::start_routed(
        &tree.sessions(),
        &tree.workspace()?,
        None,
        &binding()?,
    )?);
    let (mut outside, _left) = Outside::onto(
        &tree,
        &connected([]),
        conversation,
        session,
        Transcript::new(),
    )?;
    let (sessions, workspace) = (tree.sessions(), tree.workspace()?);
    let first = recording(&outside)?;
    let stale = Outcome::Refused(ErrorCode::StaleQuestions.into());
    let launched = outside.launched().len();
    let refused = |outside: &mut Outside, left: &Era| -> Result<(), Failed> {
        let id = crucible_types::QuestionId::numbered(0);
        let (response, _) = turned(&mut outside.conversation, &rust(left, id)?, &questions)?;
        assert_eq!(response.outcome, stale, "{left:?}");
        Ok(())
    };

    drop(runtime()?.block_on(outside.conversation.clear(&sessions, &workspace, None))?);

    let cleared = recording(&outside)?;
    assert_ne!(cleared, first);
    assert_eq!(emptied(&questions)?, cleared.clone());
    refused(&mut outside, &first)?;

    let Era::Session(back) = &first else {
        return Err("the route's session records".into());
    };
    let _left = runtime()?.block_on(outside.conversation.resume(
        &sessions,
        &workspace,
        back,
        &[],
    ))?;

    assert_eq!(emptied(&questions)?, first.clone());
    refused(&mut outside, &cleared)?;

    let inside = Arc::new(Session::start(&sessions, &workspace, None)?);
    let _left = runtime()?.block_on(outside.conversation.inside(inside, Transcript::new()));

    let native = recording(&outside)?;
    assert_ne!(native, first);
    assert_eq!(emptied(&questions)?, native);
    refused(&mut outside, &first)?;
    assert_eq!(outside.launched().len(), launched, "a run was launched");
    Ok(())
}
