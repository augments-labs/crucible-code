//! The profile an agent the catalog reviewed is run under, owned, and made
//! from its entry and the target it was installed from.
//!
//! A [`CatalogProfile`] holds data the intake recorded and a reviewer passed,
//! and no code specific to any agent: the `agentInfo` the agent must answer
//! `initialize` with, the sign-in methods it carries out itself, the arguments
//! and environment its target was reviewed with, and the hosts it reaches.
//! [`Acp::cataloged`](crate::acp::Acp::cataloged) runs it.
//!
//! An agent is admitted only where it calls itself by the name, at the
//! version, its entry gives, answering in the protocol version this client
//! speaks; one that says anything else is sent nothing more. Of its sign-in
//! methods, only those it carries out once asked through `authenticate` are
//! the profile's: one given to it as a variable, or run as a command of its
//! own, is not asked that way.
//!
//! The environment it is started in begins empty and holds the few variables
//! a program needs to run at all, taken from the shell ([`PASSED`]), then the
//! variables its target sets, each of which replaces a passed one of the same
//! name. A variable a loader reads was refused when the catalog was read. The
//! passed values may be shown where what the agent says is kept; the target's
//! are not marked so, so one long enough to be a credential is withheld there
//! as a credential would be.
//!
//! The hosts it reaches are parsed here, once, by the sandbox's own domain
//! parser, and a pattern that names every host, `*` or a wildcard over a single
//! label such as `*.com`, is refused, so no entry opens the whole network.

use std::ffi::OsString;

use crucible_execution::Failure;
use crucible_sandbox::{SandboxDomainPattern, SandboxPolicyError};

use super::{Admits, Entry, Method, Target, Variable};
use crate::Request;
use crate::acp::{Initialized, PROTOCOL, SignIn};

/// The variables taken from the shell, which say where programs and the
/// user's own files are and how text is written, and nothing about any
/// account, so their values may be shown where what the agent says is kept.
pub(crate) const PASSED: [&str; 15] = [
    "PATH",
    "HOME",
    "USERPROFILE",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
    "USER",
    "LOGNAME",
    "SYSTEMROOT",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "SYSTEMDRIVE",
    "NUMBER_OF_PROCESSORS",
];

/// How an agent the catalog reviewed is run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogProfile {
    /// The `agentInfo` it must answer initializing with.
    admits: Admits,
    /// The methods it signs in with once asked through `authenticate`, in the
    /// order its entry gives them.
    sign_in: Vec<SignIn>,
    /// Its own arguments.
    args: Vec<Box<str>>,
    /// The variables its target sets.
    env: Vec<Variable>,
    /// The hosts it reaches.
    reach: Vec<SandboxDomainPattern>,
}

/// Why an entry gives no profile to run.
#[derive(Debug, thiserror::Error)]
pub enum Unprofiled {
    /// A host it reaches is not one a sandbox can name.
    #[error("reach[{index}] is not a host a sandbox can grant")]
    Host {
        /// Where in the entry's `reach`.
        index: usize,
        /// Why the sandbox's parser refused it.
        #[source]
        source: SandboxPolicyError,
    },
    /// A host it reaches names every host, or every host under one label.
    #[error("reach[{0}] names every host, or every host under one label, which is never granted")]
    Everywhere(usize),
}

impl CatalogProfile {
    /// The profile `entry` gives the agent installed from its `target`.
    ///
    /// # Errors
    ///
    /// [`Unprofiled`] for the first host it reaches that is not one a sandbox
    /// can name, or that names every host.
    pub fn from_entry(entry: &Entry, target: &Target) -> Result<Self, Unprofiled> {
        let reach = entry
            .reach
            .iter()
            .enumerate()
            .map(|(index, host)| granted(index, host))
            .collect::<Result<_, _>>()?;
        let sign_in = entry
            .auth_methods
            .iter()
            .filter_map(|method| match &method.method {
                Method::Agent => Some(SignIn::owned(&method.id)),
                Method::EnvVar { .. } | Method::Terminal { .. } => None,
            })
            .collect();
        Ok(Self {
            admits: entry.admits.clone(),
            sign_in,
            args: target.args.clone(),
            env: target.env.clone(),
            reach,
        })
    }

    /// The protocol versions its answer to initializing may carry: the one
    /// this client speaks, which is the one the intake initialized it in.
    #[must_use]
    pub fn versions(&self) -> &'static [u64] {
        &[PROTOCOL]
    }

    /// Whether the agent, as it described itself, is the one its entry
    /// reviewed.
    ///
    /// # Errors
    ///
    /// A [`Failure`] naming the first way it is not.
    pub fn admits(&self, initialized: &Initialized) -> Result<(), Failure> {
        let Admits { name, version } = &self.admits;
        if initialized.name() != Some(&**name) {
            return Err(Failure::new(&format!(
                "The program is not {name}: it calls itself {}.",
                initialized.name().unwrap_or("nothing")
            )));
        }
        let answered = initialized.version().unwrap_or("unknown");
        if answered != &**version {
            return Err(Failure::new(&format!(
                "Version {answered} of {name} is not the one its catalog entry reviewed, \
                 {version}."
            )));
        }
        Ok(())
    }

    /// The methods it signs in with once asked through `authenticate`, in the
    /// order they are tried.
    #[must_use]
    pub fn sign_in(&self) -> &[SignIn] {
        &self.sign_in
    }

    /// The hosts it reaches, which its sandbox lets it.
    #[must_use]
    pub fn reach(&self) -> &[SandboxDomainPattern] {
        &self.reach
    }

    /// What it is started with: its own arguments, and the variables of the
    /// `ambient` environment it is passed with those its target sets.
    pub(crate) fn request(
        &self,
        ambient: impl IntoIterator<Item = (OsString, OsString)>,
    ) -> Request {
        let set = |name: &str| self.env.iter().any(|variable| named(name, &variable.name));
        let shell = passed(ambient)
            .into_iter()
            .filter(|(name, _)| !set(name))
            .collect();
        let mut request =
            Request::showing(self.args.iter().map(ToString::to_string).collect(), shell);
        request.environment.extend(
            self.env
                .iter()
                .map(|variable| (variable.name.to_string(), variable.value.to_string())),
        );
        request
    }
}

/// The host `reach[index]` names, as the sandbox's parser reads it, where it
/// is one that may be granted.
fn granted(index: usize, host: &str) -> Result<SandboxDomainPattern, Unprofiled> {
    let pattern =
        SandboxDomainPattern::new(host).map_err(|source| Unprofiled::Host { index, source })?;
    let spelled = pattern.as_str();
    let everywhere = spelled == "*"
        || spelled
            .strip_prefix("*.")
            .is_some_and(|suffix| !suffix.contains('.'));
    if everywhere {
        Err(Unprofiled::Everywhere(index))
    } else {
        Ok(pattern)
    }
}

/// The variables of `ambient` that [`PASSED`] names, each under the name it is
/// listed by.
///
/// On Windows a variable is matched whatever its case, as Windows matches it;
/// one whose name or value is not text is left out, since none of those named
/// is.
pub(crate) fn passed(
    ambient: impl IntoIterator<Item = (OsString, OsString)>,
) -> Vec<(&'static str, String)> {
    ambient
        .into_iter()
        .filter_map(|(name, value)| Some((name.into_string().ok()?, value.into_string().ok()?)))
        .filter_map(|(name, value)| {
            let passed = PASSED.iter().find(|passed| named(&name, passed))?;
            Some((*passed, value))
        })
        .collect()
}

/// Whether `name` is the variable `wanted`, as the platform compares names.
fn named(name: &str, wanted: &str) -> bool {
    if cfg!(windows) {
        name.eq_ignore_ascii_case(wanted)
    } else {
        name == wanted
    }
}
