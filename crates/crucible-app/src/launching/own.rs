//! How a start of the user's own program is held to its probe record: its
//! wire records every start's answer, and a program changed since its probe
//! is probed again before it starts.

use std::path::Path;

use crucible_external_harnesses::{Program, Request};
use crucible_sandbox::SandboxPolicy;
use crucible_tools::Mode;

use std::sync::Arc;

use super::{Agent, AuthMethod, Launcher, Trust, Unlaunched, Unpiped, child_home, quoted};
use crate::backends::{AgentRow, Sandboxing, Unbelieved, Wire};
use crate::own_program::{Owned, Places, Recording};

impl Unpiped {
    /// Why the user's own program is not signed in by `method`'s command.
    pub(crate) fn own_program(method: &AuthMethod) -> Self {
        Self::OwnProgram(quoted(
            method.description.as_deref().unwrap_or(&method.name),
        ))
    }
}

impl Agent {
    /// Its wire where its program, `installed`, is the user's own outside the
    /// install store `programs`: as its row speaks to that under `sandbox`,
    /// and for a catalog agent recording every start's answer against its
    /// probe record.
    // Each is an input of its own, as for `Agent::dialect`, which passes them on.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn own_wire(
        &self,
        mode: Mode,
        trust: &Trust,
        programs: &Path,
        installed: &Path,
        sandbox: Sandboxing,
    ) -> Wire {
        let canonical = std::fs::canonicalize(installed);
        let program = canonical.as_deref().unwrap_or(installed);
        let wire = self.row.own_dialect(mode, Some(trust), program, sandbox)?;
        let crucible = programs.parent().unwrap_or(programs);
        Ok(
            match Owned::of(crucible, &self.instance, &self.row, program) {
                Some(Ok(owned)) => Box::new(Recording::new(wire, owned)),
                Some(Err(_)) | None => wire,
            },
        )
    }
}

/// A start of the user's own program, as [`Launcher::own_held`] holds it.
pub(super) struct Own<'a> {
    /// What it is started with.
    pub(super) request: &'a Request,
    /// The program.
    pub(super) program: &'a Program,
    /// Crucible's home, which its record is kept under.
    pub(super) crucible: &'a Path,
    /// Whether it is started as the agent, not to run a sign-in command.
    pub(super) started: bool,
}

impl Launcher<'_> {
    /// `policy` as `agent`'s start `own` of the user's own program is
    /// confined, once it is held to its probe record where it is started as
    /// the agent.
    ///
    /// # Errors
    ///
    /// As [`Launcher::probed_now`], or why its directory cannot be given.
    pub(super) async fn own_held(
        &self,
        agent: &Agent,
        own: &Own<'_>,
        policy: &SandboxPolicy,
    ) -> Result<SandboxPolicy, Unlaunched> {
        let home = child_home(own.request, false).or_else(|| self.home.clone());
        let places = Places {
            home: home.as_deref().unwrap_or_else(|| Path::new("/")),
            crucible: own.crucible,
            sessions: self.sessions,
        };
        if own.started {
            self.probed_now(agent, own.program, &places).await?;
        }
        Ok(crate::own_program::launched(
            policy,
            &agent.row,
            own.program.path(),
            &places,
        )?)
    }

    /// Holds a start of `agent`, whose program at `program` is the user's
    /// own, to its probe record under `places`' crucible home: a catalog
    /// agent's program that changed since its probe, as one that updates
    /// itself does, is probed again, confined, before it is started, and is
    /// started only where it is still admitted; its record is then replaced,
    /// and removed where it is not. A program of any other row is held to
    /// nothing here.
    ///
    /// # Errors
    ///
    /// [`Unlaunched::Changed`] where it is no longer admitted, and
    /// [`Unbelieved::OwnUnprobed`] where no record holds for it.
    async fn probed_now(
        &self,
        agent: &Agent,
        program: &Program,
        places: &Places<'_>,
    ) -> Result<(), Unlaunched> {
        let AgentRow::Catalog(catalog) = &agent.row else {
            return Ok(());
        };
        let Some(owned) = Owned::of(places.crucible, &agent.instance, &agent.row, program.path())
        else {
            return Ok(());
        };
        let owned = owned?;
        match owned.record() {
            Some(record) if record.current(owned.canonical()) => Ok(()),
            Some(_) => crate::own_program::reprobe(
                Arc::clone(&self.service),
                self.runtime.clone(),
                places,
                &owned.instance(),
                program.clone(),
                catalog.dialect(Mode::Ask),
            )
            .await
            .map(drop)
            .map_err(|why| Unlaunched::Changed {
                agent: agent.instance.to_string(),
                why: Box::new(why),
            }),
            None => Err(Unbelieved::OwnUnprobed {
                key: catalog.key().clone(),
            }
            .into()),
        }
    }
}
