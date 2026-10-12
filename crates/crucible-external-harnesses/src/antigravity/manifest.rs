//! The releases of the agent this build was reviewed with.
//!
//! An agent is run only as a release someone read: its name and version, the
//! protocol versions it answers in, the capabilities a client relies on and
//! the platforms it was published for. An agent that describes itself
//! otherwise when it is initialized is sent nothing more, so a release that
//! changed what it does is refused until a build lists it.

use crucible_execution::Failure;

use crate::acp::Initialized;

/// The reviewed releases of one agent.
#[derive(Debug, Clone, Copy)]
pub struct Manifest {
    /// The name the agent gives itself.
    pub name: &'static str,
    /// The versions reviewed.
    pub versions: &'static [&'static str],
    /// The protocol versions it may answer in.
    pub protocols: &'static [u64],
    /// The sign-in method it must offer.
    pub sign_in: &'static str,
    /// The platforms it was published for, as the operating system and
    /// architecture Rust names them, with the executable's file name on each.
    pub platforms: &'static [(&'static str, &'static str, &'static str)],
}

impl Manifest {
    /// The release this build was reviewed with.
    pub const REVIEWED: Self = Self {
        name: "antigravity-acp",
        versions: &["1.3.0"],
        protocols: &[1],
        sign_in: "oauth-personal",
        platforms: &[
            ("macos", "aarch64", "agy_acp_server.par"),
            ("macos", "x86_64", "agy_acp_server.par"),
            ("linux", "x86_64", "agy_acp_server.par"),
            ("linux", "aarch64", "agy_acp_server.par"),
            ("windows", "x86_64", "agy_acp_server.exe"),
            ("windows", "aarch64", "agy_acp_server.exe"),
        ],
    };

    /// The executable's file name on the platform `os` and `arch`, where it
    /// was published for it.
    #[must_use]
    pub fn executable(&self, os: &str, arch: &str) -> Option<&'static str> {
        self.platforms
            .iter()
            .find(|(published, built, _)| *published == os && *built == arch)
            .map(|(_, _, executable)| *executable)
    }

    /// Whether the agent described as `initialized`, on the platform `os` and
    /// `arch`, is a release this manifest lists.
    ///
    /// # Errors
    ///
    /// A [`Failure`] naming the first way it is not.
    pub fn admits(&self, initialized: &Initialized, os: &str, arch: &str) -> Result<(), Failure> {
        if self.executable(os, arch).is_none() {
            return Err(Failure::new(&format!(
                "This agent was not published for {os} on {arch}."
            )));
        }
        if initialized.name() != Some(self.name) {
            return Err(Failure::new(&format!(
                "The program is not {}: it calls itself {}.",
                self.name,
                initialized.name().unwrap_or("nothing")
            )));
        }
        let version = initialized.version().unwrap_or("unknown");
        if !self.versions.contains(&version) {
            return Err(Failure::new(&format!(
                "Version {version} of the agent was not reviewed for this release of Crucible, \
                 which runs {}.",
                self.versions.join(", ")
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
            (initialized.offers(self.sign_in), "its usual sign-in"),
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
