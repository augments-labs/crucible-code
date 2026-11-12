//! How a start of the user's own program is made: used as found, with no
//! probe before it, at the path its links lead to, and confined only where
//! the conversation's sandbox is on. An account step also gives back what
//! the program it started answered of itself, which a person reading the
//! user's own program signed out is shown.

use std::path::Path;
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
use crate::backends::{Sandboxing, Unbelieved, Wire};
use crate::own_program::{Introduced, Places, Trying, homeless};

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
    /// install store: as its row speaks to that under `sandbox`.
    pub(super) fn own_wire(
        &self,
        mode: Mode,
        trust: &Trust,
        installed: &Path,
        sandbox: Sandboxing,
    ) -> Wire {
        let canonical = std::fs::canonicalize(installed);
        let program = canonical.as_deref().unwrap_or(installed);
        self.row.own_dialect(mode, Some(trust), program, sandbox)
    }
}

/// A start of the user's own program, as [`Launcher::own_held`] holds it.
pub(super) struct Own<'a> {
    /// The program.
    pub(super) program: &'a Program,
    /// Crucible's home, which its directory is judged against.
    pub(super) crucible: &'a Path,
}

impl Launcher<'_> {
    /// `policy` as `agent`'s start `own` of the user's own program is
    /// confined, and the program it is started as: for a row held to its
    /// directory, the program at the path its links lead to, the path that
    /// directory is decided from. Nothing is asked of the program before it
    /// starts. Where `policy` has the sandbox off it is started under `policy`
    /// unchanged, as a plain local process; where it is on, its directory is
    /// judged against the user's own home: the one this launcher was told of
    /// where that is absolute, else this process's. A home the child is
    /// started with is never that, since an entry can set its own. That home
    /// is asked for only where it is used; where it is and no home of the
    /// user's is known, the program is not started.
    ///
    /// # Errors
    ///
    /// Why its directory cannot be given, or that the program its links lead
    /// to is not the one that was found.
    pub(super) fn own_held(
        &self,
        agent: &Agent,
        own: &Own<'_>,
        policy: &SandboxPolicy,
    ) -> Result<(SandboxPolicy, Program), Unlaunched> {
        let program = crate::own_program::started_at(&agent.row, own.program)?;
        if !crate::own_program::judged(policy, &agent.row) {
            return Ok((policy.clone(), program));
        }
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
        let places = places.as_ref().ok_or_else(|| homeless(own.program))?;
        let policy = crate::own_program::launched(policy, &agent.row, program.path(), places)?;
        Ok((policy, program))
    }
}

impl Steps {
    /// What a trial of the user's own program is found with from these
    /// steps: the home every catalog agent is given, else the shell's, their
    /// trust state, session logs and `PATH`, and whether the sandbox is on
    /// now.
    pub(crate) fn trying(&self) -> Trying {
        let home = self
            .home
            .clone()
            .filter(|home| home.is_absolute())
            .or_else(std::env::home_dir);
        Trying::new(
            home,
            (self.state.clone(), self.sessions.clone()),
            self.path.clone(),
            self.enablement.enabled(),
        )
    }

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
