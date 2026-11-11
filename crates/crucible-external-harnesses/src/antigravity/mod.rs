//! Google's Antigravity agent, spoken to over the Agent Client Protocol: a
//! release crucible installs, run in a profile of its own, or the user's own
//! program, run with no profile, so it looks in its usual home.
//!
//! [`Antigravity`] is the protocol's client run as a release the catalog
//! reviewed: the entry keyed [`AGENT`], from the catalog most lately believed
//! or else the snapshot built in, says which versions, what each must call
//! itself and what it is started with, and the [`Manifest`] says what no entry
//! does. An agent that describes itself otherwise is sent nothing more. It is
//! started with only the environment its profile builds, so no credential the
//! shell holds for another use reaches it, and whether it is signed in is read
//! from what the agent itself answers in that profile. A profile crucible makes
//! is never one another Google product shares.
//!
//! [`Antigravity::own`] is the client run as the user's own program instead,
//! such as Google's `agy`, found under one of the [`COMMANDS`] or named by its
//! path: it is admitted by what it answers, as [`Manifest::admits_own`] says,
//! never by its name or version, and is given no profile unless the user
//! names one, so it looks for its sign-in in its usual home, which the sandbox
//! hides. [`own_note`] tells the user which sign-in that leaves it. One made
//! [`Antigravity::sandboxed`] and given no profile fails a turn it is not
//! signed in for by naming the two ways it can be, [`own_ways`], rather than a
//! sign-in it could not keep.
//!
//! It signs in by any of the four methods [`methods`] lists, the one asked
//! for named by the user.
//!
//! The agent asks to be signed in by printing a link rather than answering.
//! During a sign-in the link is shown to the user until the agent says the
//! sign-in took; after that, as during a turn or a status check, it means the
//! profile is signed out, the step reads so and a turn fails saying so; during
//! a sign-out or a version check it answers nothing asked.
//!
//! The agent asks about every action itself; one made [`Antigravity::allowing`]
//! answers yes to what the conversation's mode allows without asking, as
//! [`Acp::allowing`] does.

mod account;
mod manifest;
mod profile;

pub use account::{AGENT_PLATFORM, Ended, GEMINI_API_KEY, OAUTH_BUSINESS, OAUTH_PERSONAL, methods};
pub use manifest::Manifest;
pub use profile::business;

use crucible_execution::{Authentication, Decision, Failure, TurnRequest};
use crucible_types::AgentKey;

use crate::acp::{Acp, Allowing, Initialized, Profile};
use crate::catalog::{Admits, SNAPSHOT, Trust};
use crate::release::OwnedPin;
use crate::{Account, Act, Dialect, Ran, Reading, Request, Shape, Told};

/// The key of the catalog entry Antigravity's releases are listed under, which
/// is also its directory in crucible's home.
pub const AGENT: &str = "antigravity";

/// The releases of Antigravity that `trust` offers for this platform, from the
/// newest source to the oldest.
#[must_use]
pub fn offered(trust: &Trust) -> Vec<OwnedPin> {
    AgentKey::new(AGENT).map_or_else(|_| Vec::new(), |key| trust.offered(&key))
}

/// The version whose directory under `programs/antigravity/` the program at
/// `program` is kept in, where releases are offered and it is none of them.
#[must_use]
pub fn unoffered(offered: &[OwnedPin], program: Option<&std::path::Path>) -> Option<String> {
    let kept = kept_version(program?)?;
    let found = !offered.is_empty()
        && !offered
            .iter()
            .any(|pin| std::ffi::OsStr::new(&*pin.version) == kept);
    found.then(|| kept.to_string_lossy().into_owned())
}

/// The version the directory `program` is kept in names, where it is kept
/// under `programs/antigravity/`.
fn kept_version(program: &std::path::Path) -> Option<std::ffi::OsString> {
    let version = program.parent()?;
    let agent = version.parent()?;
    let programs = agent.parent()?;
    (agent.file_name()? == AGENT && programs.file_name()? == "programs")
        .then(|| version.file_name().map(std::ffi::OsStr::to_os_string))
        .flatten()
}

/// Of the releases `offered`, those the program at `program` can be: the one
/// whose version names the directory it is in, where it is kept under
/// `programs/antigravity/`, and none if that version is not offered; all of
/// them for a program kept anywhere else, such as one installed by hand.
#[must_use]
pub fn started(offered: Vec<OwnedPin>, program: Option<&std::path::Path>) -> Vec<OwnedPin> {
    match program.and_then(kept_version) {
        Some(version) => offered
            .into_iter()
            .filter(|pin| std::ffi::OsStr::new(&*pin.version) == version)
            .collect(),
        None => offered,
    }
}

/// What the user is told of their own Antigravity program before it is
/// chosen and wherever an instance runs one: whose sign-in it uses, given
/// whether the sandbox is on.
#[must_use]
pub const fn own_note(sandboxed: bool) -> &'static str {
    if sandboxed {
        "Under the sandbox, your own Antigravity does not see the sign-in it keeps in \
         ~/.gemini; the store release keeps a separate one. A business sign-in needs \
         the store release or a profileDir you add."
    } else {
        "Your own Antigravity uses the sign-in it keeps in ~/.gemini, which another \
         Google product may share; the store release keeps a separate one. A business \
         sign-in needs the store release or a profileDir you add."
    }
}

/// The ways the user's own Antigravity program can be signed in where it is
/// run under the sandbox with no profile, and so cannot reach the sign-in it
/// keeps in `~/.gemini`: turning the sandbox off, which is left out where
/// `required` says a project requires the sandbox, and the store release.
#[must_use]
pub const fn own_ways(required: bool) -> &'static str {
    if required {
        "This project requires the sandbox, so use the store release of Antigravity, \
         which keeps its sign-in in a profile of its own."
    } else {
        "Either turn the sandbox off (see docs/security/sandboxing.md), which removes \
         confinement from every command and agent in the conversation and is not \
         available where a project requires the sandbox, or use the store release of \
         Antigravity, which keeps its sign-in in a profile of its own."
    }
}

/// The command it is looked up as.
const PROGRAM: &str = "antigravity";

/// The protocol's client, as this release of the agent is run.
const PROFILE: Profile = Profile {
    program: PROGRAM,
    versions: Manifest::REVIEWED.protocols,
    admits,
    sign_in: Manifest::REVIEWED.sign_in,
};

/// Whether the agent described as `initialized` is one of the releases
/// `offered`.
fn admits(initialized: &Initialized, offered: &[Admits]) -> Result<(), Failure> {
    Manifest::REVIEWED.admits(initialized, offered)
}

/// The command names the agent is known under, in the order they are looked
/// for: Google's command-line program, then the release's own name.
pub const COMMANDS: &[&str] = &["agy", PROGRAM];

/// The name, less its extension, of the file a release's program is
/// unpacked as.
const RELEASED_FILE: &str = "agy_acp_server";

/// Whether the program at `program` is named as a release's program is, as
/// its unpacked file or by the release's command, whatever extension the
/// platform gives it: such a program is started with a release's arguments.
#[must_use]
pub fn named_as_release(program: &std::path::Path) -> bool {
    program
        .file_stem()
        .is_some_and(|stem| stem == RELEASED_FILE || stem == PROGRAM)
}

/// The arguments the user's own program is started with, which serve the
/// protocol from Google's command-line program.
const OWN_ARGUMENTS: &[&str] = &["acp"];

/// The protocol's client, as the user's own program is run.
const OWN_PROFILE: Profile = Profile {
    program: PROGRAM,
    versions: Manifest::REVIEWED.protocols,
    admits: admits_own,
    sign_in: Manifest::REVIEWED.sign_in,
};

/// Whether the agent described as `initialized` holds what this client
/// relies on, whatever release the catalog offers.
fn admits_own(initialized: &Initialized, _offered: &[Admits]) -> Result<(), Failure> {
    Manifest::REVIEWED.admits_own(initialized)
}

/// The Antigravity agent's wire.
#[derive(Debug)]
pub struct Antigravity {
    /// The protocol's client.
    acp: Acp,
    /// The arguments it is started with.
    args: Vec<String>,
    /// Whether it is the user's own program run under the sandbox.
    sandboxed: bool,
    /// Whether a project requires the sandbox, so turning it off is not one
    /// of the ways it can be signed in.
    required: bool,
    /// Whether it was last started under the sandbox with no profile, so the
    /// sign-in it keeps in `~/.gemini` is out of its reach.
    unreachable: bool,
}

impl Default for Antigravity {
    /// The wire for the release the snapshot built into the binary lists.
    fn default() -> Self {
        let snapshot =
            AgentKey::new(AGENT).map_or_else(|_| Vec::new(), |key| SNAPSHOT.releases(&key));
        Self::new(Acp::new(PROFILE), &snapshot)
    }
}

impl Antigravity {
    /// The wire answering yes by itself to what `allowing` names, for the
    /// releases `offered`: it admits an agent that answers as one of them, and
    /// starts it with the arguments of the newest, which is the first.
    #[must_use]
    pub fn allowing(allowing: Allowing, offered: &[OwnedPin]) -> Self {
        Self::new(Acp::allowing(PROFILE, allowing), offered)
    }

    /// The wire for the user's own program at `program`, its canonical path:
    /// it is admitted by what it answers rather than as a release `offered`,
    /// and is started with the single argument `acp`, unless its file is
    /// named as a release's program is, when it is started as the newest
    /// release `offered` is. With no profile it is given no home, so it looks
    /// in its usual one; one the user names is its home, as for a release.
    #[must_use]
    pub fn own(program: &std::path::Path, allowing: Allowing, offered: &[OwnedPin]) -> Self {
        let mut own = Self::new(Acp::allowing(OWN_PROFILE, allowing), offered);
        if !named_as_release(program) {
            own.args = OWN_ARGUMENTS.iter().map(ToString::to_string).collect();
        }
        own
    }

    /// The wire for the user's own program, said to run under the sandbox
    /// where `sandboxed` holds, which a project requires where `required`
    /// does: started with no profile, it fails a turn it is not signed in for
    /// by naming [`own_ways`].
    #[must_use]
    pub const fn sandboxed(mut self, sandboxed: bool, required: bool) -> Self {
        self.sandboxed = sandboxed;
        self.required = required;
        self
    }

    /// `acp`, held to the releases `offered`.
    fn new(acp: Acp, offered: &[OwnedPin]) -> Self {
        let args = offered.first().map_or_else(Vec::new, |newest| {
            newest.target.args.iter().map(ToString::to_string).collect()
        });
        Self {
            acp: acp.listing(offered.iter().map(|pin| pin.admits.clone()).collect()),
            args,
            sandboxed: false,
            required: false,
            unreachable: false,
        }
    }
}

impl Dialect for Antigravity {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        let _ = self.acp.request(shape);
        self.unreachable = self.sandboxed && shape.profile.is_none();
        Request::showing(
            self.args.clone(),
            profile::environment(std::env::vars_os(), shape.profile),
        )
    }

    fn open(&mut self, turn: &TurnRequest) -> Result<Vec<String>, Failure> {
        self.acp.open(turn)
    }

    fn heard(&mut self, frame: &str) -> Result<Vec<Act>, Failure> {
        if account::link(frame).is_some() {
            if self.unreachable {
                return Ok(vec![Act::Fail(Failure::new(&format!(
                    "Your own Antigravity is not signed in: under the sandbox it cannot \
                     reach the sign-in it keeps in ~/.gemini. {}",
                    own_ways(self.required)
                )))]);
            }
            return Ok(vec![Act::Fail(Failure::new(
                "The agent is not signed in. Sign in with /login, then try again.",
            ))]);
        }
        self.acp.heard(frame)
    }

    fn answer(&mut self, native: &str, decision: &Decision) -> Result<String, Failure> {
        self.acp.answer(native, decision)
    }

    fn cancel(&mut self) -> Option<String> {
        self.acp.cancel()
    }

    fn account(&mut self, step: Account, shape: Shape<'_>) -> Option<Request> {
        match step {
            Account::Version | Account::Status | Account::SignIn | Account::SignOut => {
                Some(self.request(shape))
            }
        }
    }

    fn read(&self, step: Account, ran: &Ran) -> Reading {
        self.acp.read(step, ran)
    }

    fn spoken(&mut self, step: Account) -> Option<Vec<String>> {
        self.acp.spoken(step)
    }

    fn signing_in_with(&mut self, id: &str) -> bool {
        self.acp.signing_in_with(id);
        true
    }

    fn told(&mut self, step: Account, frame: &str) -> Result<Told, Failure> {
        match account::link(frame) {
            Some(Ok(link)) => Ok(match step {
                Account::SignIn if !self.acp.reading_status() => Told::Link(link),
                // Asking to be signed in while a session is being opened says
                // the agent is signed out, after a sign-in it said took as in
                // a status step.
                Account::SignIn | Account::Status => {
                    Told::Done(Reading::Signed(Authentication::SignedOut))
                }
                // Asking to be signed in says nothing of whether a sign-out
                // took or what version is running.
                Account::SignOut | Account::Version => Told::Done(Reading::Unreadable(
                    Failure::new("The agent asked to be signed in instead of answering."),
                )),
            }),
            Some(Err(failure)) => Err(failure),
            None => self.acp.told(step, frame),
        }
    }
}

#[cfg(test)]
mod tests;
