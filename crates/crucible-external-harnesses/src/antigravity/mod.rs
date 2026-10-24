//! Google's Antigravity agent, spoken to over the Agent Client Protocol in a
//! profile of its own.
//!
//! [`Antigravity`] is the protocol's client run as a release the catalog
//! reviewed: the entry keyed [`AGENT`], from the catalog most lately believed
//! or else the snapshot built in, says which versions, what each must call
//! itself and what it is started with, and the [`Manifest`] says what no entry
//! does. An agent that describes itself otherwise is sent nothing more. It is
//! started with only the environment its profile builds, so no credential the
//! shell holds for another use reaches it, and whether it is signed in is read
//! from what the agent itself answers in that profile; a sign-in another Google
//! product keeps is no evidence of its own.
//!
//! The agent asks to be signed in by printing a link rather than answering.
//! During a sign-in the link is shown to the user; during a turn or a status
//! check it means the profile is signed out, and the turn fails saying so;
//! during a sign-out or a version check it answers nothing asked.
//!
//! The agent asks about every action itself; one made [`Antigravity::allowing`]
//! answers yes to what the conversation's mode allows without asking, as
//! [`Acp::allowing`] does.

mod account;
mod manifest;
mod profile;

pub use account::Ended;
pub use manifest::Manifest;

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

/// The Antigravity agent's wire.
#[derive(Debug)]
pub struct Antigravity {
    /// The protocol's client.
    acp: Acp,
    /// The arguments it is started with.
    args: Vec<String>,
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

    /// `acp`, held to the releases `offered`.
    fn new(acp: Acp, offered: &[OwnedPin]) -> Self {
        let args = offered.first().map_or_else(Vec::new, |newest| {
            newest.target.args.iter().map(ToString::to_string).collect()
        });
        Self {
            acp: acp.listing(offered.iter().map(|pin| pin.admits.clone()).collect()),
            args,
        }
    }
}

impl Dialect for Antigravity {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        let _ = self.acp.request(shape);
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

    fn told(&mut self, step: Account, frame: &str) -> Result<Told, Failure> {
        match account::link(frame) {
            Some(Ok(link)) => Ok(match step {
                Account::SignIn => Told::Link(link),
                Account::Status => Told::Done(Reading::Signed(Authentication::SignedOut)),
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
