//! How a start of the user's own program is held to its probe record: its
//! wire records every start's answer, and a program changed since its probe
//! is probed again before it starts. An account step also gives back what
//! the program it started answered of itself, which a person reading the
//! user's own program signed out is shown.

use std::path::Path;

use std::sync::Arc;
use std::time::SystemTime;

use crucible_config::ExternalAgent;
use crucible_execution::{Decision, Failure, TurnRequest};
use crucible_external_harnesses::{
    Account, Act, Control, Dialect, Program, Ran, Reading, Refusal, Request, Shape, Told,
};
use crucible_sandbox::SandboxPolicy;
use crucible_tools::Mode;

use super::{
    Agent, AuthMethod, Launcher, Steps, Trust, Unlaunched, Unpiped, programs_beside, quoted,
};
use crate::backends::{AgentRow, Sandboxing, Unbelieved, Wire};
use crate::own_program::{Introduced, Owned, Places, Recording, Unowned};

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
    /// the agent. Its directory is judged, at its launch and at the probe a
    /// change since its probe brings, against the user's own home: the one
    /// this launcher was told of where that is absolute, else this process's.
    /// A home the child is started with is never that, since an entry can set
    /// its own. That home is asked for only where it is used; where it is and
    /// no home of the user's is known, the program is not started.
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
        let home = self
            .home
            .clone()
            .filter(|home| home.is_absolute())
            .or_else(|| self.asks_home.then(std::env::home_dir).flatten())
            .filter(|home| home.is_absolute());
        let places = home.as_deref().map(|home| Places {
            home,
            crucible: own.crucible,
            sessions: self.sessions,
        });
        let places = || places.as_ref().ok_or_else(|| homeless(own.program));
        if own.started {
            self.probed_now(agent, own.program, own.crucible, places)
                .await?;
        }
        if !crate::own_program::judged(policy, &agent.row) {
            return Ok(policy.clone());
        }
        Ok(crate::own_program::launched(
            policy,
            &agent.row,
            own.program.path(),
            places()?,
        )?)
    }

    /// Holds a start of `agent`, whose program at `program` is the user's
    /// own, to its probe record under `crucible`, crucible's home: a catalog
    /// agent's program that changed since its probe, as one that updates
    /// itself does, is probed again, confined, before it is started, against
    /// the places `places` gives, and is started only where it is still
    /// admitted; its record is then replaced, and removed where it is not. A
    /// program of any other row is held to nothing here.
    ///
    /// # Errors
    ///
    /// [`Unlaunched::Changed`] where it is no longer admitted,
    /// [`Unbelieved::OwnUnprobed`] where no record holds for it, and why
    /// `places` gives none where it is probed again.
    async fn probed_now<'p>(
        &self,
        agent: &Agent,
        program: &Program,
        crucible: &Path,
        places: impl FnOnce() -> Result<&'p Places<'p>, Unowned>,
    ) -> Result<(), Unlaunched> {
        let AgentRow::Catalog(catalog) = &agent.row else {
            return Ok(());
        };
        let Some(owned) = Owned::of(crucible, &agent.instance, &agent.row, program.path()) else {
            return Ok(());
        };
        let owned = owned?;
        match owned.record() {
            Some(record) if record.current(owned.canonical()) => Ok(()),
            Some(_) => crate::own_program::reprobe(
                Arc::clone(&self.service),
                self.runtime.clone(),
                places()?,
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

/// Why the user's own `program` is not started where its directory would be
/// judged against a home of the user's and none is known.
fn homeless(program: &Program) -> Unowned {
    Unowned::Root(format!(
        "no home directory of yours is known, so whether the directory {} lies in may be \
         given to an agent cannot be judged; set HOME, or USERPROFILE on Windows, to your \
         home directory",
        program.path().display()
    ))
}

impl Steps {
    /// [`Steps::account`], with what the program it started answered
    /// `initialize` with of itself, where that answer was heard and admits
    /// it.
    pub async fn account_answered(
        &self,
        agent: &ExternalAgent,
        step: Account,
    ) -> (Reading, Option<Introduced>) {
        let unreadable = |why: &str| Reading::Unreadable(Failure::new(why));
        let chosen = match Agent::believing(agent, self.registry, &self.state, |name| {
            (name == "PATH").then(|| self.path.clone()).flatten()
        }) {
            Ok(chosen) => chosen,
            Err(Unbelieved::NoLaunch) => {
                return (
                    unreadable("this release has no launch for the agent's driver"),
                    None,
                );
            }
            Err(why) => return (unreadable(&why.to_string()), None),
        };
        // The releases on offer as they are now, which the agent is admitted as.
        let trust = Trust::load(&self.state, SystemTime::now());
        let dialect = match chosen.dialect(
            Mode::default(),
            &trust,
            &programs_beside(&self.state),
            self.sandboxing(),
        ) {
            Ok(dialect) => dialect,
            Err(refused) => return (Reading::Unreadable(refused), None),
        };
        drop(trust);
        if self.controls.unstopped(agent.name()) {
            return (unreadable(&Refusal::Unstopped.to_string()), None);
        }
        let Some(control) = Control::new() else {
            return (unreadable("no control for the agent's program"), None);
        };
        let policy = self.policy();
        let mut hearing = Hearing {
            wire: dialect,
            answered: None,
        };
        let reading = self
            .launcher(&policy)
            .account(&chosen, &control, &mut hearing, step, &|_| {})
            .await
            .unwrap_or_else(|refused| unreadable(&refused.to_string()));
        (reading, hearing.answered)
    }
}

/// A wire that keeps what its program last answered `initialize` with of
/// itself, and is otherwise the wire it holds.
struct Hearing {
    /// The wire, which says what is sent and admits what is answered.
    wire: Box<dyn Dialect>,
    /// What the program answered of itself, once that was heard.
    answered: Option<Introduced>,
}

impl Hearing {
    /// Notes `frame`, where it answers `initialize` with a result that admits.
    fn noted(&mut self, frame: &str) {
        if let Some(answered) = Introduced::heard(frame) {
            self.answered = Some(answered);
        }
    }
}

impl Dialect for Hearing {
    fn program(&self) -> &str {
        self.wire.program()
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        self.wire.request(shape)
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        self.wire.open(turn)
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        let acts = self.wire.heard(frame)?;
        self.noted(frame);
        Ok(acts)
    }

    fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        self.wire.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.wire.cancel()
    }

    fn account(&mut self, step: Account, shape: Shape<'_>) -> Option<Request> {
        self.wire.account(step, shape)
    }

    fn read(&self, step: Account, ran: &Ran) -> Reading {
        self.wire.read(step, ran)
    }

    fn spoken(&mut self, step: Account) -> Option<Vec<String>> {
        self.wire.spoken(step)
    }

    fn told(&mut self, step: Account, frame: &str) -> Result<Told, Failure> {
        let told = self.wire.told(step, frame)?;
        self.noted(frame);
        Ok(told)
    }

    fn signing_in_with(&mut self, id: &str) -> bool {
        self.wire.signing_in_with(id)
    }
}
