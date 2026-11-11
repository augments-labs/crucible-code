//! Trying the user's own program of an agent: probed once, confined, after
//! the door's own yes, and written as the instance that runs it.

use std::path::{Path, PathBuf};

use crucible_client_api::{ErrorCode, Text};
use crucible_config::{Driver, ExternalAgent};
use crucible_external_harnesses::antigravity;
use crucible_types::AgentKey;
use crucible_types::shown::escaped;

use super::{AgentsError, Confirmation, Door, Front, Outcome, OwnAntigravity, confirmed};
use crate::backends::ANTIGRAVITY;
use crate::launching::quoted;
use crate::own_program::{Trying, Unowned};
use crate::remember;

/// The key the user's own Antigravity is tried under, which names no catalog
/// agent.
const ANTIGRAVITY_KEY: &str = "antigravity";

/// What a client is told when the user's own `program` of `agent` was not
/// taken for it, saying `why`.
pub(super) fn own_refusal(agent: &AgentKey, program: &Path, why: &Unowned) -> String {
    let program = escaped(&program.display().to_string());
    match why {
        Unowned::Probe(_) => format!(
            "Your own {agent} at {program} did not answer as an agent ({why}), so nothing was \
             written."
        ),
        Unowned::Unadmitted(_) => format!(
            "Your own {agent} at {program} answered, but not as an agent Crucible can run \
             ({why}), so nothing was written."
        ),
        Unowned::Unclosable(_) if agent.as_str() == ANTIGRAVITY_KEY => format!(
            "Your own {agent} at {program} could not be checked: {why}. Crucible's own release \
             of Antigravity works on this machine: run `crucible agents install antigravity`."
        ),
        Unowned::Unclosable(_) => {
            format!("Your own {agent} at {program} could not be checked: {why}.")
        }
        Unowned::Root(_) => format!("{why} Nothing was run or written."),
        _ => format!(
            "Your own {agent} at {program} could not be tried ({why}), so nothing was written."
        ),
    }
}

/// The user's own Antigravity `trying` finds, as a listing names it and a
/// try takes it: where no Antigravity instance is among `instances`, as
/// `/login` offers it, or where one configured runs it.
pub(super) fn own_antigravity(
    trying: &Trying,
    instances: &[ExternalAgent],
) -> Option<OwnAntigravity> {
    let trial = trying.antigravity(&ANTIGRAVITY)?;
    let mut configured = instances
        .iter()
        .filter(|instance| instance.driver() == Driver::Antigravity)
        .peekable();
    let taken = configured.peek().is_none()
        || configured.any(|instance| {
            trying.leads(instance.executable()).as_deref() == Some(trial.canonical())
        });
    taken.then_some(())?;
    Some(OwnAntigravity {
        agent: AgentKey::new(ANTIGRAVITY_KEY).ok()?,
        program: trial.canonical().to_owned(),
        note: antigravity::own_note(trying.sandboxed()),
    })
}

impl Door<'_> {
    /// The path the user's own program of the catalog agent `key` leads to,
    /// as `trying` finds it and a try of it would take it: the one a
    /// configured instance of it runs, or, where none of it is configured,
    /// the one its entry's guessed name finds. As `/login`, it never offers
    /// a second instance beside one configured.
    pub(super) fn own(&self, trying: &Trying, key: &AgentKey) -> Option<PathBuf> {
        let mut configured = self
            .instances
            .iter()
            .filter(|instance| {
                instance.driver() == Driver::Catalog && instance.agent() == Some(key)
            })
            .peekable();
        let trial = if configured.peek().is_none() {
            trying.catalog(key, None)
        } else {
            configured.find_map(|instance| trying.catalog(key, Some(instance.executable())))
        };
        trial.map(|trial| trial.canonical().to_owned())
    }

    /// Tries the user's own `program` of `agent`, a catalog agent's or
    /// Antigravity's, as the instance that already runs it or, where none of
    /// the agent is configured, a new one: the program must be the one a
    /// launch of that instance would find, its directory one an agent may be
    /// given, and `front` must say yes to the door's own question before it
    /// is started, confined, with the network closed, and asked `initialize`
    /// alone. What it answers is kept for a catalog agent's instance, and a
    /// new instance is written only once it is admitted; a refusal writes
    /// nothing and keeps nothing.
    ///
    /// # Errors
    ///
    /// [`AgentsError::Untried`] where no conversation lent the steps that
    /// start it, [`AgentsError::Own`] where it is not the program a launch
    /// would find, an instance of the agent runs another, its directory is
    /// refused or it is not admitted, and [`AgentsError::Remember`] where a
    /// new instance cannot be written.
    pub(super) async fn try_own(
        &self,
        agent: &str,
        program: &Text,
        front: &mut dyn Front,
    ) -> Result<Outcome, AgentsError> {
        let invalid = || Ok(Outcome::Refused(ErrorCode::InvalidArgument.into()));
        let Ok(key) = AgentKey::new(agent) else {
            return invalid();
        };
        let asked = PathBuf::from(program.as_str());
        if program.truncated() || !asked.is_absolute() {
            return invalid();
        }
        let steps = self
            .shelf
            .steps
            .ok_or_else(|| AgentsError::Untried(key.clone()))?;
        let trying = steps.trying();
        let refused = |why: Unowned| AgentsError::Own {
            agent: key.clone(),
            program: asked.clone(),
            why: Box::new(why),
        };
        let antigravity = key.as_str() == ANTIGRAVITY_KEY;
        let of_it = |instance: &&ExternalAgent| match instance.driver() {
            Driver::Catalog => instance.agent() == Some(&key),
            Driver::Antigravity => antigravity,
            Driver::Claude => false,
        };
        let configured = self.instances.iter().filter(of_it).find_map(|instance| {
            let trial = if antigravity {
                (trying.leads(instance.executable())? == asked)
                    .then(|| trying.antigravity(&ANTIGRAVITY))??
            } else {
                trying.catalog(&key, Some(instance.executable()))?
            };
            (trial.canonical() == asked).then(|| (instance.name().to_owned(), trial))
        });
        let (name, trial, new) = if let Some((name, trial)) = configured {
            (name, trial, false)
        } else {
            // As `/login`, a new instance is written only where none of the
            // agent is configured.
            if let Some(instance) = self.instances.iter().find(of_it) {
                return Err(refused(Unowned::Beside(quoted(instance.name()).into())));
            }
            let trial = if antigravity {
                trying.antigravity(&ANTIGRAVITY)
            } else {
                trying.catalog(&key, None)
            };
            let trial = trial
                .filter(|trial| trial.canonical() == asked)
                .ok_or_else(|| refused(Unowned::NotOwn(program.as_str().into())))?;
            let name =
                crucible_config::free_name(&key, self.instances.iter().map(ExternalAgent::name));
            (name, trial, true)
        };
        if let Some(why) = trial.refused() {
            return Err(refused(Unowned::Root(why.to_owned())));
        }
        let shown = escaped(&asked.display().to_string());
        let confirmation = Confirmation {
            asks: format!("Try your own {key} at {shown}?"),
            facts: trial.facts(&trying),
            choices: vec![
                (
                    "Try it".to_owned(),
                    "Start it once, confined with the network closed, ask it only `initialize`, \
                     and keep what it answers."
                        .to_owned(),
                ),
                ("Cancel".to_owned(), "Change nothing.".to_owned()),
            ],
        };
        if confirmed(self.request, front, &confirmation).await != Some(0) {
            return Ok(Outcome::Undecided);
        }
        let mut instances: Vec<&str> = self.instances.iter().map(ExternalAgent::name).collect();
        if new {
            instances.push(&name);
        }
        trial
            .probe(&trying, &name, &instances)
            .await
            .map_err(refused)?;
        if new {
            let written = trial
                .agent(&name)
                .ok_or_else(|| refused(Unowned::Unnamed(asked.clone())))
                .and_then(|instance| Ok(remember::adding_own(&self.user, &instance)?));
            if let Err(why) = written {
                // A record kept for an instance never written would hold
                // for an instance the user later names so.
                let _ = trying.forget(&name);
                return Err(why);
            }
        }
        Ok(Outcome::Tried {
            agent: key,
            instance: name.into(),
            program: asked,
        })
    }
}
