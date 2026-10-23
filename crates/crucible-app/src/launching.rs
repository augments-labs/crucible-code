//! Where an installed agent is started, and the one place it is.
//!
//! An [`Agent`] is a registry row and what its configured executable was
//! found to be. Finding it is a look at the filesystem through
//! `crucible_builtins::program`: an absolute path is taken as written, and a
//! bare name is looked for on the absolute elements of `PATH`, never relative
//! to a workspace. Nothing is run to find it, and nothing found is run until a
//! [`Launcher`] starts it.
//!
//! A start is refused, before any process exists, in this order: a row this
//! release does not ship, with the sentence naming its documentation; a
//! program not found or not executable, with the sentences saying what to do;
//! and a route with no yes, with what to ask. Only then is the hold taken from
//! `content_use`, the child's sandbox policy built, and the process started.
//! The first two are decided in one place, [`Agent::program`], which an
//! operation that would ask about a route before starting the agent consults
//! first, through [`Steps::refusal`], or for a conversation's next turn
//! through the [`Relaunch::refusal`] of its [`Runs`], in the words the launch
//! would be refused in, so nobody is asked to agree to a launch that is
//! refused whatever the answer.
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
//! metadata protected beneath any writable root, and the hosts its row names
//! are reachable where the conversation's commands are confined, since an
//! unconfined child reaches the host's network already. Every other rule, the
//! protected metadata included, stays as the conversation has it, so a
//! profile directory inside one, named directly or through a link, is refused
//! rather than opened.
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
//! process, as a run launched beside that one would be; [`Steps::unstopped`]
//! says which of a set of entries are so held, for an operation that would take
//! a step for all of them. A step whose own stop could not be confirmed holds
//! back none of the entry's runs. What it read is all it returns: a step that
//! could not be started reads as unreadable, saying why, so a caller claims
//! nothing the step did not establish.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use crucible_config::{Driver, ExternalAgent};
use crucible_execution::{Backend, Capabilities, Capability, Failure, SessionRef, Text};
use crucible_external_harnesses::{
    Account, Control, Dialect, Discovery, Host, Launch, PREPARING, Program, Reading, Refusal,
    Request, Shape,
};
use crucible_models::Effort;
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxDomainPattern, SandboxDomainPolicy, SandboxEnablement, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxNetworkPolicy,
    SandboxNetworkProvenance, SandboxPolicy, SandboxPolicyError, SandboxResourceLimits,
    SandboxService,
};
use crucible_session::{ExternalRef, RouteBinding};
use crucible_tools::Mode;
use tokio::runtime::Handle;

use crate::access::Route;
use crate::backends::{Registry, Row};
use crate::content_use::{self, Consent, Unheld, Warned};
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
    /// Its route is not one this build warns about.
    #[error("{0} is not an agent route this build knows, so nothing is started on it.")]
    Unknown(String),
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
    row: &'static Row,
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

/// The model and effort an agent of `driver` is started on, from what its
/// entry names: none where the driver chooses its own model.
fn started(
    driver: Driver,
    model: Option<&str>,
    effort: Option<Effort>,
) -> (Option<Box<str>>, Option<Effort>) {
    if !driver.takes_model() {
        return (None, None);
    }
    match providers::agent_started(driver.provider(), model, effort) {
        Some((model, effort)) => (Some(model), effort),
        None => (model.map(Into::into), effort),
    }
}

impl Agent {
    /// The agent `route` names, as `registry` knows its driver, with its
    /// program looked for through `lookup`; nothing where `route` names no
    /// installed agent or `registry` has no row for its driver.
    #[must_use]
    pub fn routed(
        route: &Route,
        registry: &Registry,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Option<Self> {
        let Route::External {
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
        let (model, effort) = started(*driver, model.as_deref(), *effort);
        Some(Self {
            row: registry.row(*driver)?,
            discovery: discover(executable, lookup),
            profile: profile.as_deref().map(PathBuf::from),
            model,
            effort,
        })
    }

    /// The agent the configuration's entry `agent` is, as `registry` knows
    /// its driver, with its program looked for through `lookup`; nothing where
    /// `registry` has no row for its driver.
    #[must_use]
    pub fn configured(
        agent: &ExternalAgent,
        registry: &Registry,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Option<Self> {
        let (model, effort) = started(agent.driver(), agent.model(), agent.effort());
        Some(Self {
            row: registry.row(agent.driver())?,
            discovery: discover(agent.executable(), lookup),
            profile: agent.profile().map(PathBuf::from),
            model,
            effort,
        })
    }

    /// Its registry row.
    #[must_use]
    pub const fn row(&self) -> &'static Row {
        self.row
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
        let dialect = (self.row.dialect)(mode).map_err(|refused| refused.to_string())?;
        match &self.discovery {
            Discovery::Found(_) => Ok(()),
            Discovery::Missing => Err(format!(
                "no {} command on PATH and none at externalAgents.{instance}.executable",
                dialect.program()
            )),
            Discovery::NotExecutable(path) => Err(format!("{} is not executable", path.display())),
        }
    }

    /// Its program, where nothing refuses a start before any process: its row
    /// is shipped, and its program was found. This is the one place those two
    /// are decided, in that order, so that a question put before a start and
    /// the start itself refuse the same launch for the same reason.
    ///
    /// # Errors
    ///
    /// [`Unlaunched::Withheld`] for a row this release does not ship, else
    /// [`Unlaunched::NotInstalled`] for a program not found or not
    /// executable, else [`Unlaunched::Unknown`] where its route has no
    /// sentences to say what to do.
    pub fn program(&self, consent: &Consent) -> Result<&Program, Unlaunched> {
        let row = self.row;
        let shown = consent
            .routes()
            .warned(row.route)
            .map_or(row.driver.name(), |warned| warned.shown);
        if !row.shipping {
            return Err(Unlaunched::Withheld {
                agent: shown.to_owned(),
                documented: row.documented(),
            });
        }
        match &self.discovery {
            Discovery::Found(program) => Ok(program),
            missing @ (Discovery::Missing | Discovery::NotExecutable(_)) => {
                // The command is the same in every mode, and asking is a mode
                // every row runs in.
                let command = (row.dialect)(crucible_tools::Mode::default())
                    .map_or(row.driver.name(), |dialect| dialect.program());
                Err(
                    installing::refusal(row, missing, shown, command).map_or_else(
                        || Unlaunched::Unknown(row.route.to_owned()),
                        Unlaunched::NotInstalled,
                    ),
                )
            }
        }
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
    let route = RouteBinding::new(entry.driver().name(), entry.name(), entry.profile())
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

/// Why the installed agent `instance`, which `provider` is reached through,
/// is not started: it is disconnected, and the two ways back.
pub(crate) fn disconnected(instance: &str, provider: &str) -> String {
    format!(
        "it is disconnected: set externalAgents.{instance}.enabled to true in the user \
         configuration file, or remove providers.{provider}.access there to reach \
         {provider} directly"
    )
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
}

impl std::fmt::Debug for Launcher<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Launcher").finish_non_exhaustive()
    }
}

impl<'a> Launcher<'a> {
    /// Starts children under `service` on `runtime`, each under a policy
    /// widened from `conversation` and held by `consent`.
    #[must_use]
    pub fn new(
        service: Arc<dyn SandboxService>,
        runtime: Handle,
        conversation: &'a SandboxPolicy,
        consent: &'a Consent,
    ) -> Self {
        Self {
            service,
            runtime,
            conversation,
            consent,
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
        let row = agent.row;
        let program = agent.program(self.consent)?.clone();
        let held = content_use::held(self.consent, row.route).map_err(|unheld| match unheld {
            Unheld::Asks(warned) => Unlaunched::Asks(warned),
            Unheld::Unknown => Unlaunched::Unknown(row.route.to_owned()),
        })?;
        let policy = policy(
            self.conversation,
            agent.profile.as_deref(),
            row.reaches,
            life.limits(self.conversation.limits()),
        )?;
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
            agent,
            instance: instance.into(),
            control: controls.of(instance),
        }
    }
}

impl Relaunch for Runs {
    fn admits(&self, mode: Mode) -> Result<(), Failure> {
        (self.agent.row().dialect)(mode).map(drop)
    }

    fn model(&self) -> Option<&str> {
        self.agent.model()
    }

    fn effort(&self) -> Option<Effort> {
        self.agent.effort()
    }

    fn refusal(&self) -> Option<Failure> {
        self.agent
            .program(&self.consent)
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
            let mut dialect = (self.agent.row().dialect)(mode)?;
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
        let route = RouteBinding::new(entry.driver().name(), entry.name(), entry.profile())
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
    /// would be refused before any process, as [`Agent::program`] decides it;
    /// nothing where it would not be.
    #[must_use]
    pub fn refusal(&self, row: &'static Row, agent: &ExternalAgent) -> Option<Unlaunched> {
        Agent {
            row,
            discovery: discover(agent.executable(), |name| {
                (name == "PATH").then(|| self.path.clone()).flatten()
            }),
            profile: None,
            model: None,
            effort: None,
        }
        .program(&self.consent)
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
        let mut dialect = match (chosen.row.dialect)(Mode::default()) {
            Ok(dialect) => dialect,
            Err(refused) => return Reading::Unreadable(refused),
        };
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
