//! Trying the user's own program of an agent: probed once, confined, after
//! the door's own yes, and written as the instance that runs it.

use std::path::{Path, PathBuf};

use crucible_client_api::{ErrorCode, Text};
use crucible_config::{Driver, ExternalAgent};
use crucible_external_harnesses::antigravity;
use crucible_types::AgentKey;
use crucible_types::shown::escaped;

use super::{
    AgentsError, Confirmation, Door, Front, Outcome, OwnAntigravity, OwnProgram, confirmed,
    refused_beside,
};
use crate::backends::{ANTIGRAVITY, kept_word};
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

/// Whether `instance` is one of the agent `key`: a catalog instance of it,
/// or, for Antigravity's key, an Antigravity instance.
fn of_agent(instance: &ExternalAgent, key: &AgentKey) -> bool {
    match instance.driver() {
        Driver::Catalog => instance.agent() == Some(key),
        Driver::Antigravity => key.as_str() == ANTIGRAVITY_KEY,
        Driver::Claude => false,
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

/// The path the user's own program of the catalog agent `key` leads to, as
/// `trying` finds it among the configured `instances`: the one an instance of
/// it runs, or, where none of it is configured, the one its entry's guessed
/// name finds. The agents door and `/login` both list by it.
pub(crate) fn own_of(
    trying: &Trying,
    key: &AgentKey,
    instances: &[ExternalAgent],
) -> Option<PathBuf> {
    let mut configured = instances
        .iter()
        .filter(|instance| instance.driver() == Driver::Catalog && instance.agent() == Some(key))
        .peekable();
    let trial = if configured.peek().is_none() {
        trying.catalog(key, None)
    } else {
        configured.find_map(|instance| trying.catalog(key, Some(instance.executable())))
    };
    trial.map(|trial| trial.canonical().to_owned())
}

impl Door<'_> {
    /// The path the user's own program of the catalog agent `key` leads to,
    /// as `trying` finds it and a try of it would take it: the one a
    /// configured instance of it runs, or, where none of it is configured,
    /// the one its entry's guessed name finds. As `/login`, it never offers
    /// a second instance beside one configured.
    pub(super) fn own(&self, trying: &Trying, key: &AgentKey) -> Option<PathBuf> {
        own_of(trying, key, &self.instances)
    }

    /// What the door finds of the user's own program of `key` alone, as
    /// `trying` finds it: Antigravity's as a listing names it, and a catalog
    /// or registry agent's as [`Door::own`] finds it for the row a listing
    /// would give it, wherever that row would fall, a registry entry with a
    /// target here that crucible does not install among them. Where neither
    /// finds one, it is [`Door::unrun`]'s answer.
    pub(super) fn own_program(&self, trying: &Trying, key: &AgentKey) -> OwnProgram {
        if key.as_str() == ANTIGRAVITY_KEY {
            return own_antigravity(trying, &self.instances).map_or_else(
                || self.unrun(trying, key),
                |own| OwnProgram::Found(own.program),
            );
        }
        let offered = self
            .trust
            .offers(None)
            .into_iter()
            .any(|offer| offer.entry.key == *key)
            || self.registry.as_ref().is_some_and(|view| {
                view.index
                    .listed()
                    .into_iter()
                    .any(|(entry, _)| entry.id == *key)
            });
        if kept_word(key.as_str()) || !offered {
            return OwnProgram::Unlisted;
        }
        self.own(trying, key)
            .map_or_else(|| self.unrun(trying, key), OwnProgram::Found)
    }

    /// What the door finds of the user's own program of `key` where no
    /// configured instance of it runs one: [`OwnProgram::Beside`], naming the
    /// first instance of it configured, as a try of it is refused, where the
    /// own program a guess of its name finds runs beside that instance, and
    /// [`OwnProgram::Unfound`] where none of it is configured or none is
    /// found.
    fn unrun(&self, trying: &Trying, key: &AgentKey) -> OwnProgram {
        let Some(instance) = self
            .instances
            .iter()
            .find(|instance| of_agent(instance, key))
        else {
            return OwnProgram::Unfound;
        };
        let trial = if key.as_str() == ANTIGRAVITY_KEY {
            trying.antigravity(&ANTIGRAVITY)
        } else {
            trying.catalog(key, None)
        };
        trial.map_or(OwnProgram::Unfound, |trial| OwnProgram::Beside {
            program: trial.canonical().to_owned(),
            instance: instance.name().into(),
        })
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
        let of_it = |instance: &&ExternalAgent| of_agent(instance, &key);
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
                return Err(refused_beside(key, asked, instance.name()));
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
