//! The agents door: listing, installing, adding, updating, rolling back,
//! removing and verifying the agents the reviewed catalog offers, with no
//! conversation.
//!
//! A front end with a conversation and one with none come through the same
//! door, lent a [`Shelf`]: where the catalog is published and the keys it is
//! signed with, crucible's home, from which the user's own configuration file
//! is the one whose `executable` lines are written, and the launcher of a
//! conversation where there is one. Only a client that claimed
//! [`Capability::Install`] is served, and that is checked before anything
//! else; any other is refused as [`ErrorCode::NeedsCapability`] and nothing
//! is read, fetched or changed. A command the door does not serve is refused
//! as [`ErrorCode::Busy`], as the account door refuses one.
//!
//! The names a command carries are only names. The agent is checked as an
//! agent key before it is used, and a version is found among the releases the
//! trust state offers by equality; neither is read as a path or an argument.
//! Every command first fetches the catalog as a person asking for it
//! ([`Trigger::Opening`]), whatever `updates.check` says, and keeps what is
//! believed in the trust state; a catalog that is not believed, or not
//! reached, leaves what was believed, and a listing says it was not refreshed.
//! What is offered is then read from the trust state, never from the network
//! answer itself.
//!
//! An install, add, update, rollback or removal changes nothing before it is
//! answered. It is put to the client as [`Pending::Install`], whose facts are
//! the release's version and size, every host the download may contact, its
//! licence, terms and caution, what it is started with, the instances whose
//! `executable` it moves, and whether it is smoke-tested, and why not; the
//! identity is the process's one count, and the answer is held against the
//! action put. Every string the catalog or the configuration gave is quoted
//! with its control and format characters escaped and its length bounded, so
//! none can draw a line of its own in the question. A sentence that would be
//! cut is not put to a client that does not draw the whole value, and a no, a
//! cancel, or nobody, leaves every file as it was. What is checked before that
//! question is put is everything that can refuse the release without a byte
//! fetched from its own hosts: how it is started, a release a pinned
//! interpreter starts having its entry point checked as one its launcher can
//! be written for, and its interpreter's arguments as exactly `-I`, and one a
//! pinned Node starts being refused where Node is given arguments of its own
//! or an install script is to be run at install, which this build does not
//! do; its download hosts,
//! and that every address and mirror it is pinned at is on one of them; for a
//! release a pinned Node starts, its package lock, fetched from the catalog's
//! own host by [`catalog_fetch::asset`] and read, and every tarball it pins on
//! a host the target lists, so the question names the package count, how
//! many packages are under each licence the lock gives, each quoted, where
//! any package gives one, at most eight and then how many more, every install script and what the reviewer decided of
//! it, at most sixteen and then how many more, and the hosts the registry is
//! reached through; its
//! file lists, where a list the catalog keeps as an asset is first fetched
//! from the catalog's own host by [`catalog_fetch::asset`] and read as one
//! given inline, a list that does not arrive as listed or does not read
//! refusing the release; that the program it is started as is one of its
//! files and runs; its size, its tarballs' among it, summed with checked
//! arithmetic, against the most a release may be and the room the install
//! store has, where the tarballs stand as one more download; and, for a
//! version already in place, that its receipt is of the artifacts and the
//! files the catalog pins, with the launcher of a release a pinned
//! interpreter starts, or the preload, `.bin` and the stub in it of each
//! program of one a pinned Node starts,
//! refused otherwise. A tree's own files are known from its receipt only as
//! lying where its lock installs a package, which the question says, since
//! its lock lists none of them; a receipt is bounded, so a tree of very many
//! files can make a version that is refused rather than kept.
//!
//! After a yes, the release the question named is the one installed: each
//! artifact is downloaded within its pin by [`crate::agent_download`] into a
//! file beside the staging directory, unpacked from it into that directory
//! against its file list, a wheel of a release a pinned interpreter starts
//! with its files under `site/`, and committed to the store with its receipt.
//! Such a release's launcher, `launch.py`, is written at the top of the
//! version for its entry point from a template that never changes and is
//! committed and named in the receipt with the rest, so a second install of
//! the version fetches nothing. A release a pinned Node starts has its
//! tarballs downloaded after its artifacts by [`agent_download::tarballs`]
//! into a directory of the staging's own, unpacked from there to where its
//! lock installs each package and that directory removed, then the preload
//! written at the top of the version and `.bin` beside it, holding a stub for
//! each program the tree declares, each named in the receipt; a program
//! whose script no tarball wrote is refused before anything is committed.
//! Only then is every instance whose
//! `executable` lies in the agent's directory in the store written to the new
//! program, in one update of the user file, each
//! only where its `executable` still names what the question showed, so an
//! instance edited meanwhile keeps the edit and is said to; and the store is
//! pruned to what the user file names then and one version before. A cancel, or a download or unpack
//! that fails, removes the staging and leaves the version named before. An
//! add is an install that then writes one more instance of the agent: its
//! question names the instance by the first name the user file leaves free
//! (the agent key, then the key with `-2`, `-3` and on), and once the version
//! is in place, `externalAgents.<name>` is written with its `driver`, `agent`
//! and `executable` alone, in the same update of the user file that moves the
//! instances. The name is chosen again from the file read under its lock, so
//! one taken meanwhile gives the next free one, and the outcome names the one
//! written. A write refused then leaves the version kept and the user file as
//! it was, no instance moved and none added ([`AgentsError::Unadded`]). A
//! rollback names the newest other version the store holds that no
//! revocation names, the same program in it, and fetches nothing; a removal
//! is refused while an instance still runs a version from the store.
//!
//! The one exception is an update where the user's own configuration file,
//! read alone and never a file in a checkout, says `agents.update =
//! "reviewed"`: the question before it is not put when the release is the
//! newest the catalog offers by version order wherever it is listed, a
//! revocation has not withdrawn it, it is strictly newer than every version
//! being moved, and the catalog it comes from is no older than the one each of
//! those was taken from. Anything else, a version not spelled as dotted
//! numbers included, is asked about as it is without the setting. Every other
//! step runs and refuses as it does after a yes, and the outcome says the
//! update was taken under that setting and why it was not smoke-tested where
//! it was not. A first install, an add, a rollback and a removal are always
//! asked.
//!
//! A switch is smoke-tested before any instance is moved onto it only where a
//! conversation lent its launcher and that launcher's sandbox is on with the
//! network closed and the agent's route has its yes, which the yes to the
//! switch is not: the version put in place, or for a rollback the one gone
//! back to where the catalog still lists it, is started once through
//! `launching` and must answer `initialize` as its entry reviewed it. One
//! that fails leaves every instance on the program it named, and the version
//! it failed in stays in the store for a prune to take. Elsewhere the question
//! says it is not smoke-tested, and why, and nothing is started.
//!
//! A sign-in names an instance the configuration has, by equality, and one of
//! the methods its agent signs in by, by equality among them: a catalog
//! agent's are its entry's, for the release its instance runs or else the
//! newest offered, and Antigravity's are the four its own module offers. A
//! secret never crosses the contract. A key a method is given in a variable
//! is typed on this machine and handed to [`keep_key`], which writes it to
//! crucible's credential store under the agent's name and the method's id, and
//! never under a provider's name; every start of the agent after it is given
//! the key in that variable, withheld from all the agent is seen to say. A
//! sign-in by that method writes nothing and is refused while no key is kept
//! for it. The agent learns which method it signs in by only from being
//! asked, so where a conversation lent its launcher the sign-in goes on as one
//! the agent carries out itself does, with the key in the start's
//! environment; with none, nothing is started, and the outcome says only that
//! the key is kept, is given to the agent at its next start, and that the
//! sign-in is not yet verified. A method the agent carries out itself is
//! asked for through `authenticate`, that method alone, once the agent's
//! route has its yes, and only through the launcher a conversation lent: with
//! none, it is refused saying a conversation is needed, and nothing is
//! started. Either is answered signed in only where the agent, on the same
//! start, then opens a session as a status step reads it; one it reads signed
//! out, or cannot read, is unverified. Where a sign-in starts the agent it is
//! refused if the instance, or any configured instance sharing its credential
//! scope as a sign-out reads it, has a run that could not be confirmed
//! stopped, or else a run live, since the agent's program would run beside it:
//! the refusal names those instances, says that their runs must end first, as
//! disconnecting an instance ends its run, and comes before the route is
//! asked, a setting is written or anything is started. Nothing is ended to
//! make room: the refusal is not a disconnect. Both are asked again once the
//! route has its yes, just before the agent is started, so a run that began or
//! failed to stop while it was asked is not started beside. The agent is
//! started on the instance's own control, so the first sign-in of an instance
//! holds it and a second beside the first is refused by that check as a live
//! run; it is refused as busy only where both passed the check before either
//! was started. A sign-in whose stop could not be confirmed leaves the
//! instance, and every instance on its profile, refused as a run not confirmed
//! stopped is. The runs are known only to the conversation that made
//! them, through the steps it lent, so a launcher lent with none is refused as
//! no conversation. A sign-in link the agent prints is told to
//! [`Watching::link`] as the agent's wire passed it. A method run at a
//! terminal is refused here. A business sign-in to Antigravity is billed to
//! the Google Cloud project and location its entry names: one whose entry
//! names neither is refused naming both keys, and one whose location's host
//! no sandbox policy can name is refused naming that key, each before anything
//! is asked, written or started; otherwise the two are written in the agent's
//! profile once its route has its yes, just before it starts. A sign-in on an
//! instance that names a `profileDir` its agent's entry has no variable for is
//! refused as a start of it is, naming the instance, and nothing is made or
//! started, since the agent would never read that directory. Where the sandbox
//! is on, the agent is let write the places its entry says it keeps its
//! sign-in in, so that what it signs in stays; a listing says of an agent whose
//! entry names neither such a place nor a variable that a sign-in made under
//! the sandbox is not kept ([`Listed::keeps_no_sign_in`]).

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crucible_auth::Store as Credentials;
use crucible_client_api::bounds::ITEMS;
use crucible_client_api::{
    Capability, Choice, Command, Decision, ErrorCode, InstallProgress, Name, Pending, Refusal,
    Request, Text,
};
use crucible_config::{AgentUpdate, Cloud, ConfigError, Driver, ExternalAgent, Home, Settings};
use crucible_execution::{Authentication, Failure};
use crucible_external_harnesses::catalog::{
    Artifact, AssetRef, AuthMethod, CatalogError, Disposition, Entry, File, FileKind, Files,
    Format, Launch, Method, Offer, Script, Target, Trust, Unprofiled, file_list,
};
use crucible_external_harnesses::release::{
    BIN, LAUNCHER, LaunchScript, NpmTree, PRELOAD, Preload, RELEASE_BYTES, Stub, StubError,
    UnpackError, Unpacker, Wheels,
};
use crucible_external_harnesses::store::{
    Listing, Mismatch, Provenance, Receipt, Store, StoreError, Version,
};
use crucible_external_harnesses::{Account, Reading, WITHHELD_BYTES, antigravity};
use crucible_http::Origin;
use crucible_runtime::Cancel;
use crucible_types::AgentKey;

use super::accounts;
use super::deciding::{self, Confirmation, Front, Shown, mint, turned_away};
use crate::agent_download::{self, DownloadError, Landing};
use crate::backends::{AgentRow, CatalogRow, REGISTRY};
use crate::catalog_fetch::{self, FetchError, Source, Trigger};
use crate::launching::{self, Agent, Launcher, Steps, Unlaunched, Unsmoked, quoted};
use crate::providers::{Rows, Unnamed};
use crate::remember::{self, RememberError};

/// The most catalog agents a listing names; one with more says so.
pub const LISTED: usize = 32;

/// The most bytes a key typed for an agent may have.
pub const KEY_BYTES: usize = 4096;

/// What the door is lent by the front end that asks it.
#[derive(Debug)]
pub struct Shelf<'a> {
    /// Where the catalog is published, who signed it and which hosts a
    /// redirect from it may reach.
    pub catalog: Source,
    /// Crucible's home: the programs, the trust state and the user's own
    /// configuration file are under it.
    pub home: &'a Home,
    /// The launcher of the conversation asking, where there is one, which a
    /// switch is smoke-tested through.
    pub smoke: Option<&'a Launcher<'a>>,
    /// The steps of the conversation asking, where there is one, which know
    /// the runs it has made of each configured instance.
    pub steps: Option<&'a Steps>,
}

/// Where an install says how far it has got, and how it is cancelled, and
/// where a sign-in shows the link it is waiting on.
pub struct Watching<'a> {
    /// Told after every piece of a download that arrives.
    pub progress: &'a mut (dyn FnMut(InstallProgress) + Send),
    /// Raised to give the download up, which then commits nothing.
    pub cancel: &'a Cancel,
    /// Told each sign-in link the agent prints, as its wire passed it, for a
    /// person to open while the sign-in waits.
    pub link: &'a (dyn Fn(crucible_execution::Text) + Sync),
}

impl std::fmt::Debug for Watching<'_> {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("Watching").finish_non_exhaustive()
    }
}

/// One catalog agent, as a listing names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// The agent.
    pub agent: AgentKey,
    /// The versions offered for this platform, newest source first.
    pub offered: Vec<Box<str>>,
    /// The licence of the newest offered.
    pub license: Box<str>,
    /// Where its terms are published, where it has any.
    pub terms: Option<Box<str>>,
    /// What the reviewer wants a user to know first, where anything.
    pub caution: Option<Box<str>>,
    /// The versions the install store holds with a receipt, or why the
    /// agent's directory in the store could not be read.
    pub installed: Result<Vec<Box<str>>, Box<str>>,
    /// Whether its entry names neither a variable for a profile directory
    /// nor a place it keeps its sign-in in, so that a sign-in made under the
    /// sandbox is not kept.
    pub keeps_no_sign_in: bool,
}

/// Which move a confirmed switch was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// A version installed.
    Installed,
    /// The version the catalog now offers, installed.
    Updated,
    /// The version before, named again.
    RolledBack,
    /// A version installed, and one more instance of the agent written that
    /// runs it.
    Added,
}

/// What came of one command at the agents door.
#[derive(Debug)]
pub enum Outcome {
    /// It was turned away before anything was read or changed.
    Refused(Refusal),
    /// The agents the catalog offers.
    Catalog {
        /// Each agent, at most [`LISTED`].
        agents: Vec<Listed>,
        /// Whether there were more than are named.
        more: bool,
        /// Why the catalog was not refreshed first, where it was not: what is
        /// named is what was believed before.
        unrefreshed: Option<FetchError>,
    },
    /// Nobody said yes, so nothing was fetched or changed.
    Undecided,
    /// A version is in place, and the instances named run it.
    Moved {
        /// The agent.
        agent: AgentKey,
        /// Which move it was.
        how: How,
        /// The version now named.
        version: Box<str>,
        /// The instances whose `executable` now names it. For
        /// [`How::Added`], the last is the instance written.
        instances: Vec<Box<str>>,
        /// The instances that would have moved and were left as they are,
        /// because their `executable` named something else by the time it
        /// was written.
        left: Vec<Box<str>>,
        /// Whether anything was downloaded for it.
        downloaded: bool,
        /// Whether it was an update taken without asking, because the user's
        /// own file says `agents.update = "reviewed"`.
        standing: bool,
        /// Why the switch was not smoke-tested, where it was not.
        unsmoked: Option<Box<str>>,
        /// Why the store was not pruned afterwards, where it was not.
        unpruned: Option<Box<AgentsError>>,
    },
    /// An update found the instances already on the version offered.
    Current {
        /// The agent.
        agent: AgentKey,
        /// The version.
        version: Box<str>,
    },
    /// Every version of the agent was removed.
    Removed {
        /// The agent.
        agent: AgentKey,
        /// The versions removed.
        versions: Vec<Box<str>>,
    },
    /// The instance signs in by the method named: the agent said so, and a
    /// status step on the same start then read it signed in.
    SignedIn {
        /// The instance.
        instance: Box<str>,
        /// The method, by its id.
        method: Box<str>,
        /// How it was found signed in.
        by: Signed,
    },
    /// A key typed for the method is kept, and is given to the agent at its
    /// next start; no conversation lent a launcher, so nothing was started
    /// and the sign-in is not yet verified.
    Kept {
        /// The instance.
        instance: Box<str>,
        /// The method, by its id.
        method: Box<str>,
    },
    /// The installed versions, each hashed against its receipt.
    Verified {
        /// The agent.
        agent: AgentKey,
        /// Each version and how it differs from its receipt; nothing where it
        /// is as recorded.
        versions: Vec<(Box<str>, Vec<Mismatch>)>,
    },
}

/// What a sign-in the agent was asked for through `authenticate` was made
/// with; either way a status step on the same start then read it signed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signed {
    /// The agent's own sign-in, which it carried out itself.
    Agent,
    /// A key typed for the method and kept, which the start was given in the
    /// method's variable.
    Kept,
}

/// A key a person typed on this machine for an agent to sign in with: a
/// secret from the moment it was read. Its `Debug` names nothing of it, it has
/// no `Display`, and no refusal of it repeats it.
pub struct Typed(String);

impl Typed {
    /// The key as it was typed.
    #[must_use]
    pub const fn new(typed: String) -> Self {
        Self(typed)
    }
}

impl std::fmt::Debug for Typed {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("Typed(<redacted>)")
    }
}

/// Why a command at the agents door did not do what it was asked.
#[derive(Debug, thiserror::Error)]
pub enum AgentsError {
    /// The user's configuration could not be read.
    #[error(transparent)]
    Config(#[from] ConfigError),
    /// The catalog offers no such version of the agent for this platform.
    #[error("the catalog offers no release {version} of {agent} for this platform")]
    Unoffered {
        /// The agent.
        agent: AgentKey,
        /// The version asked for, or the newest where none was.
        version: Box<str>,
    },
    /// The catalog offers no release of the agent for this platform.
    #[error("the catalog offers no release of {0} for this platform")]
    Unlisted(AgentKey),
    /// No instance runs a version of the agent from the install store.
    #[error("no instance runs {0} from crucible's install store")]
    NotInstalled(AgentKey),
    /// The store holds no other version to go back to.
    #[error("crucible's install store holds no earlier version of {0} to go back to")]
    NoPrevious(AgentKey),
    /// Instances still run a version from the store.
    #[error("{agent} is still the program of {}, so it is not removed", .instances.join(", "))]
    InUse {
        /// The agent.
        agent: AgentKey,
        /// The instances.
        instances: Vec<Box<str>>,
    },
    /// The release is of a kind this door does not install yet.
    #[error("{0}, which this build does not install through the agents door")]
    Unsupported(&'static str),
    /// The release would be started as a program it does not list as one
    /// that runs.
    #[error("the release lists no runnable {0}, which it would be started as")]
    Program(Box<str>),
    /// The version is installed, and not as the catalog pins it.
    #[error(
        "{agent} {version} is installed, and not as the catalog pins it now; remove it before \
         installing it again"
    )]
    Kept {
        /// The agent.
        agent: AgentKey,
        /// The version.
        version: Box<str>,
    },
    /// The release is larger than a release may be.
    #[error("the release is larger than the {RELEASE_BYTES} bytes a release may be")]
    TooLarge,
    /// The install store refused.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// An artifact did not arrive as its pin.
    #[error(transparent)]
    Download(#[from] DownloadError),
    /// An artifact is not what its file list says, or the launcher of a
    /// release a pinned interpreter starts cannot be written: its entry point
    /// cannot be, or its catalog entry does not give the interpreter exactly
    /// `-I`.
    #[error(transparent)]
    Unpacked(#[from] UnpackError),
    /// A program a package tree declares could not be given a stub in
    /// `.bin`.
    #[error(transparent)]
    Stub(#[from] StubError),
    /// A file list the catalog keeps as an asset did not arrive from the
    /// catalog's host as the catalog lists it.
    #[error("the release's file list could not be fetched from the catalog: {0}")]
    FileListUnfetched(FetchError),
    /// A file list the catalog keeps as an asset arrived, and is not a list
    /// of files the catalog could hold.
    #[error("the release's file list could not be read: {0}")]
    FileListUnread(CatalogError),
    /// The user's configuration could not be written.
    #[error(transparent)]
    Remember(#[from] RememberError),
    /// The version was put in place for an instance to be added, and the
    /// user file refused the one update that adds it and moves the instances;
    /// the version stays kept, and the file is as it was.
    #[error("{agent} {version} is installed, but your configuration was not changed: {why}")]
    Unadded {
        /// The agent.
        agent: AgentKey,
        /// The version in place.
        version: Box<str>,
        /// Why the user's file refused it.
        why: Box<RememberError>,
    },
    /// The version put in place did not pass its smoke test, so no instance
    /// was moved onto it.
    #[error(
        "{agent} {version} did not pass its smoke test, so every instance still runs what it \
         ran: {why}"
    )]
    Smoke {
        /// The agent.
        agent: AgentKey,
        /// The version that failed.
        version: Box<str>,
        /// Why.
        why: Unsmoked,
    },
    /// The configuration names no such instance.
    #[error("the configuration names no instance {0}")]
    NoInstance(Box<str>),
    /// The instance's agent signs in by no method this door carries out.
    #[error("{0} signs in by no method crucible carries out; sign it in at its own command")]
    Unsigned(Box<str>),
    /// The agent offers no such method.
    #[error("{agent} offers no sign-in method {method}")]
    NoMethod {
        /// The agent.
        agent: AgentKey,
        /// The method asked for, quoted.
        method: Box<str>,
    },
    /// A business sign-in, and the instance's entry names no Google Cloud
    /// project and location to bill it to.
    #[error(
        "signing in by {method} needs the Google Cloud project and location it is billed to; \
         set externalAgents.{instance}.cloudProject and externalAgents.{instance}.cloudLocation \
         in the configuration file in your home directory, so nothing was started"
    )]
    Unclouded {
        /// The method, quoted.
        method: Box<str>,
        /// The instance.
        instance: Box<str>,
    },
    /// A business sign-in whose entry's location names no host the agent
    /// could be let reach.
    #[error(
        "externalAgents.{0}.cloudLocation names a Google Cloud location too long for the name \
         of the host it is reached at, so the agent could not reach it and nothing was started"
    )]
    Unlocated(Box<str>),
    /// The method starts the agent, and no conversation lent a launcher with
    /// the steps that know its runs.
    #[error(
        "signing in by {0} starts the agent, which needs a conversation to start it in; \
         nothing was started"
    )]
    Conversation(Box<str>),
    /// The method's key has no name to be kept under.
    #[error(transparent)]
    Unnamed(#[from] Unnamed),
    /// No key is kept for the method.
    #[error("no key is kept for {agent} to sign in by {method}; type it on this machine first")]
    Unstored {
        /// The agent.
        agent: AgentKey,
        /// The method, quoted.
        method: Box<str>,
    },
    /// The method is not one whose key is typed.
    #[error("{0} is not a sign-in method that takes a key")]
    Keyless(Box<str>),
    /// The key typed is not one a key can be.
    #[error(
        "the key typed is shorter than {WITHHELD_BYTES} or longer than {KEY_BYTES} characters, \
         or holds a space or an unprintable character, so it was not kept"
    )]
    Typed,
    /// The credential store could not keep the key.
    #[error(transparent)]
    Credentials(#[from] crucible_auth::AuthError),
    /// The entry gives no profile to run the agent under.
    #[error(transparent)]
    Unprofiled(#[from] Unprofiled),
    /// The agent's program was not started.
    #[error(transparent)]
    Unlaunched(#[from] Unlaunched),
    /// The yes to the agent's route could not be written down, so nothing
    /// was started.
    #[error("the yes to this agent's route could not be written down, so nothing was started: {0}")]
    HoldUnkept(RememberError),
    /// The agent did not read as signed in afterwards.
    #[error("the agent did not read as signed in afterwards: {}", .0.reason().as_str())]
    Unverified(Failure),
    /// A sign-in that starts the agent, and an instance on its profile has a
    /// run that could not be confirmed stopped.
    #[error(
        "run not confirmed stopped: {}. {}",
        .0.join(", "),
        crucible_external_harnesses::Refusal::Unstopped
    )]
    Unstopped(Vec<Box<str>>),
    /// A sign-in that starts the agent, and an instance on its profile has a
    /// live run, beside which the agent's program would run. A run that could
    /// not be confirmed stopped is [`AgentsError::Unstopped`] instead.
    #[error(
        "live run: {}. The runs of these instances must end first, as disconnecting an \
         instance ends its run; nothing was started",
        .0.join(", ")
    )]
    Running(Vec<Box<str>>),
}

/// Carries out `request` where it is one of the agent catalog's commands,
/// lent `shelf`, asking `front` before anything changes, and telling
/// `watching` how far a download has got.
///
/// # Errors
///
/// [`AgentsError`] naming what stopped a command a client was entitled to.
pub async fn run(
    request: &Request,
    shelf: &Shelf<'_>,
    front: &mut dyn Front,
    watching: Watching<'_>,
) -> Result<Outcome, AgentsError> {
    let command = request.command();
    match command.needs() {
        Some(needed) if request.capabilities().has(needed) => {}
        Some(_) => return Ok(Outcome::Refused(ErrorCode::NeedsCapability.into())),
        None => return Ok(Outcome::Refused(ErrorCode::Busy.into())),
    }
    let agent = match command {
        // A listing names no agent, and a sign-in names an instance instead.
        Command::AgentCatalog | Command::SignInAgent { .. } => None,
        Command::InstallAgent { agent, .. }
        | Command::AddAgent { agent, .. }
        | Command::UpdateAgent { agent }
        | Command::RollbackAgent { agent }
        | Command::RemoveAgent { agent }
        | Command::VerifyAgent { agent } => match AgentKey::new(agent.as_str()) {
            Ok(key) => Some(key),
            Err(_) => return Ok(Outcome::Refused(ErrorCode::InvalidArgument.into())),
        },
        _ => return Ok(Outcome::Refused(ErrorCode::Busy.into())),
    };
    let state = catalog_fetch::state_in(shelf.home.path());
    let unrefreshed = catalog_fetch::refresh(&state, &shelf.catalog, Trigger::Opening)
        .await
        .err();
    let settings = Settings::read_home(shelf.home)?;
    let door = Door {
        request,
        shelf,
        trust: Trust::load(&state, SystemTime::now()),
        store: Store::new(shelf.home.path().join("programs")),
        user: crucible_config::user(shelf.home),
        instances: settings.external_agents(),
        // The user's own file alone: a checkout cannot ask for this.
        standing: settings.agent_update() == AgentUpdate::Reviewed,
    };
    if let Command::SignInAgent { instance, method } = command {
        return door
            .sign_in(instance.as_str(), method.as_str(), front, watching.link)
            .await;
    }
    let Some(agent) = agent else {
        return Ok(door.listing(unrefreshed));
    };
    match command {
        Command::InstallAgent { version, .. } | Command::AddAgent { version, .. } => {
            let offer = door
                .trust
                .offers(Some(&agent))
                .into_iter()
                .find(|offer| *offer.pin.version == *version.as_str())
                .ok_or_else(|| AgentsError::Unoffered {
                    agent: agent.clone(),
                    version: version.as_str().into(),
                })?;
            let how = if matches!(command, Command::AddAgent { .. }) {
                How::Added
            } else {
                How::Installed
            };
            door.switch(offer, how, front, watching).await
        }
        Command::UpdateAgent { .. } => {
            let mut offers = door.trust.offers(Some(&agent));
            if offers.is_empty() {
                return Err(AgentsError::Unlisted(agent));
            }
            let moving = door.moving(&agent);
            if moving.is_empty() {
                return Err(AgentsError::NotInstalled(agent));
            }
            // Asked about the first one listed, as ever, unless the user's
            // file stands in for the question and one release is plainly the
            // newest: then that one, wherever it is listed.
            let at = door.standing_target(&agent, &offers, &moving).unwrap_or(0);
            let offer = offers.swap_remove(at);
            if moving.iter().all(|(_, version)| {
                version.version() == &*offer.pin.version
                    && version.lock_revision() == offer.pin.lock_revision
            }) {
                return Ok(Outcome::Current {
                    agent,
                    version: offer.pin.version,
                });
            }
            door.switch(offer, How::Updated, front, watching).await
        }
        Command::RollbackAgent { .. } => door.rollback(&agent, front).await,
        Command::RemoveAgent { .. } => door.remove(&agent, front).await,
        Command::VerifyAgent { .. } => door.verify(agent),
        _ => Ok(Outcome::Refused(ErrorCode::Busy.into())),
    }
}

/// A version spelled as dot-separated decimal numbers, as those numbers, or
/// nothing where it is spelled any other way.
fn numbers(version: &str) -> Option<Vec<u64>> {
    version
        .split('.')
        .map(|part| {
            (!part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| part.parse().ok())
                .flatten()
        })
        .collect()
}

/// Which of two versions' numbers is the later, a missing trailing part
/// counting as zero.
fn compare(left: &[u64], right: &[u64]) -> Ordering {
    (0..left.len().max(right.len()))
        .map(|at| {
            let part = |from: &[u64]| from.get(at).copied().unwrap_or(0);
            part(left).cmp(&part(right))
        })
        .find(|order| order.is_ne())
        .unwrap_or(Ordering::Equal)
}

/// What one command is carried out with.
struct Door<'a> {
    request: &'a Request,
    shelf: &'a Shelf<'a>,
    trust: Trust,
    store: Store,
    user: PathBuf,
    instances: Vec<ExternalAgent>,
    /// Whether the user's own file says `agents.update = "reviewed"`, which
    /// stands in for the yes to an update of a release the catalog offers.
    /// Every check that can refuse the release still runs.
    standing: bool,
}

impl Door<'_> {
    /// The agents offered, but those whose key is a word a built-in driver is
    /// spelled with: the entry that backs a built-in is that driver's, and is
    /// no catalog row.
    fn listing(&self, unrefreshed: Option<FetchError>) -> Outcome {
        let mut agents: Vec<Listed> = Vec::new();
        let mut more = false;
        for offer in self.trust.offers(None) {
            let key = &offer.entry.key;
            if Driver::ALL
                .iter()
                .any(|driver| driver.name() == key.as_str())
            {
                continue;
            }
            if let Some(listed) = agents.iter_mut().find(|listed| listed.agent == *key) {
                listed.offered.push(offer.pin.version);
                continue;
            }
            if agents.len() == LISTED {
                more = true;
                continue;
            }
            // One agent's store that cannot be read is said on its row, and
            // the rest are listed as they are.
            let installed = self
                .store
                .installed(key)
                .map(|receipts| {
                    receipts
                        .into_iter()
                        .map(|receipt| receipt.version)
                        .collect()
                })
                .map_err(|unread| unread.to_string().into());
            let keeps_no_sign_in = launching::keeps_no_sign_in(&offer.entry);
            agents.push(Listed {
                agent: key.clone(),
                offered: vec![offer.pin.version],
                license: offer.entry.license,
                terms: offer.entry.terms,
                caution: offer.entry.caution,
                installed,
                keeps_no_sign_in,
            });
        }
        Outcome::Catalog {
            agents,
            more,
            unrefreshed,
        }
    }

    /// Every instance whose `executable` lies in the store's directory of
    /// `agent`, with the version it lies in.
    fn moving(&self, agent: &AgentKey) -> Vec<(&ExternalAgent, Version)> {
        self.instances
            .iter()
            .filter_map(|instance| {
                let version = self
                    .store
                    .version_of(agent, Path::new(instance.executable()))?;
                Some((instance, version))
            })
            .collect()
    }

    /// Whether the user's standing yes covers moving `moving` onto `offer`:
    /// the release is not revoked, is strictly newer than every version being
    /// moved, and comes from a catalog no older than the one each of those was
    /// taken from, so a stale or replayed catalog cannot step an agent back.
    /// A version number that is not plain dotted numbers is never newer.
    fn stands_for(
        &self,
        agent: &AgentKey,
        offer: &Offer,
        moving: &[(&ExternalAgent, Version)],
    ) -> bool {
        if self
            .trust
            .revoked(agent, &offer.pin.version, offer.pin.lock_revision)
        {
            return false;
        }
        moving.iter().all(|(_, installed)| {
            let newer = match (numbers(&offer.pin.version), numbers(installed.version())) {
                (Some(offered), Some(held)) => match compare(&offered, &held) {
                    Ordering::Greater => true,
                    Ordering::Equal => offer.pin.lock_revision > installed.lock_revision(),
                    Ordering::Less => false,
                },
                _ => false,
            };
            newer
                && self
                    .store
                    .receipt(agent, installed)
                    .ok()
                    .flatten()
                    .is_some_and(|receipt| offer.serial >= receipt.listing.serial)
        })
    }

    /// Which of `offers` the standing yes would take: the newest by version
    /// order where it stands for the move, none otherwise.
    fn standing_target(
        &self,
        agent: &AgentKey,
        offers: &[Offer],
        moving: &[(&ExternalAgent, Version)],
    ) -> Option<usize> {
        if !self.standing {
            return None;
        }
        let mut best: Option<(usize, Vec<u64>)> = None;
        for (at, offer) in offers.iter().enumerate() {
            let Some(found) = numbers(&offer.pin.version) else {
                continue;
            };
            if best
                .as_ref()
                .is_none_or(|(_, held)| compare(&found, held) == Ordering::Greater)
            {
                best = Some((at, found));
            }
        }
        let (at, _) = best?;
        offers
            .get(at)
            .is_some_and(|offer| self.stands_for(agent, offer, moving))
            .then_some(at)
    }

    /// The launcher a switch onto a release of `agent` is smoke-tested with,
    /// or why it is not smoke-tested.
    fn smoking(&self, agent: &AgentKey) -> Result<&Launcher<'_>, String> {
        let launcher = self
            .shelf
            .smoke
            .ok_or_else(|| "no conversation lent a launcher to start it with".to_owned())?;
        match launcher.unsmokable(agent) {
            None => Ok(launcher),
            Some(why) => Err(why.to_string()),
        }
    }

    /// What the question says of the smoke test `smoking` planned.
    fn smoked(smoking: &Result<&Launcher<'_>, String>) -> String {
        match smoking {
            Ok(_) => "Smoke-tested before any instance is moved onto it: started once in the \
                      sandbox with the network closed, it must answer initialize as its catalog \
                      entry reviewed it."
                .to_owned(),
            Err(why) => format!("Not smoke-tested: {why}."),
        }
    }

    /// Smoke-tests `program`, of the release `entry` lists as `target`, in
    /// `root`, with `smoking`'s launcher where it planned one, after the
    /// question was answered: a failure leaves every instance as it is.
    async fn smoke(
        smoking: &Result<&Launcher<'_>, String>,
        (entry, target): (&Entry, &Target),
        root: &Path,
        program: &Path,
    ) -> Result<(), AgentsError> {
        let Ok(launcher) = smoking else {
            return Ok(());
        };
        launcher
            .smoke(entry, target, root, program)
            .await
            .map_err(|why| AgentsError::Smoke {
                agent: entry.key.clone(),
                version: quoted(&entry.version).into(),
                why,
            })
    }

    /// Installs `offer` of `agent` after a yes, and moves every instance the
    /// store runs it for onto it.
    async fn switch(
        &self,
        mut offer: Offer,
        how: How,
        front: &mut dyn Front,
        watching: Watching<'_>,
    ) -> Result<Outcome, AgentsError> {
        // A release a pinned interpreter starts is started through a launcher
        // written for its entry point, which is held before anything is asked;
        // one a pinned Node starts, under the preload alone, with no install
        // script run, and its lock is fetched below.
        let (script, lock) = launched(&offer)?;
        let mut reached: Vec<String> = Vec::new();
        let hosts = &offer.pin.target.hosts;
        for artifact in &offer.pin.target.artifacts {
            for place in agent_download::reachable(&self.shelf.catalog, hosts, artifact)? {
                if !reached.contains(&place) {
                    reached.push(place);
                }
            }
        }
        fetch_file_lists(&self.shelf.catalog, &mut offer).await?;
        // The lock from the catalog's own host, read, and every tarball it
        // pins on a host the target lists, all before anything is asked.
        let tree = match &lock {
            Some(asset) => Some(
                fetch_tree(
                    &self.shelf.catalog,
                    asset,
                    &offer.pin.target.hosts,
                    &mut reached,
                )
                .await?,
            ),
            None => None,
        };
        let offer = &offer;
        let agent = &offer.entry.key;
        let target = &offer.pin.target;
        let mut listed = listed(target);
        runnable(&listed, &target.cmd)?;
        let (total, fitted) = sized(offer, tree.as_ref())?;
        let version = Version::new(&offer.pin.version, offer.pin.lock_revision)?;
        let root = self.store.directory(agent, &version);
        let program = root.join(&*target.cmd);
        // The launcher is committed with the version, so a receipt of it in
        // place names it too; so are the preload and `.bin` beside a tree,
        // and the stub in it of each program the tree declares.
        listed.extend(script.as_ref().map(LaunchScript::file));
        let stubs = match &tree {
            Some(tree) => stubs(tree, &root, &program)?,
            None => Vec::new(),
        };
        if tree.is_some() {
            listed.extend([
                Preload::file(),
                File {
                    path: BIN.into(),
                    kind: FileKind::Dir,
                },
            ]);
            listed.extend(stubs.iter().map(Stub::file));
        }
        let kept = match self.store.receipt(agent, &version)? {
            Some(receipt) if same(&receipt, target, &listed, tree.as_ref()) => true,
            Some(_) => {
                return Err(AgentsError::Kept {
                    agent: agent.clone(),
                    version: offer.pin.version.clone(),
                });
            }
            None => false,
        };
        // Room for what it unpacks to, and for the download beside it while
        // it is unpacked; a version kept takes none.
        if !kept {
            self.store.fits_while_downloading(&fitted)?;
        }
        let moving = self.moving(agent);

        let plan = Plan {
            kept,
            total,
            tree: tree.as_ref(),
            reached,
            smoking: self.smoking(agent),
            // The name the question shows; the one written is chosen again
            // from the file under its lock.
            adding: (how == How::Added).then(|| {
                crucible_config::free_name(agent, self.instances.iter().map(ExternalAgent::name))
            }),
        };
        // Only an update can stand on the user's file; any other move is asked.
        let standing =
            self.standing && how == How::Updated && self.stands_for(agent, offer, &moving);
        if !standing {
            let confirmation = Self::installing(offer, how, &plan, &moving);
            if confirmed(self.request, front, &confirmation).await != Some(0) {
                return Ok(Outcome::Undecided);
            }
        }

        if !kept {
            let staging = self.store.stage(agent)?;
            let name = Name::new(agent.as_str()).ok();
            let mut files = Vec::new();
            let mut fetched = Vec::new();
            let mut before: u64 = 0;
            for artifact in &target.artifacts {
                let progress = &mut *watching.progress;
                let mut told = |done: u64, _: u64| {
                    let done = before.saturating_add(done).min(total);
                    if let Some(said) = name
                        .clone()
                        .and_then(|name| InstallProgress::new(name, done, total))
                    {
                        progress(said);
                    }
                };
                let mut landing = staging.download()?;
                let arrived = agent_download::artifact(
                    &self.shelf.catalog,
                    &target.hosts,
                    artifact,
                    Landing {
                        file: &mut landing,
                        progress: &mut told,
                    },
                    watching.cancel,
                )
                .await?;
                drop(landing);
                before = before.saturating_add(arrived.size);
                let mut reading = staging.downloaded()?;
                let staged = if script.is_some() && artifact.format == Format::Wheel {
                    Wheels::unpack_from(&mut reading, artifact, staging.directory())?
                } else {
                    Unpacker::unpack_from(&mut reading, artifact, staging.directory())?
                };
                files.extend_from_slice(staged.files());
                fetched.push(arrived);
            }
            if let Some(script) = &script {
                files.push(script.write(staging.directory())?);
            }
            if let Some(tree) = &tree {
                // The last artifact's download is unpacked; emptied, it takes
                // no room the tarballs need.
                drop(staging.download()?);
                let progress = &mut *watching.progress;
                let mut told = |done: u64, _: u64| {
                    let done = before.saturating_add(done).min(total);
                    if let Some(said) = name
                        .clone()
                        .and_then(|name| InstallProgress::new(name, done, total))
                    {
                        progress(said);
                    }
                };
                files.extend(
                    noded(
                        &self.shelf.catalog,
                        agent_download::Tree {
                            hosts: &target.hosts,
                            tree,
                        },
                        &staging,
                        &mut told,
                        watching.cancel,
                    )
                    .await?,
                );
                files.extend(stubbed(&stubs, staging.directory())?);
            }
            let provenance = Provenance {
                listing: Listing {
                    serial: offer.serial,
                    review: offer.entry.review.clone(),
                    terms: offer.entry.terms.clone(),
                },
                fetched: SystemTime::now(),
                artifacts: fetched,
            };
            self.store.commit(staging, &version, &files, provenance)?;
        }
        Self::smoke(&plan.smoking, (&offer.entry, target), &root, &program).await?;
        let (instances, left) = if plan.adding.is_some() {
            self.add(agent, &offer.pin.version, &moving, &program)?
        } else {
            self.point(&moving, &program)?
        };
        Ok(Outcome::Moved {
            agent: agent.clone(),
            how,
            version: offer.pin.version.clone(),
            instances,
            left,
            downloaded: !kept,
            standing,
            unsmoked: plan.smoking.as_ref().err().map(|why| why.as_str().into()),
            unpruned: self.prune(agent, &program).err().map(Box::new),
        })
    }

    /// The question an install, add or update puts.
    fn installing(
        offer: &Offer,
        how: How,
        plan: &Plan<'_>,
        moving: &[(&ExternalAgent, Version)],
    ) -> Confirmation {
        let kept = plan.kept;
        let target = &offer.pin.target;
        let entry = &offer.entry;
        let total = plan.total;
        let (key, version) = (entry.key.as_str(), quoted(&offer.pin.version));
        let putting = format!("Put {version} in place and move the instances named onto it");
        let (asks, choice, does) = match (how, &plan.adding) {
            (How::Updated, _) => (format!("Update {key} to {version}?"), "Update", putting),
            (How::Added, Some(name)) => (
                format!("Add {key} {version} as the instance {}?", quoted(name)),
                "Add",
                format!(
                    "Put {version} in place, move the instances named onto it and add the instance {}",
                    quoted(name)
                ),
            ),
            // The plan of an add always holds the name it shows.
            (How::Installed | How::RolledBack | How::Added, _) => {
                (format!("Install {key} {version}?"), "Install", putting)
            }
        };
        let mut facts = vec![if kept {
            format!("{key} {version} is already installed, so nothing is downloaded.")
        } else {
            format!("{key} {version}, {total} bytes to download.")
        }];
        if kept && plan.tree.is_some() {
            facts.push(
                "Known by its receipt: Node's artifacts by SHA-256, and Node's files, the preload \
                 and .bin as pinned; its package tree's files only as lying where its lock \
                 installs a package, which are not read again."
                    .to_owned(),
            );
        }
        if !kept {
            let reached: Vec<String> = plan.reached.iter().map(|place| quoted(place)).collect();
            let mut mirrors: Vec<String> = target
                .artifacts
                .iter()
                .filter(|artifact| artifact.replaced_by.is_none())
                .filter_map(|artifact| artifact.mirror.as_deref().and_then(Origin::of))
                .map(|origin| quoted(origin.host()))
                .collect();
            dedup(&mut mirrors);
            facts.push(if mirrors.is_empty() {
                format!(
                    "Fetched from {}, the only hosts it may reach.",
                    reached.join(", ")
                )
            } else {
                format!(
                    "Fetched from {}, the only hosts it may reach, the catalog's copy at {} \
                     if the vendor's address fails.",
                    reached.join(", "),
                    mirrors.join(", ")
                )
            });
            if let Some(tree) = plan.tree {
                let count = tree.packages().len();
                let packages = if count == 1 { "package" } else { "packages" };
                facts.push(format!(
                    "Its package tree holds {count} {packages}, each unpacked from the tarball its \
                     lock pins by SHA-512."
                ));
            }
        }
        let license = quoted(&entry.license);
        facts.push(match &entry.terms {
            Some(terms) => format!("Licence {license}; terms at {}.", quoted(terms)),
            None => format!("Licence {license}."),
        });
        if let Some(tree) = plan.tree {
            facts.extend(licences(tree));
        }
        if let Some(caution) = &entry.caution {
            facts.push(format!("Caution: {}", quoted(caution)));
        }
        let mut started = vec![quoted(&target.cmd)];
        match &target.launch {
            Launch::Uvx {
                interpreter_args, ..
            } => {
                started.extend(interpreter_args.iter().map(|argument| quoted(argument)));
                started.push(quoted(LAUNCHER));
            }
            Launch::Npm { bin, scripts, .. } => {
                facts.extend(unrun(scripts));
                started.extend(["--require".to_owned(), PRELOAD.to_owned(), quoted(bin)]);
            }
            Launch::Binary => {}
        }
        started.extend(target.args.iter().map(|argument| quoted(argument)));
        facts.push(format!("Started as: {}", started.join(" ")));
        if matches!(target.launch, Launch::Npm { .. }) {
            facts.push(format!(
                "Node is given the preload first, which refuses a package the version's tree \
                 does not hold, and {BIN}, a stub for each program the tree declares, last on \
                 its PATH."
            ));
        }
        facts.push(if target.env.is_empty() {
            "Started with no environment of its own.".to_owned()
        } else {
            let set: Vec<String> = target
                .env
                .iter()
                .map(|variable| format!("{}={}", quoted(&variable.name), quoted(&variable.value)))
                .collect();
            format!("Started with the environment {}.", set.join(", "))
        });
        facts.push(moves(&entry.key, &offer.pin.version, moving));
        if let Some(name) = &plan.adding {
            facts.push(format!(
                "Adds the instance {} to your configuration, running {version}.",
                quoted(name)
            ));
        }
        facts.push(Self::smoked(&plan.smoking));
        Confirmation {
            asks,
            facts,
            choices: vec![
                (choice.to_owned(), does),
                ("Cancel".to_owned(), "Leave everything as it is".to_owned()),
            ],
        }
    }

    /// Writes each of `moving` onto `program`, in one update of the user
    /// file, where its `executable` still names what the question showed;
    /// gives the instances written, and those left because they name
    /// something else by now.
    fn point(
        &self,
        moving: &[(&ExternalAgent, Version)],
        program: &Path,
    ) -> Result<Written, AgentsError> {
        if moving.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }
        let program = program.to_string_lossy();
        let left = remember::pointing(&self.user, &moved(moving, &program))?;
        Ok(written(moving, left))
    }

    /// What [`Self::point`] writes, and one more instance of `agent` started
    /// as `program`, in the same update of the user file; gives the instances
    /// written, the one added last, and those left. A refusal writes neither,
    /// and says `version` is kept.
    fn add(
        &self,
        agent: &AgentKey,
        version: &str,
        moving: &[(&ExternalAgent, Version)],
        program: &Path,
    ) -> Result<Written, AgentsError> {
        let program = program.to_string_lossy();
        let (left, added) = remember::adding(&self.user, &moved(moving, &program), agent, &program)
            .map_err(|why| AgentsError::Unadded {
                agent: agent.clone(),
                version: version.into(),
                why: Box::new(why),
            })?;
        let (mut instances, left) = written(moving, left);
        instances.push(added.into());
        Ok((instances, left))
    }

    /// Prunes the store of `agent` to `program`, the one just put in place,
    /// and what the instances name now, read again from the user file, so an
    /// instance changed since the question keeps the version it names, and
    /// the newest other version is kept to go back to even where no instance
    /// runs the agent from the store.
    fn prune(&self, agent: &AgentKey, program: &Path) -> Result<(), AgentsError> {
        let instances = Settings::read_home(self.shelf.home)?.external_agents();
        let named: Vec<&Path> = std::iter::once(program)
            .chain(
                instances
                    .iter()
                    .map(|instance| Path::new(instance.executable())),
            )
            .collect();
        self.store.prune(agent, &named)?;
        Ok(())
    }

    /// Names the version before again, after a yes.
    async fn rollback(
        &self,
        agent: &AgentKey,
        front: &mut dyn Front,
    ) -> Result<Outcome, AgentsError> {
        let moving = self.moving(agent);
        let Some((first, current)) = moving.first() else {
            return Err(AgentsError::NotInstalled(agent.clone()));
        };
        let within = Path::new(first.executable())
            .strip_prefix(self.store.directory(agent, current))
            .map(Path::to_path_buf)
            .ok();
        let previous = self
            .store
            .installed(agent)?
            .into_iter()
            .filter(|receipt| {
                !moving.iter().any(|(_, named)| {
                    named.version() == &*receipt.version
                        && named.lock_revision() == receipt.lock_revision
                })
            })
            .filter(|receipt| {
                !self
                    .trust
                    .revoked(agent, &receipt.version, receipt.lock_revision)
            })
            .max_by_key(|receipt| receipt.fetched)
            .ok_or_else(|| AgentsError::NoPrevious(agent.clone()))?;
        let version = Version::new(&previous.version, previous.lock_revision)?;
        let program = within
            .filter(|within| {
                let spelled = within
                    .components()
                    .map(|part| part.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/");
                previous.files.iter().any(|file| {
                    *file.path == *spelled
                        && matches!(
                            file.kind,
                            crucible_external_harnesses::catalog::FileKind::File { exec: true, .. }
                        )
                })
            })
            .map(|within| self.store.directory(agent, &version).join(within))
            .ok_or_else(|| AgentsError::NoPrevious(agent.clone()))?;
        let mut facts = vec![format!(
            "{agent} {} is installed, so nothing is downloaded.",
            quoted(&previous.version)
        )];
        facts.push(match &previous.listing.terms {
            Some(terms) => format!(
                "Its terms, accepted when it was installed: {}.",
                quoted(terms)
            ),
            None => "It was installed with no terms to accept.".to_owned(),
        });
        facts.push(moves(agent, &previous.version, &moving));
        // Smoke-tested only as the catalog still lists it: its entry is what
        // says what it must answer as.
        let listed = self.trust.offers(Some(agent)).into_iter().find(|offer| {
            *offer.pin.version == *previous.version
                && offer.pin.lock_revision == previous.lock_revision
        });
        let smoking = match &listed {
            Some(_) => self.smoking(agent),
            None => Err(format!(
                "the catalog no longer lists {}, so nothing says what it must answer as",
                quoted(&previous.version)
            )),
        };
        facts.push(Self::smoked(&smoking));
        let confirmation = Confirmation {
            asks: format!("Roll {agent} back to {}?", quoted(&previous.version)),
            facts,
            choices: vec![
                (
                    "Roll back".to_owned(),
                    format!(
                        "Move the instances named back onto {}",
                        quoted(&previous.version)
                    ),
                ),
                ("Cancel".to_owned(), "Leave everything as it is".to_owned()),
            ],
        };
        if confirmed(self.request, front, &confirmation).await != Some(0) {
            return Ok(Outcome::Undecided);
        }
        if let Some(offer) = &listed {
            let root = self.store.directory(agent, &version);
            Self::smoke(&smoking, (&offer.entry, &offer.pin.target), &root, &program).await?;
        }
        let (instances, left) = self.point(&moving, &program)?;
        Ok(Outcome::Moved {
            agent: agent.clone(),
            how: How::RolledBack,
            version: previous.version,
            instances,
            left,
            downloaded: false,
            standing: false,
            unsmoked: smoking.as_ref().err().map(|why| why.as_str().into()),
            unpruned: self.prune(agent, &program).err().map(Box::new),
        })
    }

    /// Removes every version of `agent`, after a yes, where no instance runs
    /// one.
    async fn remove(
        &self,
        agent: &AgentKey,
        front: &mut dyn Front,
    ) -> Result<Outcome, AgentsError> {
        let moving = self.moving(agent);
        if !moving.is_empty() {
            return Err(AgentsError::InUse {
                agent: agent.clone(),
                instances: moving
                    .iter()
                    .map(|(instance, _)| instance.name().into())
                    .collect(),
            });
        }
        let installed: Vec<Box<str>> = self
            .store
            .installed(agent)?
            .into_iter()
            .map(|receipt| receipt.version)
            .collect();
        if installed.is_empty() {
            return Err(AgentsError::NotInstalled(agent.clone()));
        }
        let confirmation = Confirmation {
            asks: format!("Remove {agent} from this machine?"),
            facts: vec![
                format!(
                    "Removes {agent} {}.",
                    installed
                        .iter()
                        .map(|version| quoted(version))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                "No configured instance runs it from crucible's install store.".to_owned(),
            ],
            choices: vec![
                ("Remove".to_owned(), "Remove every version".to_owned()),
                ("Cancel".to_owned(), "Leave everything as it is".to_owned()),
            ],
        };
        if confirmed(self.request, front, &confirmation).await != Some(0) {
            return Ok(Outcome::Undecided);
        }
        // Read again: an instance set to run it since the question keeps it.
        let instances = Settings::read_home(self.shelf.home)?.external_agents();
        let executables: Vec<&Path> = instances
            .iter()
            .map(|instance| Path::new(instance.executable()))
            .collect();
        let removed = self.store.remove(agent, &executables)?;
        Ok(Outcome::Removed {
            agent: agent.clone(),
            versions: removed
                .iter()
                .map(|version| version.version().into())
                .collect(),
        })
    }

    /// Signs the configured `instance` in by the method its agent offers as
    /// `method`, telling `link` each sign-in link it prints.
    async fn sign_in(
        &self,
        instance: &str,
        method: &str,
        front: &mut dyn Front,
        link: &(dyn Fn(crucible_execution::Text) + Sync),
    ) -> Result<Outcome, AgentsError> {
        let Some(configured) = self.instances.iter().find(|one| one.name() == instance) else {
            return Ok(Outcome::Refused(ErrorCode::InvalidArgument.into()));
        };
        let (row, agent, found) = signing(&self.trust, &self.store, configured, method)?;
        let signed = |by| {
            Ok(Outcome::SignedIn {
                instance: instance.into(),
                method: found.id.clone(),
                by,
            })
        };
        let (launcher, by, billed) = match &found.method {
            Method::EnvVar { .. } => {
                let rows = Rows::production();
                let name = rows.signing_in(&agent, &found.id)?;
                let stored = Credentials::in_home(self.shelf.home.path())
                    .naming(rows.names())
                    .read();
                if !stored.has_key(&name) {
                    return Err(AgentsError::Unstored {
                        agent,
                        method: quoted(&found.id).into(),
                    });
                }
                // The agent learns which method it signs in by only from
                // being asked, so with nothing lent to start it the key is
                // kept and no more is known.
                let Some(launcher) = self.shelf.smoke else {
                    return Ok(Outcome::Kept {
                        instance: instance.into(),
                        method: found.id.clone(),
                    });
                };
                (launcher, Signed::Kept, None)
            }
            Method::Terminal { .. } => {
                return Err(AgentsError::Unsupported(
                    "a sign-in run at a terminal, which this build does not run through the \
                     agents door",
                ));
            }
            Method::Agent => {
                let billed = match &row {
                    AgentRow::Built(built)
                        if built.driver == Driver::Antigravity
                            && &*found.id == antigravity::OAUTH_BUSINESS =>
                    {
                        Some(billed(configured, instance, &found.id)?)
                    }
                    AgentRow::Built(_) | AgentRow::Catalog(_) => None,
                };
                let launcher = self
                    .shelf
                    .smoke
                    .ok_or_else(|| AgentsError::Conversation(quoted(&found.id).into()))?;
                (launcher, Signed::Agent, billed)
            }
        };
        // The agent's program is started beside any run of an instance on its
        // profile, so none may be one that could not be confirmed stopped or
        // one live, and what runs there are is known only to the conversation
        // that made them.
        let steps = self
            .shelf
            .steps
            .ok_or_else(|| AgentsError::Conversation(quoted(&found.id).into()))?;
        let sharing: Vec<ExternalAgent> = self
            .instances
            .iter()
            .filter(|other| accounts::scope(other) == accounts::scope(configured))
            .cloned()
            .collect();
        beside(steps, &sharing)?;
        let started = Agent::of_row(configured, row, |name| std::env::var_os(name));
        // Its settings are written once the route has its yes, just before
        // the agent is started.
        let started = match billed {
            Some(cloud) => started.billed_to(cloud),
            None => started,
        };
        if let Some(refused) = launcher.refusal(&started) {
            return Err(refused.into());
        }
        // Nothing of the agent's is started before its route has a yes, and a
        // yes is written down before anything is.
        if let Some(warned) = launcher.consent().asks(started.row().route()).cloned() {
            if !deciding::warned(self.request.capabilities(), front, &warned).await {
                return Ok(Outcome::Undecided);
            }
            launcher
                .consent()
                .accept(&warned)
                .map_err(AgentsError::HoldUnkept)?;
        }
        let trust = Trust::load(
            &catalog_fetch::state_in(self.shelf.home.path()),
            SystemTime::now(),
        );
        let mut dialect = started
            .row()
            .dialect(
                crucible_tools::Mode::default(),
                Some(&trust),
                started.installed(),
            )
            .map_err(AgentsError::Unverified)?;
        drop(trust);
        if !dialect.signing_in_with(&found.id) {
            return Err(AgentsError::Unsupported(
                "an agent whose wire signs in by no method named",
            ));
        }
        // The route was asked, and a run may have begun, or failed to stop,
        // while it was.
        beside(steps, &sharing)?;
        // The agent's program is started on the instance's own control, so a
        // run of the instance is refused beside it, and it beside one, and a
        // stop that cannot be confirmed leaves the instance, and so every
        // instance on its profile, refused as a run not confirmed stopped is.
        let control = steps.control(configured.name()).ok_or_else(|| {
            AgentsError::Unverified(Failure::new("no control for the agent's program"))
        })?;
        // The start is given every key kept for the agent, so a kept key's
        // method is asked for with its key already in the agent's environment.
        match launcher
            .account(&started, &control, dialect.as_mut(), Account::SignIn, link)
            .await?
        {
            Reading::Signed(Authentication::SignedIn) => signed(by),
            Reading::Unreadable(failure) => Err(AgentsError::Unverified(failure)),
            Reading::Signed(_) | Reading::Version(_) => Err(AgentsError::Unverified(Failure::new(
                "The agent did not read as signed in after the sign-in.",
            ))),
        }
    }

    /// Hashes every installed version of `agent` against its receipt.
    fn verify(&self, agent: AgentKey) -> Result<Outcome, AgentsError> {
        let installed = self.store.installed(&agent)?;
        if installed.is_empty() {
            return Err(AgentsError::NotInstalled(agent));
        }
        let mut versions = Vec::new();
        for receipt in installed {
            let version = Version::new(&receipt.version, receipt.lock_revision)?;
            versions.push((receipt.version, self.store.verify(&agent, &version)?));
        }
        Ok(Outcome::Verified { agent, versions })
    }
}

/// Whether the agent's program may be started beside the runs of `sharing`, the
/// configured instances on one profile, as `steps` knows them: none may have a
/// run that could not be confirmed stopped, which is named first since such a
/// run also holds its control, and then none a run live. Nothing is stopped,
/// disconnected or interrupted to make it so.
///
/// # Errors
///
/// [`AgentsError::Unstopped`] or [`AgentsError::Running`] naming those
/// instances.
fn beside(steps: &Steps, sharing: &[ExternalAgent]) -> Result<(), AgentsError> {
    let held = steps.unstopped(sharing);
    if !held.is_empty() {
        return Err(AgentsError::Unstopped(
            held.into_iter().map(Into::into).collect(),
        ));
    }
    let live = steps.running(sharing);
    if !live.is_empty() {
        return Err(AgentsError::Running(
            live.into_iter().map(Into::into).collect(),
        ));
    }
    Ok(())
}

/// Keeps `key`, typed on this machine, for the configured `instance` to sign
/// in by the method its agent offers as `method`, in crucible's credential
/// store in `home`, under the agent's name and the method's id; every start of
/// the agent after it is given the key in the variable the method names.
///
/// Nothing is fetched: the methods are those the trust state offers now, for
/// the release the instance runs or else the newest.
///
/// # Errors
///
/// [`AgentsError`] where the configuration cannot be read or names no such
/// instance, its agent offers no such method or the method takes no key, the
/// agent has a provider's name, the key is not one a key can be, or the store
/// cannot keep it. None repeats the key.
pub fn keep_key(home: &Home, instance: &str, method: &str, key: Typed) -> Result<(), AgentsError> {
    let settings = Settings::read_home(home)?;
    let instances = settings.external_agents();
    let configured = instances
        .iter()
        .find(|one| one.name() == instance)
        .ok_or_else(|| AgentsError::NoInstance(quoted(instance).into()))?;
    let trust = Trust::load(&catalog_fetch::state_in(home.path()), SystemTime::now());
    let store = Store::new(home.path().join("programs"));
    let (_, agent, found) = signing(&trust, &store, configured, method)?;
    if !matches!(found.method, Method::EnvVar { .. }) {
        return Err(AgentsError::Keyless(quoted(&found.id).into()));
    }
    let rows = Rows::production();
    let name = rows.signing_in(&agent, &found.id)?;
    let Typed(typed) = key;
    let typed = typed.trim();
    let sound = (WITHHELD_BYTES..=KEY_BYTES).contains(&typed.len())
        && typed.bytes().all(|byte| byte.is_ascii_graphic());
    if !sound {
        return Err(AgentsError::Typed);
    }
    Credentials::in_home(home.path())
        .naming(rows.names())
        .keep(&name, typed)?;
    Ok(())
}

/// The ids of the methods the configured `instance` in `home` signs in by, in
/// the order its agent's row lists them: the methods a sign-in through this
/// door finds the one it is asked for among.
///
/// Nothing is fetched: the methods are those the trust state offers now, for
/// the release the instance runs or else the newest.
///
/// # Errors
///
/// [`AgentsError`] where the configuration cannot be read or names no such
/// instance, or its agent is one this door signs nothing in for or the trust
/// state offers no release of.
pub fn sign_in_methods(home: &Home, instance: &str) -> Result<Vec<Box<str>>, AgentsError> {
    let settings = Settings::read_home(home)?;
    let instances = settings.external_agents();
    let configured = instances
        .iter()
        .find(|one| one.name() == instance)
        .ok_or_else(|| AgentsError::NoInstance(quoted(instance).into()))?;
    let trust = Trust::load(&catalog_fetch::state_in(home.path()), SystemTime::now());
    let store = Store::new(home.path().join("programs"));
    let (row, _) = signing_row(&trust, &store, configured)?;
    Ok(row
        .methods()
        .into_iter()
        .map(|offered| offered.id)
        .collect())
}

/// The Google Cloud project and location the configured `instance`, named
/// `name`, bills a business sign-in by `method` to, where its entry names
/// both and the location names a host its agent can be let reach.
///
/// # Errors
///
/// [`AgentsError::Unclouded`] where the entry names neither, and
/// [`AgentsError::Unlocated`] where the location's host is not one a sandbox
/// policy can name.
fn billed(instance: &ExternalAgent, name: &str, method: &str) -> Result<Cloud, AgentsError> {
    let cloud = instance.cloud().ok_or_else(|| AgentsError::Unclouded {
        method: quoted(method).into(),
        instance: name.into(),
    })?;
    if launching::located(cloud.location()).is_err() {
        return Err(AgentsError::Unlocated(name.into()));
    }
    Ok(cloud.clone())
}

/// The row the configured `instance` runs as, its agent's name, and the
/// method among those it signs in by whose id is `method`, the row found by
/// [`signing_row`].
fn signing(
    trust: &Trust,
    store: &Store,
    instance: &ExternalAgent,
    method: &str,
) -> Result<(AgentRow, AgentKey, AuthMethod), AgentsError> {
    let (row, agent) = signing_row(trust, store, instance)?;
    let found = row
        .methods()
        .into_iter()
        .find(|offered| *offered.id == *method)
        .ok_or_else(|| AgentsError::NoMethod {
            agent: agent.clone(),
            method: quoted(method).into(),
        })?;
    Ok((row, agent, found))
}

/// The row the configured `instance` runs as and its agent's name: a catalog
/// agent's row made from the entry `trust` offers for the release it runs in
/// `store`, or else the newest offered, and Antigravity's built in.
fn signing_row(
    trust: &Trust,
    store: &Store,
    instance: &ExternalAgent,
) -> Result<(AgentRow, AgentKey), AgentsError> {
    let row = match instance.driver() {
        Driver::Catalog => {
            let key = instance
                .agent()
                .ok_or_else(|| AgentsError::Unsigned(quoted(instance.name()).into()))?;
            let offers = trust.offers(Some(key));
            let running = store.version_of(key, Path::new(instance.executable()));
            let offer = offers
                .iter()
                .find(|offer| {
                    running.as_ref().is_some_and(|version| {
                        version.version() == &*offer.pin.version
                            && version.lock_revision() == offer.pin.lock_revision
                    })
                })
                .or_else(|| offers.first())
                .ok_or_else(|| AgentsError::Unlisted(key.clone()))?;
            AgentRow::Catalog(CatalogRow::new(&offer.entry, &offer.pin.target)?)
        }
        Driver::Antigravity => REGISTRY
            .built(Driver::Antigravity)
            .map(AgentRow::Built)
            .ok_or_else(|| AgentsError::Unsigned(quoted(instance.name()).into()))?,
        Driver::Claude => return Err(AgentsError::Unsigned(quoted(instance.name()).into())),
    };
    let agent = AgentKey::new(row.named())
        .map_err(|_| AgentsError::Unsigned(quoted(instance.name()).into()))?;
    Ok((row, agent))
}

/// What downloading `offer` and `tree`, where it has one, takes, in bytes,
/// refused where it or what it unpacks to is more than a release may be;
/// and its target as the install store's room is asked for it, with one more
/// artifact standing for the tree.
fn sized(offer: &Offer, tree: Option<&NpmTree>) -> Result<(u64, Target), AgentsError> {
    let mut downloaded: u64 = 0;
    let mut unpacked: u64 = 0;
    let within = |sum: Option<u64>| {
        sum.filter(|bytes| *bytes <= RELEASE_BYTES)
            .ok_or(AgentsError::TooLarge)
    };
    for artifact in &offer.pin.target.artifacts {
        let size = artifact
            .replaced_by
            .as_ref()
            .map_or(artifact.size, |asset| asset.size);
        downloaded = within(downloaded.checked_add(size))?;
        unpacked = within(unpacked.checked_add(artifact.unpacked))?;
    }
    let mut room = offer.pin.target.clone();
    if let Some(tree) = tree {
        // Every tarball is downloaded once, however many packages it is
        // unpacked for, and each package unpacks its own; both sums are
        // checked, so a lock that overflows them is refused, not wrapped.
        let mut seen = std::collections::HashSet::new();
        let (mut tarred, mut spread): (u64, u64) = (0, 0);
        for tarball in tree
            .packages()
            .iter()
            .filter_map(|package| package.tarball.as_ref())
        {
            if seen.insert(&*tarball.sha512) {
                tarred = within(tarred.checked_add(tarball.size))?;
            }
            spread = within(spread.checked_add(tarball.unpacked))?;
        }
        downloaded = within(downloaded.checked_add(tarred))?;
        within(unpacked.checked_add(spread))?;
        // The tarballs lie in the staging until they are unpacked, so the
        // room asked for stands them as one download beside what the tree
        // unpacks to.
        room.artifacts.push(Artifact {
            url: String::new().into(),
            mirror: None,
            sha256: String::new().into(),
            size: tarred,
            format: Format::TarGz,
            unpacked: spread,
            decompressed: spread,
            files: Files::Listed(Vec::new()),
            kept: Vec::new(),
            replaced_by: None,
        });
    }
    Ok((downloaded, room))
}

/// What starting `offer` takes beyond its artifacts: the launcher of a
/// release a pinned interpreter starts, or the lock of one a pinned Node
/// starts, refused where Node is given arguments of its own or an install
/// script is to be run at install, which this build does not do.
fn launched(offer: &Offer) -> Result<(Option<LaunchScript>, Option<AssetRef>), AgentsError> {
    Ok(match &offer.pin.target.launch {
        Launch::Binary => (None, None),
        Launch::Uvx {
            interpreter_args,
            entry_point,
        } => (
            Some(LaunchScript::new(
                &offer.entry.key,
                interpreter_args,
                entry_point,
            )?),
            None,
        ),
        Launch::Npm {
            node_args,
            lock,
            scripts,
            ..
        } => {
            if !Preload::admits(node_args) {
                return Err(AgentsError::Unsupported(
                    "the release gives its pinned Node arguments of its own",
                ));
            }
            if scripts
                .iter()
                .any(|script| matches!(script.disposition, Disposition::Local { .. }))
            {
                return Err(AgentsError::Unsupported(
                    "the release has an install script run at install",
                ));
            }
            (None, Some(lock.asset.clone()))
        }
    })
}

/// The facts naming each install script of a tree, which none is run as npm
/// would run it, at most [`SCRIPTS`] of them, then how many more.
fn unrun(scripts: &[Script]) -> Vec<String> {
    let mut facts: Vec<String> = scripts
        .iter()
        .take(SCRIPTS)
        .map(|script| {
            let why = match &script.disposition {
                Disposition::Inert => "inert: it does nothing the agent needs",
                Disposition::Replaced => "replaced: a platform package does its work",
                Disposition::Lost => "lost: the agent works without what it would add",
                Disposition::Built => "built at review and installed as an asset",
                // Refused before the question is put.
                Disposition::Local { .. } => "run at install, which this build does not do",
            };
            format!(
                "Install script not run: {} {}, {why}.",
                quoted(&script.package),
                quoted(&script.event)
            )
        })
        .collect();
    let more = scripts.len().saturating_sub(SCRIPTS);
    if more > 0 {
        facts.push(format!("And {more} more install scripts, none run."));
    }
    facts
}

/// The most install scripts a question names one by one.
const SCRIPTS: usize = 16;

/// How many packages of `tree` are under each licence its lock gives, those
/// that give none counted together, the most packages first and then by
/// licence; the row of those that give none is placed by its count like any
/// other, after every licence given as many packages: no more than
/// [`LICENCES`] of them, then how many more. A lock in which no package gives
/// a licence carries no counts, so none is given. Each licence is quoted
/// between double quotes, so no text a lock gives can read as the row of
/// those that give none, which is the question's own words.
fn licences(tree: &NpmTree) -> Vec<String> {
    if tree
        .packages()
        .iter()
        .all(|package| package.license.is_none())
    {
        return Vec::new();
    }
    let mut counted: HashMap<Option<&str>, usize> = HashMap::new();
    for package in tree.packages() {
        *counted.entry(package.license.as_deref()).or_default() += 1;
    }
    let mut counted: Vec<(Option<&str>, usize)> = counted.into_iter().collect();
    counted.sort_by(|(one, many), (other, more)| {
        more.cmp(many)
            .then(one.is_none().cmp(&other.is_none()))
            .then(one.cmp(other))
    });
    let mut facts: Vec<String> = counted
        .iter()
        .take(LICENCES)
        .map(|(license, count)| {
            let packages = if *count == 1 { "package" } else { "packages" };
            match license {
                Some(license) => {
                    format!(
                        "{count} {packages} of its tree under \"{}\".",
                        quoted(license)
                    )
                }
                None => format!("{count} {packages} of its tree under no licence given."),
            }
        })
        .collect();
    let more = counted.len().saturating_sub(LICENCES);
    if more > 0 {
        facts.push(format!("And {more} more licences."));
    }
    facts
}

/// The most licences a question counts the packages under one by one.
const LICENCES: usize = 8;

/// The files of `tree` installed into `staging`: each tarball
/// downloaded by [`agent_download::tarballs`] into a directory of the
/// staging's own, unpacked from there and the directory removed, so it is not
/// committed; then the preload, written new and writable by its owner alone,
/// and `.bin`, made empty and reachable by its owner alone.
async fn noded(
    source: &Source,
    tree: agent_download::Tree<'_>,
    staging: &crucible_external_harnesses::store::Staging,
    progress: &mut (dyn FnMut(u64, u64) + Send),
    cancel: &Cancel,
) -> Result<Vec<File>, AgentsError> {
    let root = staging.directory();
    let tarballs = root.join(TARBALLS);
    agent_download::tarballs(source, tree, &tarballs, progress, cancel).await?;
    let mut files = tree
        .tree
        .unpack(&tarballs, root)
        .map_err(DownloadError::Tree)?;
    std::fs::remove_dir_all(&tarballs).map_err(DownloadError::Write)?;
    // Made new, so nothing the tree unpacked to its name is written through,
    // and with the modes the tree's own files and directories are, whatever
    // the umask would allow: Node loads the preload before the agent.
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o644);
    let mut preload = options
        .open(root.join(PRELOAD))
        .map_err(DownloadError::Write)?;
    std::io::Write::write_all(&mut preload, Preload::bytes()).map_err(DownloadError::Write)?;
    let bin = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let bin = {
        let mut bin = bin;
        std::os::unix::fs::DirBuilderExt::mode(&mut bin, 0o700);
        bin
    };
    bin.create(root.join(BIN)).map_err(DownloadError::Write)?;
    files.extend([
        Preload::file(),
        File {
            path: BIN.into(),
            kind: FileKind::Dir,
        },
    ]);
    Ok(files)
}

/// Writes each of `stubs` into the `.bin` of the version staged at `root`,
/// made new, so nothing the tree unpacked is written through, writable by its
/// owner alone and runnable where it is a script `sh` runs, whatever the umask
/// would allow; gives each as the receipt names it.
fn stubbed(stubs: &[Stub], root: &Path) -> Result<Vec<File>, AgentsError> {
    let mut files = Vec::with_capacity(stubs.len());
    for stub in stubs {
        let file = stub.file();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(
            &mut options,
            if matches!(file.kind, FileKind::File { exec: true, .. }) {
                0o755
            } else {
                0o644
            },
        );
        let mut written = options
            .open(root.join(stub.path()))
            .map_err(DownloadError::Write)?;
        std::io::Write::write_all(&mut written, stub.bytes()).map_err(DownloadError::Write)?;
        files.push(file);
    }
    Ok(files)
}

/// The stub in `.bin` of each program `tree` declares, for the version at
/// `root` whose pinned Node is `node`: a `.cmd` file on Windows, and an `sh`
/// script on every other system.
fn stubs(tree: &NpmTree, root: &Path, node: &Path) -> Result<Vec<Stub>, AgentsError> {
    let preload = root.join(PRELOAD);
    let mut stubs = Vec::new();
    for (name, script) in tree.programs() {
        let script = script
            .split('/')
            .fold(root.to_path_buf(), |at, part| at.join(part));
        stubs.push(if cfg!(windows) {
            Stub::windows(name, node, &preload, &script)?
        } else {
            Stub::unix(name, node, &preload, &script)?
        });
    }
    Ok(stubs)
}

/// The directory of the staging a tree's tarballs are downloaded into.
const TARBALLS: &str = "tarballs";

/// The instances a switch wrote onto the new program, and those it left as
/// they are because they named something else by then.
type Written = (Vec<Box<str>>, Vec<Box<str>>);

/// What a switch writes for each of `moving`: its name, the program the
/// question showed it starting, and `program`.
fn moved<'a>(
    moving: &'a [(&ExternalAgent, Version)],
    program: &'a str,
) -> Vec<(&'a str, &'a str, &'a str)> {
    moving
        .iter()
        .map(|(instance, _)| (instance.name(), instance.executable(), program))
        .collect()
}

/// The instances of `moving` a switch wrote, every one but those `left`,
/// beside those left.
fn written(moving: &[(&ExternalAgent, Version)], left: Vec<String>) -> Written {
    let wrote = moving
        .iter()
        .map(|(instance, _)| instance.name())
        .filter(|name| !left.iter().any(|kept| kept == name))
        .map(Into::into)
        .collect();
    (wrote, left.into_iter().map(Into::into).collect())
}

/// What a switch found before it asked: whether the version is already in
/// place, every host its download may contact, the launcher it is
/// smoke-tested with, or why it is not, and for an add the name the instance
/// would be written under.
struct Plan<'a> {
    kept: bool,
    total: u64,
    tree: Option<&'a NpmTree>,
    reached: Vec<String>,
    smoking: Result<&'a Launcher<'a>, String>,
    adding: Option<String>,
}

/// Each file list `offer` keeps as a catalog asset, fetched from the
/// catalog's own host and read in its place, so the rest of a switch reads it
/// as it reads a list given inline. The `AssetRef` is the one the trust state
/// gave, and the list is held by `offer`, for the one call.
async fn fetch_file_lists(source: &Source, offer: &mut Offer) -> Result<(), AgentsError> {
    for artifact in &mut offer.pin.target.artifacts {
        let Files::Asset(asset) = &artifact.files else {
            continue;
        };
        let bytes = catalog_fetch::asset(source, asset)
            .await
            .map_err(AgentsError::FileListUnfetched)?;
        let files = file_list(&offer.entry, &bytes).map_err(AgentsError::FileListUnread)?;
        artifact.files = Files::Listed(files);
    }
    Ok(())
}

/// The tree the lock at `asset` gives, fetched from the catalog's own host
/// and read, with every place its tarballs may be fetched through, on
/// `hosts`, added to `reached`.
async fn fetch_tree(
    source: &Source,
    asset: &AssetRef,
    hosts: &[Box<str>],
    reached: &mut Vec<String>,
) -> Result<NpmTree, AgentsError> {
    let bytes = catalog_fetch::asset(source, asset)
        .await
        .map_err(DownloadError::Lock)?;
    let tree = NpmTree::parse(&bytes).map_err(DownloadError::Tree)?;
    for place in agent_download::tree_reachable(source, hosts, &tree)? {
        if !reached.contains(&place) {
            reached.push(place);
        }
    }
    Ok(tree)
}

/// Every file the target's artifacts list, as they would be committed; a
/// list still kept as a catalog asset gives none until it is fetched.
fn listed(target: &Target) -> Vec<File> {
    target
        .artifacts
        .iter()
        .filter_map(|artifact| match &artifact.files {
            Files::Listed(files) => Some(files.iter().cloned()),
            Files::Asset(_) => None,
        })
        .flatten()
        .collect()
}

/// Refused unless `listed` holds `cmd` as a file that runs.
fn runnable(listed: &[File], cmd: &str) -> Result<(), AgentsError> {
    let runs = listed
        .iter()
        .any(|file| *file.path == *cmd && matches!(file.kind, FileKind::File { exec: true, .. }));
    if runs {
        Ok(())
    } else {
        Err(AgentsError::Program(quoted(cmd).into()))
    }
}

/// Whether `receipt`, of a version already in place, is the release `target`
/// pins: the same artifacts, by size and SHA-256 as pinned, and the same files;
/// beside a `tree`, the files where its lock installs a package are set
/// apart first, since its lock lists none of them, and every other file is
/// held to `listed`.
fn same(receipt: &Receipt, target: &Target, listed: &[File], tree: Option<&NpmTree>) -> bool {
    let pinned = target
        .artifacts
        .iter()
        .map(|artifact| match &artifact.replaced_by {
            Some(asset) => (asset.size, &*asset.sha256),
            None => (artifact.size, &*artifact.sha256),
        });
    let fetched = receipt
        .artifacts
        .iter()
        .map(|fetched| (fetched.size, &*fetched.sha256));
    let by_path = |files: &[File]| {
        let mut files = files.to_vec();
        files.sort_by(|one, other| one.path.cmp(&other.path));
        files
    };
    let installed = |file: &File| {
        tree.is_some_and(|tree| {
            tree.packages().iter().any(|package| {
                file.path
                    .strip_prefix(&*package.location)
                    .is_some_and(|rest| rest.starts_with('/'))
            })
        })
    };
    let own: Vec<File> = receipt
        .files
        .iter()
        .filter(|file| !installed(file))
        .cloned()
        .collect();
    receipt.artifacts.len() == target.artifacts.len()
        && pinned.eq(fetched)
        && by_path(&own) == by_path(listed)
}

/// The sentence saying which instances move onto `version` of `agent`.
fn moves(agent: &AgentKey, version: &str, moving: &[(&ExternalAgent, Version)]) -> String {
    if moving.is_empty() {
        return format!(
            "Moves no configured instance: none runs {agent} from crucible's install store."
        );
    }
    let named: Vec<String> = moving
        .iter()
        .map(|(instance, from)| {
            format!(
                "{} from {}",
                quoted(instance.name()),
                quoted(from.version())
            )
        })
        .collect();
    format!("Moves {} onto {}.", named.join(", "), quoted(version))
}

/// `names` with each kept once, in the order first given.
fn dedup(names: &mut Vec<String>) {
    let mut seen: Vec<String> = Vec::new();
    names.retain(|name| {
        if seen.contains(name) {
            false
        } else {
            seen.push(name.clone());
            true
        }
    });
}

/// Puts `confirmation` through `front` as an install confirmation, and
/// answers with the place in its choices of the one picked, or `None` where
/// nobody picked one: it declined, answered nothing, or was turned away
/// [`TRIES`](super::TRIES) times.
///
/// Put whole or not at all: a sentence about what would be fetched, cut short,
/// is not what the person agreed to.
async fn confirmed(
    request: &Request,
    front: &mut dyn Front,
    confirmation: &Confirmation,
) -> Option<usize> {
    if !request.capabilities().has(Capability::Install) || confirmation.choices.len() > ITEMS {
        return None;
    }
    let asks = Text::cut(&confirmation.asks);
    let facts: Vec<Text> = confirmation
        .facts
        .iter()
        .map(|fact| Text::cut(fact))
        .collect();
    let choices: Vec<Choice> = confirmation
        .choices
        .iter()
        .enumerate()
        .map(|(at, (name, says))| Choice {
            name: Text::cut(name),
            says: Text::cut(says),
            recommended: at == 0,
        })
        .collect();
    let cut = asks.truncated()
        || facts.iter().any(Text::truncated)
        || choices
            .iter()
            .any(|choice| choice.name.truncated() || choice.says.truncated());
    if facts.len() > ITEMS || (cut && !front.draws_whole()) {
        return None;
    }
    let pending = Pending::Install {
        id: mint(),
        asks,
        facts,
        choices,
    };

    let mut tries = 0;
    loop {
        let decision = front.put(&pending, Shown::Install(confirmation)).await?;
        let refusal: Refusal = if decision.id() == pending.id() {
            match decision {
                Decision::Picked { choice, .. } => {
                    match confirmation
                        .choices
                        .iter()
                        .position(|(name, _)| name == choice.as_str())
                    {
                        Some(at) => return Some(at),
                        None => ErrorCode::InvalidArgument.into(),
                    }
                }
                Decision::Declined { .. } => return None,
                Decision::Ruled { .. } | Decision::Answered { .. } | Decision::Accepted { .. } => {
                    ErrorCode::WrongDecision.into()
                }
            }
        } else {
            ErrorCode::StaleDecision.into()
        };
        if turned_away(front, refusal, &mut tries) {
            return None;
        }
    }
}
