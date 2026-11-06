//! Questions held for later, answered while the conversation is on an
//! installed agent.
//!
//! The agent asks through its own tools, not the runner's, so nothing on the
//! route delivers a settled answer: answering settles the batch and starts
//! nothing the agent would hear.

use std::sync::Arc;

use crucible_client_api::{AnswerOutcome, Outcome};
use crucible_session::Session;
use crucible_tools::Deferred;
use crucible_types::{Answer, Question, Transcript};

use super::super::Script;
use super::super::deferring::{held, rust, stops, turned};
use super::{Failed, Outside, Tree, binding, connected};

/// On the route an answer is settled where the questions wait, and no run of
/// the agent is launched to deliver it.
#[test]
fn on_an_agent_s_route_an_answer_settles_and_starts_no_turn() -> Result<(), Failed> {
    let tree = Tree::new("external-deferring")?;
    let questions = Deferred::default();
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
    let id = questions.register(vec![Question::new(
        "Language",
        "Which language?",
        [Answer::new("Rust"), Answer::new("Python")],
    )])?;
    let (launched, heard) = (outside.launched().len(), outside.heard().len());

    let (response, streamed) = turned(&mut outside.conversation, &rust(id)?, &questions)?;

    assert_eq!(response.outcome, Outcome::Answer(AnswerOutcome::Settled));
    assert!(questions.any_settled(), "the answer was taken");
    assert!(stops(&streamed).is_empty(), "a turn ran: {streamed:?}");
    assert_eq!(outside.launched().len(), launched, "a run was launched");
    assert_eq!(outside.heard().len(), heard, "the agent heard of it");
    Ok(())
}
