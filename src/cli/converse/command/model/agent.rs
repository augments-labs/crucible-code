//! `/model` on an installed agent's route: the models the agent says it can
//! be started on, asked of it, and one of them taken for its next run.
//!
//! The agent is what knows which models it offers, so the list is whatever it
//! answers, its own choice first, and a value it did not list is refused
//! rather than handed to its program. What is taken starts the next turn's
//! run, which continues the agent's own session, and is written into the
//! agent's entry so the next start is on it too.

use crucible_app::Conversation;
use crucible_app::client::{AgentModel, AgentModels, Performed};
use crucible_app::outside::Unchosen;
use crucible_client_api::{Command, Name};
use crucible_tui::{Offered, Panel, Renderer, Terminal};

use super::{LEFT, Terms, say};
use crate::cli::Fatal;
use crate::cli::client::astray;
use crate::cli::converse::picking::{self, Picked};

/// What the agent's own choice is named by on the line.
const OWN: &str = "default";

/// What a row of the agent's own choice says beneath its name.
const OWN_SAYS: &str = "the agent's own choice";

/// Runs it where the conversation is on an installed agent's route: the
/// model named, one taken off a panel of what the agent lists, or, with no
/// keyboard, the lines that would ask for each. Nothing off the route.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
pub(super) fn run<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
) -> Option<Result<(), Fatal>> {
    let crucible_client_api::Execution::Instance(instance) =
        crucible_app::client::execution(conversation)
    else {
        return None;
    };
    let instance = instance.as_str();
    Some(if said.is_empty() {
        listed(renderer, conversation, terms, keys, instance)
    } else {
        named(said, renderer, conversation, terms, instance)
    })
}

/// The model `said` names taken for the agent `instance`, where it can be.
fn named<T: Terminal>(
    said: &str,
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    instance: &str,
) -> Result<(), Fatal> {
    let model = if said == OWN {
        None
    } else {
        match Name::new(said) {
            Ok(model) => Some(model),
            Err(refusal) => return say(renderer, &format!("! {refusal}")),
        }
    };
    chosen(renderer, conversation, terms, instance, model.as_ref())
}

/// What the agent `instance` lists, on a panel to take one from where there
/// are `keys`, else as the lines that would ask for each.
///
/// # Errors
///
/// [`Fatal::Terminal`] if the terminal could not be drawn on.
fn listed<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    keys: bool,
    instance: &str,
) -> Result<(), Fatal> {
    let (offered, current) = match terms.perform(conversation, Command::ListAgentModels) {
        Performed::AgentModels(AgentModels::Listed { offered, current }) => (offered, current),
        Performed::AgentModels(AgentModels::Unoffered) => {
            return say(renderer, &unoffered(instance, conversation));
        }
        Performed::AgentModels(AgentModels::Unreachable(failure)) => {
            return say(
                renderer,
                &format!("! the agent {instance} could not be asked for its models: {failure}"),
            );
        }
        other => return say(renderer, &astray(&other)),
    };
    // Each row as the agent spelled it: its value, and what it calls it; at
    // most the harness's bound of them.
    let rows: Vec<(Option<String>, String)> = offered
        .iter()
        .map(|row| {
            (
                row.model.as_ref().map(ToString::to_string),
                row.shown.to_string(),
            )
        })
        .collect();
    // The row in use, where the agent still lists it.
    let at = rows
        .iter()
        .position(|(model, _)| model.as_deref() == current.as_deref());

    if keys {
        let shown: Vec<Offered<'_>> = rows
            .iter()
            .map(|(model, name)| Offered {
                name,
                says: model.as_deref().unwrap_or(OWN_SAYS),
            })
            .collect();
        let title = format!("Model of {instance}");
        let panel = Panel {
            source: None,
            title: &title,
            said: Some("Taken from the next message on, continuing this session."),
            told: &[],
            shown: &shown,
            chosen: at.unwrap_or_default(),
            footer: "esc to go back",
        };
        match picking::pick(renderer, terms.style(), panel)? {
            Picked::Took(took) => {
                let Some((model, _)) = rows.get(took) else {
                    return say(renderer, LEFT);
                };
                let model = match model.as_deref().map(Name::new).transpose() {
                    Ok(model) => model,
                    Err(refusal) => return say(renderer, &format!("! {refusal}")),
                };
                return chosen(renderer, conversation, terms, instance, model.as_ref());
            }
            Picked::Left => return say(renderer, LEFT),
            // No room for a panel: the lines below ask the same.
            Picked::Cramped => {}
        }
    }

    for (index, (model, name)) in rows.iter().enumerate() {
        let mark = if Some(index) == at { "  (in use)" } else { "" };
        say(
            renderer,
            &format!("/model {}  {name}{mark}", model.as_deref().unwrap_or(OWN)),
        )?;
    }
    Ok(())
}

/// What the agent is started on from the next turn, or why it is not.
fn chosen<T: Terminal>(
    renderer: &mut Renderer<T>,
    conversation: &mut Conversation,
    terms: &Terms,
    instance: &str,
    model: Option<&Name>,
) -> Result<(), Fatal> {
    let named = model.map(Name::as_str);
    let unwritten = match terms.perform(
        conversation,
        Command::SelectAgentModel {
            model: model.cloned(),
        },
    ) {
        Performed::AgentModel(AgentModel::Taken { unwritten }) => unwritten,
        Performed::AgentModel(AgentModel::Unchosen(Unchosen::Unlisted)) => {
            return say(
                renderer,
                &format!(
                    "! the agent {instance} does not list {}; /model shows what it does",
                    named.unwrap_or(OWN)
                ),
            );
        }
        Performed::AgentModel(AgentModel::Unchosen(Unchosen::Unoffered)) => {
            return say(renderer, &unoffered(instance, conversation));
        }
        Performed::AgentModel(AgentModel::Unchosen(Unchosen::Unreachable(failure))) => {
            return say(
                renderer,
                &format!("! the agent {instance} could not be asked for its models: {failure}"),
            );
        }
        other => return say(renderer, &astray(&other)),
    };
    say(
        renderer,
        &match named {
            Some(model) => format!("the agent {instance} starts on {model} from the next message"),
            None => format!(
                "the agent {instance} starts on its own choice of model from the next message"
            ),
        },
    )?;
    match unwritten {
        None => Ok(()),
        Some(problem) => say(
            renderer,
            &format!("! {problem}; asked for this session only"),
        ),
    }
}

/// What is said of an agent that does not list its models: what it starts on
/// stays its entry's.
fn unoffered(instance: &str, conversation: &Conversation) -> String {
    match crucible_app::client::model(conversation) {
        Some(model) => format!(
            "the agent {instance} does not say which models it offers; it starts on {}, the model its entry names",
            model.text().as_str()
        ),
        None => {
            format!("the agent {instance} does not say which models it offers, and chooses its own")
        }
    }
}
