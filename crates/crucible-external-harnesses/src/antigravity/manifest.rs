//! What this build knows of the agent that no catalog entry says.
//!
//! An agent is run only as a release someone read: which releases those are,
//! with the name each answers initializing with and the archive each was
//! published in, is the catalog entry keyed `antigravity`, from the catalog
//! the trust state most lately believed or else the snapshot built into the
//! binary. What stays here is what a release must hold to be run by this
//! client at all: the protocol versions it answers in, the capabilities this
//! client relies on and the sign-in method it must offer. An agent that
//! describes itself otherwise when it is initialized is sent nothing more.

use crucible_execution::Failure;

use crate::acp::{Initialized, SignIn};
use crate::catalog::Admits;

/// What a release of the agent must hold to be run.
#[derive(Debug, Clone, Copy)]
pub struct Manifest {
    /// The protocol versions it may answer in.
    pub protocols: &'static [u64],
    /// The sign-in methods it must offer.
    pub sign_in: &'static [SignIn],
}

impl Manifest {
    /// What this build was reviewed with.
    pub const REVIEWED: Self = Self {
        protocols: &[1],
        sign_in: &[SignIn::named("oauth-personal")],
    };

    /// Whether the agent described as `initialized` is one of the releases
    /// `offered`, each as the name and version its entry says it answers with.
    ///
    /// # Errors
    ///
    /// A [`Failure`] naming the first way it is not.
    pub fn admits(&self, initialized: &Initialized, offered: &[Admits]) -> Result<(), Failure> {
        let Some(first) = offered.first() else {
            return Err(Failure::new(&format!(
                "This agent is offered no release for {} on {}.",
                std::env::consts::OS,
                std::env::consts::ARCH
            )));
        };
        let named: Vec<&Admits> = offered
            .iter()
            .filter(|one| initialized.name() == Some(&*one.name))
            .collect();
        if named.is_empty() {
            return Err(Failure::new(&format!(
                "The program is not {}: it calls itself {}.",
                first.name,
                initialized.name().unwrap_or("nothing")
            )));
        }
        let version = initialized.version().unwrap_or("unknown");
        if !named.iter().any(|one| *one.version == *version) {
            let versions: Vec<&str> = named.iter().map(|one| &*one.version).collect();
            return Err(Failure::new(&format!(
                "Version {version} of the agent was not reviewed. The versions reviewed are {}.",
                versions.join(", ")
            )));
        }
        if !self.protocols.contains(&initialized.protocol()) {
            return Err(Failure::new(
                "The agent answered in a protocol version its release does not use.",
            ));
        }
        let missing = [
            (initialized.loads(), "loading a session"),
            (initialized.resumes(), "resuming a session"),
            (initialized.signs_out(), "signing out"),
            (
                self.sign_in
                    .iter()
                    .all(|method| initialized.offers(method.id())),
                "its usual sign-in",
            ),
        ]
        .into_iter()
        .find_map(|(offered, what)| (!offered).then_some(what));
        match missing {
            Some(what) => Err(Failure::new(&format!(
                "The agent no longer offers {what}, which its reviewed release did."
            ))),
            None => Ok(()),
        }
    }
}
