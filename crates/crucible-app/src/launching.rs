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
//! reason, since nothing installed could change that; an instance that sets a
//! profile directory on a catalog agent whose entry names no variable it reads
//! its home from, with the sentence naming the instance and saying to remove
//! the directory, since the agent would never read it; a program not found or
//! not executable, with the sentences saying what to do; a program that lies
//! in a version in crucible's install store, `programs` beside the state
//! directory, whose release a revocation the trust state holds names, with
//! the catalog's reason where a catalog still held gives it and the offer to
//! roll back or update, or while the trust state cannot be used, naming its
//! file, since then no revocation can be known, or that lies there, as its
//! path is written or where it leads, in another agent's directory or in no
//! version of its own; and a route with no yes, with
//! what to ask. A catalog row always ships, since it is listed only for an
//! entry the catalog reviewed. Only then is the hold taken from
//! `content_use`, the child's sandbox policy built, and the process started.
//! The first five are decided in one place, [`Agent::release`], after
//! [`Agent::program`] has decided the first four, which an operation that
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
//! A profile directory is given to a catalog agent through the variable its
//! entry names, which is the whole of how the agent is told of it. On macOS and
//! Linux it is made private to the user, created owner-only where it is missing
//! and closed to everyone else where it is not, before the process that keeps
//! its sign-in there is started. Elsewhere it is created where it is missing
//! and its access left as it is.
//! A business sign-in to Antigravity then has the Google Cloud project and
//! location its entry names written in that directory, as the settings the
//! agent reads them from, replacing what is there and following no link; a
//! start for one with no profile directory is refused.
//!
//! The child's policy is the conversation's, widened in exactly three ways:
//! its profile directory, where one is configured, is writable but for the
//! metadata protected beneath any writable root; where the sandbox is on, and
//! the instance has no profile directory, the places under the user's home its
//! catalog entry says it keeps its sign-in in are writable in the same way, so
//! that a sign-in made under the sandbox is still there at the next start; and
//! the hosts its row names, for a catalog row those its entry's profile grants,
//! with the host of the Google Cloud location its entry names where that
//! location has one of its own, are reachable where the conversation's
//! commands are confined, since an unconfined child already reaches the host's
//! network. Every other rule, the
//! protected metadata included, stays as the conversation has it, so a profile
//! directory inside one, named directly or through a link, is refused rather
//! than opened. A program in a version of the install store is started with
//! that version's directory read-only, at and beneath it, whatever writable
//! root it lies in, so it can change none of the bytes that were reviewed, and
//! a profile directory inside it is refused; and with the variable its catalog
//! entry turns the agent's own updater off with, or where the catalog lists
//! that release no longer, the one named by the release an update would
//! install.
//!
//! An instance with a profile directory of its own is given that directory and
//! none of those places: it keeps its sign-in in the directory, through the
//! variable its entry names, and a place it could also write is the sign-in
//! another instance of the agent is signed in by.
//!
//! The places an entry keeps its sign-in in are joined to the home the child
//! is given: `HOME`, or `USERPROFILE` on Windows, read from the request that
//! is to start it, so that it is the value the child will see. Where that
//! variable is unset or not absolute, or the entry's own target or profile
//! variable sets it, no place is joined and the agent keeps no sign-in under
//! the sandbox. Each place joined becomes a writable root where its links
//! lead, created private to the user where it is missing and left as it is
//! where it is not, once the policy permits writing there. No place is made
//! writable that lies within or contains one on the list of places never made
//! writable, or the home crucible keeps its state in, however the entry spells
//! its way there: it is judged by its path components, as it is joined and
//! again where its links lead, ignoring case where the file system does and,
//! on Windows, the dots and spaces that end a name, and the refusal names the
//! instance and the path the entry gave. Where its links lead must also lie
//! within the home, so that a link from an allowed name, such as a dotfile
//! manager's, cannot lead the sandbox to a place nobody listed. A built-in
//! agent has no catalog entry, and the smoke test of a release signs nothing
//! in, so neither is given any.
//!
//! Every start, a turn's or an account step's, is given each key the user
//! typed for one of the methods its agent signs in by, kept in crucible's
//! credential store under the agent's name and the method's id, in the
//! variable that method names, where the agent's entry, or its driver's own
//! module, still lists the method. Such a variable is never one whose value
//! may be shown, so the exact value sent is withheld from all the agent is
//! seen to say, as any credential in its environment is. An agent named as a
//! provider is given none, and the store is not read for an agent that signs
//! in by no key.
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
//! picked back up on that agent from elsewhere. An account step that does not
//! sign in, a status read or a sign-out, runs on a control of its own, so it
//! takes no part in the one-at-a-time bound of its entry's runs, and a status
//! check beside a live run is never refused as busy. Of the control those runs
//! share it reads one thing, whether a run could not be confirmed stopped, and
//! where one could not it is refused, before any process, as a run launched
//! beside that one would be; `Steps::unstopped` says which of a set of entries
//! are so held, for an operation that would take a step for all of them. Such a
//! step whose own stop could not be confirmed holds back none of the entry's
//! runs. What it read is all it returns: a step that could not be started
//! reads as unreadable, saying why, so a caller claims nothing the step did not
//! establish.
//!
//! A step that signs in is held against the entry's runs instead: `Steps`
//! lends the control they share, so it is started only where no run holds it,
//! a run, or another such step, of the entry is refused as busy while it runs,
//! and one whose stop could not be confirmed leaves every later run of the
//! entry refused as an unconfirmed run does. `Steps::running` says which of a
//! set of entries have a run live, as `Steps::unstopped` says which could not
//! be confirmed stopped, for an operation that must not start beside either.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use crucible_config::{Cloud, Driver, ExternalAgent, Serves};
use crucible_execution::{
    Backend, Capabilities, Capability, Failure, SessionRef, Text, TurnRequest,
};
use crucible_external_harnesses::acp::{Acp, Allowing};
use crucible_external_harnesses::catalog::{
    CatalogProfile, Entry, Method, Target, Trust, Unprofiled, Unusable, Variable,
};
use crucible_external_harnesses::store::Store;
use crucible_external_harnesses::{
    Account, Act, Control, Dialect, Discovery, Held, Host, Launch, PREPARING, Program, Ran,
    Reading, Refusal, Request, Shape, Told, antigravity,
};
use crucible_models::Effort;
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxCapability, SandboxDomainPattern, SandboxDomainPolicy, SandboxEnablement,
    SandboxFeature, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxNetworkEndpoint, SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxPolicy,
    SandboxPolicyError, SandboxResourceLimits, SandboxService,
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
    /// Its program lies in crucible's install store where no version of its
    /// own is kept.
    #[error("{agent}'s program lies in crucible's install store {why}, so it is not started.")]
    Unstored {
        /// The agent, as its row names it.
        agent: String,
        /// Where, quoted.
        why: String,
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
    /// Its instance sets a profile directory and its catalog entry names no
    /// variable the agent reads its home from, so the directory could not be
    /// given to it.
    #[error(
        "externalAgents.{instance} sets profileDir, but {agent} offers no separate profile; \
         remove profileDir from externalAgents.{instance}."
    )]
    NoSeparateProfile {
        /// The instance, quoted.
        instance: String,
        /// The agent, as its entry names it.
        agent: String,
    },
    /// A place its catalog entry says it keeps its sign-in in is, lies within
    /// or contains a place no agent is made able to write.
    #[error(
        "externalAgents.{instance}: {agent}'s catalog entry keeps its sign-in in {path}, which \
         {} {place} ({why}), so it is not started.",
        .overlap.says()
    )]
    StatePath {
        /// The instance, quoted.
        instance: String,
        /// The agent, as its entry names it.
        agent: String,
        /// The path the entry gives, quoted.
        path: String,
        /// How it lies against `place`.
        overlap: Overlap,
        /// The place it overlaps.
        place: Box<str>,
        /// Why that place is never made writable.
        why: &'static str,
    },
    /// Its profile directory could not be made private to the user.
    #[error("the profile directory could not be made private to you: {0}")]
    Profile(std::io::Error),
    /// A place it keeps its sign-in in could not be created.
    #[error("a place the agent keeps its sign-in in could not be created: {0}")]
    StateRoot(std::io::Error),
    /// The settings a business sign-in is made with could not be written in
    /// its profile directory.
    #[error("the agent's settings could not be written in its profile directory: {0}")]
    Settings(std::io::Error),
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
    /// The `externalAgents` entry it is.
    instance: Box<str>,
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
    /// The Google Cloud location its entry names, where it names one: every
    /// run reaches that location's host besides its row's.
    location: Option<Box<str>>,
    /// The project and location a business sign-in started on it is billed
    /// to, written in its profile before it starts; only that sign-in sets
    /// it.
    billed: Option<Cloud>,
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
            instance,
            driver,
            executable,
            profile,
            model,
            effort,
            location,
            ..
        } = route
        else {
            return None;
        };
        let (model, effort) = started(*driver, serves, model.as_deref(), *effort);
        Some(Self {
            instance: instance.clone(),
            row: registry.row(*driver, serves.agent())?,
            discovery: discover(executable, lookup),
            profile: profile.as_deref().map(PathBuf::from),
            model,
            effort,
            location: location.clone(),
            billed: None,
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
            instance: agent.name().into(),
            row: registry.row(agent.driver(), agent.agent())?,
            discovery: discover(agent.executable(), lookup),
            profile: agent.profile().map(PathBuf::from),
            model,
            effort,
            location: located_in(agent),
            billed: None,
        })
    }

    /// The configuration's entry `agent`, run as `row`, with its program
    /// looked for through `lookup`: for an agent whose row no registry lists,
    /// as a catalog agent's is made from the entry the trust state offers.
    #[must_use]
    pub(crate) fn of_row(
        agent: &ExternalAgent,
        row: AgentRow,
        lookup: impl Fn(&str) -> Option<OsString>,
    ) -> Self {
        let (model, effort) = started(
            agent.driver(),
            &agent.serves(),
            agent.model(),
            agent.effort(),
        );
        Self {
            instance: agent.name().into(),
            row,
            discovery: discover(agent.executable(), lookup),
            profile: agent.profile().map(PathBuf::from),
            model,
            effort,
            location: located_in(agent),
            billed: None,
        }
    }

    /// The same agent started for a business sign-in billed to `cloud`, whose
    /// settings are written in its profile once its route has its yes, just
    /// before it is started.
    #[must_use]
    pub(crate) fn billed_to(self, cloud: Cloud) -> Self {
        Self {
            billed: Some(cloud),
            ..self
        }
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
    /// its instance sets no profile directory that its catalog agent cannot
    /// take, and its program was found. This is the one place those are
    /// decided, in that order, so that a question put before a start and the
    /// start itself refuse the same launch for the same reason. What the
    /// refusal says to do about a program not found is what `trust` offers.
    ///
    /// # Errors
    ///
    /// [`Unlaunched::Withheld`] for a row this release does not ship, else
    /// [`Unlaunched::Unstated`] for a catalog agent whose entry gives no
    /// statement of content use, else [`Unlaunched::NoSeparateProfile`] for a
    /// catalog agent whose entry names no variable it reads its home from, on
    /// an instance that sets a profile directory, else
    /// [`Unlaunched::NotInstalled`] for a program not found or not
    /// executable, else [`Unlaunched::Unknown`] where its route has no
    /// sentences to say what to do.
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
        // A directory given to an agent that reads its home from no variable
        // would be one it never reads, and it would sign in somewhere the
        // user did not choose.
        if let AgentRow::Catalog(catalog) = row
            && self.profile.is_some()
            && catalog.profile().profile_env().is_none()
        {
            return Err(Unlaunched::NoSeparateProfile {
                instance: quoted(&self.instance),
                agent: quoted(row.named()),
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
    /// used, so its revocations cannot be known. Whether it lies in the store
    /// is judged by where its path leads as well as how it is written, and
    /// one that lies there in another agent's directory, or in none of its
    /// own versions, is refused rather than taken for the user's own. A
    /// program outside the store is the user's own, and no release of the
    /// catalog's.
    ///
    /// # Errors
    ///
    /// As [`Agent::program`], else [`Unlaunched::Unstored`],
    /// [`Unlaunched::Untrusted`] or [`Unlaunched::Revoked`].
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
        let Some(owner) = stored_under(programs, program.path()) else {
            return Ok((program, None));
        };
        let unstored = |why: String| Unlaunched::Unstored {
            agent: key.to_string(),
            why,
        };
        if owner != key.as_str() {
            return Err(unstored(format!(
                "in the directory of {}, not its own",
                quoted(&owner)
            )));
        }
        let Some(version) = store.version_of(&key, program.path()) else {
            return Err(unstored("in no version the store keeps".to_owned()));
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
                version: quoted(named).into(),
                reason: trust
                    .reason(&key, named, revision)
                    .map(|reason| quoted(reason).into()),
            });
        }
        // Its own entry's variable where the catalog still lists it; else
        // that of the release an update would install, since a release
        // superseded or gone back to is still that agent's program.
        let offers = trust.offers(Some(&key));
        let updater_off = offers
            .iter()
            .find(|offer| *offer.pin.version == *named && offer.pin.lock_revision == revision)
            .or_else(|| offers.first())
            .and_then(|offer| offer.entry.updater_off.clone());
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
    /// The variable its entry turns the agent's own updater off with, or
    /// where the catalog no longer lists it, the one the release an update
    /// would install names.
    updater_off: Option<Variable>,
}

/// The most of one string the catalog, the configuration or an agent gave
/// that a refusal or a fact quotes, in characters.
const QUOTED: usize = 256;

/// `text`, a string the catalog, the configuration or an agent gave, as a
/// refusal or a fact quotes it: every control and format character written
/// as its escape, so no line of its own can be drawn from it, and cut at
/// [`QUOTED`] characters with an ellipsis, so none is made long enough to push
/// what is said beside it out of sight.
pub(crate) fn quoted(text: &str) -> String {
    let escaped = crucible_types::shown::escaped(text);
    if escaped.chars().count() <= QUOTED {
        return escaped;
    }
    let mut cut: String = escaped.chars().take(QUOTED).collect();
    cut.push('…');
    cut
}

/// Crucible's install store: `programs` in crucible's home, beside the
/// directory `state` the trust state is kept under.
fn programs_beside(state: &Path) -> PathBuf {
    state.parent().unwrap_or(state).join("programs")
}

/// The directory `program` lies in directly beneath `programs`, as either is
/// written or once both are resolved, where it lies beneath it in either
/// form; resolved first, so a link or another spelling of a directory is
/// judged by where it leads.
fn stored_under(programs: &Path, program: &Path) -> Option<String> {
    let resolved = |path: &Path| std::fs::canonicalize(path).ok();
    [
        (resolved(programs), resolved(program)),
        (Some(programs.to_path_buf()), Some(program.to_path_buf())),
    ]
    .into_iter()
    .find_map(|(root, path)| {
        let within = path?.strip_prefix(root?).ok()?.to_path_buf();
        within
            .components()
            .next()
            .map(|first| first.as_os_str().to_string_lossy().into_owned())
    })
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

/// `request` with each key the credential store in `home` keeps for `row`'s
/// agent set in the variable of the method it was typed for, where the agent
/// still signs in by that method, and that variable taken off the ones whose
/// value may be shown, so its value is withheld from what the agent says.
pub(crate) fn keyed(mut request: Request, row: &AgentRow, home: &Path) -> Request {
    let methods = row.methods();
    let variables: Vec<(&str, &str)> = methods
        .iter()
        .filter_map(|method| match &method.method {
            Method::EnvVar { var } => Some((&*method.id, &**var)),
            Method::Agent | Method::Terminal { .. } => None,
        })
        .collect();
    if variables.is_empty() {
        return request;
    }
    let Ok(agent) = AgentKey::new(row.named()) else {
        return request;
    };
    let rows = providers::Rows::production();
    let stored = crucible_auth::Store::in_home(home)
        .naming(rows.names())
        .read();
    for (id, var) in variables {
        let Ok(name) = rows.signing_in(&agent, id) else {
            continue;
        };
        if let Some(key) = stored.get(&name) {
            request.shown.retain(|shown| {
                if cfg!(windows) {
                    !shown.eq_ignore_ascii_case(var)
                } else {
                    *shown != var
                }
            });
            key.set_in(&mut request.environment, var);
        }
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

/// The policy an agent's child runs under: `conversation`'s, with `profile`
/// and each of `kept`, where their links lead, writable but for the metadata
/// protected beneath any writable root, and the hosts in `reaches` reachable
/// where the conversation confines its commands, held to `limits`.
///
/// # Errors
///
/// A [`SandboxPolicyError`] where the profile or a kept place is not an
/// absolute path, cannot be followed where it exists, lies under a path the
/// conversation protects, or a host is not one a policy can name.
pub fn policy(
    conversation: &SandboxPolicy,
    profile: Option<&Path>,
    kept: &[PathBuf],
    reaches: &[&str],
    limits: SandboxResourceLimits,
) -> Result<SandboxPolicy, SandboxPolicyError> {
    let mut filesystem = conversation.filesystem().to_vec();
    for place in profile.into_iter().chain(kept.iter().map(PathBuf::as_path)) {
        // Named where it is: a rule says where a sandbox may write, and one
        // naming a link would open whatever the link leads to.
        let place = resolved(place)?;
        filesystem.push(SandboxFilesystemRule::new(
            &place,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::UserConfiguration,
        )?);
        for metadata in SandboxFilesystemRule::protected_beneath(&place)? {
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

/// How a path an agent's entry gives lies against a place no agent is made
/// able to write, closest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Overlap {
    /// It is the place.
    Is,
    /// It lies within the place.
    Within,
    /// It contains the place.
    Contains,
    /// It does not stay inside the home it is joined to.
    Outside,
}

impl Overlap {
    /// How the refusal says it.
    const fn says(self) -> &'static str {
        match self {
            Self::Is => "is",
            Self::Within => "lies within",
            Self::Contains => "contains",
            Self::Outside => "lies outside",
        }
    }
}

/// How far a place of [`NEVER_WRITABLE`] reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reach {
    /// The place and everything beneath it.
    Beneath,
    /// The place itself only, and what contains it: the directories that hold
    /// the agents' own are named for what writing them directly does, and the
    /// agents' own are not what is refused.
    Itself,
}

/// One place under a user's home that no agent's state path may make
/// writable.
#[derive(Debug, Clone, Copy)]
struct Place {
    /// Where, relative to the home, with `/` between components.
    path: &'static str,
    /// How far it reaches.
    reach: Reach,
    /// Why it is listed, as a refusal says it.
    why: &'static str,
}

/// Why a shell's startup files are listed.
const STARTS_SHELLS: &str =
    "a shell runs it each time one starts, so what is written there runs as you";
/// Why git's own configuration is listed.
const RUNS_WHAT_GIT_NAMES: &str = "git runs the programs it names, as you, each time you use it";
/// Why the files a shell reads as a login ends are listed.
const ENDS_SHELLS: &str =
    "a shell runs it each time a login ends, so what is written there runs as you";
/// Why the places that start programs when the user signs in are listed.
const STARTS_AT_LOGIN: &str =
    "what is listed there starts each time you sign in to this machine, so it runs as you";
/// Why the files a session reads as it begins are listed.
const READ_AT_SIGN_IN: &str = "it is read each time you sign in to a session, so what is written there runs, or sets what runs, as you";
/// Why the directories programs are found in are listed.
const ON_SEARCH_PATH: &str = "the programs found there run as you from your search path";

/// The places under a user's home that an agent's state path is never made
/// writable at, beneath, or around: a path that is one of them, lies within
/// one, or contains one is refused, since a child that can write there can
/// run what it writes outside the sandbox, later, as the user, or read what
/// signs in as the user. Each is relative to the home, and is compared by its
/// components. The list is kept whole by naming each place and why, and not by
/// naming the places that are allowed, so a place that runs as the user and is
/// not named here is a place to add.
///
/// - The shell startup and logout files `.bashrc`, `.bash_profile`,
///   `.bash_login`, `.bash_logout`, `.bash_aliases`, `.profile`, `.zshrc`,
///   `.zprofile`, `.zshenv`, `.zlogin` and `.zlogout`, and the directory fish
///   reads its startup from, `.config/fish`: a shell runs them each time one
///   starts or a login ends.
/// - The files a session reads as it begins, `.xprofile`, `.xinitrc`,
///   `.xsession`, `.xsessionrc` and `.pam_environment`.
/// - `.ssh`, `.gnupg` and `.aws`: the keys that sign in to other machines, sign
///   and decrypt as the user, and the credentials of the user's cloud account.
/// - The configuration of programs that run what it names as the user, among
///   them `.gitconfig` and `.config/git`, `.cargo/config.toml`, `.npmrc` and
///   `.docker`, and `.config/environment.d`, which sets the environment of
///   every program the user starts.
/// - The directories programs are found in, `bin`, `.local/bin` and
///   `.cargo/bin`: programs there are found on the user's search path.
/// - The places that start programs when the user signs in:
///   `.config/autostart`, `.config/systemd`, `.local/share/systemd`,
///   `.local/share/applications`, `Library/LaunchAgents`,
///   `Library/LaunchDaemons` and `AppData/Roaming/Microsoft/Windows/Start Menu`,
///   which holds the Startup folder Windows runs the user's programs from.
/// - The bare `.config`, `.local`, `.local/share`, `.local/state`, `.cache`,
///   `Library`, `Library/Application Support`, `AppData`, `AppData/Roaming`
///   and `AppData/Local`, which hold every program's settings, data and caches,
///   among them the places above. Only the directories themselves are listed,
///   and what contains them: the directory of an agent lies beneath them
///   (`.config/goose`, `.local/share/goose`), and so does the home of every
///   program the user runs.
///
/// The home crucible keeps its own state in is refused beside these, since it
/// holds the keys the user typed.
const NEVER_WRITABLE: &[Place] = &[
    Place {
        path: ".bashrc",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".bash_profile",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".bash_login",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".profile",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".zshrc",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".zprofile",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".zshenv",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".config/fish",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".ssh",
        reach: Reach::Beneath,
        why: "the keys that sign in to other machines as you",
    },
    Place {
        path: ".gnupg",
        reach: Reach::Beneath,
        why: "the keys that sign and decrypt as you",
    },
    Place {
        path: ".aws",
        reach: Reach::Beneath,
        why: "the credentials of your cloud account",
    },
    Place {
        path: ".config",
        reach: Reach::Itself,
        why: "it holds the settings of every program you run, and the places that start programs when you sign in",
    },
    Place {
        path: ".local",
        reach: Reach::Itself,
        why: "it holds the data of every program you run, and the programs found on your search path",
    },
    Place {
        path: ".local/bin",
        reach: Reach::Beneath,
        why: ON_SEARCH_PATH,
    },
    Place {
        path: ".config/autostart",
        reach: Reach::Beneath,
        why: STARTS_AT_LOGIN,
    },
    Place {
        path: ".config/systemd",
        reach: Reach::Beneath,
        why: STARTS_AT_LOGIN,
    },
    Place {
        path: "Library/LaunchAgents",
        reach: Reach::Beneath,
        why: STARTS_AT_LOGIN,
    },
    Place {
        path: "AppData/Roaming/Microsoft/Windows/Start Menu",
        reach: Reach::Beneath,
        why: STARTS_AT_LOGIN,
    },
    Place {
        path: ".bash_logout",
        reach: Reach::Beneath,
        why: ENDS_SHELLS,
    },
    Place {
        path: ".bash_aliases",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".zlogin",
        reach: Reach::Beneath,
        why: STARTS_SHELLS,
    },
    Place {
        path: ".zlogout",
        reach: Reach::Beneath,
        why: ENDS_SHELLS,
    },
    Place {
        path: ".xprofile",
        reach: Reach::Beneath,
        why: READ_AT_SIGN_IN,
    },
    Place {
        path: ".xinitrc",
        reach: Reach::Beneath,
        why: READ_AT_SIGN_IN,
    },
    Place {
        path: ".xsession",
        reach: Reach::Beneath,
        why: READ_AT_SIGN_IN,
    },
    Place {
        path: ".xsessionrc",
        reach: Reach::Beneath,
        why: READ_AT_SIGN_IN,
    },
    Place {
        path: ".pam_environment",
        reach: Reach::Beneath,
        why: READ_AT_SIGN_IN,
    },
    Place {
        path: ".gitconfig",
        reach: Reach::Beneath,
        why: RUNS_WHAT_GIT_NAMES,
    },
    Place {
        path: ".config/git",
        reach: Reach::Beneath,
        why: RUNS_WHAT_GIT_NAMES,
    },
    Place {
        path: ".config/environment.d",
        reach: Reach::Beneath,
        why: "it sets the environment of every program you start, the variables that load code among them",
    },
    Place {
        path: ".local/share/systemd",
        reach: Reach::Beneath,
        why: STARTS_AT_LOGIN,
    },
    Place {
        path: ".local/share/applications",
        reach: Reach::Beneath,
        why: "the desktop entries there run as you when you open what they handle",
    },
    Place {
        path: "bin",
        reach: Reach::Beneath,
        why: ON_SEARCH_PATH,
    },
    Place {
        path: ".cargo/bin",
        reach: Reach::Beneath,
        why: ON_SEARCH_PATH,
    },
    Place {
        path: ".cargo/config.toml",
        reach: Reach::Beneath,
        why: "cargo runs the programs it names, a linker or a wrapper among them, as you each time you build",
    },
    Place {
        path: ".npmrc",
        reach: Reach::Beneath,
        why: "npm takes the registry, the tokens and the script shell it uses from it each time you install",
    },
    Place {
        path: ".docker",
        reach: Reach::Beneath,
        why: "docker runs the credential helpers and plugins it names, as you",
    },
    Place {
        path: "Library/LaunchDaemons",
        reach: Reach::Beneath,
        why: STARTS_AT_LOGIN,
    },
    Place {
        path: ".local/share",
        reach: Reach::Itself,
        why: "it holds the data of every program you run, and the places that start programs when you sign in",
    },
    Place {
        path: ".local/state",
        reach: Reach::Itself,
        why: "it holds the state of every program you run",
    },
    Place {
        path: ".cache",
        reach: Reach::Itself,
        why: "it holds the caches of every program you run, which each trusts as it finds them",
    },
    Place {
        path: "Library",
        reach: Reach::Itself,
        why: "it holds the data and settings of every program you run, and the places that start programs when you sign in",
    },
    Place {
        path: "Library/Application Support",
        reach: Reach::Itself,
        why: "it holds the data of every program you run",
    },
    Place {
        path: "AppData",
        reach: Reach::Itself,
        why: "it holds the data and settings of every program you run",
    },
    Place {
        path: "AppData/Roaming",
        reach: Reach::Itself,
        why: "it holds the settings and data of every program you run, and the places that start programs when you sign in",
    },
    Place {
        path: "AppData/Local",
        reach: Reach::Itself,
        why: "it holds the data of every program you run",
    },
];

/// Why a state path that leaves the home it is joined to is refused.
const STAYS_HOME: &str = "only a place within it is made writable, where links lead as well";

/// Why the home crucible keeps its state in is never made writable.
const HOLDS_KEYS: &str =
    "it holds the keys you typed for your agents and the record of the releases revoked";

/// Whether the agent its entry describes keeps its sign-in nowhere crucible
/// can open to it under the sandbox: its entry names neither a variable it
/// reads its home from nor a place it keeps its sign-in in.
#[must_use]
pub(crate) fn keeps_no_sign_in(entry: &Entry) -> bool {
    entry.profile_env.is_none() && entry.state_paths.is_empty()
}

/// The variable a program reads the user's home from.
const HOME: &str = if cfg!(windows) { "USERPROFILE" } else { "HOME" };

/// Whether `name` is the variable `wanted`, as the platform compares names.
fn same_variable(name: &str, wanted: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(wanted)
    } else {
        name == wanted
    }
}

/// Whether the home a catalog agent is started with is its entry's and not
/// the shell's: its target sets the variable, or its profile directory is
/// given through it.
fn homes_itself(profile: &CatalogProfile) -> bool {
    profile.sets(HOME)
        || profile
            .profile_env()
            .is_some_and(|name| same_variable(name, HOME))
}

/// The home the child `request` starts is given, where it is one: none where
/// it is unset or not absolute, or where `chosen` says its entry gave it,
/// since a home the catalog picked is not the user's.
fn child_home(request: &Request, chosen: bool) -> Option<PathBuf> {
    if chosen {
        return None;
    }
    let (_, home) = request
        .environment
        .iter()
        .rev()
        .find(|(name, _)| same_variable(name, HOME))?;
    let home = PathBuf::from(home);
    home.is_absolute().then_some(home)
}

/// `request` with `home` as the variable the child reads its home from, in
/// place of the shell's.
fn with_home(mut request: Request, home: &Path) -> Request {
    if let Some(home) = home.to_str() {
        request
            .environment
            .retain(|(name, _)| !same_variable(name, HOME));
        request.environment.push((HOME.to_owned(), home.to_owned()));
    }
    request
}

/// `path` relative to `home`, as the list writes it: a component at a time.
fn beneath(home: &Path, path: &str) -> PathBuf {
    path.split('/')
        .fold(home.to_path_buf(), |joined, name| joined.join(name))
}

/// Whether two names of one path component are the same name, as the file
/// system compares them: by case where it ignores case, and on Windows
/// without the dots and spaces that end a name, which it drops.
fn same_name(a: &OsStr, b: &OsStr) -> bool {
    alike(a, b, cfg!(any(windows, target_os = "macos")), cfg!(windows))
}

/// Whether `a` and `b` name one component to a file system that ignores case
/// where `folds`, and drops the dots and spaces that end a name where `trims`.
fn alike(a: &OsStr, b: &OsStr, folds: bool, trims: bool) -> bool {
    let (a, b) = (ended(a, trims), ended(b, trims));
    if folds {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

/// `name` as bytes, without the dots and spaces that end it where `trims`.
fn ended(name: &OsStr, trims: bool) -> &[u8] {
    let mut bytes = name.as_encoded_bytes();
    if trims {
        while let [rest @ .., b'.' | b' '] = bytes {
            bytes = rest;
        }
    }
    bytes
}

/// Whether `path` is `base` or lies beneath it, compared by components and
/// not by how their text begins.
fn is_beneath(path: &Path, base: &Path) -> bool {
    let mut path = path.components();
    base.components().all(|wanted| {
        path.next()
            .is_some_and(|found| same_name(found.as_os_str(), wanted.as_os_str()))
    })
}

/// How `path` lies against `place`, which reaches as `reach` says.
fn overlap(path: &Path, place: &Path, reach: Reach) -> Option<Overlap> {
    match (is_beneath(path, place), is_beneath(place, path), reach) {
        (true, true, _) => Some(Overlap::Is),
        (true, false, Reach::Beneath) => Some(Overlap::Within),
        (false, true, _) => Some(Overlap::Contains),
        (true, false, Reach::Itself) | (false, false, _) => None,
    }
}

/// A place no state path may make writable, as it is written for the user's
/// `home`, and everywhere it lies once links are followed.
struct Guarded {
    /// What the refusal calls it.
    place: String,
    /// How far it reaches.
    reach: Reach,
    /// Why it is guarded.
    why: &'static str,
    /// Where it lies: as written, under the home once its links are followed,
    /// and where it leads itself.
    at: Vec<PathBuf>,
}

impl Guarded {
    /// The place at `named`, which also lies at `elsewhere`.
    fn new(
        place: String,
        reach: Reach,
        why: &'static str,
        named: PathBuf,
        elsewhere: Option<PathBuf>,
    ) -> Self {
        let mut at = vec![named];
        for other in elsewhere
            .into_iter()
            .chain(at.first().and_then(|named| resolved(named).ok()))
        {
            if !at.contains(&other) {
                at.push(other);
            }
        }
        Self {
            place,
            reach,
            why,
            at,
        }
    }
}

/// Every place no state path under `home` may make writable: those of
/// [`NEVER_WRITABLE`] and the `crucible` home.
fn guarded(home: &Path, crucible: &Path) -> Vec<Guarded> {
    let real = resolved(home).ok();
    let listed = NEVER_WRITABLE.iter().map(|place| {
        Guarded::new(
            format!("~/{}", place.path),
            place.reach,
            place.why,
            beneath(home, place.path),
            real.as_deref().map(|real| beneath(real, place.path)),
        )
    });
    let own = Guarded::new(
        "crucible's own home".to_owned(),
        Reach::Beneath,
        HOLDS_KEYS,
        crucible.to_path_buf(),
        None,
    );
    listed.chain(std::iter::once(own)).collect()
}

impl Agent {
    /// The places under `home` the child of this agent is given to write, as
    /// its catalog entry says it keeps its sign-in in them: each resolved, and
    /// none that is, lies within or contains a place of [`NEVER_WRITABLE`] or
    /// `crucible`, the home crucible keeps its own state in. None is checked
    /// by how its text begins, and each is checked as it is joined to `home`
    /// and again where its links lead, so a name that is allowed but leads
    /// into a place that is not is refused, as is one that leads out of
    /// `home` altogether.
    ///
    /// # Errors
    ///
    /// [`Unlaunched::StatePath`] for a path that does, else
    /// [`Unlaunched::Policy`] for one that cannot be followed.
    fn state_roots(&self, home: &Path, crucible: &Path) -> Result<Vec<PathBuf>, Unlaunched> {
        let AgentRow::Catalog(catalog) = &self.row else {
            return Ok(Vec::new());
        };
        let written = catalog.profile().state_paths();
        if written.is_empty() {
            return Ok(Vec::new());
        }
        let refused = |path: &str, overlap, place: &str, why| Unlaunched::StatePath {
            instance: quoted(&self.instance),
            agent: quoted(self.row.named()),
            path: quoted(path),
            overlap,
            place: place.into(),
            why,
        };
        let guarded = guarded(home, crucible);
        let inside = resolved(home)?;
        let mut roots: Vec<PathBuf> = Vec::new();
        for path in written {
            let mut joined = home.to_path_buf();
            for component in Path::new(&**path).components() {
                let Component::Normal(name) = component else {
                    return Err(refused(path, Overlap::Outside, "your home", STAYS_HOME));
                };
                joined.push(name);
            }
            let real = resolved(&joined)?;
            // The closest a path comes to a place is what is said of it: it
            // is a place, rather than containing a place within it.
            let closest = guarded
                .iter()
                .flat_map(|guard| {
                    [&joined, &real].into_iter().flat_map(move |subject| {
                        guard
                            .at
                            .iter()
                            .filter_map(move |place| overlap(subject, place, guard.reach))
                            .map(move |overlap| (overlap, guard))
                    })
                })
                .min_by_key(|(overlap, _)| *overlap);
            if let Some((overlap, guard)) = closest {
                return Err(refused(path, overlap, &guard.place, guard.why));
            }
            // Where its links lead is what the sandbox is given, so a name
            // that is allowed but leads out of the home, as a dotfile
            // manager's link does, is a place nobody listed.
            if !is_beneath(&real, &inside) || is_beneath(&inside, &real) {
                return Err(refused(path, Overlap::Outside, "your home", STAYS_HOME));
            }
            if !roots.contains(&real) {
                roots.push(real);
            }
        }
        Ok(roots)
    }
}

/// The location the entry `agent` names, where it names one.
fn located_in(agent: &ExternalAgent) -> Option<Box<str>> {
    agent.cloud().map(|cloud| cloud.location().into())
}

/// The host an agent's business sign-in billed in the Google Cloud
/// `location` reaches besides its row's, as the location names it: a
/// location with a hyphen, a region, has a host of its own, the
/// multi-regions `us` and `eu` one under their own name, and any other,
/// `global` among them, none besides the row's.
///
/// # Errors
///
/// [`SandboxPolicyError`] where the host so named is not one a policy can
/// name, as one whose first label is longer than a host name's can be.
pub(crate) fn located(
    location: &str,
) -> Result<Option<SandboxNetworkEndpoint>, SandboxPolicyError> {
    let host = if location.contains('-') {
        format!("{location}-businessaicode.googleapis.com")
    } else if matches!(location, "us" | "eu") {
        format!("businessaicode.{location}.rep.googleapis.com")
    } else {
        return Ok(None);
    };
    SandboxNetworkEndpoint::new(host, 443, SandboxNetworkProvenance::User).map(Some)
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
    /// The home every catalog agent it starts is given, where it is not the
    /// shell's.
    home: Option<PathBuf>,
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
            home: None,
        }
    }

    /// The same launcher, giving every catalog agent it starts `home` as its
    /// home in place of the shell's.
    #[must_use]
    pub fn homed(self, home: impl Into<PathBuf>) -> Self {
        Self {
            home: Some(home.into()),
            ..self
        }
    }

    /// The content-use answers a launch is held by.
    pub(crate) const fn consent(&self) -> &Consent {
        self.consent
    }

    /// Why a start of `agent` would be refused before any process, as
    /// [`Agent::release`] decides it with the trust state as it is now, before
    /// its route is asked about; nothing where it would not be.
    pub(crate) fn refusal(&self, agent: &Agent) -> Option<Unlaunched> {
        agent
            .release(
                self.consent,
                &Trust::load(self.state, SystemTime::now()),
                &programs_beside(self.state),
            )
            .err()
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

    /// `request` as `agent` is started with it, and the places under its
    /// home that its child may write.
    ///
    /// A catalog agent whose home is the shell's is given the home this
    /// launcher was told of, where it was told of one. Where the sandbox is on
    /// and that home is one, an absolute path in the request, each place its
    /// entry keeps its sign-in in is one the child may write, checked by
    /// [`Agent::state_roots`]. A built-in agent, an agent whose entry gives
    /// its home, an instance with a profile directory of its own, and a
    /// conversation without the sandbox get none.
    ///
    /// # Errors
    ///
    /// [`Unlaunched::StatePath`] for a place that may not be written, else
    /// [`Unlaunched::Policy`] for one that cannot be followed.
    fn homed_request(
        &self,
        agent: &Agent,
        request: Request,
        crucible: &Path,
    ) -> Result<(Request, Vec<PathBuf>), Unlaunched> {
        let AgentRow::Catalog(catalog) = &agent.row else {
            return Ok((request, Vec::new()));
        };
        let own = homes_itself(catalog.profile());
        let request = match (&self.home, own) {
            (Some(home), false) => with_home(request, home),
            _ => request,
        };
        // An instance with a directory of its own keeps its sign-in there, in
        // the variable its entry names, so the places the default sign-in is
        // kept in are not its to write: it could change the account every
        // other instance of the agent is signed in to.
        let kept = match child_home(&request, own)
            .filter(|_| self.conversation.enabled() && agent.profile.is_none())
        {
            Some(home) => agent.state_roots(&home, crucible)?,
            None => Vec::new(),
        };
        Ok((request, kept))
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
        // A location whose host no policy can name adds none, and the
        // sandbox then refuses what the agent asks of it; a business sign-in
        // there is refused before anything is started.
        let located = agent
            .location
            .as_deref()
            .and_then(|location| located(location).ok().flatten());
        let mut reaches = row.reaches();
        reaches.extend(located.as_ref().map(SandboxNetworkEndpoint::host));
        let crucible = self.state.parent().unwrap_or(self.state);
        let (request, kept) = self.homed_request(agent, keyed(request, row, crucible), crucible)?;
        let mut request = request;
        let mut policy = policy(
            self.conversation,
            agent.profile.as_deref(),
            &kept,
            &reaches,
            life.limits(self.conversation.limits()),
        )?;
        if let Some(stored) = &stored {
            policy = read_only(&policy, &stored.root)?;
            request = updating_off(request, stored.updater_off.as_ref());
        }
        for root in &kept {
            // Made only where the child may write it, as the profile is.
            if !policy.permits_path(root, SandboxFilesystemAccess::ReadWrite) {
                return Err(SandboxPolicyError::InvalidPath.into());
            }
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
            if let Some(cloud) = &agent.billed {
                antigravity::business(&profile, cloud.project(), cloud.location())
                    .map_err(Unlaunched::Settings)?;
            }
        } else if agent.billed.is_some() {
            return Err(Unlaunched::Settings(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "a business sign-in's settings are written in the agent's profile directory, and none is configured",
            )));
        }
        for root in &kept {
            // A place that is there is the user's and is left as it is.
            if std::fs::symlink_metadata(root).is_err() {
                private(root).map_err(Unlaunched::StateRoot)?;
            }
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
            return Err(Unsmoked::Program(quoted(&program.display().to_string())));
        };
        let root = resolved(root)?;
        let readable = SandboxFilesystemRule::new(
            &root,
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime,
        )?;
        // No place the agent keeps a sign-in in is writable here, as one is
        // for a turn: the smoke test signs nothing in, runs no instance and
        // so has no profile directory, and a root it does not need would
        // widen its sandbox for nothing.
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
            // What the agent said is quoted before it is told anyone.
            Reading::Unreadable(failure) => {
                Err(Unsmoked::Answered(quoted(failure.reason().as_str())))
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
/// the entry's first is asked for, and the one a step that signs the entry in
/// is started on, so that it is held against those runs and they against it.
/// An entry is one the configuration names, so there are no more than it has.
/// Clones share the controls.
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

    /// Whether a run of the entry `instance` holds its control, which a run
    /// that could not be confirmed stopped does for good; an entry no run has
    /// been asked for yet has no control, and no such run.
    fn running(&self, instance: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(instance)
            .is_some_and(Control::running)
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
    /// The home every catalog agent it starts is given, where it is not the
    /// shell's.
    home: Option<PathBuf>,
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
            home: launcher.home,
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
            let host = Launcher {
                home: self.home.clone(),
                ..Launcher::new(
                    Arc::clone(&self.service),
                    self.runtime.clone(),
                    &policy,
                    &self.consent,
                    &self.state,
                )
            }
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
    /// launched on, what [`Steps::control`] lends a step that must be held
    /// against them, and what an account step of the entry reads whether one
    /// of them could not be confirmed stopped from, and [`Steps::running`]
    /// whether one is live.
    controls: Controls,
    /// The home every catalog agent it starts is given, where it is not the
    /// shell's.
    home: Option<PathBuf>,
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
            home: launcher.home,
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
            home: self.home.clone(),
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

    /// The names among `entries` of those with a run live, which a step that
    /// signs in is refused beside; none where no run of theirs has been asked
    /// for yet. A run that could not be confirmed stopped is live for good, so
    /// [`Steps::unstopped`] is read first to tell the two apart.
    #[must_use]
    pub(crate) fn running<'a>(&self, entries: &'a [ExternalAgent]) -> Vec<&'a str> {
        entries
            .iter()
            .map(ExternalAgent::name)
            .filter(|name| self.controls.running(name))
            .collect()
    }

    /// The control every run of the entry `instance` is launched on, for an
    /// account step that must be held against them: started on it, the step
    /// is refused beside a run of the entry as busy, and a run of the entry
    /// beside it; none once this process has numbered every control it can.
    #[must_use]
    pub(crate) fn control(&self, instance: &str) -> Option<Control> {
        self.controls.of(instance)
    }

    /// Why a start of the configured `agent`, whose registry row is `row`,
    /// would be refused before any process, as [`Agent::release`] decides it;
    /// nothing where it would not be.
    #[must_use]
    pub fn refusal(&self, row: &AgentRow, agent: &ExternalAgent) -> Option<Unlaunched> {
        Agent::of_row(agent, row.clone(), |name| {
            (name == "PATH").then(|| self.path.clone()).flatten()
        })
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
        Launcher {
            home: self.home.clone(),
            ..Launcher::new(
                Arc::clone(&self.service),
                self.runtime.clone(),
                &policy,
                &self.consent,
                &self.state,
            )
        }
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
