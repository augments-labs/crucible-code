//! What starting another vendor's agent is made of, settled before it starts.
//!
//! A [`Launch`] is every value one start needs, written out as plain values by
//! the composition root: the program, which was found by an absolute lookup
//! and is identified by what it was when it was selected; the arguments and
//! the environment, which start from nothing and hold only what the dialect
//! names; the sandbox policy the child runs under; the session to resume and
//! the profile directory, where there are any; and a [`Held`], which says the
//! route's content-use hold answered yes.
//!
//! A `Held` is taken by value and is neither `Clone` nor `Copy`, so it starts
//! at most one process: the launch that carries it is consumed by the one call
//! that starts it. It is built by [`Held::granted`] alone, and the repository
//! check names the one shipped file allowed to call that, the file that asked
//! the route's hold.
//!
//! Finding a program is a look at the filesystem and runs nothing: a program
//! is code, and even asking it its version is a launch. [`Discovery`] says
//! whether the program is there, absent, or there and not executable, with the
//! sentences each is refused with.

use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use crucible_execution::Confinement;
use crucible_sandbox::SandboxPolicy;
use sha2::{Digest, Sha256};

/// The largest executable whose bytes are digested for its identity. A larger
/// one is identified by its size and modification time alone, because reading
/// a few hundred megabytes before every start would be a cost paid on every
/// turn.
pub const DIGESTED: u64 = 64 * 1024 * 1024;

/// The most bytes a resume reference keeps, which is the session reference
/// the execution contract allows.
pub const RESUME_BYTES: usize = 256;

/// Whether `name` can be kept as a program's name for its session and handed
/// back to it as an argument: not empty, no longer than [`RESUME_BYTES`], not
/// read as an option, and made only of ASCII letters, digits and `-_.:`, so
/// that nothing a shell, a terminal or a line reader acts on is in it.
pub(crate) fn nameable(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= RESUME_BYTES
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:".contains(&byte))
}

/// The content-use hold for one route, answered yes.
///
/// ```compile_fail
/// # use crucible_external_harnesses::Held;
/// let held = Held::granted("agent:example");
/// let again = held.clone();
/// ```
///
/// ```compile_fail
/// # use crucible_external_harnesses::Held;
/// let held = Held::granted("agent:example");
/// let first = held;
/// let second = held;
/// ```
pub struct Held {
    /// The route that answered.
    route: Box<str>,
}

impl Held {
    /// The hold for `route`, which its owner has asked and been answered yes.
    #[must_use]
    pub fn granted(route: &str) -> Self {
        Self {
            route: route.into(),
        }
    }

    /// The route that answered.
    #[must_use]
    pub fn route(&self) -> &str {
        &self.route
    }
}

impl fmt::Debug for Held {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Held")
            .field("route", &self.route)
            .finish()
    }
}

/// What a file looked like when it was selected.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    /// Its length.
    size: u64,
    /// When it was last written, where the platform says.
    modified: Option<SystemTime>,
    /// A digest of its bytes, where it is no larger than [`DIGESTED`].
    digest: Option<[u8; 32]>,
}

impl Identity {
    /// What `path` is now.
    fn of(path: &Path) -> io::Result<Self> {
        let metadata = fs::metadata(path)?;
        let size = metadata.len();
        let digest = if size <= DIGESTED {
            let mut file = File::open(path)?.take(DIGESTED);
            let mut hasher = Sha256::new();
            let mut buffer = vec![0_u8; 64 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                let Some(chunk) = buffer.get(..read) else {
                    break;
                };
                if chunk.is_empty() {
                    break;
                }
                hasher.update(chunk);
            }
            Some(hasher.finalize().into())
        } else {
            None
        };
        Ok(Self {
            size,
            modified: metadata.modified().ok(),
            digest,
        })
    }
}

/// A program found where it was looked for, and what it was then.
#[derive(Debug, Clone)]
pub struct Program {
    /// Where it is: absolute.
    path: PathBuf,
    /// What it was when it was found.
    identity: Identity,
}

impl Program {
    /// Where it is.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Refuses when the file is no longer the one that was found.
    ///
    /// # Errors
    ///
    /// [`Refusal::Changed`] when its size, modification time or digest moved,
    /// or it can no longer be read.
    pub fn unchanged(&self) -> Result<(), Refusal> {
        match Identity::of(&self.path) {
            Ok(now) if now == self.identity => Ok(()),
            Ok(_) | Err(_) => Err(Refusal::Changed(self.path.clone())),
        }
    }
}

/// What looking for an agent's program found. Nothing was run to find it.
#[derive(Debug, Clone)]
pub enum Discovery {
    /// It is there, executable, and this is what it was.
    Found(Program),
    /// Nothing is there.
    Missing,
    /// Something is there that cannot be run.
    NotExecutable(PathBuf),
}

impl Discovery {
    /// Looks at `candidate`: the configured executable when it is absolute,
    /// else what an absolute lookup on `PATH` found, else nothing.
    #[must_use]
    pub fn of(candidate: Option<&Path>) -> Self {
        let Some(path) = candidate.filter(|path| path.is_absolute()) else {
            return Self::Missing;
        };
        let Ok(metadata) = fs::metadata(path) else {
            return Self::Missing;
        };
        if !metadata.is_file() || !executable(&metadata) {
            return Self::NotExecutable(path.to_path_buf());
        }
        match Identity::of(path) {
            Ok(identity) => Self::Found(Program {
                path: path.to_path_buf(),
                identity,
            }),
            Err(_) => Self::NotExecutable(path.to_path_buf()),
        }
    }

    /// What a refusal over this says, as the sentence and what to do next, for
    /// the agent called `agent` whose command is `command`. Nothing for a
    /// program that was found.
    #[must_use]
    pub fn refusal(&self, agent: &str, command: &str) -> Option<[String; 2]> {
        match self {
            Self::Found(_) => None,
            Self::Missing => Some([
                format!(
                    "Not installed: no {command} command on PATH and none at the configured executable."
                ),
                format!("Install {agent}, then choose Check again."),
            ]),
            Self::NotExecutable(path) => Some([
                format!("Found {} but it is not executable.", path.display()),
                "Make it executable, then choose Check again.".to_owned(),
            ]),
        }
    }
}

/// Whether the platform would run this file.
#[cfg(unix)]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

/// Whether the platform would run this file: on Windows that is its name,
/// which the lookup already chose, so any file is.
#[cfg(not(unix))]
const fn executable(_metadata: &fs::Metadata) -> bool {
    true
}

/// What the dialect asks a process to be started with. Printed for debugging,
/// it names each environment variable and withholds its value, which can be a
/// credential.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Request {
    /// The argument vector, without the program.
    pub arguments: Vec<String>,
    /// The environment, which starts from nothing.
    pub environment: Vec<(String, String)>,
}

impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names = self
            .environment
            .iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        f.debug_struct("Request")
            .field("arguments", &self.arguments)
            .field("environment", &names)
            .finish()
    }
}

/// Everything one start of an agent's program needs, settled before it starts.
///
/// A launch with no hold does not compile:
///
/// ```compile_fail
/// # use crucible_external_harnesses::{Launch, Program};
/// # use crucible_sandbox::SandboxPolicy;
/// # fn start(program: Program, policy: SandboxPolicy) {
/// let launch = Launch::new(program, policy);
/// # }
/// ```
#[derive(Debug)]
pub struct Launch {
    /// The program, as it was found.
    program: Program,
    /// What the dialect asked it to be started with.
    request: Request,
    /// The confinement it runs under.
    policy: SandboxPolicy,
    /// The session to resume, where there is one.
    resume: Option<Box<str>>,
    /// The profile directory, where the route has one.
    profile: Option<PathBuf>,
    /// The silence a working turn sits through.
    patience: Duration,
    /// The route's content-use hold, answered yes.
    held: Held,
}

impl Launch {
    /// `program` under `policy`, which the route's hold `held` let start, with
    /// nothing asked of it yet.
    #[must_use]
    pub const fn new(program: Program, policy: SandboxPolicy, held: Held) -> Self {
        Self {
            program,
            request: Request {
                arguments: Vec::new(),
                environment: Vec::new(),
            },
            policy,
            resume: None,
            profile: None,
            patience: crate::host::SILENCE,
            held,
        }
    }

    /// Resuming the session `reference` names.
    ///
    /// # Errors
    ///
    /// [`Refusal::Resume`] when it could not be kept as a session's name.
    pub fn resuming(mut self, reference: &str) -> Result<Self, Refusal> {
        if !nameable(reference) {
            return Err(Refusal::Resume);
        }
        self.resume = Some(reference.into());
        Ok(self)
    }

    /// With the profile directory `directory`.
    #[must_use]
    pub fn in_profile(mut self, directory: PathBuf) -> Self {
        self.profile = Some(directory);
        self
    }

    /// Started with what `request` asks.
    #[must_use]
    pub fn asking(mut self, request: Request) -> Self {
        self.request = request;
        self
    }

    /// Given up on after `patience` of silence while working, rather than
    /// after [`SILENCE`](crate::SILENCE).
    #[must_use]
    pub const fn waiting(mut self, patience: Duration) -> Self {
        self.patience = patience;
        self
    }

    /// The silence a working turn sits through.
    #[must_use]
    pub const fn patience(&self) -> Duration {
        self.patience
    }

    /// The program.
    #[must_use]
    pub const fn program(&self) -> &Program {
        &self.program
    }

    /// What it is started with.
    #[must_use]
    pub const fn request(&self) -> &Request {
        &self.request
    }

    /// The confinement it runs under.
    #[must_use]
    pub const fn policy(&self) -> &SandboxPolicy {
        &self.policy
    }

    /// The session it resumes, where it resumes one.
    #[must_use]
    pub fn resume(&self) -> Option<&str> {
        self.resume.as_deref()
    }

    /// The profile directory, where the route has one.
    #[must_use]
    pub fn profile(&self) -> Option<&Path> {
        self.profile.as_deref()
    }

    /// The route whose hold let it start.
    #[must_use]
    pub fn route(&self) -> &str {
        self.held.route()
    }
}

/// Why a launch was refused before anything ran.
#[derive(Debug, thiserror::Error)]
pub enum Refusal {
    /// The program is not the one selected.
    #[error("{} changed after it was selected; choose Check again to use it", .0.display())]
    Changed(PathBuf),
    /// A sandbox is required and this one cannot be enforced.
    #[error(
        "a sandbox is required, and the most this agent could be run under is {}",
        words(*.0)
    )]
    Unconfined(Confinement),
    /// The resume reference is empty or too long.
    #[error("the session to resume is not one this agent could name")]
    Resume,
    /// A process is already running for this host.
    #[error("this agent is already running")]
    Busy,
    /// This process has started the agent as many times as it can number.
    #[error("crucible has started this agent as many times as it can; restart crucible")]
    Spent,
    /// The sandbox would not start it.
    #[error("the sandbox would not start this agent: {0}")]
    Sandbox(#[from] crucible_sandbox::SandboxError),
    /// It started without the pipes it is spoken to over, and was stopped.
    #[error("this agent started without a pipe to speak to it over, and was stopped")]
    Unspoken,
}

/// A confinement claim, in a sentence's words.
const fn words(claim: Confinement) -> &'static str {
    match claim {
        Confinement::Unsupported => "no confinement at all",
        Confinement::Observed => "confinement that is observed but not enforced",
        Confinement::HardEnforced => "enforced confinement",
    }
}
