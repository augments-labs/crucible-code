//! `crucible update`: whether a newer release is out, and, in an install
//! `install.sh` made, putting it in place.
//!
//! **One question, one answer.** The newest release is the one the release
//! source names, and it is offered only when it is later than the release
//! that would be replaced, by [`Version`]'s order. A name that is not a
//! release number is refused rather than read past, so a prerelease, a
//! fourth part or a downgrade is never installed, and the name is never
//! printed: it is the source's text, not crucible's.
//!
//! **The source is GitHub, or a loopback address for a test.**
//! `CRUCIBLE_CODE_UPDATE_SOURCE` names another source only when it is an
//! `http://` URL on `127.0.0.1` or `[::1]`, so a value planted in the
//! environment can point an update at nothing but this machine; any other
//! value is refused before anything is asked. It is read from the process
//! environment alone. A configuration's `env` block sets the environment of
//! the commands a session runs, and is not read here.
//!
//! **Only what install.sh made is changed.** Checking reads nothing on disk.
//! A dry run and an update first take the layout the running executable
//! belongs to; a build cargo made, a copy put in place by hand and a Windows
//! install are each refused with the way they are updated instead. An update
//! takes the install's lock, compares again with the release active under
//! it, downloads `SHA256SUMS` and the archive into a directory of its own,
//! stages and activates the release through [`crate::RecoverableActivation`],
//! and then runs the new `crucible --version`; a release that does not say it
//! is the version it was installed as is rolled back.

#[cfg(unix)]
mod apply;
#[cfg(unix)]
use apply::replace;

use std::ffi::OsStr;

use crucible_http::{Http, Lookups, PlainLookups, ProxyEnv};
use crucible_runtime::Cancel;
use tokio::runtime::Handle;

use crate::release::{LATEST, asked_at};
use crate::{UpdateCrateReleaseCheck, Version};

/// The environment variable that names a loopback release source.
pub const SOURCE: &str = "CRUCIBLE_CODE_UPDATE_SOURCE";

/// Where GitHub serves a release's files, before `v<version>/<name>`.
const DOWNLOADS: &str = "https://github.com/augments-labs/crucible-code/releases/download";

/// What the person asked `crucible update` to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Asked {
    /// Say whether a newer release is out, and change nothing.
    Check,
    /// Say what an update would install, and change nothing.
    DryRun,
    /// Install the newer release and make it the active one.
    Apply,
}

/// One `crucible update`, as the command line gave it.
#[derive(Debug, Clone, Copy)]
pub struct SelfUpdateCommand<'a> {
    /// What was asked.
    pub asked: Asked,
    /// `CRUCIBLE_CODE_UPDATE_SOURCE` as the process environment holds it,
    /// where it is set.
    pub source: Option<&'a OsStr>,
    /// The executable that is running, as the operating system names it.
    pub executable: &'a std::path::Path,
    /// The release this build is.
    pub running: &'a str,
}

/// What came of an update that was not refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// Nothing later than `running` is out.
    Current {
        /// The release that would have been replaced.
        running: Version,
    },
    /// `newest` is out and later than `running`; nothing was changed.
    Available {
        /// The release running.
        running: Version,
        /// The newest release.
        newest: Version,
    },
    /// An update would install `newest` in place of `running`.
    Proposed {
        /// The release that would be replaced.
        running: Version,
        /// The release it would install.
        newest: Version,
    },
    /// `to` is installed and active in place of `from`.
    Updated {
        /// The release that was active.
        from: Version,
        /// The release now active.
        to: Version,
    },
}

impl Answer {
    /// The exit status that says this answer: 3 when a check found a newer
    /// release, so a script can tell it from being current, 0 otherwise.
    #[must_use]
    pub fn exit(&self) -> u8 {
        match self {
            Self::Available { .. } => 3,
            Self::Current { .. } | Self::Proposed { .. } | Self::Updated { .. } => 0,
        }
    }
}

impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Current { running } => write!(f, "crucible {running} is the newest release"),
            Self::Available { running, newest } => write!(
                f,
                "crucible {newest} is out; this is {running}. `crucible update` installs it"
            ),
            Self::Proposed { running, newest } => write!(
                f,
                "crucible update would install {newest} in place of {running}; nothing was changed"
            ),
            Self::Updated { from, to } => write!(
                f,
                "crucible {to} is installed and active in place of {from}"
            ),
        }
    }
}

/// Why `crucible update` changed nothing, or what it put back.
///
/// Each says what to do about it in its own words. A refusal that has a typed
/// cause keeps it as its source, for a caller to inspect; that cause can hold
/// text from an archive or a response, a member name among it, so what a
/// person is shown is this `Display` alone, never the chain behind it.
#[derive(Debug, thiserror::Error)]
pub enum Refused {
    /// The release source named is not a loopback `http://` URL.
    #[error(
        "{SOURCE} may only name an http:// URL on 127.0.0.1 or [::1]; unset it to update from GitHub"
    )]
    Source,
    /// This build's own version is not a release number.
    #[error("this build of crucible is not a release, so there is nothing to compare it with")]
    Running,
    /// This crucible was built by cargo.
    #[error(
        "this crucible was built by cargo, not installed by install.sh: update it with `cargo \
         install` or by building it again, or install a release with install.sh, which `crucible \
         update` can then keep current"
    )]
    Cargo,
    /// Windows installs are updated by their own installer.
    #[error("on Windows, crucible is updated by running install.ps1 again")]
    Windows,
    /// This crucible is not in a layout install.sh made.
    #[error(
        "this crucible was not installed by install.sh, so it cannot replace itself: install a \
         release with install.sh, which `crucible update` can then keep current"
    )]
    Unmanaged,
    /// The install's layout is not one an update may change.
    #[cfg(unix)]
    #[error("the install cannot be updated: {0}")]
    Layout(#[source] crate::LayoutError),
    /// The release source did not say which release is newest.
    #[error("could not ask the release source which release is newest")]
    Unreachable,
    /// The release source named something that is not a release number.
    #[error("the newest release the source names is not a release number, so nothing is installed")]
    NotStable,
    /// A file of the release could not be downloaded whole.
    #[error("could not download {name} from the release source")]
    Download {
        /// The file, as the release names it.
        name: String,
    },
    /// The directory the download goes into could not be made.
    #[error("could not make a private directory to download the release into")]
    Scratch,
    /// The release did not stage; the active release is unchanged.
    #[cfg(unix)]
    #[error("the release was not installed: {0}")]
    Stage(#[source] crate::StageError),
    /// The active release has a sandbox broker and the new one has none.
    #[error(
        "the new release holds no crucible-sandbox-broker while the active one does, so it was not \
         installed"
    )]
    Broker,
    /// The release did not become active, or did without being synced.
    #[cfg(unix)]
    #[error("the release was not made active: {0}")]
    Activation(#[source] crate::ActivationError),
    /// The new release did not say it was `version`, and was rolled back.
    #[error(
        "crucible {version} did not run as the release it was installed as, so the release before \
         it is active again"
    )]
    RolledBack {
        /// The release that was rolled back.
        version: Version,
    },
    /// The new release did not run as it should and could not be rolled back.
    #[cfg(unix)]
    #[error(
        "the new release did not run as the release it was installed as, and the one before could \
         not be made active again: {0}"
    )]
    RollBackFailed(#[source] crate::ActivationError),
}

/// Where the release is asked for and downloaded from.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Source {
    /// GitHub, through the release check's own client.
    GitHub,
    /// A loopback URL, without a trailing `/`.
    Loopback(Box<str>),
}

impl Source {
    /// The source `named` names, or GitHub where it names none.
    fn named(named: Option<&OsStr>) -> Result<Self, Refused> {
        let Some(named) = named else {
            return Ok(Self::GitHub);
        };
        let text = named.to_str().ok_or(Refused::Source)?;
        let uri: http::Uri = text.parse().map_err(|_| Refused::Source)?;
        let authority = uri.authority().ok_or(Refused::Source)?;
        let loopback = matches!(authority.host(), "127.0.0.1" | "[::1]");
        if uri.scheme_str() != Some("http")
            || !loopback
            || authority.as_str().contains('@')
            || uri.query().is_some()
            || text.contains('#')
        {
            return Err(Refused::Source);
        }
        Ok(Self::Loopback(text.trim_end_matches('/').into()))
    }

    /// Where the newest release is named.
    fn latest(&self) -> String {
        match self {
            Self::GitHub => LATEST.to_owned(),
            Self::Loopback(base) => format!("{base}/releases/latest"),
        }
    }

    /// Where `version`'s file `name` is.
    fn download(&self, version: &Version, name: &str) -> String {
        match self {
            Self::GitHub => format!("{DOWNLOADS}/v{version}/{name}"),
            Self::Loopback(base) => format!("{base}/releases/download/v{version}/{name}"),
        }
    }
}

impl UpdateCrateReleaseCheck {
    /// Runs `command`, asking its release source on `runtime`.
    ///
    /// # Errors
    ///
    /// [`Refused`] naming why nothing was installed, or what was put back.
    pub fn update(
        &self,
        runtime: &Handle,
        command: &SelfUpdateCommand<'_>,
    ) -> Result<Answer, Refused> {
        let source = Source::named(command.source)?;
        let running = Version::parse(command.running.as_bytes()).ok_or(Refused::Running)?;
        let http = self.source_client(&source).ok_or(Refused::Unreachable)?;
        match command.asked {
            Asked::Check => {
                let newest = newest(runtime, &http, &source)?;
                Ok(if later(&newest, &running) {
                    Answer::Available { running, newest }
                } else {
                    Answer::Current { running }
                })
            }
            Asked::DryRun | Asked::Apply => replace(runtime, &http, &source, command),
        }
    }

    /// The client `source` is asked through: the release check's own for
    /// GitHub, and one that dials the loopback address itself, past any proxy
    /// the environment names, for a loopback source.
    fn source_client(&self, source: &Source) -> Option<Http> {
        match source {
            Source::GitHub => self.release_client().cloned(),
            Source::Loopback(_) => {
                let plain = Lookups::plain(std::num::NonZeroUsize::MIN);
                let targets: Lookups = PlainLookups::clone(&plain).into();
                Some(Http::new(
                    self.tls()?,
                    targets,
                    plain,
                    ProxyEnv::read(|_| None),
                ))
            }
        }
    }
}

/// What an update would do on a platform whose installs it does not manage.
#[cfg(not(unix))]
fn replace(
    _runtime: &Handle,
    _http: &Http,
    _source: &Source,
    _command: &SelfUpdateCommand<'_>,
) -> Result<Answer, Refused> {
    Err(Refused::Windows)
}

/// Whether `newest` is a release to put in place of `running`: only one
/// later than it, so an update never goes back.
fn later(newest: &Version, running: &Version) -> bool {
    newest > running
}

/// The newest release the source names.
fn newest(runtime: &Handle, http: &Http, source: &Source) -> Result<Version, Refused> {
    let named = runtime
        .block_on(asked_at(http, &source.latest(), &Cancel::new()))
        .ok_or(Refused::Unreachable)?;
    Version::parse(named.as_bytes()).ok_or(Refused::NotStable)
}

#[cfg(test)]
mod tests;
