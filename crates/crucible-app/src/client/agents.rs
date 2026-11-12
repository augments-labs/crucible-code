//! The agents door: listing, installing, adding, updating, rolling back,
//! removing and verifying the agents the signed catalog and the ACP Registry
//! offer, with no conversation.
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
//! pinned Node starts being refused where Node is given arguments of its own,
//! with more install scripts run at install or built at review than a question
//! shows, or with one run at install where it cannot be: with the sandbox off,
//! anywhere but Linux, or with no launcher lent, as from a command line; its
//! download hosts,
//! and that every address and mirror it is pinned at is on one of them; for a
//! release a pinned Node starts, its package lock, fetched from the catalog's
//! own host by [`catalog_fetch::asset`] and read, and every tarball it pins on
//! a host the target lists, so the question names the package count, how
//! many packages are under each licence the lock gives, each quoted, where
//! any package gives one, at most eight and then how many more, every install script and what the reviewer decided of
//! it, at most sixteen left unrun and then how many more and each one run at
//! install, and the hosts the registry is reached through; its
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
//! A release the ACP Registry offers is asked about as a registry row, and
//! its question says that crucible will download the vendor's archive, survey
//! it and run the vendor's program once for a probe after the yes; a failed
//! probe installs nothing. Its one archive is downloaded within the registry's
//! cap, surveyed from the file it landed in, and the probe is taken before
//! anything is committed. Its receipt records the source and the probe record
//! `Door::switch` took, with the platform the probe was for, what the program
//! answered and the sizes the survey found; `verify` hashes its files and names
//! a `source` that is not the registry, or a `probe` that is missing or was
//! taken for another machine.
//!
//! After a yes, the release the question named is the one installed: each
//! artifact is downloaded within its pin by [`crate::agent_download`] into a
//! file beside the staging directory, unpacked from it into that directory
//! against its file list, a wheel of a release a pinned interpreter starts
//! with its files under `site/`, and committed to the store with its receipt.
//! A registry artifact has no exact size and no file list: it is downloaded
//! by [`agent_download::capped`] and committed from the list its survey wrote.
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
//! A script built at review runs nothing here, on any platform and with no
//! launcher or sandbox: its asset is fetched from the catalog's own host by
//! the same download and held to its pin, its package's directory removed
//! and made new, so it ends as its review listed it and not as its tarball
//! left it, and the asset unpacked there with [`Unpacker::unpack_plain_from`],
//! its files named in the receipt in place of what lay there, so `crucible
//! agents verify` covers them. A list that leaves out a program its package
//! declares is refused before the question.
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
//! numbers included, is asked about as it is without the setting. So is a
//! release with an install script to run at install, whatever its version:
//! its command and hosts are what that yes is to, and the setting does not
//! stand in for reading them. A built script runs no code, so it is an
//! artifact like any other, and the setting stands for it. Every other step runs and refuses as it does
//! after a yes, and the outcome says the update was taken under that setting
//! and why it was not smoke-tested where it was not. A first install, an add, a rollback and a removal are always
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
//! [`Watching::link`] as the agent's wire passed it. The user's own program
//! is run by no sign-in command at all ([`Unpiped::OwnProgram`]), refused
//! before anything is asked or started with its entry's description, since
//! that command is its own and not a program of a version crucible installed.
//! A method the agent runs as a command with no real terminal is started only where the front end lent
//! [`Watching::sign_in`], since someone must read what it says and answer it:
//! with nothing lent it is refused saying a terminal is needed, before the
//! route is asked or anything started. A method that needs a real terminal is
//! started only where the front end lent [`Watching::terminal`], and it is
//! asked about on every run because it runs unconfined; it is refused where a
//! project requires the sandbox or the platform has no terminal, before
//! anything is asked or started. Once the route has its yes,
//! and beside no run as above, a command that is no program of the version
//! installed is refused ([`Unpiped::Outside`]) before anything starts; the agent
//! is then read over its own wire, and what it says of the method now must be what its signed entry records
//! ([`Unpiped::Disagrees`]). What the command says reaches the channel redacted
//! and bounded by line and in total, a cut said where it fell; each line the
//! channel gives goes to its input, and a [`Typing::Secret`] is withheld from
//! all it is seen to say and carried nowhere else. It ending well is not taken
//! as signed in: the agent is then read as for any other sign-in. A business
//! sign-in to Antigravity is billed to the Google Cloud project and location
//! its entry names: one whose entry names neither is refused naming both keys,
//! and one whose location's host no sandbox policy can name is refused naming
//! that key, each before anything is asked, written or started; otherwise the
//! two are written in the agent's profile once its route has its yes, just
//! before it starts. A sign-in on an instance that names a `profileDir` its
//! agent's entry has no variable for is refused as a start of it is, naming the
//! instance, and nothing is made or started, since the agent would never read
//! that directory. Where the sandbox is on, the agent is let write the places
//! its entry says it keeps its sign-in in, so that what it signs in stays; a
//! listing says of an agent whose entry names neither such a place nor a
//! variable that a sign-in made under the sandbox is not kept
//! ([`Listed::keeps_no_sign_in`]).
//!
//! A try of the user's own program of an agent, a catalog agent's or
//! Antigravity's, starts it, so it is refused as no conversation where no
//! steps were lent ([`AgentsError::Untried`]). The door finds the program again
//! as a launch of the instance that runs it, or of a new one where none of the
//! agent is configured, would, as `/login` offers it, and refuses where that
//! is not the program the move named, where an instance of the agent runs
//! another, or where its directory is one an agent may not be given; it then
//! asks its own yes, with what the program is, what it is given and what it
//! can read, and starts nothing on a no. A catalog agent's answer is kept, and a new
//! instance is written under a name of its own only once it was admitted: a
//! refusal, or a write that fails, keeps nothing and writes nothing.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

pub use crate::launching::{HandoverFailed, SignInTerminal};
use crucible_auth::Store as Credentials;
use crucible_client_api::bounds::ITEMS;
use crucible_client_api::{
    Capabilities, Capability, Choice, Command, Correlation, Decision, ErrorCode, InstallProgress,
    Name, Pending, Refusal, Request, Text,
};
use crucible_config::{AgentUpdate, Cloud, ConfigError, Driver, ExternalAgent, Home, Settings};
/// A line a sign-in command said, as a channel is given it.
pub use crucible_execution::Text as SaidLine;
use crucible_execution::{Authentication, Failure};
use crucible_external_harnesses::catalog::registry::{self, Unoffered};
use crucible_external_harnesses::catalog::{
    Artifact, AssetRef, AuthMethod, CatalogError, Disposition, Entry, File, FileKind, Files,
    Format, Launch, Method, Offer, Route, Script, Target, Trust, Unprofiled, built_list, file_list,
    result_list,
};
use crucible_external_harnesses::release::{
    BIN, LAUNCHER, LaunchScript, NpmTree, PRELOAD, Preload, RELEASE_BYTES, Stub, StubError,
    UnpackError, Unpacker, Wheels,
};
use crucible_external_harnesses::store::{
    Fetched, Listing, Mismatch, Probe, Provenance, Receipt, Source as Released, Store, StoreError,
    Version,
};
use crucible_external_harnesses::{Account, Held, Reading, WITHHELD_BYTES, antigravity};
pub use crucible_external_harnesses::{SignInChannel, Typed, Typing};
use crucible_http::{Origin, ProxyEnv};
use crucible_runtime::Cancel;
use crucible_sandbox::SandboxService;
use crucible_sandbox_local::LocalSandbox;
use crucible_types::AgentKey;
use crucible_workspace::Workspace;
use tokio::runtime::Handle;

use super::accounts;
use super::deciding::{self, Confirmation, Front, Shown, mint, turned_away};
use crate::agent_download::{self, DownloadError, Landing};
use crate::backends::{
    AgentRow, CatalogRow, REGISTRY, Sandboxing, Started, Unbelieved, believed_registry,
    believed_registry_row, believed_row, kept_word,
};
use crate::catalog_fetch::{self, FetchError, RegistryRead, RegistrySource, Source, Trigger};
use crate::content_use::{self, Consent, agent_route};
use crate::launching::{
    self, Agent, Launcher, Steps, Unlaunched, Unpiped, Unscripted, Unsmoked, quoted,
};
use crate::own_program::Unowned;
use crate::providers::{Rows, Unnamed};
use crate::remember::{self, RememberError};
use crate::services::Services;
pub(super) use trying::own_of;
use trying::{own_antigravity, own_refusal};

mod trying;

/// The most catalog agents a listing names, offered or listed only as the
/// user's own program; one with more says so.
pub const LISTED: usize = 32;

/// The most bytes a key typed for an agent may have.
pub const KEY_BYTES: usize = 4096;

/// What the door is lent by the front end that asks it.
#[derive(Debug)]
pub struct Shelf<'a> {
    /// Where the catalog is published, who signed it and which hosts a
    /// redirect from it may reach.
    pub catalog: Source,
    /// Where the ACP Registry is read from, and which hosts a redirect from it
    /// may reach; none for a front end that lists no registry agent.
    pub registry: Option<RegistrySource>,
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
    /// Where a sign-in the agent runs as a command is carried, where this front
    /// end has someone to carry it to; a method of that kind is refused without
    /// one.
    pub sign_in: Option<&'a dyn SignInChannel>,
    /// This front end's terminal, where it can lend one to a sign-in the agent
    /// runs as a command that needs a real one. A method of that kind is
    /// refused without one.
    pub terminal: Option<&'a dyn SignInTerminal>,
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
    /// Why crucible installs no version of it, where its entry has a target
    /// for this platform that crucible does not install, or is published only
    /// as a package: such a row is listed only where the user's own program
    /// of it was found, offers no version, and is reached only as that
    /// program.
    pub unoffered: Option<Box<str>>,
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
    /// Whether the row is the ACP Registry's, whose reach and sign-in hosts
    /// are not recorded.
    pub registry: bool,
    /// The probe record the newest offered version was installed under, where
    /// it is a registry release installed under one.
    pub probed: Option<Probe>,
    /// The path the user's own program of it leads to, where the door was
    /// lent a conversation's steps and found one on their `PATH` as a try of
    /// it finds one: the program a configured instance of it runs, else the
    /// one its entry's guessed name finds. Nothing is started to find it.
    pub own: Option<PathBuf>,
}

/// What a list of only the agents crucible installs says beside the rows it
/// keeps, which leaves out each agent listed only as the user's own program.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Installable {
    /// Whether an agent crucible installs a version of was past the most a
    /// listing names.
    pub more: bool,
    /// Whether the registry line stands beside those rows alone: one of them
    /// is the ACP Registry's, or its index could not be read or verified.
    pub registry: bool,
}

/// The user's own Antigravity, found on the `PATH` a conversation's steps
/// look on, as a listing names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnAntigravity {
    /// The key a try of it names it by.
    pub agent: AgentKey,
    /// The path its links lead to.
    pub program: PathBuf,
    /// What it sees of the sign-in it keeps, as it would be launched now.
    pub note: &'static str,
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
        /// What a list of only the agents crucible installs says of these.
        installable: Installable,
        /// Why the catalog was not refreshed first, where it was not: what is
        /// named is what was believed before.
        unrefreshed: Option<FetchError>,
        /// The line a listing shows beside the agents it lists for the ACP
        /// Registry: the day the index was read and that it is not reviewed,
        /// why it was the cached one where it was not verified, or why none
        /// could be read.
        registry: Option<Box<str>>,
        /// The user's own Antigravity, where the door was lent a
        /// conversation's steps and found one.
        antigravity: Option<OwnAntigravity>,
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
        /// The probe record a registry release was installed under, where one
        /// was taken.
        probe: Option<Probe>,
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
    /// The user's own program of the agent answered as one in a sandbox that
    /// closed the network, and an instance runs it: one written for it, or
    /// the one that already ran it.
    Tried {
        /// The agent.
        agent: AgentKey,
        /// The instance that runs it.
        instance: Box<str>,
        /// The canonical path of the program.
        program: PathBuf,
    },
    /// The installed versions, each hashed against its receipt, with a
    /// registry receipt's source and probe record checked too.
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
    /// A command the agent's entry records, run with the person at the channel
    /// lent for it.
    Command,
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
    /// The user's own program of the agent was not taken for it, and nothing
    /// was written.
    #[error("{}", own_refusal(.agent, .program, .why))]
    Own {
        /// The agent.
        agent: AgentKey,
        /// The program, as it was asked to be tried.
        program: PathBuf,
        /// Why not.
        why: Box<Unowned>,
    },
    /// No signed catalog offers it, and the ACP Registry was not read or was
    /// not verified: the line says which.
    #[error("no agent catalog offers {agent} {version}; {why}")]
    UnreadInstall {
        /// The agent.
        agent: AgentKey,
        /// The version asked for.
        version: Box<str>,
        /// The registry's line.
        why: Box<str>,
    },
    /// No signed catalog offers a release of it, and the ACP Registry was not
    /// read or was not verified: the line says which.
    #[error("no agent catalog offers a release of {agent}; {why}")]
    UnreadUpdate {
        /// The agent.
        agent: AgentKey,
        /// The registry's line.
        why: Box<str>,
    },
    /// The ACP Registry lists the agent but offers no target this build can
    /// install.
    #[error("the ACP Registry does not offer {agent}: {why}")]
    Unregistered {
        /// The agent.
        agent: AgentKey,
        /// Why it is not offered.
        why: Unoffered,
    },
    /// The ACP Registry's entry could not be read as a catalog entry.
    #[error("{agent}'s ACP Registry entry cannot be read: {why}")]
    Unreadable {
        /// The agent.
        agent: AgentKey,
        /// Why not.
        why: CatalogError,
    },
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
    /// A sign-in method run as a command was asked for by a front end with no
    /// one to carry it to.
    #[error(
        "the sign-in method {0} runs a command that needs a terminal: run `crucible agents \
         sign-in` from one"
    )]
    Unlent(Box<str>),
    /// A sign-in method that needs a real terminal runs the agent's command
    /// outside the sandbox, and a sandbox is required here.
    #[error(
        "the sign-in method {0} needs a real terminal, and a sign-in at one runs the agent's \
         command outside the sandbox; a sandbox is required here"
    )]
    Unconfined(Box<str>),
    /// A sign-in run as a command was not run, or did not end signed in.
    #[error(transparent)]
    Piped(#[from] Unpiped),
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
    /// A registry archive surveys to more files than a catalog row can carry.
    #[error(
        "the registry archive of {agent} {version} lists {files} files, more than the {bound} a \
         catalog row carries, so nothing was installed"
    )]
    TooManyFiles {
        /// The agent.
        agent: AgentKey,
        /// The version.
        version: Box<str>,
        /// How many files the survey wrote.
        files: usize,
        /// The most a catalog row carries.
        bound: usize,
    },
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
    /// The vendor's program did not answer the install-time probe, so nothing
    /// was put in place.
    #[error(
        "{agent} {version} did not answer the install-time probe, so nothing was installed: \
         {why}"
    )]
    Probe {
        /// The agent.
        agent: AgentKey,
        /// The version.
        version: Box<str>,
        /// Why the probe answered nothing it could record.
        why: Box<crate::probing::Unprobed>,
    },
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
    /// An install script the release runs at install did not run as its
    /// review found it, so nothing was put in place.
    #[error("{agent} {version} was not installed: its install script {why}")]
    Script {
        /// The agent.
        agent: AgentKey,
        /// The version refused.
        version: Box<str>,
        /// Why.
        why: Unscripted,
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
    /// Trying the user's own program of the agent starts it, and no
    /// conversation lent the steps that start it.
    #[error(
        "trying your own {0} starts it, which needs a conversation to start it in; \
         nothing was started"
    )]
    Untried(AgentKey),
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
        // A listing names no agent, a sign-in names an instance instead, and a
        // try is told its agent below.
        Command::AgentCatalog | Command::SignInAgent { .. } | Command::TryOwnAgent { .. } => None,
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
    let (door, unrefreshed) = opened(request, shelf).await?;
    if let Command::SignInAgent { instance, method } = command {
        return door
            .sign_in(instance.as_str(), method.as_str(), front, &watching)
            .await;
    }
    if let Command::TryOwnAgent { agent, program } = command {
        return door.try_own(agent.as_str(), program, front).await;
    }
    let Some(agent) = agent else {
        return Ok(door.listing(unrefreshed));
    };
    match command {
        Command::InstallAgent { version, .. } | Command::AddAgent { version, .. } => {
            let offer = door
                .offers(&agent)?
                .into_iter()
                .find(|offer| *offer.pin.version == *version.as_str());
            let Some(offer) = offer else {
                return Err(match door.unread_line() {
                    Some(why) => AgentsError::UnreadInstall {
                        agent,
                        version: version.as_str().into(),
                        why,
                    },
                    None => AgentsError::Unoffered {
                        agent,
                        version: version.as_str().into(),
                    },
                });
            };
            let how = if matches!(command, Command::AddAgent { .. }) {
                How::Added
            } else {
                How::Installed
            };
            let source = door.released(&agent);
            let fact = door.registry_fact();
            door.switch(offer, how, source, fact.as_ref(), front, watching)
                .await
        }
        Command::UpdateAgent { .. } => {
            let mut offers = door.offers(&agent)?;
            if offers.is_empty() {
                return Err(match door.unread_line() {
                    Some(why) => AgentsError::UnreadUpdate { agent, why },
                    None => AgentsError::Unlisted(agent),
                });
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
            let source = door.released(&agent);
            let fact = door.registry_fact();
            door.switch(offer, How::Updated, source, fact.as_ref(), front, watching)
                .await
        }
        Command::RollbackAgent { .. } => door.rollback(&agent, front).await,
        Command::RemoveAgent { .. } => door.remove(&agent, front).await,
        Command::VerifyAgent { .. } => door.verify(agent),
        _ => Ok(Outcome::Refused(ErrorCode::Busy.into())),
    }
}

/// The door `request` is carried out with, lent `shelf`, once the catalog
/// was fetched as a person asking for it and, for a command that offers,
/// installs, updates or rolls back an agent, the registry read; and why the
/// catalog was not refreshed, where it was not.
///
/// # Errors
///
/// [`AgentsError::Config`] where the configuration cannot be read.
async fn opened<'a>(
    request: &'a Request,
    shelf: &'a Shelf<'a>,
) -> Result<(Door<'a>, Option<FetchError>), AgentsError> {
    let state = catalog_fetch::state_in(shelf.home.path());
    let unrefreshed = catalog_fetch::refresh(&state, &shelf.catalog, Trigger::Opening)
        .await
        .err();
    // Only a command that offers, installs, updates or rolls back an agent
    // reads the registry; the cache is reused for its five minutes, and a
    // failure is carried as the line the listing shows.
    let (registry, registry_notice) = if needs_registry(request.command()) {
        match &shelf.registry {
            Some(source) => {
                match catalog_fetch::registry(shelf.home.path(), source, SystemTime::now()).await {
                    Ok(view) => {
                        let notice = view.unverified.clone();
                        (Some(view), notice)
                    }
                    Err(why) => (None, Some(why.to_string().into())),
                }
            }
            None => (None, None),
        }
    } else {
        (None, None)
    };
    let settings = Settings::read_home(shelf.home)?;
    let door = Door {
        request,
        shelf,
        trust: Trust::load(&state, SystemTime::now()),
        store: Store::new(shelf.home.path().join("programs")),
        registry,
        registry_notice,
        user: crucible_config::user(shelf.home),
        instances: settings.external_agents(),
        // The user's own file alone: a checkout cannot ask for this.
        standing: settings.agent_update() == AgentUpdate::Reviewed,
    };
    Ok((door, unrefreshed))
}

/// What the door finds of the user's own program of one agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnProgram {
    /// The path the program's links lead to, as a try of it takes it.
    Found(PathBuf),
    /// The user's own program of the agent is found, leading to `program`,
    /// but the instance `instance` of the agent is configured and runs
    /// another program, so a try of it is refused as [`refused_beside`] says:
    /// `/login` offers no second instance beside one configured.
    Beside {
        /// The path the program's links lead to.
        program: PathBuf,
        /// The configured instance's name, as the configuration spells it.
        instance: Box<str>,
    },
    /// The agent is offered, or is Antigravity, but no program of the user's
    /// own is found for it.
    Unfound,
    /// Neither the signed catalog nor the ACP Registry offers an agent of
    /// that name.
    Unlisted,
}

/// The door's refusal of a try of the user's own `program` of `agent`, which
/// writes nothing, where the instance `instance` of the agent is configured
/// and runs another program.
pub fn refused_beside(agent: AgentKey, program: PathBuf, instance: &str) -> AgentsError {
    AgentsError::Own {
        agent,
        program,
        why: Box::new(Unowned::Beside(quoted(instance).into())),
    }
}

/// The user's own program of `agent` the door, lent `shelf`, would try: found
/// for that one agent as a listing finds it for each row it shows, after the
/// same fetch of the catalog and read of the registry, so an agent past what a
/// listing shows is found as one within it. Nothing is started or written.
///
/// # Errors
///
/// [`AgentsError::Untried`] where `shelf` lent no steps to start a program
/// with, and [`AgentsError::Config`] where the configuration cannot be read.
pub async fn own_program(shelf: &Shelf<'_>, agent: &str) -> Result<OwnProgram, AgentsError> {
    let Ok(key) = AgentKey::new(agent) else {
        return Ok(OwnProgram::Unlisted);
    };
    let steps = shelf
        .steps
        .ok_or_else(|| AgentsError::Untried(key.clone()))?;
    let request = Request::new(
        Capabilities::none().with(Capability::Install),
        Correlation::new(1),
        Command::AgentCatalog,
    );
    let (door, _) = opened(&request, shelf).await?;
    Ok(door.own_program(&steps.trying(), &key))
}

/// Whether `command` may offer, install, update or roll back an agent the ACP
/// Registry lists, so the registry is read before it is carried out.
const fn needs_registry(command: &Command) -> bool {
    matches!(
        command,
        Command::AgentCatalog
            | Command::InstallAgent { .. }
            | Command::AddAgent { .. }
            | Command::UpdateAgent { .. }
            | Command::RollbackAgent { .. }
    )
}

/// Whether the user's standing yes can stand in for the update question:
/// only an update of a signed catalog release can, since a registry release
/// has no review for the policy to stand on.
const fn reviewed(standing: bool, how: How, source: Released) -> bool {
    standing && matches!(how, How::Updated) && matches!(source, Released::Catalog)
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

/// The source fact a registry question says: the index's read date, and why it
/// was the cached one where it was not verified.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RegistryFact<'a> {
    read: String,
    unverified: Option<&'a str>,
}

/// What one command is carried out with.
struct Door<'a> {
    request: &'a Request,
    shelf: &'a Shelf<'a>,
    trust: Trust,
    store: Store,
    /// The ACP Registry index this run read, where its front end lent a
    /// source; none for a front end that lists no registry agent.
    registry: Option<RegistryRead>,
    /// Why the registry index was the cached one, or why none could be read.
    registry_notice: Option<Box<str>>,
    user: PathBuf,
    instances: Vec<ExternalAgent>,
    /// Whether the user's own file says `agents.update = "reviewed"`, which
    /// stands in for the yes to an update of a release the catalog offers.
    /// Every check that can refuse the release still runs.
    standing: bool,
}

/// Refuses `method`'s command for the user's own program, which runs its
/// sign-in command itself in a terminal: none is run for it, since it is no
/// program of a version crucible installed.
fn unrun_for_own(row: &AgentRow, method: &AuthMethod) -> Result<(), AgentsError> {
    match (row, &method.method) {
        (AgentRow::Catalog(catalog), Method::Terminal { .. }) if catalog.is_own() => {
            Err(AgentsError::Piped(Unpiped::own_program(method)))
        }
        _ => Ok(()),
    }
}

impl Door<'_> {
    /// The offer the ACP Registry makes for `agent` on this platform, where it
    /// lists the agent at all: nothing where it does not, and why where it
    /// lists it but cannot offer it.
    fn registry_offer(&self, agent: &AgentKey) -> Result<Option<Offer>, AgentsError> {
        // A key a built-in driver or the native runner is spelled with is no
        // registry row: the built-in owns that route.
        if kept_word(agent.as_str()) {
            return Ok(None);
        }
        let Some(view) = &self.registry else {
            return Ok(None);
        };
        if view.index.entry(agent).is_none() {
            return Ok(None);
        }
        let offered = view
            .index
            .offer(agent)
            .map_err(|why| AgentsError::Unregistered {
                agent: agent.clone(),
                why,
            })?;
        let offer = offered.offer().map_err(|why| AgentsError::Unreadable {
            agent: agent.clone(),
            why,
        })?;
        Ok(Some(offer))
    }

    /// Every release this build offers for `agent`: the signed catalog's where
    /// it lists the agent, else the ACP Registry's. A signed or compiled
    /// entry is never shadowed by an unsigned one.
    fn offers(&self, agent: &AgentKey) -> Result<Vec<Offer>, AgentsError> {
        let signed = self.trust.offers(Some(agent));
        if !signed.is_empty() {
            return Ok(signed);
        }
        match self.registry_offer(agent)? {
            Some(offer) => Ok(vec![offer]),
            None => Ok(Vec::new()),
        }
    }

    /// Where a release of `agent` read from this door comes from.
    fn released(&self, agent: &AgentKey) -> Released {
        if !self.trust.offers(Some(agent)).is_empty() {
            return Released::Catalog;
        }
        match self.registry_offer(agent) {
            Ok(Some(_)) => Released::Registry,
            _ => Released::Catalog,
        }
    }

    /// The source fact a registry question says, where this run read an index.
    fn registry_fact(&self) -> Option<RegistryFact<'_>> {
        self.registry.as_ref().map(|view| RegistryFact {
            read: view.index.read_date(),
            unverified: self.registry_notice.as_deref(),
        })
    }

    /// Why no registry offer was read, with the cached index's read day where
    /// one was read: a stale cache is told from a fresh one in the refusal.
    fn unread_line(&self) -> Option<Box<str>> {
        self.registry_notice
            .as_ref()
            .map(|why| match &self.registry {
                Some(view) => format!(
                    "{why}; the cached index was read {}",
                    view.index.read_date()
                )
                .into(),
                None => why.clone(),
            })
    }

    /// The agents offered, but those whose key is a word a built-in driver or
    /// the native runner is spelled with: the entry that backs a built-in is
    /// that driver's, and is no catalog row.
    fn listing(&self, unrefreshed: Option<FetchError>) -> Outcome {
        let trying = self.shelf.steps.map(Steps::trying);
        let mut agents: Vec<Listed> = Vec::new();
        let mut more = false;
        let mut installable = Installable::default();
        for offer in self.trust.offers(None) {
            let key = &offer.entry.key;
            if kept_word(key.as_str()) {
                continue;
            }
            if let Some(listed) = agents.iter_mut().find(|listed| listed.agent == *key) {
                listed.offered.push(offer.pin.version);
                continue;
            }
            if agents.len() == LISTED {
                more = true;
                installable.more = true;
                continue;
            }
            // One agent's store that cannot be read is said on its row, and
            // the rest are listed as they are.
            let installed = self.installed(key);
            let keeps_no_sign_in = launching::keeps_no_sign_in(&offer.entry);
            agents.push(Listed {
                agent: key.clone(),
                offered: vec![offer.pin.version],
                unoffered: None,
                license: offer.entry.license,
                terms: offer.entry.terms,
                caution: offer.entry.caution,
                installed,
                keeps_no_sign_in,
                registry: false,
                probed: None,
                own: trying.as_ref().and_then(|trying| self.own(trying, key)),
            });
        }
        // The ACP Registry's agents, after what the signed catalog offers: a
        // key it lists once is not listed twice, and a key a built-in driver
        // is spelled with gets no registry row.
        let mut from_registry = false;
        let mut offered_from_registry = false;
        if let Some(view) = &self.registry {
            for (entry, offer) in view.index.listed() {
                let key = &entry.id;
                if kept_word(key.as_str()) || agents.iter().any(|listed| listed.agent == *key) {
                    continue;
                }
                if agents.len() == LISTED {
                    // Past the most a listing names, an entry crucible
                    // installs no version of is not looked for on `PATH`, and
                    // is said to be one more, since it may be listed; it is
                    // never one more to a list of what crucible installs.
                    more = true;
                    installable.more |= offer.is_ok();
                    continue;
                }
                let offer = match offer {
                    Ok(offer) => offer,
                    // An entry crucible installs no version of is listed only
                    // as the user's own program, where one is found.
                    Err(why) => {
                        let Some(own) = trying.as_ref().and_then(|trying| self.own(trying, key))
                        else {
                            continue;
                        };
                        from_registry = true;
                        agents.push(Listed {
                            agent: key.clone(),
                            offered: Vec::new(),
                            unoffered: Some(why.to_string().into()),
                            license: entry.license.clone(),
                            terms: entry.terms.clone(),
                            caution: None,
                            installed: self.installed(key),
                            keeps_no_sign_in: true,
                            registry: true,
                            probed: None,
                            own: Some(own),
                        });
                        continue;
                    }
                };
                let probed = self.probed(key, offer.version());
                from_registry = true;
                offered_from_registry = true;
                agents.push(Listed {
                    agent: key.clone(),
                    offered: vec![offer.version().into()],
                    unoffered: None,
                    license: offer.entry().license.clone(),
                    terms: offer.entry().terms.clone(),
                    caution: None,
                    installed: self.installed(key),
                    // The registry gives an agent no variable for its home and
                    // no place it keeps its sign-in in.
                    keeps_no_sign_in: true,
                    registry: true,
                    probed,
                    own: trying.as_ref().and_then(|trying| self.own(trying, key)),
                });
            }
        }
        // A failed read is said even where it lists no agent; an index that
        // read but listed none keeps the signed screen as it was, and so does
        // a list of only what crucible installs where none of those is the
        // registry's.
        let registry = (from_registry || self.registry_notice.is_some())
            .then(|| self.registry_line())
            .flatten();
        installable.registry = offered_from_registry || self.registry_notice.is_some();
        Outcome::Catalog {
            agents,
            more,
            installable,
            unrefreshed,
            registry,
            antigravity: trying
                .as_ref()
                .and_then(|trying| own_antigravity(trying, &self.instances)),
        }
    }

    /// The line a listing shows beside what it lists of the ACP Registry: the
    /// day the index was read and that it is not reviewed by Crucible, and why
    /// it was the cached one where it was not verified; or why none could be
    /// read.
    fn registry_line(&self) -> Option<Box<str>> {
        match (&self.registry, &self.registry_notice) {
            (Some(view), Some(why)) => Some(
                format!(
                    "Source: ACP Registry, read {}; {why}; not reviewed by Crucible.",
                    view.index.read_date()
                )
                .into(),
            ),
            (Some(view), None) => Some(
                format!(
                    "Source: ACP Registry, read {}; not reviewed by Crucible.",
                    view.index.read_date()
                )
                .into(),
            ),
            (None, Some(why)) => Some(why.clone()),
            (None, None) => None,
        }
    }

    /// The probe record the version `version` of `agent` was installed under,
    /// where the store holds one and it was a registry release.
    fn probed(&self, agent: &AgentKey, version: &str) -> Option<Probe> {
        let version = Version::new(version, None).ok()?;
        let receipt = self.store.receipt(agent, &version).ok().flatten()?;
        (receipt.source == Released::Registry
            && receipt
                .unprobed(&crucible_external_harnesses::catalog::registry::platform())
                .is_none())
        .then_some(receipt.probe)
        .flatten()
        .map(|probe| *probe)
    }

    /// The versions of `agent` the store holds with a receipt, or why its
    /// directory in the store could not be read.
    fn installed(&self, agent: &AgentKey) -> Result<Vec<Box<str>>, Box<str>> {
        self.store
            .installed(agent)
            .map(|receipts| {
                receipts
                    .into_iter()
                    .map(|receipt| receipt.version)
                    .collect()
            })
            .map_err(|unread| unread.to_string().into())
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
    /// A version number that is not plain dotted numbers is never newer, and
    /// a release with an install script to run at install never stands.
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
        if let Launch::Npm { scripts, .. } = &offer.pin.target.launch
            && scripts
                .iter()
                .any(|script| matches!(script.disposition, Disposition::Local { .. }))
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

    /// The launcher a switch onto `offer` is smoke-tested with, or why it is
    /// not smoke-tested.
    fn smoking(&self, offer: &Offer, row: Option<&CatalogRow>) -> Result<&Launcher<'_>, String> {
        let agent = &offer.entry.key;
        let launcher = self
            .shelf
            .smoke
            .ok_or_else(|| "no conversation lent a launcher to start it with".to_owned())?;
        if let Some(route) = borrowed(launcher.consent(), offer, row) {
            return Err(Unsmoked::Borrowed(route).to_string());
        }
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

    /// The launcher an install script of `offer` is run through, and the hold
    /// of its agent's route, asked for through `front` where it has no yes
    /// yet; `Err(())` where that question was answered no.
    async fn scripting(
        &self,
        offer: &Offer,
        front: &mut dyn Front,
    ) -> Result<Result<(&Launcher<'_>, Held), ()>, AgentsError> {
        let agent = &offer.entry.key;
        // Refused before anything was fetched where none is lent.
        let launcher = self.shelf.smoke.ok_or(AgentsError::Unsupported(UNLENT))?;
        let route = agent_route(agent.as_str());
        let row = CatalogRow::new(&offer.entry, &offer.pin.target).ok();
        if let Some(route) = borrowed(launcher.consent(), offer, row.as_ref()) {
            return Err(AgentsError::Script {
                agent: agent.clone(),
                version: quoted(&offer.pin.version).into(),
                why: Unscripted::Borrowed(route),
            });
        }
        if let Some(warned) = launcher.consent().asks(&route) {
            if !deciding::warned(self.request.capabilities(), front, &warned).await {
                return Ok(Err(()));
            }
            launcher
                .consent()
                .accept(&warned)
                .map_err(AgentsError::HoldUnkept)?;
        }
        let held =
            content_use::held(launcher.consent(), &route).map_err(|_| AgentsError::Script {
                agent: agent.clone(),
                version: quoted(&offer.pin.version).into(),
                why: Unscripted::Unheld(route.clone()),
            })?;
        Ok(Ok((launcher, held)))
    }

    /// Installs `offer` of `agent` after a yes, and moves every instance the
    /// store runs it for onto it.
    // The install reads as one sequence from the question through the commit;
    // its inputs are the offer, the move and the four values the door lent,
    // and splitting it would put the order it must keep in more than one place.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    async fn switch(
        &self,
        mut offer: Offer,
        how: How,
        source: Released,
        registry: Option<&RegistryFact<'_>>,
        front: &mut dyn Front,
        watching: Watching<'_>,
    ) -> Result<Outcome, AgentsError> {
        // A release a pinned interpreter starts is started through a launcher
        // written for its entry point, which is held before anything is asked;
        // one a pinned Node starts, under the preload alone, and its lock is
        // fetched below.
        let (script, lock) = launched(&offer, self.shelf.smoke)?;
        let mut reached: Vec<String> = Vec::new();
        let hosts = &offer.pin.target.hosts;
        for artifact in &offer.pin.target.artifacts {
            reach(&self.shelf.catalog, hosts, artifact, &mut reached)?;
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
        // What each install script run at install must leave, fetched from
        // the catalog's own host and read before anything is asked.
        let locals = locals(&self.shelf.catalog, offer, tree.as_ref()).await?;
        // What each built script's asset unpacks to, the same way, and where
        // that asset is fetched from for the question to name.
        let builts = builts(&self.shelf.catalog, offer, tree.as_ref()).await?;
        for built in &builts {
            reach(
                &self.shelf.catalog,
                &target.hosts,
                &built.artifact,
                &mut reached,
            )?;
        }
        let mut listed = listed(target);
        // A registry offer's file list is the survey, which has not been read
        // before the download; the program it starts is checked then.
        if source == Released::Catalog {
            runnable(&listed, &target.cmd)?;
        }
        let (total, fitted) = sized(offer, tree.as_ref(), &builts)?;
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
        for built in &builts {
            listed.extend(under(built.package, built.files()));
        }
        // A registry offer's provisional target reserves the archive's cap
        // rather than the bytes that arrived, so `same` cannot hold it. Its
        // receipt is what says the version is kept: the registry source, the
        // archive's SHA-256 pin, and a probe for this platform. Its surveyed
        // files and probe record are reused, so a second add downloads and
        // probes nothing.
        let mut registered: Option<Probe> = None;
        let kept = match self.store.receipt(agent, &version)? {
            Some(receipt) if source == Released::Registry && registry_kept(&receipt, target) => {
                registered = receipt.probe.as_deref().cloned();
                true
            }
            Some(receipt)
                if source == Released::Catalog
                    && same(&receipt, target, &listed, tree.as_ref(), &builts) =>
            {
                true
            }
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
            smoking: self.smoking(
                offer,
                CatalogRow::new(&offer.entry, &offer.pin.target)
                    .ok()
                    .as_ref(),
            ),
            // The name the question shows; the one written is chosen again
            // from the file under its lock.
            adding: (how == How::Added).then(|| {
                crucible_config::free_name(agent, self.instances.iter().map(ExternalAgent::name))
            }),
        };
        // Only an update can stand on the user's file; any other move is asked.
        // A registry release has no review, so the policy that stands in for
        // the update question does not apply to it: only a signed catalog
        // release can be taken without asking.
        let standing =
            reviewed(self.standing, how, source) && self.stands_for(agent, offer, &moving);
        if !standing {
            let confirmation = Self::installing(offer, how, &plan, &moving, source, registry);
            if confirmed(self.request, front, &confirmation).await != Some(0) {
                return Ok(Outcome::Undecided);
            }
        }
        // An install script reaches what its agent's route answers for, so
        // none is run before that route has its yes; a version kept runs none.
        let scripting = if kept || locals.is_empty() {
            None
        } else {
            Some(self.scripting(offer, front).await?)
        };
        let Ok(scripting) = scripting.transpose() else {
            return Ok(Outcome::Undecided);
        };

        // A registry offer's file list is the survey, which is taken after the
        // yes: the one archive is downloaded within the registry's cap, its
        // file list is what the survey wrote, and the vendor's program is run
        // once in the closed sandbox for the receipt's probe record. A failed
        // probe installs nothing, so neither a provisional offer nor an
        // unprobed receipt is ever committed.
        if !kept && source == Released::Registry {
            let staging = self.store.stage(agent)?;
            let Some(artifact) = target.artifacts.first() else {
                return Err(AgentsError::Unsupported(
                    "a registry release with no archive",
                ));
            };
            let name = Name::new(agent.as_str()).ok();
            let mut told = telling(name.as_ref(), 0, total, &mut *watching.progress);
            let arrived = landed(
                &self.shelf.catalog,
                &target.hosts,
                artifact,
                &staging,
                (&mut told, watching.cancel),
                true,
            )
            .await?;
            let surveyed = {
                let artifact = crate::installing::registry_artifact(
                    &arrived.url,
                    &arrived.sha256,
                    arrived.size,
                    artifact.format,
                );
                let mut reading = staging.downloaded()?;
                Unpacker::survey_from(&mut reading, &artifact, staging.directory())?
            };
            let files = surveyed.files().to_vec();
            if files.len() > crucible_external_harnesses::catalog::LISTED_FILES {
                return Err(AgentsError::TooManyFiles {
                    agent: agent.clone(),
                    version: offer.pin.version.clone(),
                    files: files.len(),
                    bound: crucible_external_harnesses::catalog::LISTED_FILES,
                });
            }
            runnable(&files, &target.cmd)?;
            let entry = probe_entry(offer)?;
            // The probe starts its own services and runtime, so it runs on a
            // blocking thread of this one rather than inside the turn.
            let root = staging.directory().to_owned();
            let record = tokio::task::spawn_blocking(move || {
                crate::installing::probed(&root, &entry, crate::probing::BOUND)
            })
            .await
            .map_err(|_| AgentsError::Unsupported("a registry probe that did not finish"))?
            .map_err(|why| AgentsError::Probe {
                agent: agent.clone(),
                version: offer.pin.version.clone(),
                why: Box::new(why),
            })?;
            let provenance = Provenance {
                listing: Listing {
                    serial: offer.serial,
                    review: offer.entry.review.clone(),
                    terms: offer.entry.terms.clone(),
                },
                source,
                probe: Some(record.clone()),
                fetched: SystemTime::now(),
                artifacts: vec![arrived],
                decompressed: surveyed.decompressed(),
            };
            self.store.commit(staging, &version, &files, provenance)?;
            registered = Some(record);
        }

        if !kept && source != Released::Registry {
            let staging = self.store.stage(agent)?;
            let name = Name::new(agent.as_str()).ok();
            let mut files = Vec::new();
            let mut fetched = Vec::new();
            let mut before: u64 = 0;
            let mut decompressed: u64 = 0;
            for artifact in &target.artifacts {
                let mut told = telling(name.as_ref(), before, total, &mut *watching.progress);
                let arrived = landed(
                    &self.shelf.catalog,
                    &target.hosts,
                    artifact,
                    &staging,
                    (&mut told, watching.cancel),
                    false,
                )
                .await?;
                before = before.saturating_add(arrived.size);
                let mut reading = staging.downloaded()?;
                let staged = if script.is_some() && artifact.format == Format::Wheel {
                    Wheels::unpack_from(&mut reading, artifact, staging.directory())?
                } else {
                    Unpacker::unpack_from(&mut reading, artifact, staging.directory())?
                };
                files.extend_from_slice(staged.files());
                decompressed = decompressed.saturating_add(staged.decompressed());
                fetched.push(arrived);
            }
            if let Some(script) = &script {
                files.push(script.write(staging.directory())?);
            }
            if let Some(tree) = &tree {
                // The last artifact's download is unpacked; emptied, it takes
                // no room the tarballs need.
                drop(staging.download()?);
                let mut told = telling(name.as_ref(), before, total, &mut *watching.progress);
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
            for built in &builts {
                let mut told = telling(name.as_ref(), before, total, &mut *watching.progress);
                let arrived = landed(
                    &self.shelf.catalog,
                    &target.hosts,
                    &built.artifact,
                    &staging,
                    (&mut told, watching.cancel),
                    false,
                )
                .await?;
                before = before.saturating_add(arrived.size);
                built.install(&mut staging.downloaded()?, staging.directory(), &mut files)?;
            }
            if let Some((launcher, held)) = &scripting {
                run_scripts(
                    (launcher, held),
                    offer,
                    &locals,
                    staging.directory(),
                    &mut files,
                )
                .await?;
            }
            let provenance = Provenance {
                listing: Listing {
                    serial: offer.serial,
                    review: offer.entry.review.clone(),
                    terms: offer.entry.terms.clone(),
                },
                source,
                probe: None,
                fetched: SystemTime::now(),
                artifacts: fetched,
                decompressed,
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
            probe: registered,
            unsmoked: plan.smoking.as_ref().err().map(|why| why.as_str().into()),
            unpruned: self.prune(agent, &program).err().map(Box::new),
        })
    }

    /// The question an install, add or update puts.
    // The question is one value read from the move it asks about; splitting
    // its inputs would put the facts it says in more than one place.
    #[allow(clippy::too_many_arguments)]
    fn installing(
        offer: &Offer,
        how: How,
        plan: &Plan<'_>,
        moving: &[(&ExternalAgent, Version)],
        source: Released,
        registry: Option<&RegistryFact<'_>>,
    ) -> Confirmation {
        let kept = plan.kept;
        let target = &offer.pin.target;
        let entry = &offer.entry;
        let total = plan.total;
        let (key, version) = (entry.key.as_str(), quoted(&offer.pin.version));
        let putting = if source == Released::Registry {
            format!(
                "Download {version}, survey it, run it once in the sandbox to probe it, put it in \
                 place and move the instances named onto it"
            )
        } else {
            format!("Put {version} in place and move the instances named onto it")
        };
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
        } else if source == Released::Registry {
            let archive = target.artifacts.first().map_or_else(
                || "its registry archive".to_owned(),
                |artifact| quoted(&artifact.url),
            );
            format!(
                "Download {key} {version} from {archive}, up to {total} bytes, the registry's cap."
            )
        } else {
            format!("{key} {version}, {total} bytes to download.")
        }];
        if source == Released::Registry {
            facts.push(
                "Crucible will download the vendor's archive, survey it, and run the vendor's \
                 program once in the closed sandbox for a probe after your yes; a failed probe \
                 installs nothing."
                    .to_owned(),
            );
        }
        if let Some(fact) = registry.filter(|_| source == Released::Registry) {
            facts.push(match fact.unverified {
                Some(why) => format!(
                    "Source: ACP Registry, read {}; not verified: {}; not reviewed by Crucible.",
                    quoted(&fact.read),
                    quoted(why)
                ),
                None => format!(
                    "Source: ACP Registry, read {}; not reviewed by Crucible.",
                    quoted(&fact.read)
                ),
            });
            // The registry's generic warning is the route's hold; the install
            // says it now, and the launch puts it again before anything starts.
            facts.push(
                content_use::generic(entry.key.as_str(), &fact.read)
                    .warning
                    .sentence
                    .into_owned(),
            );
            // No registry entry records the hosts an agent reaches.
            facts.push(content_use::UNRECORDED.to_owned());
        }
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
                // A registry receipt's own hash and probe record are what hold
                // its release, not the signed catalog's revocations.
                receipt.source == Released::Registry
                    || !self
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
        // Smoke-tested only as the source still lists it: its entry is what
        // says what it must answer as. A registry receipt is read from the
        // cached registry index and its own probe record, never from the
        // signed catalog's trust state.
        let registry = previous.source == Released::Registry;
        let (listed, row, why) = if registry {
            match self.registry.as_ref() {
                None => (
                    None,
                    None,
                    Some(
                        self.registry_notice
                            .as_deref()
                            .unwrap_or("no ACP Registry index could be read")
                            .to_owned(),
                    ),
                ),
                Some(view) => match previous.probe.as_deref() {
                    None => (
                        None,
                        None,
                        Some(crucible_external_harnesses::store::UNPROBED.to_owned()),
                    ),
                    Some(probe) => {
                        match believed_registry(&view.index, &self.store, agent, &program, probe) {
                            Ok(offer) => {
                                let row = CatalogRow::registry(
                                    &offer.entry,
                                    &offer.pin.target,
                                    view.index.read_date().into(),
                                )
                                .ok();
                                (Some(offer), row, None)
                            }
                            Err(why) => (None, None, Some(why.to_string())),
                        }
                    }
                },
            }
        } else {
            let offer = self.trust.offers(Some(agent)).into_iter().find(|offer| {
                *offer.pin.version == *previous.version
                    && offer.pin.lock_revision == previous.lock_revision
            });
            let row = offer
                .as_ref()
                .and_then(|offer| CatalogRow::new(&offer.entry, &offer.pin.target).ok());
            (offer, row, None)
        };
        let smoking = if let Some(offer) = &listed {
            self.smoking(offer, row.as_ref())
        } else {
            let why = why.unwrap_or_else(|| {
                format!("the catalog no longer lists {}", quoted(&previous.version))
            });
            let why = match (registry, self.registry_notice.as_deref()) {
                (true, Some(line)) if !why.contains(line) => format!("{why}; {line}"),
                _ => why,
            };
            Err(format!(
                "{} {} cannot be smoke-tested: {why}",
                agent,
                quoted(&previous.version)
            ))
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
            probe: previous.probe.map(|probe| *probe),
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
        watching: &Watching<'_>,
    ) -> Result<Outcome, AgentsError> {
        let link = watching.link;
        let Some(configured) = self.instances.iter().find(|one| one.name() == instance) else {
            return Ok(Outcome::Refused(ErrorCode::InvalidArgument.into()));
        };
        // A sign-in needs the registry only for an installed registry row; it
        // reads the cache and fetches nothing on its own.
        let registry = self
            .registry
            .clone()
            .or_else(|| catalog_fetch::cached_registry(self.shelf.home.path()).ok());
        let settings = Settings::read_home(self.shelf.home)?;
        let here = Here::of(self.shelf.home, &settings);
        let (row, agent, found) = signing(
            &self.trust,
            &self.store,
            registry.as_ref(),
            configured,
            (method, &here),
        )?;
        let signed = |by| {
            Ok(Outcome::SignedIn {
                instance: instance.into(),
                method: found.id.clone(),
                by,
            })
        };
        unrun_for_own(&row, &found)?;
        let (launcher, by, billed, channel, handover) = match &found.method {
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
                (launcher, Signed::Kept, None, None, None)
            }
            Method::Terminal {
                route: Route::Terminal,
                ..
            } => {
                // A terminal is Unix: the controlling terminal, the foreground
                // process group and the signal Ctrl+C sends are the platform's
                // own. Elsewhere the route is refused before anything starts.
                if cfg!(unix) {
                    let terminal = watching
                        .terminal
                        .ok_or_else(|| AgentsError::Unlent(quoted(&found.id).into()))?;
                    let steps = self
                        .shelf
                        .steps
                        .ok_or_else(|| AgentsError::Conversation(quoted(&found.id).into()))?;
                    if steps.sandbox_required() {
                        return Err(AgentsError::Unconfined(quoted(&found.id).into()));
                    }
                    let launcher = self
                        .shelf
                        .smoke
                        .ok_or_else(|| AgentsError::Conversation(quoted(&found.id).into()))?;
                    (launcher, Signed::Command, None, None, Some(terminal))
                } else {
                    return Err(AgentsError::Unsupported(
                        "a sign-in at a real terminal, which this platform does not have",
                    ));
                }
            }
            Method::Terminal {
                route: Route::Piped,
                ..
            } => {
                // Refused before anything is asked or started: with no one to
                // carry it to, a yes would be asked for nothing.
                let channel = watching
                    .sign_in
                    .ok_or_else(|| AgentsError::Unlent(quoted(&found.id).into()))?;
                let launcher = self
                    .shelf
                    .smoke
                    .ok_or_else(|| AgentsError::Conversation(quoted(&found.id).into()))?;
                (launcher, Signed::Command, None, Some(channel), None)
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
                (launcher, Signed::Agent, billed, None, None)
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
        if let Some(warned) = launcher.consent().asks(started.row().route()) {
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
        // A command is not asked for over the wire, so the wire has no say.
        if channel.is_none() && handover.is_none() && !dialect.signing_in_with(&found.id) {
            return Err(AgentsError::Unsupported(
                "an agent whose wire signs in by no method named",
            ));
        }
        // The route was asked, and a run may have begun, or failed to stop,
        // while it was.
        beside(steps, &sharing)?;
        // A sign-in at a real terminal is unconfined, so it is asked about on
        // every run, in the words that say so, rather than once for the route.
        if handover.is_some()
            && !agreed_to_run_unconfined(self.request, front, instance, &found).await
        {
            return Ok(Outcome::Undecided);
        }
        // The agent's program is started on the instance's own control, so a
        // run of the instance is refused beside it, and it beside one, and a
        // stop that cannot be confirmed leaves the instance, and so every
        // instance on its profile, refused as a run not confirmed stopped is.
        let control = steps.control(configured.name()).ok_or_else(|| {
            AgentsError::Unverified(Failure::new("no control for the agent's program"))
        })?;
        // The start is given every key kept for the agent, so a kept key's
        // method is asked for with its key already in the agent's environment.
        let reading = match (channel, handover) {
            (Some(channel), _) => {
                launcher
                    .piped(
                        &started,
                        &control,
                        &found,
                        dialect.as_mut(),
                        channel,
                        watching.cancel,
                        link,
                    )
                    .await?
            }
            #[cfg(unix)]
            (None, Some(terminal)) => {
                launcher
                    .terminal(&started, &control, &found, dialect.as_mut(), terminal, link)
                    .await?
            }
            #[cfg(not(unix))]
            (None, Some(_)) => {
                return Err(AgentsError::Unsupported(
                    "a sign-in at a real terminal, which this platform does not have",
                ));
            }
            (None, None) => {
                launcher
                    .account(&started, &control, dialect.as_mut(), Account::SignIn, link)
                    .await?
            }
        };
        match reading {
            Reading::Signed(Authentication::SignedIn) => signed(by),
            Reading::Unreadable(failure) => Err(AgentsError::Unverified(failure)),
            Reading::Signed(_) | Reading::Version(_) => Err(AgentsError::Unverified(Failure::new(
                "The agent did not read as signed in after the sign-in.",
            ))),
        }
    }

    /// Hashes every installed version of `agent` against its receipt, and
    /// names a registry receipt whose source is not the registry, or whose
    /// probe record is missing or was taken for another machine.
    fn verify(&self, agent: AgentKey) -> Result<Outcome, AgentsError> {
        let installed = self.store.installed(&agent)?;
        if installed.is_empty() {
            return Err(AgentsError::NotInstalled(agent));
        }
        let mut versions = Vec::new();
        for receipt in installed {
            let version = Version::new(&receipt.version, receipt.lock_revision)?;
            let mut mismatches = self.store.verify(&agent, &version)?;
            mismatches.extend(registry::receipt_mismatches(
                &receipt,
                &registry::platform(),
            ));
            versions.push((receipt.version, mismatches));
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
    let registry = catalog_fetch::cached_registry(home.path()).ok();
    let here = Here::of(home, &settings);
    let (_, agent, found) = signing(
        &trust,
        &store,
        registry.as_ref(),
        configured,
        (method, &here),
    )?;
    if !matches!(found.method, Method::EnvVar { .. }) {
        return Err(AgentsError::Keyless(quoted(&found.id).into()));
    }
    let rows = Rows::production();
    let name = rows.signing_in(&agent, &found.id)?;
    let typed = key.applied();
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
    let registry = catalog_fetch::cached_registry(home.path()).ok();
    let here = Here::of(home, &settings);
    let (row, _, started) = signing_row(&trust, &store, registry.as_ref(), configured, &here)?;
    Ok(row
        .methods(started)
        .into_iter()
        .map(|offered| offered.id)
        .collect())
}

/// Whether the method the configured `instance` offers as `method` is one run
/// as a command at this terminal, piped or on the real one, which a front end
/// lends a channel or a terminal handover for.
///
/// # Errors
///
/// As signing in by it would, for an instance or a method there is not.
pub fn runs_a_command(home: &Home, instance: &str, method: &str) -> Result<bool, AgentsError> {
    let settings = Settings::read_home(home)?;
    let instances = settings.external_agents();
    let configured = instances
        .iter()
        .find(|one| one.name() == instance)
        .ok_or_else(|| AgentsError::NoInstance(quoted(instance).into()))?;
    let trust = Trust::load(&catalog_fetch::state_in(home.path()), SystemTime::now());
    let store = Store::new(home.path().join("programs"));
    let registry = catalog_fetch::cached_registry(home.path()).ok();
    let here = Here::of(home, &settings);
    let (_, _, found) = signing(
        &trust,
        &store,
        registry.as_ref(),
        configured,
        (method, &here),
    )?;
    Ok(matches!(
        found.method,
        Method::Terminal {
            route: Route::Piped | Route::Terminal,
            ..
        }
    ))
}

/// `with` lent what a front end with no conversation needs to start an agent:
/// a launcher and the steps it counts as, under the sandbox the user's own file
/// says for the directory this process is in, with the content-use answers of
/// `services`. A checkout's own file has no say in how a sign-in is confined,
/// as it has none at this door.
///
/// # Errors
///
/// [`AgentsError::Config`] where the configuration or the policy cannot be
/// read, and [`AgentsError::Piped`] where this directory cannot be opened.
pub fn lending_here<R>(
    home: &Home,
    services: &Services,
    runtime: &Handle,
    with: impl FnOnce(&Launcher<'_>, &Steps) -> R,
) -> Result<R, AgentsError> {
    let settings = Settings::read_home(home)?;
    let here = std::env::current_dir()
        .ok()
        .and_then(|here| Workspace::open(here).ok())
        .ok_or(Unpiped::Here)?;
    let policy = settings.sandbox().enforcing_policy(&here)?;
    services.consent().keeps_in(crucible_config::user(home));
    services
        .consent()
        .recorded(settings.content_accepted().into_iter().map(str::to_owned));
    let sandbox: Arc<dyn SandboxService> = Arc::new(
        LocalSandbox::new()
            .watching_on(runtime.clone())
            .through(ProxyEnv::read(|name: &str| std::env::var(name).ok())),
    );
    let state = catalog_fetch::state_in(home.path());
    let launcher = Launcher::new(
        sandbox,
        runtime.clone(),
        &policy,
        services.consent(),
        &state,
        home.sessions(),
    );
    let steps = Steps::new(
        launcher,
        settings.sandbox().enablement(),
        &REGISTRY,
        std::env::var_os("PATH"),
    );
    Ok(steps.lending(|launcher| with(launcher, &steps)))
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
    registry: Option<&RegistryRead>,
    instance: &ExternalAgent,
    (method, here): (&str, &Here<'_>),
) -> Result<(AgentRow, AgentKey, AuthMethod), AgentsError> {
    let (row, agent, started) = signing_row(trust, store, registry, instance, here)?;
    let found = row
        .methods(started)
        .into_iter()
        .find(|offered| *offered.id == *method)
        .ok_or_else(|| AgentsError::NoMethod {
            agent: agent.clone(),
            method: quoted(method).into(),
        })?;
    Ok((row, agent, found))
}

/// Tells `consent` the route of `row`, built from the offer it is the warning
/// of, before that route is looked for, and whether the route is one the
/// row's key learned. One it cannot hold it does not learn, so a catalog
/// agent's goes on unknown and refused.
fn teach(consent: &Consent, row: Option<&CatalogRow>) -> bool {
    row.and_then(|row| row.warned().ok())
        .is_some_and(|warned| consent.learn(warned))
}

/// The route `offer`'s key would reach under a yes given to something else:
/// one `consent` holds, that the key did not learn, where the key is not the
/// built-in its entry backs. A signed key can spell a built-in's name, and that
/// agent's yes says nothing about what this entry's vendor does. `None` is no
/// verdict on the route, which the hold still answers for.
fn borrowed(consent: &Consent, offer: &Offer, row: Option<&CatalogRow>) -> Option<String> {
    let key = offer.entry.key.as_str();
    let learned = teach(consent, row);
    let backs = REGISTRY
        .all()
        .any(|row| row.catalogued && row.driver.name() == key);
    let route = agent_route(key);
    (!learned && !backs && consent.routes().warned(&route).is_some()).then_some(route)
}

/// The row the configured `instance` runs as and its agent's name: a catalog
/// agent's row as [`believed_row`] makes it, a registry agent's as
/// [`believed_registry_row`] makes it from the cached registry index and its
/// receipt's probe record, and Antigravity's built in.
fn signing_row(
    trust: &Trust,
    store: &Store,
    registry: Option<&RegistryRead>,
    instance: &ExternalAgent,
    here: &Here<'_>,
) -> Result<(AgentRow, AgentKey, Started), AgentsError> {
    // The user's own program of a catalog agent signs in as a launch of it is
    // believed: by the methods its entry gives, with nothing asked of the
    // program first.
    if let (Driver::Catalog, Some(key)) = (instance.driver(), instance.agent()) {
        let state = catalog_fetch::state_in(here.home.path());
        match crate::own_program::believed(instance, key, &state, |name| std::env::var_os(name)) {
            Some(Ok((row, _))) => return here.named(AgentRow::Catalog(row), instance),
            None => {}
            Some(Err(why)) => return Err(Unlaunched::from(why).into()),
        }
    }
    let row = match instance.driver() {
        Driver::Catalog => {
            let key = instance
                .agent()
                .ok_or_else(|| AgentsError::Unsigned(quoted(instance.name()).into()))?;
            let executable = Path::new(instance.executable());
            let version = executable
                .is_absolute()
                .then(|| store.version_of(key, executable))
                .flatten();
            let receipt = version
                .as_ref()
                .and_then(|version| store.receipt(key, version).ok().flatten());
            let listed = registry.is_some_and(|view| view.index.entry(key).is_some());
            let registry_row = receipt
                .as_ref()
                .is_some_and(|receipt| receipt.source == Released::Registry)
                || (receipt.is_none() && listed);
            if registry_row {
                let view = registry.ok_or_else(|| AgentsError::Unlisted(key.clone()))?;
                let probe = receipt
                    .as_ref()
                    .and_then(|receipt| receipt.probe.as_deref())
                    .ok_or_else(|| {
                        AgentsError::Unlaunched(Unlaunched::Unbelieved(Unbelieved::Unprobed {
                            key: key.clone(),
                            version: version.as_ref().map_or_else(
                                || "its version".into(),
                                |version| quoted(version.version()).into(),
                            ),
                            why: crucible_external_harnesses::store::UNPROBED,
                        }))
                    })?;
                match believed_registry_row(&view.index, store, key, executable, probe) {
                    Ok(row) => AgentRow::Catalog(row),
                    Err(Unbelieved::Unregistered { .. }) => {
                        return Err(AgentsError::Unlisted(key.clone()));
                    }
                    Err(Unbelieved::Unprofiled { why, .. }) => return Err(why.into()),
                    Err(why) => return Err(Unlaunched::from(why).into()),
                }
            } else {
                match believed_row(trust, store, key, executable) {
                    Ok(row) => AgentRow::Catalog(row),
                    Err(Unbelieved::NoCatalog { key }) => return Err(AgentsError::Unlisted(key)),
                    Err(Unbelieved::Unprofiled { why, .. }) => return Err(why.into()),
                    // Where it is signed in is what the agent decides, whichever
                    // release it is: one the user runs, or the catalog has moved
                    // past, signs in as the release an update would install, and
                    // a run of it is refused or not as a run is.
                    Err(
                        Unbelieved::NotInstalled { .. }
                        | Unbelieved::Unreceipted { .. }
                        | Unbelieved::Elsewhere { .. }
                        | Unbelieved::Unoffered { .. },
                    ) => {
                        let offers = trust.offers(Some(key));
                        let offer = offers
                            .first()
                            .ok_or_else(|| AgentsError::Unlisted(key.clone()))?;
                        AgentRow::Catalog(CatalogRow::new(&offer.entry, &offer.pin.target)?)
                    }
                    Err(why) => return Err(Unlaunched::from(why).into()),
                }
            }
        }
        Driver::Antigravity => REGISTRY
            .built(Driver::Antigravity)
            .map(AgentRow::Built)
            .ok_or_else(|| AgentsError::Unsigned(quoted(instance.name()).into()))?,
        Driver::Claude => return Err(AgentsError::Unsigned(quoted(instance.name()).into())),
    };
    here.named(row, instance)
}

/// Where this door finds a configured instance's program, and the sandbox a
/// sign-in through it is started under: crucible's home, and the sandbox the
/// user's own file says, as [`lending_here`] confines one. A checkout's own
/// file has no say in it, as it has none at this door.
struct Here<'a> {
    /// Crucible's home, whose install store says which programs are its own.
    home: &'a Home,
    /// The sandbox a sign-in is started under.
    sandbox: Sandboxing,
}

impl<'a> Here<'a> {
    /// The door at `home`, under the sandbox `settings` say.
    fn of(home: &'a Home, settings: &Settings) -> Self {
        Self {
            home,
            sandbox: Sandboxing {
                on: settings.sandbox_enabled(),
                required: settings.sandbox().required_by_project(),
            },
        }
    }

    /// `row`, the configured `instance`'s, with its agent's name and how its
    /// program, looked for on this process's `PATH`, is started from here.
    fn named(
        &self,
        row: AgentRow,
        instance: &ExternalAgent,
    ) -> Result<(AgentRow, AgentKey, Started), AgentsError> {
        let agent = AgentKey::new(row.named())
            .map_err(|_| AgentsError::Unsigned(quoted(instance.name()).into()))?;
        let found = launching::discover(instance.executable(), &row.commands(), |name| {
            std::env::var_os(name)
        });
        let origin = match found {
            crucible_external_harnesses::Discovery::Found(program) => {
                launching::Origin::of(&self.home.path().join("programs"), program.path())
            }
            crucible_external_harnesses::Discovery::Missing
            | crucible_external_harnesses::Discovery::NotExecutable(_) => launching::Origin::Stored,
        };
        let started = Started {
            origin,
            sandbox: self.sandbox,
            profiled: instance.profile().is_some(),
        };
        Ok((row, agent, started))
    }
}

/// What downloading `offer`, `tree`, where it has one, and `builts` takes, in
/// bytes, refused where it or what it unpacks to is more than a release may
/// be; and its target as the install store's room is asked for it, with one
/// more artifact standing for the tree and one for each build.
fn sized(
    offer: &Offer,
    tree: Option<&NpmTree>,
    builts: &[Built<'_>],
) -> Result<(u64, Target), AgentsError> {
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
    for built in builts {
        downloaded = within(downloaded.checked_add(built.artifact.size))?;
        unpacked = within(unpacked.checked_add(built.artifact.unpacked))?;
        room.artifacts.push(built.artifact.clone());
    }
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
/// starts, refused where Node is given arguments of its own, where more
/// install scripts run at install or were built at review than a question
/// shows, or where one is run at install where `lent`, the launcher a
/// conversation lends, is none or cannot run it. A built script runs nothing
/// here, so it asks for neither a launcher nor a sandbox.
fn launched(
    offer: &Offer,
    lent: Option<&Launcher<'_>>,
) -> Result<(Option<LaunchScript>, Option<AssetRef>), AgentsError> {
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
            let local = scripts
                .iter()
                .filter(|script| matches!(script.disposition, Disposition::Local { .. }))
                .count();
            if more_scripts_than_shown(scripts) {
                return Err(AgentsError::Unsupported(
                    "a release with more install scripts run at install and built at review \
                     together than a question shows",
                ));
            }
            if local > 0 {
                // First, so nothing says to try from a conversation where that
                // would be refused too.
                if let Some(why) = launching::unscripted_platform() {
                    return Err(AgentsError::Unsupported(why));
                }
                let launcher = lent.ok_or(AgentsError::Unsupported(UNLENT))?;
                if let Some(why) = launcher.unscriptable() {
                    return Err(AgentsError::Unsupported(why));
                }
            }
            (None, Some(lock.asset.clone()))
        }
    })
}

/// The facts naming each install script of a tree, which none is run as npm
/// would run it: first those that change nothing, at most [`SCRIPTS`] and then
/// how many more; then those built at review, by their package and the size of
/// their asset, and those run at install in a sandbox of their own, every one
/// of both, as [`launched`] refuses a release with more than [`SCRIPTS`] of
/// them.
fn unrun(scripts: &[Script]) -> Vec<String> {
    let (installed, other): (Vec<&Script>, Vec<&Script>) = scripts.iter().partition(|script| {
        matches!(
            script.disposition,
            Disposition::Built { .. } | Disposition::Local { .. }
        )
    });
    let mut facts: Vec<String> = other
        .iter()
        .take(SCRIPTS)
        .map(|script| {
            let why = match &script.disposition {
                Disposition::Inert => "inert: it does nothing the agent needs",
                Disposition::Replaced => "replaced: a platform package does its work",
                Disposition::Lost => "lost: the agent works without what it would add",
                // Named below.
                Disposition::Built { .. } | Disposition::Local { .. } => "installed",
            };
            format!(
                "Install script not run: {} {}, {why}.",
                quoted(&script.package),
                quoted(&script.event)
            )
        })
        .collect();
    let more = other.len().saturating_sub(SCRIPTS);
    if more > 0 {
        facts.push(format!("And {more} more install scripts, none run."));
    }
    for script in installed.iter().copied() {
        if let Disposition::Built { asset, .. } = &script.disposition {
            facts.push(format!(
                "Install script not run: {} {}, built at review: its package's directory is \
                 replaced by the catalog's build, {} bytes.",
                quoted(&script.package),
                quoted(&script.event),
                asset.size,
            ));
        }
    }
    for script in installed {
        let Disposition::Local { command, hosts, .. } = &script.disposition else {
            continue;
        };
        let reach = if hosts.is_empty() {
            "no host".to_owned()
        } else {
            hosts
                .iter()
                .map(|host| quoted(host))
                .collect::<Vec<_>>()
                .join(", ")
        };
        // The whole command, escaped and delimited: it is what the yes is
        // to, the catalog bounds it, and nothing in it can read as this
        // sentence's own words.
        let delimited = command.replace('\\', "\\\\").replace('"', "\\\"");
        facts.push(format!(
            "Install script run at install: {} {}, in a sandbox of its own as sh -c \"{}\", \
             reaching {reach}, writing only its package and a private temporary directory; \
             its package must then hold exactly what its review found, or nothing is installed.",
            quoted(&script.package),
            quoted(&script.event),
            crucible_types::shown::escaped(&delimited),
        ));
    }
    facts
}

/// Why a release with an install script run at install is refused where no
/// conversation lent a launcher to run it through.
const UNLENT: &str = "a release whose install script runs in a conversation's sandbox, from a \
                      command line (install it from /catalog in a conversation)";

/// One install script of a release run at install, as its review found it.
struct Local<'a> {
    /// Its package, as its place in the tree.
    package: &'a str,
    /// Its package's name and version, as the lock gives them.
    named: (&'a str, &'a str),
    /// The npm event it runs on.
    event: &'a str,
    /// What `sh -c` is given.
    command: &'a str,
    /// The host patterns it may reach.
    hosts: &'a [Box<str>],
    /// What its package must hold once it ends.
    result: Vec<File>,
}

/// Each install script of `offer` run at install, with the listing its
/// review left fetched from the catalog's own host and read, each refused
/// where `tree` installs no package where it says it belongs.
async fn locals<'a>(
    source: &Source,
    offer: &'a Offer,
    tree: Option<&'a NpmTree>,
) -> Result<Vec<Local<'a>>, AgentsError> {
    let Launch::Npm { scripts, .. } = &offer.pin.target.launch else {
        return Ok(Vec::new());
    };
    let mut locals = Vec::new();
    for script in scripts {
        let Disposition::Local {
            command,
            hosts,
            result,
        } = &script.disposition
        else {
            continue;
        };
        let package = tree
            .and_then(|tree| {
                tree.packages()
                    .iter()
                    .find(|package| package.location == script.package)
            })
            .ok_or(AgentsError::Unsupported(
                "a release whose install script belongs to no package its lock installs",
            ))?;
        let bytes = catalog_fetch::asset(source, result)
            .await
            .map_err(AgentsError::FileListUnfetched)?;
        let result = result_list(&offer.entry, &bytes).map_err(AgentsError::FileListUnread)?;
        locals.push(Local {
            package: &script.package,
            named: (&package.name, &package.version),
            event: &script.event,
            command,
            hosts,
            result,
        });
    }
    Ok(locals)
}

/// One install script of a release built at review, its asset described as
/// the download the owner of an archive unpacks.
struct Built<'a> {
    /// Its package, as its place in the tree.
    package: &'a str,
    /// Its asset, under that asset's pin, with the list it is held to.
    artifact: Artifact,
}

impl Built<'_> {
    /// What its package's directory holds once it is installed.
    fn files(&self) -> &[File] {
        match &self.artifact.files {
            Files::Listed(files) => files,
            // `builts` makes every one with its list read.
            Files::Asset(_) => &[],
        }
    }

    /// Makes its package's directory in `staging` exactly what its review
    /// listed: the download in `reading` was held to its pin as it arrived,
    /// what the package's tarball left there is removed rather than
    /// overlaid, and the archive unpacks into a directory made new, so
    /// nothing it holds is written through. Its files take the place in
    /// `files` of whatever lay under the package, a package nested there
    /// included, since the build's listing is of the whole directory.
    fn install(
        &self,
        reading: &mut std::fs::File,
        staging: &Path,
        files: &mut Vec<File>,
    ) -> Result<(), AgentsError> {
        let directory = walked(staging, self.package)?;
        std::fs::remove_dir_all(&directory).map_err(DownloadError::Write)?;
        owner_only(&directory)?;
        let staged = Unpacker::unpack_plain_from(reading, &self.artifact, &directory)?;
        let prefix = format!("{}/", self.package);
        files.retain(|file| !file.path.starts_with(&prefix));
        files.extend(under(self.package, staged.files()));
        Ok(())
    }
}

/// The directory `package` names under `staging`, any part of it the tree did
/// not unpack made, and refused where a part of it is not a directory, a link
/// among them: nothing is followed out of the staging.
fn walked(staging: &Path, package: &str) -> Result<PathBuf, AgentsError> {
    let mut directory = staging.to_path_buf();
    for part in package.split('/') {
        directory.push(part);
        match std::fs::symlink_metadata(&directory) {
            Ok(held) if held.is_dir() => {}
            Ok(_) => {
                return Err(AgentsError::Unsupported(
                    "a release whose build's package is not a directory its tree unpacks",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                owner_only(&directory)?;
            }
            Err(error) => return Err(DownloadError::Write(error).into()),
        }
    }
    Ok(directory)
}

/// Makes `directory`, reachable by its owner alone, as an unpack makes one.
fn owner_only(directory: &Path) -> Result<(), DownloadError> {
    let made = std::fs::DirBuilder::new();
    #[cfg(unix)]
    let made = {
        let mut made = made;
        std::os::unix::fs::DirBuilderExt::mode(&mut made, 0o700);
        made
    };
    made.create(directory).map_err(DownloadError::Write)
}

/// `files` as the receipt names them, under `package`.
fn under<'a>(package: &'a str, files: &'a [File]) -> impl Iterator<Item = File> + 'a {
    files.iter().map(move |file| File {
        path: format!("{package}/{}", file.path).into(),
        kind: file.kind.clone(),
    })
}

/// Whether `one` is `other`, or lies above or below it.
fn nested(one: &str, other: &str) -> bool {
    let below = |lower: &str, upper: &str| {
        lower
            .strip_prefix(upper)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    };
    below(one, other) || below(other, one)
}

/// Whether a build in `scripts` lies beside another build or a script run at
/// install on its directory or any part of it. A build replaces its package's
/// whole directory, so which of them the package would end as is nothing its
/// review found. `tests/fixtures/agent-catalog/overlapping-scripts.json` is the
/// rule's table, held to this and to the catalog intake, which publishes no
/// such target.
pub(super) fn overlapping_build(scripts: &[Script]) -> bool {
    scripts.iter().enumerate().any(|(index, script)| {
        matches!(script.disposition, Disposition::Built { .. })
            && scripts.iter().enumerate().any(|(at, other)| {
                at != index
                    && matches!(
                        other.disposition,
                        Disposition::Built { .. } | Disposition::Local { .. }
                    )
                    && nested(&script.package, &other.package)
            })
    })
}

/// Each install script of `offer` built at review, with the list its asset is
/// held to fetched from the catalog's own host and read, refused where `tree`
/// installs no package where it says it belongs, or where
/// [`overlapping_build`] holds.
async fn builts<'a>(
    source: &Source,
    offer: &'a Offer,
    tree: Option<&NpmTree>,
) -> Result<Vec<Built<'a>>, AgentsError> {
    let Launch::Npm { scripts, .. } = &offer.pin.target.launch else {
        return Ok(Vec::new());
    };
    if overlapping_build(scripts) {
        return Err(AgentsError::Unsupported(
            "a release with two builds, or a build and a script run at install, of one package",
        ));
    }
    let mut builts = Vec::new();
    for script in scripts {
        let Disposition::Built {
            asset,
            files,
            unpacked,
            decompressed,
        } = &script.disposition
        else {
            continue;
        };
        if !tree.is_some_and(|tree| {
            tree.packages()
                .iter()
                .any(|package| package.location == script.package)
        }) {
            return Err(AgentsError::Unsupported(
                "a release whose install script belongs to no package its lock installs",
            ));
        }
        let bytes = catalog_fetch::asset(source, files)
            .await
            .map_err(AgentsError::FileListUnfetched)?;
        let list = built_list(&offer.entry, &bytes).map_err(AgentsError::FileListUnread)?;
        let prefix = format!("{}/", script.package);
        if tree.is_some_and(|tree| {
            tree.programs().any(|(_, path)| {
                path.strip_prefix(&*prefix).is_some_and(|path| {
                    !list.iter().any(|file| {
                        *file.path == *path && matches!(file.kind, FileKind::File { .. })
                    })
                })
            })
        }) {
            return Err(AgentsError::Unsupported(
                "a release whose build leaves out a program its package declares",
            ));
        }
        builts.push(Built {
            package: &script.package,
            artifact: Artifact {
                url: String::new().into(),
                mirror: None,
                sha256: asset.sha256.clone(),
                size: asset.size,
                format: Format::TarGz,
                unpacked: *unpacked,
                decompressed: *decompressed,
                files: Files::Listed(list),
                kept: Vec::new(),
                replaced_by: Some(asset.clone()),
            },
        });
    }
    Ok(builts)
}

/// Runs each of `locals` of `offer` in the version staged at `staging`, one
/// at a time, and puts what each left into `files` as its review listed it, in
/// place of what its package's tarball unpacked to.
async fn run_scripts(
    (launcher, held): (&Launcher<'_>, &Held),
    offer: &Offer,
    locals: &[Local<'_>],
    staging: &Path,
    files: &mut Vec<File>,
) -> Result<(), AgentsError> {
    let agent = &offer.entry.key;
    let node = staging.join(&*offer.pin.target.cmd);
    let node = node.parent().unwrap_or(staging);
    for local in locals {
        launcher
            .script(
                held,
                launching::Scripted {
                    agent,
                    staging,
                    package: local.package,
                    named: local.named,
                    event: local.event,
                    command: local.command,
                    hosts: local.hosts,
                    node,
                    result: &local.result,
                },
            )
            .await
            .map_err(|why| AgentsError::Script {
                agent: agent.clone(),
                version: quoted(&offer.pin.version).into(),
                why,
            })?;
        let prefix = format!("{}/", local.package);
        files.retain(|file| !file.path.starts_with(&prefix));
        files.extend(local.result.iter().map(|file| File {
            path: format!("{prefix}{}", file.path).into(),
            kind: file.kind.clone(),
        }));
    }
    Ok(())
}

/// The most install scripts a question names one by one.
pub(super) const SCRIPTS: usize = 16;

/// Whether `scripts` holds more run at install and built at review together
/// than [`SCRIPTS`]; one a reviewer decided otherwise is not counted.
/// `tests/fixtures/agent-catalog/overlapping-scripts.json` holds the number,
/// and this and the catalog intake to it, which publishes no such target.
pub(super) fn more_scripts_than_shown(scripts: &[Script]) -> bool {
    scripts
        .iter()
        .filter(|script| {
            matches!(
                script.disposition,
                Disposition::Built { .. } | Disposition::Local { .. }
            )
        })
        .count()
        > SCRIPTS
}

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

/// Adds to `reached` each place `artifact` may be fetched from that it does
/// not hold yet, for the question to name.
fn reach(
    source: &Source,
    hosts: &[Box<str>],
    artifact: &Artifact,
    reached: &mut Vec<String>,
) -> Result<(), AgentsError> {
    for place in agent_download::reachable(source, hosts, artifact)? {
        if !reached.contains(&place) {
            reached.push(place);
        }
    }
    Ok(())
}

/// What a download is told to say of itself: its bytes on top of the `before`
/// the install has already fetched, of the `total` it will.
fn telling<'a>(
    name: Option<&'a Name>,
    before: u64,
    total: u64,
    progress: &'a mut (dyn FnMut(InstallProgress) + Send),
) -> impl FnMut(u64, u64) + Send + 'a {
    move |done, _| {
        let done = before.saturating_add(done).min(total);
        if let Some(said) = name
            .cloned()
            .and_then(|name| InstallProgress::new(name, done, total))
        {
            progress(said);
        }
    }
}

/// Downloads `artifact` to `staging`'s download file, held to its pin as it
/// arrives, and closes the file so it can be read back.
// Each is an input of its own to the download, and a value gathering them
// would be built at each caller and taken apart here.
#[allow(clippy::too_many_arguments)]
async fn landed(
    source: &Source,
    hosts: &[Box<str>],
    artifact: &Artifact,
    staging: &crucible_external_harnesses::store::Staging,
    (told, cancel): (&mut (dyn FnMut(u64, u64) + Send), &Cancel),
    bounded: bool,
) -> Result<Fetched, AgentsError> {
    let mut landing = staging.download()?;
    let landing = Landing {
        file: &mut landing,
        progress: told,
    };
    let arrived = if bounded {
        // A registry archive has no exact size: the capped path accepts one
        // shorter than its bound and records the size that arrived.
        agent_download::capped(
            source,
            hosts,
            &artifact.url,
            &artifact.sha256,
            landing,
            cancel,
        )
        .await?
    } else {
        agent_download::artifact(source, hosts, artifact, landing, cancel).await?
    };
    Ok(arrived)
}

/// The provisional entry of a registry offer, as [`crate::installing::probed`]
/// reads it: the platform's target, its program and arguments, and the archive
/// the survey was taken from. The probe starts the program from the surveyed
/// staging, so the entry carries no file list; it is read again through
/// `Entry::unsigned` before anything runs, as every provisional entry is.
fn probe_entry(offer: &Offer) -> Result<Vec<u8>, AgentsError> {
    let target = &offer.pin.target;
    let artifact = target.artifacts.first().ok_or(AgentsError::Unsupported(
        "a registry release with no archive",
    ))?;
    let format = match artifact.format {
        Format::Zip => "zip",
        Format::TarGz => "tar.gz",
        Format::TarBz2 => "tar.bz2",
        Format::Bare => "bare",
        Format::Wheel => "wheel",
    };
    let document = serde_json::json!({
        "key": offer.entry.key.as_str(),
        "registry_id": &*offer.entry.registry_id,
        "version": &*offer.entry.version,
        "kind": "binary",
        "license": &*offer.entry.license,
        "review": &*offer.entry.review,
        "admits": {
            "name": &*offer.entry.admits.name,
            "version": &*offer.entry.admits.version,
        },
        "targets": [{
            "platform": &*target.platform,
            "cmd": &*target.cmd,
            "args": target.args.iter().map(|argument| &**argument).collect::<Vec<_>>(),
            "env": target.env.iter().map(|variable| (&*variable.name, &*variable.value)).collect::<std::collections::BTreeMap<_, _>>(),
            "hosts": target.hosts.iter().map(|host| &**host).collect::<Vec<_>>(),
            "artifacts": [{
                "url": &*artifact.url,
                "sha256": &*artifact.sha256,
                "size": artifact.size,
                "format": format,
                "unpacked": artifact.unpacked,
                "decompressed": artifact.decompressed,
                "files": [],
            }],
        }],
    });
    serde_json::to_vec(&document)
        .map_err(|_| AgentsError::Unsupported("a registry probe entry that cannot be written"))
}

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
/// held to `listed`, what `builts` left under its package among them.
fn same(
    receipt: &Receipt,
    target: &Target,
    listed: &[File],
    tree: Option<&NpmTree>,
    builts: &[Built<'_>],
) -> bool {
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
    let beneath = |file: &File, location: &str| {
        file.path
            .strip_prefix(location)
            .is_some_and(|rest| rest.starts_with('/'))
    };
    let installed = |file: &File| {
        !builts.iter().any(|built| beneath(file, built.package))
            && tree.is_some_and(|tree| {
                tree.packages()
                    .iter()
                    .any(|package| beneath(file, &package.location))
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

/// Whether `receipt` is the registry release `target` pins: its source is the
/// registry, its archive's SHA-256 is the target's pin (the surveyed size is
/// not), and it carries a probe for this platform. The version is the store
/// key the receipt was read under.
fn registry_kept(receipt: &Receipt, target: &Target) -> bool {
    receipt.source == Released::Registry
        && receipt
            .unprobed(&crucible_external_harnesses::catalog::registry::platform())
            .is_none()
        && target.artifacts.first().is_some_and(|artifact| {
            receipt
                .artifacts
                .first()
                .is_some_and(|fetched| fetched.sha256 == artifact.sha256)
        })
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
/// Whether `front` agrees to run the sign-in `method` of `instance` at a real
/// terminal, unconfined, in a question made for that run alone.
async fn agreed_to_run_unconfined(
    request: &Request,
    front: &mut dyn Front,
    instance: &str,
    method: &AuthMethod,
) -> bool {
    let run = Confirmation {
        asks: format!(
            "Run {} as {instance} at this terminal, outside the sandbox?",
            quoted(&method.id)
        ),
        facts: vec![
            "The agent's own command is started as you, with this terminal's input and output."
                .to_owned(),
            "It is not held by the sandbox: nothing about what it reads or writes is checked."
                .to_owned(),
            "Ctrl+C reaches only the command, and it is stopped with everything it started after \
             ten minutes."
                .to_owned(),
        ],
        choices: vec![
            (
                "Run it".to_owned(),
                "Hand this terminal to the command until it ends.".to_owned(),
            ),
            ("Cancel".to_owned(), "Start nothing.".to_owned()),
        ],
    };
    confirmed(request, front, &run).await == Some(0)
}

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

#[cfg(test)]
mod tests {
    use crucible_external_harnesses::release::OwnedPin;

    use super::*;

    /// An offer of a release whose one artifact is a zip of one program, so a
    /// question can be put without a catalog or a store.
    fn offered() -> Offer {
        let hash = "a".repeat(64);
        let entry = Entry::unsigned(
            serde_json::json!({
                "key": "goose",
                "registry_id": "goose-acp",
                "version": "1.4.1",
                "kind": "binary",
                "license": "Apache-2.0",
                "review": "https://cdn.agentclientprotocol.com/registry/v1/latest/registry.json",
                "admits": {"name": "goose", "version": "1.4.1"},
                "auth_methods": [],
                "targets": [{
                    "platform": "freebsd-x86_64",
                    "cmd": "goose",
                    "args": [],
                    "env": {},
                    "hosts": ["github.com"],
                    "artifacts": [{
                        "url": "https://github.com/example/goose/releases/download/v1.4.1/goose.zip",
                        "sha256": hash,
                        "size": 100,
                        "format": "zip",
                        "unpacked": 100,
                        "decompressed": 100,
                        "files": [{"path": "goose", "size": 100, "sha256": "a".repeat(64), "exec": true}]
                    }]
                }]
            })
            .to_string()
            .as_bytes(),
        )
        .expect("an entry");
        let target = entry
            .targets
            .first()
            .cloned()
            .expect("the entry has a target");
        Offer {
            pin: OwnedPin {
                key: entry.key.clone(),
                version: entry.version.clone(),
                lock_revision: None,
                admits: entry.admits.clone(),
                target,
            },
            entry,
            serial: 0,
        }
    }

    fn plan() -> Plan<'static> {
        Plan {
            kept: false,
            total: 100,
            tree: None,
            reached: vec!["github.com".to_owned()],
            smoking: Err("no conversation lent a launcher to start it with".to_owned()),
            adding: None,
        }
    }

    fn registry_fact() -> RegistryFact<'static> {
        RegistryFact {
            read: "2026-11-04".to_owned(),
            unverified: None,
        }
    }

    #[test]
    fn a_registry_install_question_says_the_probe_runs_the_vendors_program() {
        let offer = offered();
        let fact = registry_fact();

        let question = Door::installing(
            &offer,
            How::Installed,
            &plan(),
            &[],
            Released::Registry,
            Some(&fact),
        );

        let said = question.facts.join(" ");
        assert!(said.contains("run the vendor's program"), "{said}");
        assert!(said.contains("probe"), "{said}");
        assert!(
            question
                .choices
                .first()
                .is_some_and(|(_, does)| does.contains("probe")),
            "{:?}",
            question.choices
        );
    }

    #[test]
    fn a_registry_install_question_says_its_source_and_read_date() {
        let offer = offered();
        let fact = registry_fact();

        let question = Door::installing(
            &offer,
            How::Installed,
            &plan(),
            &[],
            Released::Registry,
            Some(&fact),
        );

        let said = question.facts.join(" ");
        assert!(
            said.contains("Source: ACP Registry, read 2026-11-04"),
            "{said}"
        );
        assert!(said.contains("not reviewed by Crucible"), "{said}");
        assert!(!said.contains("not verified"), "{said}");
    }

    #[test]
    fn a_registry_install_from_an_unverified_cache_says_so_and_its_date() {
        let offer = offered();
        let fact = RegistryFact {
            read: "2026-11-04".to_owned(),
            unverified: Some("the ACP Registry was not verified: the host answered 500"),
        };

        let question = Door::installing(
            &offer,
            How::Installed,
            &plan(),
            &[],
            Released::Registry,
            Some(&fact),
        );

        let said = question.facts.join(" ");
        assert!(
            said.contains("Source: ACP Registry, read 2026-11-04; not verified:"),
            "{said}"
        );
        assert!(said.contains("the host answered 500"), "{said}");
    }

    #[test]
    fn a_catalog_install_question_with_a_registry_index_read_does_not_claim_the_registry() {
        let offer = offered();
        let fact = registry_fact();

        let question = Door::installing(
            &offer,
            How::Updated,
            &plan(),
            &[],
            Released::Catalog,
            Some(&fact),
        );

        let said = question.facts.join(" ");
        assert!(!said.contains("Source: ACP Registry"), "{said}");
        assert!(!said.contains("not reviewed by Crucible"), "{said}");
    }

    #[test]
    fn a_catalog_install_question_does_not_say_a_probe_runs() {
        let offer = offered();

        let question = Door::installing(
            &offer,
            How::Installed,
            &plan(),
            &[],
            Released::Catalog,
            None,
        );

        let said = question.facts.join(" ");
        assert!(!said.contains("probe"), "{said}");
        assert!(!said.contains("Source: ACP Registry"), "{said}");
        assert!(
            !question
                .choices
                .first()
                .is_some_and(|(_, does)| does.contains("probe")),
            "{:?}",
            question.choices
        );
    }

    #[test]
    fn a_reviewed_update_is_asked_for_a_registry_release_and_only_for_a_signed_one() {
        assert!(!reviewed(true, How::Updated, Released::Registry));
        assert!(!reviewed(true, How::Installed, Released::Catalog));
        assert!(!reviewed(false, How::Updated, Released::Catalog));
        assert!(reviewed(true, How::Updated, Released::Catalog));
    }

    #[test]
    fn a_try_with_no_conversation_says_so_in_one_sentence() {
        let agent = AgentKey::new("goose").expect("a key");

        assert_eq!(
            AgentsError::Untried(agent).to_string(),
            "trying your own goose starts it, which needs a conversation to start it in; \
             nothing was started"
        );
    }
}
