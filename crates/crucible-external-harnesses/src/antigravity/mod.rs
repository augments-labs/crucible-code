//! Google's Antigravity agent, spoken to over the Agent Client Protocol in a
//! profile of its own.
//!
//! [`Antigravity`] is the protocol's client run as one reviewed release: the
//! [`Manifest`] says which, and an agent that describes itself otherwise is
//! sent nothing more. It is started with only the environment its profile
//! builds, so no credential the shell holds for another use reaches it, and
//! whether it is signed in is read from what the agent itself answers in that
//! profile; a sign-in another Google product keeps is no evidence of its own.
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

use crate::acp::{Acp, Allowing, Initialized, Profile};
use crate::{Account, Act, Dialect, Ran, Reading, Request, Shape, Told};

/// The command it is looked up as.
const PROGRAM: &str = "antigravity";

/// The protocol's client, as this release of the agent is run.
const PROFILE: Profile = Profile {
    program: PROGRAM,
    versions: Manifest::REVIEWED.protocols,
    admits,
    sign_in: Some(Manifest::REVIEWED.sign_in),
};

/// Whether the agent described as `initialized` is the reviewed release, on
/// this platform.
fn admits(initialized: &Initialized) -> Result<(), Failure> {
    Manifest::REVIEWED.admits(initialized, std::env::consts::OS, std::env::consts::ARCH)
}

/// The arguments it is started with: on Linux an empty user id, as its
/// published integration starts it there.
fn arguments() -> Vec<String> {
    if cfg!(target_os = "linux") {
        vec!["--uid=".to_owned()]
    } else {
        Vec::new()
    }
}

/// The Antigravity agent's wire.
#[derive(Debug)]
pub struct Antigravity {
    /// The protocol's client.
    acp: Acp,
}

impl Default for Antigravity {
    fn default() -> Self {
        Self {
            acp: Acp::new(PROFILE),
        }
    }
}

impl Antigravity {
    /// The wire answering yes by itself to what `allowing` names.
    #[must_use]
    pub fn allowing(allowing: Allowing) -> Self {
        Self {
            acp: Acp::allowing(PROFILE, allowing),
        }
    }
}

impl Dialect for Antigravity {
    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn request(&mut self, shape: Shape<'_>) -> Request {
        let _ = self.acp.request(shape);
        Request {
            arguments: arguments(),
            environment: profile::environment(std::env::vars_os(), shape.profile),
        }
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
            // The models a session offers are not read as an account step.
            Account::Models => None,
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
                Account::SignOut | Account::Version | Account::Models => {
                    Told::Done(Reading::Unreadable(Failure::new(
                        "The agent asked to be signed in instead of answering.",
                    )))
                }
            }),
            Some(Err(failure)) => Err(failure),
            None => self.acp.told(step, frame),
        }
    }
}

#[cfg(test)]
mod tests;
