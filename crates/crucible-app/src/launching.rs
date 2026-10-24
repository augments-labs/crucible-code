//! Where an installed agent is started, and the one place it is.
//!
//! An [`Agent`] is a registry row, one built in or one made from the catalog
//! entry of the agent its instance names, and what its configured executable
//! was found to be. Finding it is a look at the filesystem through
//! `crucible_builtins::program`: an absolute path is taken as written, and a
//! bare name is looked for on the absolute elements of `PATH`, never relative
//! to a workspace. Nothing is run to find it, and nothing found is run until a
//! [`Launcher`] starts it.
//!
//! A session on an agent's route is bound to the agent as `bound` names
//! it: its driver's word, or for a catalog agent `catalog:<key>`, so the
//! session goes on only on the agent that ran it and never on another agent
//! its instance was pointed at since.
//!
//! A start is refused, before any process exists, in this order: a row this
//! release does not ship, with the sentence naming its documentation; a
//! catalog agent whose entry gives no statement of content use, with the
//! reason, since nothing installed could change that; a program not found or
//! not executable, with the sentences saying what to do; a program that lies
//! in a version in crucible's install store, `programs` beside the state
//! directory, whose release a revocation the trust state holds names, with
//! the catalog's reason where a catalog still held gives it and the offer to
//! roll back or update, or while the trust state cannot be used, naming its
//! file, since then no revocation can be known; and a route with no yes, with
//! what to ask. A catalog row always ships, since it is listed only for an
//! entry the catalog reviewed. Only then is the hold taken from
//! `content_use`, the child's sandbox policy built, and the process started.
//! The first four are decided in one place, [`Agent::release`], after
//! [`Agent::program`] has decided the first three, which an operation that
//! would ask about a route before starting the agent consults first, through
//! [`Steps::refusal`], or for a conversation's next turn through the
//! [`Relaunch::refusal`] of its [`Runs`], in the words the launch would be
//! refused in, so nobody is asked to agree to a launch that is refused
//! whatever the answer. Stopping a revoked agent that already runs needs no
//! one's consent, and none is asked.
//!
//! The releases of a built-in agent that the catalog offers are the trust
//! state's as it is at the time of a start: a [`Launcher`], and the [`Steps`]
//! and [`Runs`] made from one, are given the directory it is kept under, and
//! load it where a start decides, so a catalog taken in or a release revoked
//! since the agent was selected is the one the start is held to. What an agent
//! built in is admitted as, and started with, is what those releases say; with
//! the trust state unusable it is admitted as none.
//! The catalog intake starts an agent's program too, with `probing`: from
//! the directory its release was unpacked into and the entry that describes it,
//! under a policy that requires the sandbox, closes the network and leaves that
//! directory read-only. No route was asked about content use, since all the
//! program is sent is one fixed `initialize` that carries nothing a user wrote.
//! Confined, the sandbox closes the network, so nothing it is sent can be
//! passed on. Where this machine's sandbox cannot close the network the start
//! is refused, unless the caller says the run is on a disposable machine, which
//! [`crate::probing`] allows only to a process that says so, and then the
//! program is started unconfined, with the network the machine has and nothing
//! of a user's on it, and the output says so.
//! A probe and an account step are starts like a turn's and go through the
//! same gates, each under a hold of its own. An account step the dialect
//! speaks over the program's own wire is bounded: a version or status within
//! the time a launch has to prepare, a sign-in within five minutes.
//!
//! On macOS and Linux a profile directory is made private to the user,
//! created owner-only where it is missing and closed to everyone else where
//! it is not, before the process that keeps its sign-in there is started.
//! Elsewhere it is created where it is missing and its access left as it is.
//!
//! The child's policy is the conversation's, widened in exactly two ways: its
//! profile directory, where one is configured, is writable but for the
//! metadata protected beneath any writable root, and the hosts its row names,
//! for a catalog row those its entry's profile grants, are reachable where the
//! conversation's commands are confined, since an unconfined child reaches the
//! host's network already. Every other rule, the protected metadata included,
//! stays as the conversation has it, so a profile directory inside one, named
//! directly or through a link, is refused rather than opened. A program in a
//! version of the install store is started with that version's directory
//! read-only, at and beneath it, whatever writable root it lies in, so it can
//! change none of the bytes that were reviewed, and a profile directory inside
//! it is refused; and with the variable its catalog entry turns the agent's
//! own updater off with, where the catalog still lists the release with one.
//!
//! A switch the agents door makes is smoke-tested through [`Launcher::smoke`]
//! only where the sandbox is on, the network it leaves commands is closed and
//! the agent's route already has its yes, which [`Launcher::unsmokable`]
//! decides, the yes to the install standing in for none of it. The program is
//! started once, under that route's hold, reaching no host, with its version's
//! directory read-only and nothing else of the user's readable, and is sent
//! `initialize` and nothing more: the agent's wire reads and admits the answer,
//! and the step that would go on to open a session is ended there.
//!
//! A run that answers the conversation's turns is held to none of the ceilings
//! that measure one command's life, its wall time, its processor time and its
//! output, since it lives as long as the conversation and the host bounds each
//! message it reads. An account step is one command and keeps all three.
//!
//! A conversation on an agent's route launches through [`Runs`]: each launch
//! takes the policy the conversation's commands would run under at that
//! moment, confined or not as the sandbox is then, so a run started after the
//! sandbox was turned off or on is held to the same choice a command would be.
//! Every run of one [`Runs`] is launched on one control, so none starts beside
//! another still held or one that could not be confirmed stopped. The controls
//! are kept by [`Controls`], one per `externalAgents` entry, and every [`Runs`]
//! is made on them: the one for the route a conversation is opened on at
//! startup, by [`Runs::new`], and those [`Steps`] hands out. They share one
//! control between them for one entry, so a route rebuilt for that entry, by a
//! resume, a move onto the agent or a connect, starts no run beside one the
//! route it replaces left held. Startup makes the [`Controls`] once, before
//! the first route and whether or not [`Steps`] can be built.
//!
//! An account step on any instance the configuration names, the conversation's
//! route or not, is taken through [`Steps`], under the same policy at that
//! moment and the same gates, and so is the run of a session an agent ran,
//! picked back up on that agent from elsewhere. An account step runs on a
//! control of its own, so it takes no part in the one-at-a-time bound of its
//! entry's runs, and a status check beside a live run is never refused as busy.
//! Of the control those runs share it reads one thing, whether a run could not
//! be confirmed stopped, and where one could not it is refused, before any
//! process, as a run launched beside that one would be; `Steps::unstopped`
//! says which of a set of entries are so held, for an operation that would take
//! a step for all of them. A step whose own stop could not be confirmed holds
//! back none of the entry's runs. What it read is all it returns: a step that
//! could not be started reads as unreadable, saying why, so a caller claims
//! nothing the step did not establish.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use crucible_config::{Driver, ExternalAgent, Serves};
use crucible_execution::{
    Backend, Capabilities, Capability, Failure, SessionRef, Text, TurnRequest,
};
use crucible_external_harnesses::acp::{Acp, Allowing};
use crucible_external_harnesses::catalog::{
    CatalogProfile, Entry, Target, Trust, Unprofiled, Unusable, Variable,
};
use crucible_external_harnesses::store::Store;
use crucible_external_harnesses::{
    Account, Act, Control, Dialect, Discovery, Held, Host, Launch, PREPARING, Program, Ran,
    Reading, Refusal, Request, Shape, Told,
};
use crucible_models::Effort;
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxCapability, SandboxDomainPattern, SandboxDomainPolicy, SandboxEnablement,
    SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy, SandboxPolicyError,
    SandboxResourceLimits, SandboxService,
};
use crucible_session::{ExternalRef, RouteBinding};
use crucible_tools::Mode;
use crucible_types::AgentKey;
use tokio::runtime::Handle;

use crate::access::Route;
use crate::backends::{AgentRow, Registry};
use crate::content_use::{self, Consent, Unheld, Unstated, Warned, agent_route};
use crate::external::External;
use crate::installing;
use crate::outside::Relaunch;
use crate::providers;

/// Why nothing was started.
#[derive(Debug, thiserror::Error)]
pub enum Unlaunched {
    /// This release does not run this agent.
    #[error("{agent} is not available in this release; see {documented}.")]
    Withheld {
        /// The agent, as the panel names it.
        agent: String,
        /// Where it is documented.
        documented: String,
    },
    /// Its program is absent or cannot be run: the sentence, and what to do.
    #[error("{} {}", .0[0], .0[1])]
    NotInstalled([String; 2]),
    /// Its route has no yes, and this is what to ask.
    #[error("{} needs an answer about content use before it is started.", .0.shown)]
    Asks(Box<Warned>),
    /// Its catalog entry gives no statement of content use a launch can be
    /// asked under.
    #[error("{agent} cannot be launched: {why}.")]
    Unstated {
        /// The agent, as its entry names it.
        agent: String,
        /// Why its entry gives none.
        why: Unstated,
    },
    /// Its route is not one this build warns about.
    #[error("{0} is not an agent route this build knows, so nothing is started on it.")]
    Unknown(String),
    /// The release its program is in was revoked by the catalog.
    #[error(
        "{agent} {version} was revoked by the agent catalog{}, so it is not started; roll it \
         back to the version installed before it, or update it to the release the catalog \
         offers now.",
        .reason.as_deref().map_or_else(String::new, |reason| format!(" ({reason})"))
    )]
    Revoked {
        /// The agent, as its row names it.
        agent: String,
        /// The version revoked.
        version: Box<str>,
        /// Why, as the catalog said it and quoted, where a catalog still held
        /// says.
        reason: Option<Box<str>>,
    },
    /// Its program is in crucible's install store, and the trust state that
    /// says whether its release was revoked cannot be used.
    #[error(
        "The catalog trust state at {} cannot be used ({why}), so whether {agent}'s release \
         was revoked is not known and it is not started. Repair or remove that file.",
        .file.display()
    )]
    Untrusted {
        /// The agent, as its row names it.
        agent: String,
        /// The file the trust state is kept in.
        file: PathBuf,
        /// Why it cannot be used.
        why: Unusable,
    },
    /// Its profile directory could not be made private to the user.
    #[error("the profile directory could not be made private to you: {0}")]
    Profile(std::io::Error),
    /// The sandbox policy its child would run under is not a valid one.
    #[error("the sandbox policy for this agent could not be built: {0}")]
    Policy(#[from] SandboxPolicyError),
    /// The host refused the launch.
    #[error(transparent)]
    Refused(#[from] Refusal),
}

/// An agent as it was selected: its row, and what its program was found to
/// be.
#[derive(Debug, Clone)]
pub struct Agent {
    /// Its registry row.
    row: AgentRow,
    /// What looking for its program found, and what the program was then.
    discovery: Discovery,
    /// Its profile directory, where one is configured.
    profile: Option<PathBuf>,
    /// The model it is started with, where its driver takes one: its
    /// entry's, else its provider's default.
    model: Option<Box<str>>,
    /// The effort it is started at, where its model takes one: its entry's
    /// where the model serves it, else the model's default rung.
    effort: Option<Effort>,
}

/// The model and effort an agent of `driver`, serving `serves`, is started
/// on, from what its entry names: none where the driver chooses its own
/// model, as a catalog agent does.
fn started(
    driver: Driver,
    serves: &Serves,
    model: Option<&str>,
    effort: Option<Effort>,
) -> (Option<Box<str>>, Option<Effort>) {
    let Some(provider) = serves.provider().filter(|_| driver.takes_model()) else {
        return (None, None);
    };
    match providers::agent_started(provider, model, effort) {
        Some((model, effort)) => (Some(model), effort),
        None => (model.map(Into::into), effort),
    }
}

/// The driver a session run by an instance of `driver` is bound to, as its
/// route names it: the driver's word, and for a catalog agent
/// `catalog:<key>`, so a session one catalog agent ran never goes on on
/// another its instance was pointed at since.
#[must_use]
pub(crate) fn bound(driver: Driver, agent: Option<&AgentKey>) -> Cow<'static, str> {
    match (driver, agent) {
        (Driver::Catalog, Some(agent)) => Cow::Owned(format!("{}:{agent}", driver.name())),
        (Driver::Claude | Driver::Antigravity | Driver::Catalog, _) => Cow::Borrowed(driver.name()),
    }
}

/// The driver of the agent a route names `written`, as [`bound`] spells it,
/// where it is one this build has.
#[must_use]
pub(crate) fn bound_driver(written: &str) -> Option<Driver> {
    match written.split_once(':') {
        Some((driver, agent)) => (driver == Driver::Catalog.name() && AgentKey::new(agent).is_ok())
            .then_some(Driver::Catalog),
        None => Driver::ALL.into_iter().find(|one| one.name() == written),
    }
}

impl Agent {
    /// The agent `route` names, as `registry` knows its driver and the
    /// catalog agent it serves, with its program looked for through `lookup`;
    /// nothing where `route` names no installed agent or `registry` has no
    /// row for it.
    #[must_use]
    pub fn routed(
        route: &Route,
        registry: &Registry,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Option<Self> {
        let Route::External {
            serves,
            driver,
            executable,
            profile,
            model,
            effort,
            ..
        } = route
        else {
            return None;
        };
        let (model, effort) = started(*driver, serves, model.as_deref(), *effort);
        Some(Self {
            row: registry.row(*driver, serves.agent())?,
            discovery: discover(executable, lookup),
            profile: profile.as_deref().map(PathBuf::from),
            model,
            effort,
        })
    }

    /// The agent the configuration's entry `agent` is, as `registry` knows
    /// its driver and the catalog agent it names, with its program looked for
    /// through `lookup`; nothing where `registry` has no row for it.
    #[must_use]
    pub fn configured(
        agent: &ExternalAgent,
        registry: &Registry,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Option<Self> {
        let (model, effort) = started(
            agent.driver(),
            &agent.serves(),
            agent.model(),
            agent.effort(),
        );
        Some(Self {
            row: registry.row(agent.driver(), agent.agent())?,
            discovery: discover(agent.executable(), lookup),
            profile: agent.profile().map(PathBuf::from),
            model,
            effort,
        })
    }

    /// Its registry row.
    #[must_use]
    pub const fn row(&self) -> &AgentRow {
        &self.row
    }

    /// The model it is started with, where its driver takes one.
    #[must_use]
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
    }

    /// The effort it is started at, where its model takes one.
    #[must_use]
    pub const fn effort(&self) -> Option<Effort> {
        self.effort
    }

    /// The same agent started on `model` at `effort`.
    #[must_use]
    pub fn on(&self, model: Option<&str>, effort: Option<Effort>) -> Self {
        Self {
            model: model.map(Into::into),
            effort,
            ..self.clone()
        }
    }

    /// Where its program was found, where it was.
    #[must_use]
    pub fn installed(&self) -> Option<&Path> {
        match &self.discovery {
            Discovery::Found(program) => Some(program.path()),
            Discovery::Missing | Discovery::NotExecutable(_) => None,
        }
    }

    /// What looking for its program found.
    #[must_use]
    pub const fn discovery(&self) -> &Discovery {
        &self.discovery
    }

    /// Whether it, the `externalAgents` entry `instance`, can be started in
    /// `mode`: found now, where a turn would find it only once a log had been
    /// written that no agent ever ran.
    ///
    /// # Errors
    ///
    /// What to say where `mode` has no launch for it or its program cannot be
    /// run.
    pub fn startable(&self, mode: Mode, instance: &str) -> Result<(), String> {
        self.row
            .dialect(mode, None, None)
            .map_err(|refused| refused.to_string())?;
        match &self.discovery {
            Discovery::Found(_) => Ok(()),
            Discovery::Missing => Err(format!(
                "no {} command on PATH and none at externalAgents.{instance}.executable",
                self.row.command()
            )),
            Discovery::NotExecutable(path) => Err(format!("{} is not executable", path.display())),
        }
    }

    /// Its program, where nothing refuses a start before any process: its row
    /// is shipped, a catalog agent's entry gives a statement of content use,
    /// and its program was found. This is the one place those are decided, in
    /// that order, so that a question put before a start and the start itself
    /// refuse the same launch for the same reason. What the refusal says to do
    /// about a program not found is what `trust` offers.
    ///
    /// # Errors
    ///
    /// [`Unlaunched::Withheld`] for a row this release does not ship, else
    /// [`Unlaunched::Unstated`] for a catalog agent whose entry gives no
    /// statement of content use, else [`Unlaunched::NotInstalled`] for a
    /// program not found or not executable, else [`Unlaunched::Unknown`] where
    /// its route has no sentences to say what to do.
    pub fn program(&self, consent: &Consent, trust: &Trust) -> Result<&Program, Unlaunched> {
        let row = &self.row;
        let shown = consent
            .routes()
            .warned(row.route())
            .map_or(row.named(), |warned| &*warned.shown);
        if let AgentRow::Built(built) = row
            && !built.shipping
        {
            return Err(Unlaunched::Withheld {
                agent: shown.to_owned(),
                documented: built.documented(),
            });
        }
        if let AgentRow::Catalog(catalog) = row
            && let Err(why) = catalog.warned()
        {
            return Err(Unlaunched::Unstated {
                agent: row.named().to_owned(),
                why: why.clone(),
            });
        }
        match &self.discovery {
            Discovery::Found(program) => Ok(program),
            missing @ (Discovery::Missing | Discovery::NotExecutable(_)) => {
                Err(unfound(row, missing, shown, trust).map_or_else(
                    || Unlaunched::Unknown(row.route().to_owned()),
                    Unlaunched::NotInstalled,
                ))
            }
        }
    }

    /// Its program, as [`Agent::program`] decides, and where that program
    /// lies in a version in crucible's install store under `programs`, that
    /// release: refused before any process where a revocation `trust` holds
    /// names it, with the reason and what to do, or where `trust` cannot be
    /// used, so its revocations cannot be known. A program outside the store
    /// is the user's own, and no release of the catalog's.
    ///
    /// # Errors
    ///
    /// As [`Agent::program`], else [`Unlaunched::Untrusted`] or
    /// [`Unlaunched::Revoked`].
    pub fn release(
        &self,
        consent: &Consent,
        trust: &Trust,
        programs: &Path,
    ) -> Result<(&Program, Option<Stored>), Unlaunched> {
        let program = self.program(consent, trust)?;
        let Ok(key) = AgentKey::new(self.row.named()) else {
            return Ok((program, None));
        };
        let store = Store::new(programs.to_path_buf());
        let Some(version) = store.version_of(&key, program.path()) else {
            return Ok((program, None));
        };
        if let Some(why) = trust.unusable() {
            return Err(Unlaunched::Untrusted {
                agent: key.to_string(),
                file: trust.file(),
                why,
            });
        }
        let (named, revision) = (version.version(), version.lock_revision());
        if trust.revoked(&key, named, revision) {
            return Err(Unlaunched::Revoked {
                agent: key.to_string(),
                version: crucible_types::shown::escaped(named).into(),
                reason: trust
                    .reason(&key, named, revision)
                    .map(|reason| crucible_types::shown::escaped(reason).into()),
            });
        }
        let updater_off = trust
            .offers(Some(&key))
            .into_iter()
            .find(|offer| *offer.pin.version == *named && offer.pin.lock_revision == revision)
            .and_then(|offer| offer.entry.updater_off);
        Ok((
            program,
            Some(Stored {
                root: store.directory(&key, &version),
                updater_off,
            }),
        ))
    }

    /// The shape every start of it in `workspace` is given.
    fn shape<'a>(&'a self, resume: Option<&'a str>, workspace: &'a Path) -> Shape<'a> {
        Shape {
            resume,
            profile: self.profile.as_deref(),
            workspace: Some(workspace),
            model: self.model.as_deref(),
            effort: self.effort.map(Effort::as_str),
        }
    }
}

/// A release of the catalog's in crucible's install store, as a start of its
/// program is held to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    /// The directory of its version, which the program is started with
    /// read-only.
    root: PathBuf,
    /// The variable its entry turns the agent's own updater off with, where
    /// the catalog still lists it with one.
    updater_off: Option<Variable>,
}

/// Crucible's install store: `programs` in crucible's home, beside the
/// directory `state` the trust state is kept under.
fn programs_beside(state: &Path) -> PathBuf {
    state.parent().unwrap_or(state).join("programs")
}

/// `request` with `variable` set where there is one, in place of any value of
/// the same name, so the agent's own updater stays off whatever else it is
/// started with.
pub(crate) fn updating_off(mut request: Request, variable: Option<&Variable>) -> Request {
    if let Some(variable) = variable {
        request.environment.retain(|(name, _)| {
            if cfg!(windows) {
                !name.eq_ignore_ascii_case(&variable.name)
            } else {
                *name != *variable.name
            }
        });
        request
            .environment
            .push((variable.name.to_string(), variable.value.to_string()));
    }
    request
}

/// `policy` with `root`, where its links lead, read-only at and beneath it: a
/// rule that would let anything there be written is dropped, so the program
/// started from it can change none of the reviewed bytes it runs, and a
/// profile directory inside it is refused as one the child may not write.
///
/// # Errors
///
/// A [`SandboxPolicyError`] where `root` cannot be followed or the policy
/// left is not a valid one, as under a path the policy keeps unreadable.
pub(crate) fn read_only(
    policy: &SandboxPolicy,
    root: &Path,
) -> Result<SandboxPolicy, SandboxPolicyError> {
    let root = resolved(root)?;
    let mut filesystem: Vec<SandboxFilesystemRule> = policy
        .filesystem()
        .iter()
        .filter(|rule| {
            !(rule.path().starts_with(&root) && rule.access() == SandboxFilesystemAccess::ReadWrite)
        })
        .cloned()
        .collect();
    // A rule left at the root itself already grants no more than reading.
    if !filesystem.iter().any(|rule| rule.path() == root) {
        filesystem.push(SandboxFilesystemRule::new(
            &root,
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime,
        )?);
    }
    SandboxPolicy::new(
        policy.enabled(),
        filesystem,
        policy.working_directory(),
        policy.network().clone(),
        policy.limits(),
    )?
    .with_unreadable_patterns(policy.unreadable_patterns().iter().cloned())
    .map(|narrowed| {
        narrowed
            .with_command_policy(policy.commands().clone())
            .with_session_state(policy.persistent(), policy.snapshots())
    })
}

/// What a refusal over `discovery` says of `row`'s agent called `shown`, as the
/// sentence and what to do next: for an agent built in whose release `trust`
/// offers, that what to do is to run the command that installs it. Nothing for
/// a program that was found.
pub(crate) fn unfound(
    row: &AgentRow,
    discovery: &Discovery,
    shown: &str,
    trust: &Trust,
) -> Option<[String; 2]> {
    match row {
        AgentRow::Built(built) => {
            installing::refusal(built, discovery, shown, &row.command(), trust)
        }
        AgentRow::Catalog(_) => discovery.refusal(shown, &row.command()),
    }
}

/// Why a session an installed agent ran does not go on on that agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unresumable {
    /// No entry of the configuration is the agent as the session was opened
    /// on: the entry is gone, or names another driver or profile.
    Rerouted,
    /// The entry is, and cannot be started, for this reason.
    Unstartable(String),
}

/// The agent the session opened on `opened` goes on on: the entry of
/// `entries` that is still that agent, as `registry` knows its driver, its
/// program looked for through `lookup`, and startable in `mode`; beside it,
/// the route that entry is now, which the run records from here on rather
/// than what the log was opened with.
///
/// # Errors
///
/// [`Unresumable`] where no entry is that agent, or the one that is cannot be
/// started.
pub fn reopening(
    entries: &[ExternalAgent],
    registry: &Registry,
    lookup: impl Fn(&str) -> Option<OsString>,
    mode: Mode,
    opened: &RouteBinding,
) -> Result<(Agent, RouteBinding), Unresumable> {
    let entry = entries
        .iter()
        .find(|entry| entry.name() == opened.instance())
        .ok_or(Unresumable::Rerouted)?;
    let route = RouteBinding::new(
        &bound(entry.driver(), entry.agent()),
        entry.name(),
        entry.profile(),
    )
    .ok()
    .filter(|route| route.same_route(opened))
    .ok_or(Unresumable::Rerouted)?;
    if !entry.enabled() {
        return Err(Unresumable::Unstartable(format!(
            "it is disconnected: set externalAgents.{}.enabled to true in the user \
             configuration file",
            entry.name()
        )));
    }
    let agent = startable(entry, registry, lookup, mode).map_err(Unresumable::Unstartable)?;
    Ok((agent, route))
}

/// Why the installed agent `instance`, which serves `serves`, is not
/// started: it is disconnected, and the ways back, one of them reaching the
/// provider it serves directly where it serves one.
pub(crate) fn disconnected(instance: &str, serves: &Serves) -> String {
    match serves {
        Serves::Provider(provider) => format!(
            "it is disconnected: set externalAgents.{instance}.enabled to true in the user \
             configuration file, or remove providers.{provider}.access there to reach \
             {provider} directly"
        ),
        Serves::Agent(_) => format!(
            "it is disconnected: set externalAgents.{instance}.enabled to true in the user \
             configuration file"
        ),
    }
}

/// The agent the configuration's `entry` is, as `registry` knows its driver,
/// its program looked for through `lookup`, where it can be started in `mode`.
///
/// # Errors
///
/// What to say where `registry` has no row for its driver, `mode` has no
/// launch for it, or its program cannot be run.
pub fn startable(
    entry: &ExternalAgent,
    registry: &Registry,
    lookup: impl Fn(&str) -> Option<OsString>,
    mode: Mode,
) -> Result<Agent, String> {
    let agent = Agent::configured(entry, registry, lookup)
        .ok_or_else(|| "this release has no launch for its driver".to_owned())?;
    agent.startable(mode, entry.name())?;
    Ok(agent)
}

/// What the program configured as `executable` is: the path as written when
/// it is absolute, else what an absolute lookup on `PATH` finds.
pub fn discover(executable: &str, lookup: impl Fn(&str) -> Option<OsString>) -> Discovery {
    let written = Path::new(executable);
    let found = if written.is_absolute() {
        Some(written.to_path_buf())
    } else {
        crucible_builtins::program::on_path(
            lookup,
            &crucible_builtins::program::spelled(executable),
        )
    };
    Discovery::of(found.as_deref())
}

/// The policy an agent's child runs under: `conversation`'s, with `profile`,
/// where its links lead, writable but for the metadata protected beneath any
/// writable root, and the hosts in `reaches` reachable where the conversation
/// confines its commands, held to `limits`.
///
/// # Errors
///
/// A [`SandboxPolicyError`] where the profile is not an absolute path, cannot
/// be followed where it exists, lies under a path the conversation protects, or a host is not one a policy can
/// name.
pub fn policy(
    conversation: &SandboxPolicy,
    profile: Option<&Path>,
    reaches: &[&str],
    limits: SandboxResourceLimits,
) -> Result<SandboxPolicy, SandboxPolicyError> {
    let mut filesystem = conversation.filesystem().to_vec();
    if let Some(profile) = profile {
        // Named where it is: a rule says where a sandbox may write, and one
        // naming a link would open whatever the link leads to.
        let profile = resolved(profile)?;
        filesystem.push(SandboxFilesystemRule::new(
            &profile,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::UserConfiguration,
        )?);
        for metadata in SandboxFilesystemRule::protected_beneath(&profile)? {
            if !filesystem.iter().any(|old| old.path() == metadata.path()) {
                filesystem.push(metadata);
            }
        }
    }
    // An unconfined child already reaches the host's whole network, and a
    // grant of hosts is one an unconfined run cannot hold to, so the sandbox
    // would refuse to start it.
    let network = if reaches.is_empty() || !conversation.enabled() {
        conversation.network().clone()
    } else {
        let vendor = reaches
            .iter()
            .map(|host| SandboxDomainPattern::new(host))
            .collect::<Result<Vec<_>, _>>()?;
        SandboxNetworkPolicy::Domains(match conversation.network() {
            SandboxNetworkPolicy::Closed => {
                SandboxDomainPolicy::new(vendor, [], false, [], SandboxNetworkProvenance::User)?
            }
            SandboxNetworkPolicy::Domains(domains) => SandboxDomainPolicy::new(
                domains.allowed().iter().cloned().chain(vendor),
                domains.denied().iter().cloned(),
                domains.allow_local_binding(),
                domains.unix_sockets().iter().cloned(),
                domains.provenance(),
            )?,
        })
    };
    SandboxPolicy::new(
        conversation.enabled(),
        filesystem,
        conversation.working_directory(),
        network,
        limits,
    )?
    .with_unreadable_patterns(conversation.unreadable_patterns().iter().cloned())
    .map(|policy| {
        policy
            .with_command_policy(conversation.commands().clone())
            .with_session_state(conversation.persistent(), conversation.snapshots())
    })
}

/// How long a started agent is to live.
#[derive(Clone, Copy)]
enum Life<'a> {
    /// As long as the conversation whose turns it answers, resuming the
    /// agent's own session named, where one is.
    Conversation { resume: Option<&'a str> },
    /// As long as one account step.
    Step,
}

impl Life<'_> {
    /// The conversation's `limits` as a run of this life is held to them: a
    /// run that lives as long as the conversation has no ceiling on one
    /// command's wall time, processor time or output, each of which would end
    /// it partway through.
    const fn limits(self, limits: SandboxResourceLimits) -> SandboxResourceLimits {
        match self {
            Self::Conversation { .. } => SandboxResourceLimits {
                command_time: None,
                cpu_seconds: None,
                output_bytes: None,
                ..limits
            },
            Self::Step => limits,
        }
    }
}

/// `path` with every link in the part of it that exists followed, and the part
/// that does not yet exist joined on as written; as written where nothing of
/// it resolves.
///
/// # Errors
///
/// [`SandboxPolicyError::InvalidPath`] where a part of it exists and cannot be
/// followed, as a link leading nowhere yet: what it leads to could be made
/// later, under metadata a rule naming the link would not see.
fn resolved(path: &Path) -> Result<PathBuf, SandboxPolicyError> {
    let mut missing = Vec::new();
    let mut existing = path;
    loop {
        if let Ok(real) = std::fs::canonicalize(existing) {
            return Ok(missing
                .iter()
                .rev()
                .fold(real, |resolved, name| resolved.join(name)));
        }
        if std::fs::symlink_metadata(existing).is_ok() {
            return Err(SandboxPolicyError::InvalidPath);
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                existing = parent;
            }
            _ => return Ok(path.to_path_buf()),
        }
    }
}

/// How long a spoken account step may take: a sign-in waits on the user and
/// their browser, every other step on the agent alone.
const fn within(step: Account) -> Duration {
    match step {
        Account::SignIn => SIGNING_IN,
        Account::Version | Account::Status | Account::SignOut => PREPARING,
    }
}

/// How long a sign-in waits for the user to finish it in their browser.
const SIGNING_IN: Duration = Duration::from_mins(5);

/// Makes `profile` a directory only its owner can enter, creating it, and any
/// missing parent, that way.
fn private(profile: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        builder.mode(0o700);
        builder.create(profile)?;
        std::fs::set_permissions(profile, std::fs::Permissions::from_mode(0o700))
    }
    #[cfg(not(unix))]
    builder.create(profile)
}

/// What starts an agent's processes for one conversation.
pub struct Launcher<'a> {
    /// The sandbox every child is prepared by.
    service: Arc<dyn SandboxService>,
    /// Where each child's pipes are read and written.
    runtime: Handle,
    /// The conversation's own policy, which a child's widens.
    conversation: &'a SandboxPolicy,
    /// The content-use answers, which a launch is held by.
    consent: &'a Consent,
    /// The directory the catalog's trust state is kept under, which what a
    /// launch is told of the releases on offer is read from as it is then.
    state: &'a Path,
}

impl std::fmt::Debug for Launcher<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Launcher").finish_non_exhaustive()
    }
}

impl<'a> Launcher<'a> {
    /// Starts children under `service` on `runtime`, each under a policy
    /// widened from `conversation` and held by `consent`, told of the releases
    /// on offer by the trust state kept under `state`.
    #[must_use]
    pub fn new(
        service: Arc<dyn SandboxService>,
        runtime: Handle,
        conversation: &'a SandboxPolicy,
        consent: &'a Consent,
        state: &'a Path,
    ) -> Self {
        Self {
            service,
            runtime,
            conversation,
            consent,
            state,
        }
    }

    /// Starts `agent` for its turns, as `dialect` asks, resuming the session
    /// its program names `resume` where there is one.
    ///
    /// # Errors
    ///
    /// [`Unlaunched`] for each reason nothing was started.
    pub async fn launch(
        &self,
        agent: &Agent,
        control: &Control,
        dialect: &mut dyn Dialect,
        resume: Option<&str>,
    ) -> Result<Host, Unlaunched> {
        let request = dialect.request(agent.shape(resume, self.conversation.working_directory()));
        self.start(agent, control, request, Life::Conversation { resume })
            .await
    }

    /// Runs the account step `step` of `agent` to its end, and reads what it
    /// said through `dialect`: spoken over the program's own wire where the
    /// dialect speaks it, with each sign-in link handed to `shown` as it
    /// comes, else run as a command and read once it has ended.
    ///
    /// # Errors
    ///
    /// [`Unlaunched`] for each reason nothing was started, in which case the
    /// account is as unverified as it was before.
    // Each is an input of its own to the step, and a value gathering them
    // would be built at each caller and taken apart here.
    #[allow(clippy::too_many_arguments)]
    pub async fn account(
        &self,
        agent: &Agent,
        control: &Control,
        dialect: &mut dyn Dialect,
        step: Account,
        shown: &(dyn Fn(Text) + Sync),
    ) -> Result<Reading, Unlaunched> {
        let shape = agent.shape(None, self.conversation.working_directory());
        let Some(request) = dialect.account(step, shape) else {
            return Ok(Reading::Unreadable(Failure::new(
                "this agent has no such step",
            )));
        };
        let spoken = dialect.spoken(step);
        let host = self.start(agent, control, request, Life::Step).await?;
        if let Some(frames) = spoken {
            return Ok(host
                .speak(dialect, step, &frames, shown, within(step))
                .await);
        }
        let ran = host.run().await;
        Ok(dialect.read(step, &ran))
    }

    /// Starts `agent` with `request` for `life`, once every gate before a
    /// process has passed.
    async fn start(
        &self,
        agent: &Agent,
        control: &Control,
        request: Request,
        life: Life<'_>,
    ) -> Result<Host, Unlaunched> {
        let row = &agent.row;
        let trust = Trust::load(self.state, SystemTime::now());
        let (program, stored) =
            agent.release(self.consent, &trust, &programs_beside(self.state))?;
        let program = program.clone();
        let held = content_use::held(self.consent, row.route()).map_err(|unheld| match unheld {
            Unheld::Asks(warned) => Unlaunched::Asks(warned),
            Unheld::Unknown => Unlaunched::Unknown(row.route().to_owned()),
        })?;
        let mut policy = policy(
            self.conversation,
            agent.profile.as_deref(),
            &row.reaches(),
            life.limits(self.conversation.limits()),
        )?;
        let mut request = request;
        if let Some(stored) = &stored {
            policy = read_only(&policy, &stored.root)?;
            request = updating_off(request, stored.updater_off.as_ref());
        }
        let resume = match life {
            Life::Conversation { resume } => resume,
            Life::Step => None,
        };
        if let Some(profile) = &agent.profile {
            // Made only where the child may write it, so that a profile the
            // conversation protects is refused before anything is created.
            let profile = resolved(profile)?;
            if !policy.permits_path(&profile, SandboxFilesystemAccess::ReadWrite) {
                return Err(SandboxPolicyError::InvalidPath.into());
            }
            private(&profile).map_err(Unlaunched::Profile)?;
        }
        let mut launch = Launch::new(program, policy, held).asking(request);
        if let Some(profile) = &agent.profile {
            launch = launch.in_profile(profile.clone());
        }
        if let Some(resume) = resume {
            launch = launch.resuming(resume)?;
        }
        Ok(Host::launch(
            Arc::clone(&self.service),
            self.runtime.clone(),
            control,
            launch,
        )
        .await?)
    }
}

/// The route the intake's probe is started under. No user is asked: what a
/// probe sends the program is one fixed `initialize` that carries no content,
/// the sandbox closes the network when it can, and when it cannot the probe is
/// run only on a disposable runner with nothing of a user's on it.
const PROBING: &str = "catalog-probe";

/// How far the sandbox held a probe in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Sandbox {
    /// The sandbox was required, enforced, and had the network closed.
    Closed,
    /// The sandbox here could not close the network, for this reason, and the
    /// program was started unconfined at the caller's word.
    Unavailable(String),
}

/// A program started for the intake's probe.
#[derive(Debug)]
pub(crate) struct Probe {
    /// The running program.
    pub(crate) host: Host,
    /// The wire it is spoken to over.
    pub(crate) dialect: Acp,
    /// How far it is held in.
    pub(crate) sandbox: Sandbox,
}

/// Why nothing was started for a probe.
#[derive(Debug, thiserror::Error)]
pub enum Unprobeable {
    /// The entry lists no target for the platform this build runs on.
    #[error("the entry lists no target for {0}")]
    Platform(String),
    /// The entry gives no profile to run.
    #[error(transparent)]
    Profile(#[from] Unprofiled),
    /// The directory it was unpacked into cannot be reached.
    #[error("{} cannot be reached", .0.display())]
    Root(PathBuf, #[source] std::io::Error),
    /// Its program is not in that directory, or does not lie inside it once
    /// its links are followed.
    #[error("the program {} is not an executable inside {}", .program, .root.display())]
    Program {
        /// The program, as the entry spells it.
        program: Box<str>,
        /// The directory.
        root: PathBuf,
    },
    /// The sandbox here cannot close the network, and no run without it was
    /// asked for.
    #[error(
        "the sandbox here cannot close the network ({0}), so the program is not started; on a \
         disposable machine, run again with `--unconfined` to run `initialize` alone"
    )]
    Unclosable(String),
    /// The sandbox policy is not a valid one.
    #[error("the sandbox policy for the probe could not be built: {0}")]
    Policy(#[from] SandboxPolicyError),
    /// This process cannot number another start.
    #[error("no control for the agent's program")]
    Control,
    /// The host refused the launch.
    #[error(transparent)]
    Refused(#[from] Refusal),
}

/// Starts the program the `entry` lists for this platform, found under the
/// unpacked directory `root`, so that `initialize` can be sent to it.
///
/// The sandbox is required, the network closed and `root` read-only. Where
/// `service` cannot enforce that, the start is refused, or with `unconfined`
/// the program is started without the sandbox and the start says why.
///
/// # Errors
///
/// [`Unprobeable`] for each reason nothing was started.
pub(crate) async fn probing(
    service: Arc<dyn SandboxService>,
    runtime: Handle,
    root: &Path,
    entry: &Entry,
    unconfined: bool,
) -> Result<Probe, Unprobeable> {
    let target = entry.here().ok_or_else(|| {
        Unprobeable::Platform(format!(
            "{}-{}",
            std::env::consts::OS,
            std::env::consts::ARCH
        ))
    })?;
    let profile = CatalogProfile::from_entry(entry, target)?;
    let root =
        std::fs::canonicalize(root).map_err(|why| Unprobeable::Root(root.to_owned(), why))?;
    let unfound = || Unprobeable::Program {
        program: target.cmd.clone(),
        root: root.clone(),
    };
    // Followed, so a link the unpacked tree holds cannot lead out of it.
    let found = std::fs::canonicalize(root.join(&*target.cmd)).map_err(|_| unfound())?;
    if !found.starts_with(&root) {
        return Err(unfound());
    }
    let Discovery::Found(program) = Discovery::of(Some(&found)) else {
        return Err(unfound());
    };
    let unclosable = unclosable(service.as_ref()).await;
    if let (Some(reason), false) = (&unclosable, unconfined) {
        return Err(Unprobeable::Unclosable(reason.clone()));
    }
    let readable = SandboxFilesystemRule::new(
        &root,
        SandboxFilesystemAccess::ReadOnly,
        SandboxFilesystemProvenance::Workspace,
    )?;
    let policy = SandboxPolicy::new(
        true,
        [readable],
        &root,
        SandboxNetworkPolicy::Closed,
        // The program lives as long as the probe waits for it, which is
        // bounded by the caller, not by a ceiling on one command.
        SandboxResourceLimits {
            command_time: None,
            cpu_seconds: None,
            output_bytes: None,
            ..SandboxResourceLimits::confining()
        },
    )?
    // Without the sandbox its limits go with it, as for a command.
    .with_enabled(unclosable.is_none());
    let mut dialect = Acp::cataloged(profile, Allowing::Nothing);
    let request = dialect.request(Shape {
        workspace: Some(&root),
        ..Shape::default()
    });
    let control = Control::new().ok_or(Unprobeable::Control)?;
    let launch = Launch::new(program, policy, Held::granted(PROBING)).asking(request);
    let host = Host::launch(service, runtime, &control, launch).await?;
    Ok(Probe {
        host,
        dialect,
        sandbox: unclosable.map_or(Sandbox::Closed, Sandbox::Unavailable),
    })
}

/// Why `service` cannot hold a program in with its network closed and its
/// filesystem enforced, where it cannot; nothing where it can.
async fn unclosable(service: &dyn SandboxService) -> Option<String> {
    match tokio::time::timeout(PREPARING, service.probe()).await {
        Ok(Ok((_, capabilities))) => {
            let enforced = [SandboxFeature::NetworkDeny, SandboxFeature::Filesystem]
                .into_iter()
                .all(|feature| capabilities.claim(feature) == SandboxCapability::Enforced);
            (!enforced)
                .then(|| "it does not enforce both a closed network and the filesystem".to_owned())
        }
        Ok(Err(problem)) => Some(problem.to_string()),
        Err(_) => Some("it did not answer in time".to_owned()),
    }
}

/// Why a release is not smoke-tested before it is used, or why its smoke
/// test failed.
#[derive(Debug, thiserror::Error)]
pub enum Unsmoked {
    /// The sandbox is off.
    #[error("the sandbox is off, and a smoke test runs only inside it")]
    Unconfined,
    /// The sandbox lets the conversation's commands reach the network.
    #[error(
        "the sandbox lets commands reach the network, and a smoke test runs only with it closed"
    )]
    Open,
    /// The agent's route has not been agreed to.
    #[error("its route {0} has no yes about content use yet")]
    Unasked(String),
    /// The agent's route is not one this build asks about.
    #[error("its route {0} is not one this build asks about content use on")]
    Unknown(String),
    /// Its entry gives no profile to run it under.
    #[error(transparent)]
    Profile(#[from] Unprofiled),
    /// Its program is not one that runs.
    #[error("its program {0} is not an executable")]
    Program(String),
    /// The sandbox policy is not a valid one.
    #[error("its sandbox policy could not be built: {0}")]
    Policy(#[from] SandboxPolicyError),
    /// This process cannot number another start.
    #[error("no control for the agent's program")]
    Control,
    /// The host refused the launch.
    #[error(transparent)]
    Refused(#[from] Refusal),
    /// It did not answer initializing as its entry reviewed it.
    #[error("it did not answer initialize as its catalog entry reviewed it: {0}")]
    Answered(String),
}

/// How long a smoke test waits for the agent's answer to `initialize`.
const SMOKING: Duration = crate::probing::BOUND;

impl Launcher<'_> {
    /// Why a release of the agent `key` would not be smoke-tested here; nothing
    /// where it would be, which is only where the sandbox is on, the network
    /// it leaves the conversation's commands is closed, and the agent's route
    /// already has its yes. A yes to installing it is not that yes.
    #[must_use]
    pub fn unsmokable(&self, key: &AgentKey) -> Option<Unsmoked> {
        if !self.conversation.enabled() {
            return Some(Unsmoked::Unconfined);
        }
        if !matches!(self.conversation.network(), SandboxNetworkPolicy::Closed) {
            return Some(Unsmoked::Open);
        }
        let route = agent_route(key.as_str());
        content_use::held(self.consent, &route)
            .err()
            .map(|unheld| unsmoked(&unheld, route))
    }

    /// Starts `program`, the release `entry` lists as `target`, unpacked into
    /// `root`, once, and sends it `initialize`, where [`Launcher::unsmokable`]
    /// finds nothing against it: in the sandbox, under the route's hold, with
    /// the network closed, no host to reach, `root` read-only and nothing else
    /// readable of the user's, and its updater off. Nothing is sent past
    /// `initialize`: the wire reads the answer and admits it, and the step
    /// that would go on to open a session is stopped there.
    ///
    /// # Errors
    ///
    /// [`Unsmoked`] for why it was not started, or did not answer as its
    /// entry reviewed it.
    pub async fn smoke(
        &self,
        entry: &Entry,
        target: &Target,
        root: &Path,
        program: &Path,
    ) -> Result<(), Unsmoked> {
        if let Some(why) = self.unsmokable(&entry.key) {
            return Err(why);
        }
        let route = agent_route(entry.key.as_str());
        let held = content_use::held(self.consent, &route)
            .map_err(|unheld| unsmoked(&unheld, route.clone()))?;
        let profile = CatalogProfile::from_entry(entry, target)?;
        let Discovery::Found(found) = Discovery::of(Some(program)) else {
            return Err(Unsmoked::Program(program.display().to_string()));
        };
        let root = resolved(root)?;
        let readable = SandboxFilesystemRule::new(
            &root,
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime,
        )?;
        let policy = SandboxPolicy::new(
            true,
            [readable],
            &root,
            SandboxNetworkPolicy::Closed,
            // Bounded by the wait for its answer, not by a ceiling on one
            // command.
            SandboxResourceLimits {
                command_time: None,
                cpu_seconds: None,
                output_bytes: None,
                ..SandboxResourceLimits::confining()
            },
        )?;
        let mut dialect = Smoking(Acp::cataloged(profile, Allowing::Nothing));
        let request = updating_off(
            dialect.request(Shape {
                workspace: Some(&root),
                ..Shape::default()
            }),
            entry.updater_off.as_ref(),
        );
        let control = Control::new().ok_or(Unsmoked::Control)?;
        let launch = Launch::new(found, policy, held).asking(request);
        let host = Host::launch(
            Arc::clone(&self.service),
            self.runtime.clone(),
            &control,
            launch,
        )
        .await?;
        let Some(frames) = dialect.spoken(Account::Status) else {
            return Err(Unsmoked::Answered("it speaks no initialize".to_owned()));
        };
        match host
            .speak(&mut dialect, Account::Status, &frames, &|_| {}, SMOKING)
            .await
        {
            Reading::Version(_) => Ok(()),
            Reading::Unreadable(failure) => {
                Err(Unsmoked::Answered(failure.reason().as_str().to_owned()))
            }
            Reading::Signed(_) => Err(Unsmoked::Answered(
                "it answered as though it had been asked more".to_owned(),
            )),
        }
    }
}

/// Why a smoke test of the agent on `route` is not run, where its hold is
/// not had.
fn unsmoked(unheld: &Unheld, route: String) -> Unsmoked {
    match unheld {
        Unheld::Asks(_) => Unsmoked::Unasked(route),
        Unheld::Unknown => Unsmoked::Unknown(route),
    }
}

/// An agent's wire for a smoke test: the step that checks whether a session
/// can be opened, ended the moment its answer to `initialize` is admitted, so
/// what reads and admits that answer is the wire's own, and the frame it
/// would send next is never sent.
struct Smoking(Acp);

impl Dialect for Smoking {
    fn program(&self) -> &str {
        self.0.program()
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        self.0.request(shape)
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        self.0.open(turn)
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        self.0.heard(frame)
    }

    fn answer(
        &mut self,
        native: &str,
        decision: &crucible_execution::Decision,
    ) -> Result<String, Failure> {
        self.0.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.0.cancel()
    }

    fn account(&mut self, step: Account, shape: Shape<'_>) -> Option<Request> {
        self.0.account(step, shape)
    }

    fn read(&self, step: Account, ran: &Ran) -> Reading {
        self.0.read(step, ran)
    }

    fn spoken(&mut self, step: Account) -> Option<Vec<String>> {
        self.0.spoken(step)
    }

    fn told(&mut self, step: Account, frame: &str) -> Result<Told, Failure> {
        Ok(match self.0.told(step, frame)? {
            // Admitted: what it would send next is past `initialize`.
            Told::Send(_) => Told::Done(Reading::Version(Text::new("initialized"))),
            told @ (Told::Link(_) | Told::Nothing | Told::Done(_)) => told,
        })
    }
}

/// What each shipping dialect lets a run of its agent do: stop a turn,
/// answer a request for leave once or for the session, and resume the agent's
/// own session. Neither answers a question the agent asks with words, so that
/// is left unclaimed.
fn claims() -> Capabilities {
    Capabilities::unknown()
        .declared(Capability::Cancel, true)
        .declared(Capability::PermissionOnce, true)
        .declared(Capability::PermissionSession, true)
        .declared(Capability::Resume, true)
}

/// The control each `externalAgents` entry's runs are launched on, made when
/// the entry's first is asked for. An entry is one the configuration names, so
/// there are no more than it has. Clones share the controls.
#[derive(Clone, Default)]
pub struct Controls(Arc<Mutex<HashMap<Box<str>, Control>>>);

impl std::fmt::Debug for Controls {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Controls").finish_non_exhaustive()
    }
}

impl Controls {
    /// What every run of the entry `instance` is launched on; none once this
    /// process has numbered every control it can.
    fn of(&self, instance: &str) -> Option<Control> {
        let mut kept = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(control) = kept.get(instance) {
            return Some(control.clone());
        }
        let control = Control::new()?;
        kept.insert(instance.into(), control.clone());
        Some(control)
    }

    /// Whether a run of the entry `instance` could not be confirmed stopped;
    /// an entry no run has been asked for yet has no control, and no such
    /// run.
    fn unstopped(&self, instance: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(instance)
            .is_some_and(Control::unstopped)
    }
}

/// What launches an installed agent's runs for a conversation on its route.
pub struct Runs {
    /// The sandbox every run is prepared by.
    service: Arc<dyn SandboxService>,
    /// Where each run's pipes are read and written.
    runtime: Handle,
    /// The conversation's own policy, confining.
    policy: SandboxPolicy,
    /// Whether the sandbox confines at the moment a run is launched.
    enablement: Arc<SandboxEnablement>,
    /// The content-use answers, which every launch is held by.
    consent: Consent,
    /// The directory the catalog's trust state is kept under.
    state: PathBuf,
    /// The agent launched.
    agent: Agent,
    /// The `externalAgents` entry it is.
    instance: Box<str>,
    /// What every run is launched on, so that none is started beside another
    /// still held or one that could not be confirmed stopped; none once this
    /// process has numbered every one it can.
    control: Option<Control>,
}

impl std::fmt::Debug for Runs {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Runs")
            .field("instance", &self.instance)
            .finish_non_exhaustive()
    }
}

impl Runs {
    /// Runs of `agent`, the entry `instance`, each started as `launcher`
    /// starts one, under its policy confining as `enablement` then says, and
    /// on the control `controls` keeps for the entry.
    #[must_use]
    pub fn new(
        launcher: Launcher<'_>,
        enablement: Arc<SandboxEnablement>,
        agent: Agent,
        instance: &str,
        controls: &Controls,
    ) -> Self {
        Self {
            service: launcher.service,
            runtime: launcher.runtime,
            policy: launcher.conversation.clone(),
            enablement,
            consent: launcher.consent.clone(),
            state: launcher.state.to_owned(),
            agent,
            instance: instance.into(),
            control: controls.of(instance),
        }
    }
}

impl Runs {
    /// The agent's wire for a run in `mode`, admitting it as the releases on
    /// offer as they are now, not as they were when the agent was selected: a
    /// release installed since is one to be run.
    fn dialect(&self, mode: Mode) -> Result<Box<dyn Dialect>, Failure> {
        let trust = Trust::load(&self.state, SystemTime::now());
        self.agent
            .row()
            .dialect(mode, Some(&trust), self.agent.installed())
    }
}

impl Relaunch for Runs {
    fn admits(&self, mode: Mode) -> Result<(), Failure> {
        self.agent.row().dialect(mode, None, None).map(drop)
    }

    fn model(&self) -> Option<&str> {
        self.agent.model()
    }

    fn effort(&self) -> Option<Effort> {
        self.agent.effort()
    }

    fn refusal(&self) -> Option<Failure> {
        let trust = Trust::load(&self.state, SystemTime::now());
        self.agent
            .release(&self.consent, &trust, &programs_beside(&self.state))
            .err()
            .map(|why| refused(&why))
    }

    fn launch<'a>(
        &'a self,
        mode: Mode,
        model: Option<&'a str>,
        effort: Option<Effort>,
        resume: Option<&'a ExternalRef>,
    ) -> BoxFuture<'a, Result<Box<dyn Backend>, Failure>> {
        Box::pin(async move {
            let mut dialect = self.dialect(mode)?;
            let control = self
                .control
                .as_ref()
                .ok_or_else(|| Failure::new("no control for the agent's run"))?;
            let session = SessionRef::new(&self.instance)
                .ok_or_else(|| Failure::new("the agent's entry has no name a run can carry"))?;
            let policy = self.policy.clone().with_enabled(self.enablement.enabled());
            let agent = self.agent.on(model, effort);
            let host = Launcher::new(
                Arc::clone(&self.service),
                self.runtime.clone(),
                &policy,
                &self.consent,
                &self.state,
            )
            .launch(
                &agent,
                control,
                dialect.as_mut(),
                resume.map(ExternalRef::reference),
            )
            .await
            .map_err(|why| refused(&why))?;
            let backend: Box<dyn Backend> = Box::new(External::new(
                host,
                dialect,
                session,
                &self.instance,
                claims(),
            ));
            Ok(backend)
        })
    }
}

/// A launch refused for `why`, in the words a run's failure is told in: the
/// same whether [`Relaunch::refusal`] says it before the route is asked about
/// or the launch says it after.
fn refused(why: &Unlaunched) -> Failure {
    Failure::new(&why.to_string())
}

/// What takes an account step of any installed agent the configuration
/// names.
pub struct Steps {
    /// The sandbox every step is prepared by.
    service: Arc<dyn SandboxService>,
    /// Where each step's pipes are read and written.
    runtime: Handle,
    /// The conversation's own policy, confining.
    policy: SandboxPolicy,
    /// Whether the sandbox confines at the moment a step is started.
    enablement: Arc<SandboxEnablement>,
    /// The content-use answers, which every step is held by.
    consent: Consent,
    /// The directory the catalog's trust state is kept under.
    state: PathBuf,
    /// The installed agents a configured entry is read against.
    registry: &'static Registry,
    /// The `PATH` an agent's program is looked for on.
    path: Option<OsString>,
    /// What every [`Runs`] this hands out for an `externalAgents` entry is
    /// launched on, and what an account step of the entry reads whether one of
    /// them could not be confirmed stopped from.
    controls: Controls,
}

impl std::fmt::Debug for Steps {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Steps").finish_non_exhaustive()
    }
}

impl Steps {
    /// Steps each started as `launcher` starts one, under its policy
    /// confining as `enablement` then says, of the agents `registry` knows,
    /// their programs looked for on `path`.
    #[must_use]
    pub fn new(
        launcher: Launcher<'_>,
        enablement: Arc<SandboxEnablement>,
        registry: &'static Registry,
        path: Option<OsString>,
    ) -> Self {
        Self {
            service: launcher.service,
            runtime: launcher.runtime,
            policy: launcher.conversation.clone(),
            enablement,
            consent: launcher.consent.clone(),
            state: launcher.state.to_owned(),
            registry,
            path,
            controls: Controls::default(),
        }
    }

    /// These steps with their runs launched on `controls` rather than on
    /// controls of their own, as the runs of the route a conversation is
    /// opened on are.
    #[must_use]
    pub(crate) fn sharing(mut self, controls: Controls) -> Self {
        self.controls = controls;
        self
    }

    /// What is installed as `executable`, looked for on the `PATH` every step
    /// is.
    #[must_use]
    pub fn discover(&self, executable: &str) -> Discovery {
        discover(executable, |name| {
            (name == "PATH").then(|| self.path.clone()).flatten()
        })
    }

    /// The agent the session opened on `opened` goes on on, as [`reopening`]
    /// finds it among `entries`, its program looked for on the `PATH` every
    /// step is, startable in `mode`, and the route that entry is now.
    ///
    /// # Errors
    ///
    /// As [`reopening`].
    pub fn reopening(
        &self,
        entries: &[ExternalAgent],
        mode: Mode,
        opened: &RouteBinding,
    ) -> Result<(Agent, RouteBinding), Unresumable> {
        reopening(
            entries,
            self.registry,
            |name| (name == "PATH").then(|| self.path.clone()).flatten(),
            mode,
            opened,
        )
    }

    /// The agent the configuration's `entry` is, as [`startable`] finds it on
    /// the `PATH` every step is, beside the route a session on it is bound
    /// to.
    ///
    /// # Errors
    ///
    /// What to say where the entry cannot be started in `mode`, or its route
    /// cannot be written down.
    pub fn onto(&self, entry: &ExternalAgent, mode: Mode) -> Result<(Agent, RouteBinding), String> {
        let route = RouteBinding::new(
            &bound(entry.driver(), entry.agent()),
            entry.name(),
            entry.profile(),
        )
        .map_err(|refused| refused.to_string())?;
        let agent = startable(
            entry,
            self.registry,
            |name| (name == "PATH").then(|| self.path.clone()).flatten(),
            mode,
        )?;
        Ok((agent, route))
    }

    /// Runs of `agent`, the entry `instance`, each launched under the policy
    /// and the yeses every step is, and on the control every other [`Runs`]
    /// this hands out for that entry is launched on.
    #[must_use]
    pub fn runs(&self, agent: Agent, instance: &str) -> Runs {
        Runs {
            service: Arc::clone(&self.service),
            runtime: self.runtime.clone(),
            policy: self.policy.clone(),
            enablement: Arc::clone(&self.enablement),
            consent: self.consent.clone(),
            state: self.state.clone(),
            agent,
            instance: instance.into(),
            control: self.controls.of(instance),
        }
    }

    /// The names among `entries` of those with a run that could not be
    /// confirmed stopped, which an account step of any of them is refused
    /// beside; none where no run of theirs has been asked for yet.
    #[must_use]
    pub(crate) fn unstopped<'a>(&self, entries: &'a [ExternalAgent]) -> Vec<&'a str> {
        entries
            .iter()
            .map(ExternalAgent::name)
            .filter(|name| self.controls.unstopped(name))
            .collect()
    }

    /// Why a start of the configured `agent`, whose registry row is `row`,
    /// would be refused before any process, as [`Agent::release`] decides it;
    /// nothing where it would not be.
    #[must_use]
    pub fn refusal(&self, row: &AgentRow, agent: &ExternalAgent) -> Option<Unlaunched> {
        Agent {
            row: row.clone(),
            discovery: discover(agent.executable(), |name| {
                (name == "PATH").then(|| self.path.clone()).flatten()
            }),
            profile: None,
            model: None,
            effort: None,
        }
        .release(
            &self.consent,
            &Trust::load(&self.state, SystemTime::now()),
            &programs_beside(&self.state),
        )
        .err()
    }

    /// What the account step `step` of the configured `agent` read, run to
    /// its end; unreadable, saying why, where it could not be started or a
    /// run of the entry could not be confirmed stopped.
    pub async fn account(&self, agent: &ExternalAgent, step: Account) -> Reading {
        let unreadable = |why: &str| Reading::Unreadable(Failure::new(why));
        let Some(chosen) = Agent::configured(agent, self.registry, |name| {
            (name == "PATH").then(|| self.path.clone()).flatten()
        }) else {
            return unreadable("this release has no launch for the agent's driver");
        };
        // The releases on offer as they are now, which the agent is admitted as.
        let trust = Trust::load(&self.state, SystemTime::now());
        let mut dialect =
            match chosen
                .row
                .dialect(Mode::default(), Some(&trust), chosen.installed())
            {
                Ok(dialect) => dialect,
                Err(refused) => return Reading::Unreadable(refused),
            };
        drop(trust);
        if self.controls.unstopped(agent.name()) {
            return unreadable(&Refusal::Unstopped.to_string());
        }
        let Some(control) = Control::new() else {
            return unreadable("no control for the agent's program");
        };
        let policy = self.policy.clone().with_enabled(self.enablement.enabled());
        Launcher::new(
            Arc::clone(&self.service),
            self.runtime.clone(),
            &policy,
            &self.consent,
            &self.state,
        )
        .account(&chosen, &control, dialect.as_mut(), step, &|_| {})
        .await
        .unwrap_or_else(|refused| unreadable(&refused.to_string()))
    }
}

/// The runs of the route a conversation is opened on at startup, where it is
/// opened on an agent's, and the steps beside it, where `stepping` lets them
/// be built: the launcher, registry and `PATH` they take. Both are made on one
/// [`Controls`], made here whether or not the steps can be, so a run of the
/// route and a run the steps hand out for its entry are held against each
/// other.
#[must_use]
pub(crate) fn first_runs(
    route: Option<(Launcher<'_>, Agent, &str)>,
    stepping: Option<(Launcher<'_>, &'static Registry, Option<OsString>)>,
    enablement: Arc<SandboxEnablement>,
) -> (Option<Runs>, Option<Steps>) {
    let controls = Controls::default();
    let runs = route.map(|(launcher, agent, instance)| {
        Runs::new(
            launcher,
            Arc::clone(&enablement),
            agent,
            instance,
            &controls,
        )
    });
    let steps = stepping.map(|(launcher, registry, path)| {
        Steps::new(launcher, enablement, registry, path).sharing(controls)
    });
    (runs, steps)
}

#[cfg(test)]
pub(crate) mod tests;
