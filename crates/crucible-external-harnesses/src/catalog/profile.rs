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
//! An entry whose admitted name or version is as long as an answer to
//! initializing is cut to gives no profile, so an answer is compared only
//! whole and a longer one cut to the same bytes is never taken for it.
//!
//! The hosts it reaches, and the hosts a sign-in link it prints may open, are
//! parsed here, once, by the sandbox's own domain parser, and a pattern that
//! names every host, `*` or a wildcard over a single label such as `*.com`, is
//! refused, so no entry opens the whole network or sends a person to any page.
//! A host is named, never numbered: an address in any spelling, bracketed,
//! with a port, under a wildcard, or as a C resolver reads `127.1` or
//! `0x7f.1`, is refused, public or not, since the sandbox lets a literal
//! address through to space it closes to every name, the loopback, private and
//! link-local ranges among it.
//!
//! Every sign-in method the entry lists is kept, whichever way it is carried
//! out, so whoever signs the agent in finds the method asked for among them by
//! its id. A sign-in link is shown only as [`CatalogProfile::link`] passes it:
//! printable, bounded, `https` on the default port, with no user before its
//! host and no fragment, at a host spelled in the one alphabet the sandbox
//! holds every host it grants to and listed by its entry for signing in, and
//! with the `state` its query carries kept. What a person pastes back from the
//! browser is a [`Callback`], a secret from the moment it is read: its `Debug`
//! names nothing of it, it has no `Display`, no refusal of it repeats it, and
//! it is taken only where the `state` its query carries is the one the link
//! carried. Neither is read for fields where it has no `?`.

use std::ffi::OsString;
use std::net::{IpAddr, SocketAddr};

use crucible_execution::Failure;
use crucible_sandbox::{
    SandboxDomainPattern, SandboxNetworkEndpoint, SandboxNetworkProvenance, SandboxPolicyError,
};
use crucible_types::AgentKey;

use super::{Admits, AuthMethod, Entry, Method, Target, Variable};
use crate::Request;
use crate::acp::{Initialized, NAME_BYTES, PROTOCOL, SignIn};

/// The longest name or version an entry may admit: shorter than an answer to
/// initializing cut at [`NAME_BYTES`] on a character boundary ever is, since
/// the cut drops at most the three bytes of a character it would split, so an
/// answer equal to it was never cut.
const ADMITTED_BYTES: usize = NAME_BYTES - 4;

/// The most bytes a sign-in link, or what is pasted back from one, may have.
pub const LINK_BYTES: usize = 16 * 1024;

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
    /// The agent, as its instance names it.
    key: AgentKey,
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
    /// How it is signed in, held apart so a profile stays small where it is
    /// carried by value.
    signing: Box<Signing>,
}

/// How an agent the catalog reviewed is signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Signing {
    /// Every sign-in method its entry lists, in the order given.
    methods: Vec<AuthMethod>,
    /// The hosts a sign-in link it prints may open.
    hosts: Vec<SandboxDomainPattern>,
}

/// Why an entry gives no profile to run.
#[derive(Debug, thiserror::Error)]
pub enum Unprofiled {
    /// A host it reaches, or one a sign-in link may open, is not one a
    /// sandbox can name.
    #[error("{field}[{index}] is not a host a sandbox can grant")]
    Host {
        /// Which list of the entry's: `reach` or `sign_in_hosts`.
        field: &'static str,
        /// Where in it.
        index: usize,
        /// Why the sandbox's parser refused it.
        #[source]
        source: SandboxPolicyError,
    },
    /// A host it lists names every host, or every host under one label.
    #[error("{0}[{1}] names every host, or every host under one label, which is never granted")]
    Everywhere(&'static str, usize),
    /// A host it lists is an address rather than a name.
    #[error("{0}[{1}] is an address, and an agent's hosts are named, never numbered")]
    Address(&'static str, usize),
    /// The name or version it must answer with is as long as an answer is cut
    /// to, so a longer answer would be taken for it.
    #[error("{0} is longer than the {ADMITTED_BYTES} bytes an agent's answer is compared whole at")]
    Admitted(&'static str),
}

impl CatalogProfile {
    /// The profile `entry` gives the agent installed from its `target`.
    ///
    /// # Errors
    ///
    /// [`Unprofiled`] for an admitted name or version an answer could be cut
    /// to, or for the first host it reaches, or a sign-in link may open, that
    /// is an address, is not one a sandbox can name, or names every host.
    pub fn from_entry(entry: &Entry, target: &Target) -> Result<Self, Unprofiled> {
        for (field, admitted) in [
            ("admits.name", &entry.admits.name),
            ("admits.version", &entry.admits.version),
        ] {
            if admitted.len() > ADMITTED_BYTES {
                return Err(Unprofiled::Admitted(field));
            }
        }
        let hosts = |field: &'static str, listed: &[Box<str>]| {
            listed
                .iter()
                .enumerate()
                .map(|(index, host)| granted(field, index, host))
                .collect::<Result<Vec<_>, _>>()
        };
        let reach = hosts("reach", &entry.reach)?;
        let sign_in_hosts = hosts("sign_in_hosts", &entry.sign_in_hosts)?;
        let sign_in = entry
            .auth_methods
            .iter()
            .filter_map(|method| match &method.method {
                Method::Agent => Some(SignIn::owned(&method.id)),
                Method::EnvVar { .. } | Method::Terminal { .. } => None,
            })
            .collect();
        Ok(Self {
            key: entry.key.clone(),
            admits: entry.admits.clone(),
            sign_in,
            args: target.args.clone(),
            env: target.env.clone(),
            reach,
            signing: Box::new(Signing {
                methods: entry.auth_methods.clone(),
                hosts: sign_in_hosts,
            }),
        })
    }

    /// The agent, as its instance names it.
    pub(crate) const fn key(&self) -> &AgentKey {
        &self.key
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

    /// Every sign-in method its entry lists, in the order given, however each
    /// is carried out.
    #[must_use]
    pub fn methods(&self) -> &[AuthMethod] {
        &self.signing.methods
    }

    /// The sign-in link `printed`, as the agent printed it, where it may be
    /// shown to a person: printable ASCII of at most [`LINK_BYTES`], `https`
    /// on the default port, no user before its host and no fragment, its host
    /// spelled as a host name is and one its entry's sign-in hosts list, and
    /// at most one `state`, read from its query alone.
    ///
    /// # Errors
    ///
    /// [`Unlinked`] naming the first way it may not be shown. The link itself
    /// is in none of them.
    pub fn link(&self, printed: &str) -> Result<SignInLink, Unlinked> {
        let printed = printed.trim();
        if printed.len() > LINK_BYTES || !printed.bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(Unlinked::Unprintable);
        }
        let rest = printed
            .strip_prefix("https://")
            .ok_or(Unlinked::Unsecured)?;
        if printed.contains('#') {
            return Err(Unlinked::Fragment);
        }
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        let (authority, after) = rest.split_at(end);
        if authority.contains('@') {
            return Err(Unlinked::User);
        }
        let host = match authority.rsplit_once(':') {
            Some((host, "443")) => host,
            Some(_) => return Err(Unlinked::Port),
            None => authority,
        };
        let host = host.to_ascii_lowercase();
        if host.is_empty() || numbered(&host) {
            return Err(Unlinked::Address);
        }
        // Held to the one alphabet the sandbox holds every host it grants to,
        // so a character a browser reads as ending the host, such as `\`,
        // never carries a link past a wildcard it does not belong under.
        let named = SandboxNetworkEndpoint::new(host, 443, SandboxNetworkProvenance::Descendant)
            .map_err(|_| Unlinked::Unnamed)?;
        if !self
            .signing
            .hosts
            .iter()
            .any(|listed| listed.matches(named.host()))
        {
            return Err(Unlinked::Unlisted);
        }
        let states: Vec<&str> = query(after)
            .filter_map(|(name, value)| (name == "state").then_some(value))
            .collect();
        let state = match states.as_slice() {
            [] => None,
            [one] => Some((*one).into()),
            _ => return Err(Unlinked::States),
        };
        Ok(SignInLink {
            text: printed.into(),
            state,
        })
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

/// The host `field[index]` names, as the sandbox's parser reads it, where it
/// is one that may be granted.
fn granted(
    field: &'static str,
    index: usize,
    host: &str,
) -> Result<SandboxDomainPattern, Unprofiled> {
    if numbered(host) {
        return Err(Unprofiled::Address(field, index));
    }
    let pattern = SandboxDomainPattern::new(host).map_err(|source| Unprofiled::Host {
        field,
        index,
        source,
    })?;
    let spelled = pattern.as_str();
    let everywhere = spelled == "*"
        || spelled
            .strip_prefix("*.")
            .is_some_and(|suffix| !suffix.contains('.'));
    if everywhere {
        Err(Unprofiled::Everywhere(field, index))
    } else {
        Ok(pattern)
    }
}

/// Whether `host` is an address rather than a name: a literal the platform
/// parses, bracketed or not, with a port or not, under a wildcard or not, or a
/// name whose last label a C resolver reads as a number.
///
/// Whoever reads a host the catalog lists, for reach or for a download, holds
/// it to this one predicate, so no list of the catalog's lets a literal
/// address through.
#[must_use]
pub fn numbered(host: &str) -> bool {
    let host = host.strip_prefix("*.").unwrap_or(host);
    let bracketed = host
        .strip_prefix('[')
        .and_then(|rest| rest.split_once(']'))
        .is_some_and(|(inside, _)| inside.parse::<IpAddr>().is_ok());
    if bracketed || host.parse::<IpAddr>().is_ok() || host.parse::<SocketAddr>().is_ok() {
        return true;
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    let host = host
        .rsplit_once(':')
        .filter(|(_, port)| port.parse::<u16>().is_ok())
        .map_or(host, |(name, _)| name);
    host.rsplit('.').next().is_some_and(numeric)
}

/// Whether `label` is a number as a C resolver reads one part of an address:
/// decimal or octal digits, or hexadecimal ones after `0x`.
fn numeric(label: &str) -> bool {
    match label
        .strip_prefix("0x")
        .or_else(|| label.strip_prefix("0X"))
    {
        Some(digits) => digits.bytes().all(|byte| byte.is_ascii_hexdigit()),
        None => !label.is_empty() && label.bytes().all(|byte| byte.is_ascii_digit()),
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

/// The fields of the query of `after`, what follows a link's host or what was
/// pasted back, as `name` and `value` pairs, each as written: everything after
/// the first `?`, split at `&` and then at the first `=`, and none where it
/// has no `?`, since a path that looks like a query is not one.
fn query(after: &str) -> impl Iterator<Item = (&str, &str)> {
    let query = after.split_once('?').map_or("", |(_, query)| query);
    query
        .split('&')
        .filter(|field| !field.is_empty())
        .map(|field| field.split_once('=').unwrap_or((field, "")))
}

/// A sign-in link an agent printed, as [`CatalogProfile::link`] passed it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignInLink {
    /// The link, as it may be shown.
    text: Box<str>,
    /// The `state` it carries, where it carries one.
    state: Option<Box<str>>,
}

impl SignInLink {
    /// The link, as it may be shown for a person to open.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The `state` it carries, which what is pasted back must carry too.
    #[must_use]
    pub fn state(&self) -> Option<&str> {
        self.state.as_deref()
    }
}

/// Why a sign-in link is not shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unlinked {
    /// It is longer than a link, or holds a character that is not printable
    /// ASCII.
    #[error("the sign-in link is longer than {LINK_BYTES} bytes or holds unprintable characters")]
    Unprintable,
    /// It is not `https`.
    #[error("the sign-in link is not https")]
    Unsecured,
    /// It has a fragment.
    #[error("the sign-in link has a fragment")]
    Fragment,
    /// It names a user before its host.
    #[error("the sign-in link names a user before its host")]
    User,
    /// It names a port other than the default.
    #[error("the sign-in link names a port other than https's own")]
    Port,
    /// Its host is missing, or an address rather than a name.
    #[error("the sign-in link's host is not a name")]
    Address,
    /// Its host is not spelled as a host name is: labels of letters, digits
    /// and inner hyphens, joined by dots.
    #[error("the sign-in link's host is not spelled as a host name")]
    Unnamed,
    /// Its host is not one its entry lists for signing in.
    #[error("the sign-in link opens a host its catalog entry does not list for signing in")]
    Unlisted,
    /// It carries more than one `state`.
    #[error("the sign-in link carries more than one state")]
    States,
}

/// What a person pasted back from the browser after opening a sign-in link,
/// once its `state` was found to be the link's: a secret, since it carries
/// the code that signs the agent in.
///
/// `Debug` names nothing of it, it has no `Display`, and no refusal of it
/// repeats any of it.
pub struct Callback(String);

impl std::fmt::Debug for Callback {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Callback(<redacted>)")
    }
}

impl Callback {
    /// `pasted`, the address the browser was sent back to or its query from
    /// the `?` on, where it answers `link`: printable ASCII of at most
    /// [`LINK_BYTES`], with a `code`, and one `state` that is the link's own.
    ///
    /// # Errors
    ///
    /// [`Uncalled`] naming the first way it does not answer the link, and
    /// nothing of what was pasted.
    pub fn checked(pasted: String, link: &SignInLink) -> Result<Self, Uncalled> {
        let said = pasted.trim().to_owned();
        drop(pasted);
        let said = said.as_str();
        if said.is_empty()
            || said.len() > LINK_BYTES
            || !said.bytes().all(|byte| byte.is_ascii_graphic())
        {
            return Err(Uncalled::Unreadable);
        }
        let said = said.split_once('#').map_or(said, |(before, _)| before);
        let fields: Vec<(&str, &str)> = query(said).collect();
        if !fields
            .iter()
            .any(|(name, value)| *name == "code" && !value.is_empty())
        {
            return Err(Uncalled::Codeless);
        }
        let Some(expected) = link.state() else {
            return Err(Uncalled::Stateless);
        };
        let states: Vec<&str> = fields
            .iter()
            .filter(|(name, _)| *name == "state")
            .map(|(_, value)| *value)
            .collect();
        match states.as_slice() {
            [one] if *one == expected => Ok(Self(said.to_owned())),
            [] => Err(Uncalled::Stateless),
            _ => Err(Uncalled::Mismatched),
        }
    }

    /// What is written to the agent that printed the link, and nowhere else.
    #[must_use]
    pub fn sent(&self) -> &str {
        &self.0
    }
}

/// Why what was pasted back from a sign-in is not taken. None repeats what
/// was pasted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Uncalled {
    /// It is empty, longer than a link, or holds a character that is not
    /// printable ASCII.
    #[error(
        "what was pasted is empty, longer than {LINK_BYTES} bytes, or holds unprintable characters"
    )]
    Unreadable,
    /// It carries no `code`.
    #[error("what was pasted carries no code")]
    Codeless,
    /// It, or the link it answers, carries no `state` to compare.
    #[error("what was pasted, or the link it answers, carries no state to compare")]
    Stateless,
    /// Its `state` is not the link's, or it carries more than one.
    #[error("what was pasted carries a state other than the sign-in link's")]
    Mismatched,
}
