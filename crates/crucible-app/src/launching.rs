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
//! A probe and an account step are starts like a turn's and go through the
//! same gates, each under a hold of its own.
//!
//! The child's policy is the conversation's, widened in exactly two ways: its
//! profile directory, where one is configured, is writable, and the hosts its
//! row names are reachable. Every other rule, the protected metadata included,
//! stays as the conversation has it, so a profile directory inside one is
//! refused rather than opened.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crucible_execution::Failure;
use crucible_external_harnesses::{
    Account, Control, Dialect, Discovery, Host, Launch, Reading, Refusal, Request, Shape,
};
use crucible_sandbox::{
    SandboxDomainPattern, SandboxDomainPolicy, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxNetworkPolicy,
    SandboxNetworkProvenance, SandboxPolicy, SandboxPolicyError, SandboxService,
};
use tokio::runtime::Handle;

use crate::access::Route;
use crate::backends::{Registry, Row};
use crate::content_use::{self, Consent, Unheld, Warned};

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
            ..
        } = route
        else {
            return None;
        };
        Some(Self {
            row: registry.row(*driver)?,
            discovery: discover(executable, lookup),
            profile: profile.as_deref().map(PathBuf::from),
        })
    }

    /// Its registry row.
    #[must_use]
    pub const fn row(&self) -> &'static Row {
        self.row
    }

    /// What looking for its program found.
    #[must_use]
    pub const fn discovery(&self) -> &Discovery {
        &self.discovery
    }

    /// The shape every start of it is given.
    fn shape<'a>(&'a self, resume: Option<&'a str>) -> Shape<'a> {
        Shape {
            resume,
            profile: self.profile.as_deref(),
        }
    }
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
/// writable but for the metadata protected beneath any writable root, and the
/// hosts in `reaches` reachable.
///
/// # Errors
///
/// A [`SandboxPolicyError`] where the profile is not an absolute path, lies
/// under a path the conversation protects, or a host is not one a policy can
/// name.
pub fn policy(
    conversation: &SandboxPolicy,
    profile: Option<&Path>,
    reaches: &[&str],
) -> Result<SandboxPolicy, SandboxPolicyError> {
    let mut filesystem = conversation.filesystem().to_vec();
    if let Some(profile) = profile {
        filesystem.push(SandboxFilesystemRule::new(
            profile,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::UserConfiguration,
        )?);
        for metadata in SandboxFilesystemRule::protected_beneath(profile)? {
            if !filesystem.iter().any(|old| old.path() == metadata.path()) {
                filesystem.push(metadata);
            }
        }
    }
    let network = if reaches.is_empty() {
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
        conversation.limits(),
    )?
    .with_unreadable_patterns(conversation.unreadable_patterns().iter().cloned())
    .map(|policy| {
        policy
            .with_command_policy(conversation.commands().clone())
            .with_session_state(conversation.persistent(), conversation.snapshots())
    })
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
        dialect: &dyn Dialect,
        resume: Option<&str>,
    ) -> Result<Host, Unlaunched> {
        let request = dialect.request(agent.shape(resume));
        self.start(agent, control, request, resume).await
    }

    /// Runs the account step `step` of `agent` to its end, and reads what it
    /// said through `dialect`.
    ///
    /// # Errors
    ///
    /// [`Unlaunched`] for each reason nothing was started, in which case the
    /// account is as unverified as it was before.
    pub async fn account(
        &self,
        agent: &Agent,
        control: &Control,
        dialect: &dyn Dialect,
        step: Account,
    ) -> Result<Reading, Unlaunched> {
        let Some(request) = dialect.account(step, agent.shape(None)) else {
            return Ok(Reading::Unreadable(Failure::new(
                "this agent has no such step",
            )));
        };
        let host = self.start(agent, control, request, None).await?;
        let ran = host.run().await;
        Ok(dialect.read(step, &ran))
    }

    /// Starts `agent` with `request`, once every gate before a process has
    /// passed.
    async fn start(
        &self,
        agent: &Agent,
        control: &Control,
        request: Request,
        resume: Option<&str>,
    ) -> Result<Host, Unlaunched> {
        let row = agent.row;
        let shown = self
            .consent
            .routes()
            .warned(row.route)
            .map_or(row.driver.name(), |warned| warned.shown);
        if !row.shipping {
            return Err(Unlaunched::Withheld {
                agent: shown.to_owned(),
                documented: row.documented(),
            });
        }
        let program = match &agent.discovery {
            Discovery::Found(program) => program.clone(),
            missing @ (Discovery::Missing | Discovery::NotExecutable(_)) => {
                let command = (row.dialect)().program();
                return Err(missing.refusal(shown, command).map_or_else(
                    || Unlaunched::Unknown(row.route.to_owned()),
                    Unlaunched::NotInstalled,
                ));
            }
        };
        let held = content_use::held(self.consent, row.route).map_err(|unheld| match unheld {
            Unheld::Asks(warned) => Unlaunched::Asks(warned),
            Unheld::Unknown => Unlaunched::Unknown(row.route.to_owned()),
        })?;
        let policy = policy(self.conversation, agent.profile.as_deref(), row.reaches)?;
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

#[cfg(test)]
mod tests;
