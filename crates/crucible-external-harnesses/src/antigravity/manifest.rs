//! The releases of the agent this build was reviewed with.
//!
//! An agent is run only as a release someone read: its name and version, the
//! protocol versions it answers in, the capabilities a client relies on and
//! the platforms it was published for. An agent that describes itself
//! otherwise when it is initialized is sent nothing more, so a release that
//! changed what it does is refused until a build lists it.
//!
//! Each platform's entry is the archive the release was published in there,
//! as [`Pin`]: what crucible downloads for the user is that archive and no
//! other, and the executable it holds is the one run.

use crucible_execution::Failure;

use crate::acp::{Initialized, SignIn};
use crate::release::{Member, Pin};

/// The release reviewed, as it names itself and as its archives name it.
const VERSION: &str = "1.3.0";

/// Where the reviewed release's archives are published.
macro_rules! published {
    ($os:literal, $platform:literal) => {
        concat!(
            "https://dl.google.com/agy-extensions/releases/",
            $os,
            "/agy-acp-server-1.3.0-",
            $platform,
            ".zip"
        )
    };
}

/// The server, as each Unix archive names it, and the harness it starts.
const PAR: &str = "agy_acp_server.par";
const HARNESS: &str = "localharness_external";
/// The same two, as each Windows archive names them.
const EXE: &str = "agy_acp_server.exe";
const HARNESS_EXE: &str = "localharness_external.exe";

/// The reviewed releases of one agent.
#[derive(Debug, Clone, Copy)]
pub struct Manifest {
    /// The name the agent gives itself.
    pub name: &'static str,
    /// The versions reviewed.
    pub versions: &'static [&'static str],
    /// The protocol versions it may answer in.
    pub protocols: &'static [u64],
    /// The sign-in methods it must offer.
    pub sign_in: &'static [SignIn],
    /// The platforms it was published for, as the operating system and
    /// architecture Rust names them, with the archive published for each.
    pub platforms: &'static [(&'static str, &'static str, Pin)],
}

impl Manifest {
    /// The release this build was reviewed with.
    pub const REVIEWED: Self = Self {
        name: "antigravity-acp",
        versions: &[VERSION],
        protocols: &[1],
        sign_in: &[SignIn::named("oauth-personal")],
        platforms: &[
            (
                "macos",
                "aarch64",
                Pin {
                    version: VERSION,
                    url: published!("macos", "darwin-arm64"),
                    sha256: "7cd97045f7b4fe81175a107cdf16f9c51484e3c78a5162cae415338bb6aa5b88",
                    archive: 111_456_962,
                    executable: Member {
                        name: PAR,
                        bytes: 278_535_456,
                    },
                    companion: Member {
                        name: HARNESS,
                        bytes: 118_611_392,
                    },
                },
            ),
            (
                "macos",
                "x86_64",
                Pin {
                    version: VERSION,
                    url: published!("macos", "darwin-x86_64"),
                    sha256: "bb23956b89984bf5d354af2c3725e6c57f0cc1b7228e77a0e91c9c2bc1d47646",
                    archive: 117_245_544,
                    executable: Member {
                        name: PAR,
                        bytes: 282_840_688,
                    },
                    companion: Member {
                        name: HARNESS,
                        bytes: 124_175_392,
                    },
                },
            ),
            (
                "linux",
                "x86_64",
                Pin {
                    version: VERSION,
                    url: published!("linux", "linux-x86_64"),
                    sha256: "9fb60956af0a9d76220a4db91ca9ac88e2a2372ad68f985ab5fceace6b825b96",
                    archive: 333_727_150,
                    executable: Member {
                        name: PAR,
                        bytes: 926_533_965,
                    },
                    companion: Member {
                        name: HARNESS,
                        bytes: 130_388_040,
                    },
                },
            ),
            (
                "linux",
                "aarch64",
                Pin {
                    version: VERSION,
                    url: published!("linux", "linux-arm64"),
                    sha256: "500b0bc0fb858e88f4df404d4cedf80bf9298c178291e39e383d6c50b111cbdf",
                    archive: 321_690_363,
                    executable: Member {
                        name: PAR,
                        bytes: 930_848_992,
                    },
                    companion: Member {
                        name: HARNESS,
                        bytes: 123_224_968,
                    },
                },
            ),
            (
                "windows",
                "x86_64",
                Pin {
                    version: VERSION,
                    url: published!("windows", "windows-x86_64"),
                    sha256: "65215e0688681fa3116e048a9eab27ef53af1bbd6f3da3f1c52bd4911d8b17f9",
                    archive: 124_509_787,
                    executable: Member {
                        name: EXE,
                        bytes: 81_437_336,
                    },
                    companion: Member {
                        name: HARNESS_EXE,
                        bytes: 145_548_952,
                    },
                },
            ),
            (
                "windows",
                "aarch64",
                Pin {
                    version: VERSION,
                    url: published!("windows", "windows-arm64"),
                    sha256: "4a0f469720e9beb9438a979f543fdbfad5022ebe0992c052c590bd78b3144ca3",
                    archive: 124_654_803,
                    executable: Member {
                        name: EXE,
                        bytes: 85_893_472,
                    },
                    companion: Member {
                        name: HARNESS_EXE,
                        bytes: 135_640_216,
                    },
                },
            ),
        ],
    };

    /// The archive published for the platform `os` and `arch`, where the
    /// release was published for it.
    #[must_use]
    pub fn release(&self, os: &str, arch: &str) -> Option<&'static Pin> {
        self.platforms
            .iter()
            .find(|(published, built, _)| *published == os && *built == arch)
            .map(|(_, _, pin)| pin)
    }

    /// The archive published for the platform this build runs on.
    #[must_use]
    pub fn here(&self) -> Option<&'static Pin> {
        self.release(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// Whether the agent described as `initialized`, on the platform `os` and
    /// `arch`, is a release this manifest lists.
    ///
    /// # Errors
    ///
    /// A [`Failure`] naming the first way it is not.
    pub fn admits(&self, initialized: &Initialized, os: &str, arch: &str) -> Result<(), Failure> {
        if self.release(os, arch).is_none() {
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
