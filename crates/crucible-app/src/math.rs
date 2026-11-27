//! Display math made into a picture by the TeX installed on this machine,
//! inside the sandbox, and the one check that says whether it can be.
//!
//! TeX is an interpreter, and what it is given here is model text, so every
//! run is a hostile program and is confined as one. Three things hold:
//!
//! - **Only the check's value starts a run.** [`available`] is the session's
//!   availability check. It prepares the exact policy a run gets, refuses an
//!   unconfined session, and runs `latex --version` and `dvisvgm --version`
//!   inside it. What it returns, a [`TexEngine`], has no public constructor,
//!   so nothing reaches TeX without having passed it, and the engine keeps the
//!   service the check verified, so a run is never prepared on another one.
//! - **No host root is writable.** A run sees the TeX tree (`/etc/texmf` and
//!   `/var/lib/texmf`, beside the backend's own read-only system mounts) and
//!   writes only into the sandbox's own scratch, one sized root holding every
//!   byte written at most [`DISK_BYTES`], each file under the wrapper's
//!   `ulimit -f`. The request carries an empty manifest, the network is
//!   closed, and the formula goes in only as the content of a file inside a
//!   fixed preamble, read from standard input by a fixed script: it is never
//!   an argument, never a path and never a part of the script.
//! - **Nothing is journaled.** A run's audit facts go to the request's own
//!   collector, made fresh with the request and dropped with it; the session
//!   journal takes sandbox facts only for an admitted tool call, and a picture
//!   is not one. Nothing a run writes reaches the terminal: its standard
//!   output is read, bounded, as the picture, and its standard error is read,
//!   bounded, and thrown away.
//!
//! # One thread per run
//!
//! Every run, the check's two and each formula's, is done whole by one named
//! thread of this module's own, which drives the sandbox's futures with the
//! runtime handle's `block_on`; neither a runtime worker nor a blocking thread
//! is spent, and the task that asked does no synchronous sandbox work. The
//! thread prepares, checks confinement, materializes and stages. Anything but
//! a staged launch is dropped, with everything it held, before that outcome
//! is sent on a first one-shot channel, its final message. A staged launch is
//! announced on that channel first: either the asker took the message and the
//! thread goes on, or the send failed and the thread drops the launch. It then
//! releases, writes the formula, waits, reads under the output limits and
//! stops, drops everything it held, and sends the outcome on a second channel,
//! its final message.
//!
//! The asker waits [`STAGING`] for the first message and, from "staged", the
//! run's command time and [`MARGIN`] for the second. A thread whose final
//! message came in time is joined, which waits only for its return. One the
//! asker stopped waiting on is late: until it has returned, no check and no
//! formula run starts on the same service, because what it still holds may be
//! the sandbox slot the next run would need.
//!
//! Math pictures are made on Linux only. Elsewhere the check answers from the
//! platform's own backend, which cannot hold this policy, and runs no TeX.

use std::fmt;
use std::io::Write as _;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxCommand, SandboxEnvironment, SandboxError, SandboxFeature, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxLaunch, SandboxManifest,
    SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead,
    SandboxRequest, SandboxResourceLimits, SandboxService, SandboxViolation,
};
use crucible_types::{Ancestry, SandboxId, ToolId};
use tokio::runtime::Handle;
use tokio::sync::oneshot;

#[cfg(test)]
mod tests;

/// TeX's configuration, as Debian-family packages install it: read-only in
/// every run, and the run's working directory.
const TEXMF_CONFIG: &str = "/etc/texmf";

/// TeX's generated state, the formats among it, as the same packages install
/// it: read-only in every run.
const TEXMF_VAR: &str = "/var/lib/texmf";

/// How long the asker waits for a run's first message, from the run's start:
/// preparing, checking, materializing and staging, which measured under
/// 70 ms on a machine with Bubblewrap, take far less on any machine that can
/// run TeX at all.
pub const STAGING: Duration = Duration::from_secs(4);

/// One formula's command time, which is also the time the check's two
/// version runs share.
pub const COMMAND_TIME: Duration = Duration::from_secs(8);

/// How long past a run's command time, from "staged", the asker waits for the
/// run's last word before it calls the run timed out.
///
/// The backend's own bounds on a stop that follows a run past its time are
/// the Linux broker's cancel grace (5 s), the wait for the stopped leader to
/// be reaped (1 s) and the wait for the cancel to end (250 ms); this is their
/// sum and 750 ms for the reads and drops after them. A test compares it with
/// the backend's values, so a change to any of them is seen there.
pub const MARGIN: Duration = Duration::from_secs(7);

/// The least command time the second version run is prepared with: with
/// less left of the check's time, the check has timed out, and no run is
/// built with no time to run in.
const FLOOR: Duration = Duration::from_secs(1);

/// The longest formula a run is given, which is the longest display formula
/// the transcript converts.
pub const FORMULA_BYTES: usize = 4 * 1024;

/// The most standard output kept from one run: the picture.
pub const STDOUT_BYTES: usize = 128 * 1024;

/// The most standard error read from one run, which is thrown away.
pub const STDERR_BYTES: usize = 16 * 1024;

/// Every byte a run may hold written, in every file and directory together.
pub const DISK_BYTES: u64 = 8 * 1024 * 1024;

/// The memory a run may use.
const MEMORY_BYTES: u64 = 512 * 1024 * 1024;

/// The processes a run may have at once.
const PROCESSES: u64 = 16;

/// The longest version line kept from either program.
const VERSION_BYTES: usize = 120;

/// How often a run's thread looks at its output and its leader.
const POLL: Duration = Duration::from_millis(5);

/// How long past its command time a run's thread reads and waits before it
/// stops the run itself, should the backend not have.
const OVERRUN: Duration = Duration::from_secs(1);

/// The script every formula is run by. Fixed: the formula is its standard
/// input, never a part of it.
///
/// `ulimit -f 4096` bounds every file a run writes at 2 MiB (512-byte
/// blocks). The formula is read, at most 4 KiB of it, into a file inside the
/// fixed preamble; a 16000 pt text height and width keep a tall formula on one
/// page. `latex` runs without shell escape and stops at its first error, its
/// own words thrown away; `dvisvgm` writes the picture, its glyphs as paths,
/// to standard output.
const WRAPPER: &str = r"ulimit -f 4096 || exit 125
cd /tmp || exit 125
{
printf '%s\n' '\documentclass{article}' '\pagestyle{empty}' '\usepackage{amsmath,amssymb}' '\setlength{\textheight}{16000pt}\setlength{\textwidth}{16000pt}' '\begin{document}' '\noindent \[\displaystyle'
head -c 4096
printf '\n%s\n' '\]' '\end{document}'
} > formula.tex || exit 125
latex -no-shell-escape -interaction=nonstopmode -halt-on-error formula.tex > /dev/null 2>&1 || exit 1
exec dvisvgm --no-fonts --no-specials -b min --stdout formula.dvi
";

/// What a formula's run is given as its environment: the system's programs,
/// and TeX told to open files only beneath its working directory and to write
/// only into the scratch the script runs in.
const ENVIRONMENT: [(&str, &str); 4] = [
    ("PATH", "/usr/bin:/bin"),
    ("openin_any", "p"),
    ("openout_any", "p"),
    ("TEXMFOUTPUT", "/tmp"),
];

/// [`ENVIRONMENT`], as a sandbox command takes it.
fn environment() -> Result<SandboxEnvironment, SandboxError> {
    SandboxEnvironment::new(
        ENVIRONMENT
            .iter()
            .map(|(name, value)| (*name, std::ffi::OsStr::new(value))),
    )
}

/// A picture of a display formula, as `dvisvgm` wrote it.
#[derive(Clone, PartialEq, Eq)]
pub struct Svg(Box<[u8]>);

impl Svg {
    /// A picture whose document is `bytes`.
    pub fn new(bytes: impl Into<Box<[u8]>>) -> Self {
        Self(bytes.into())
    }

    /// The document.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Svg {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "Svg({} bytes)", self.0.len())
    }
}

/// Why math pictures cannot be made in this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// No sandbox backend runs here.
    NoBackend,
    /// The backend cannot hold one of the limits a run is given.
    Refused(SandboxFeature),
    /// The backend refused the run for a reason of its own.
    SandboxRefused,
    /// The sandbox would run TeX unconfined.
    Unconfined,
    /// `latex` or `dvisvgm`, or the TeX tree they read, is not installed.
    ProgramMissing,
    /// The check did not finish in its time.
    TimedOut,
    /// The check never reached the version runs: the sandbox was busy, could
    /// not start a run, or did not answer in time.
    NotReached,
}

impl Unavailable {
    /// Whether this outcome says nothing about the session, so the check is
    /// worth making again.
    #[must_use]
    pub const fn decides_nothing(self) -> bool {
        match self {
            Self::NotReached => true,
            Self::NoBackend
            | Self::Refused(_)
            | Self::SandboxRefused
            | Self::Unconfined
            | Self::ProgramMissing
            | Self::TimedOut => false,
        }
    }
}

impl fmt::Display for Unavailable {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoBackend => out.write_str("no sandbox runs on this machine"),
            Self::Refused(feature) => write!(
                out,
                "the sandbox cannot hold the {} a TeX run needs",
                held(*feature)
            ),
            Self::SandboxRefused => out.write_str("the sandbox refused the TeX run"),
            Self::Unconfined => out.write_str("the sandbox would run TeX unconfined"),
            Self::ProgramMissing => {
                out.write_str("latex, dvisvgm, /etc/texmf or /var/lib/texmf is missing")
            }
            Self::TimedOut => out.write_str("the TeX check did not finish in time"),
            Self::NotReached => out.write_str("not checked yet: the sandbox was busy"),
        }
    }
}

/// A feature the sandbox refused, in the words `/settings` shows. Every
/// feature has words, so a new one makes this match decide; those a TeX run's
/// policy never asks for are named all the same.
const fn held(feature: SandboxFeature) -> &'static str {
    match feature {
        SandboxFeature::Filesystem => "filesystem confinement",
        SandboxFeature::NetworkDeny => "closed network",
        SandboxFeature::NetworkAllowlist => "network allowlist",
        SandboxFeature::DescriptorIsolation => "descriptor isolation",
        SandboxFeature::ProcessIsolation => "process isolation",
        SandboxFeature::KernelSurface => "kernel restriction",
        SandboxFeature::PrivilegeIsolation => "privilege isolation",
        SandboxFeature::Materialization => "file staging",
        SandboxFeature::CpuLimit => "CPU-time limit",
        SandboxFeature::MemoryLimit => "memory limit",
        SandboxFeature::DiskLimit => "disk limit",
        SandboxFeature::ProcessLimit => "process limit",
        SandboxFeature::OpenFileLimit => "open-file limit",
        SandboxFeature::CommandTimeLimit => "run-time limit",
        SandboxFeature::SessionTimeLimit => "session-time limit",
        SandboxFeature::OutboundByteLimit => "outbound-byte limit",
        SandboxFeature::OutputLimit => "output limit",
        SandboxFeature::ConcurrencyLimit => "concurrency limit",
        SandboxFeature::CostLimit => "cost limit",
        SandboxFeature::Pty => "terminal",
        SandboxFeature::FileOperations => "file operations",
        SandboxFeature::Persistence => "persistent session",
        SandboxFeature::Snapshot => "snapshots",
        SandboxFeature::Resume => "resumed session",
        SandboxFeature::Audit => "audit record",
        SandboxFeature::Usage => "usage report",
    }
}

/// Why one formula has no picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unmade {
    /// TeX refused the formula, or it is longer than a run is given.
    Rejected,
    /// The run crossed one of its limits.
    Limit(SandboxViolation),
    /// The run made nothing to draw.
    Empty,
    /// The run did not end in its time.
    Timeout,
    /// The formula never reached TeX: the sandbox was busy, refused the run
    /// before it began, or an earlier run is still being let go of. Worth
    /// trying again in a later answer.
    NotReached,
}

/// What makes a display formula into a picture.
///
/// [`TexEngine`] is the one that runs anything; a stand-in in a test answers
/// without running.
pub trait MathPictures: Send + Sync {
    /// The programs' versions, which a kept picture is keyed by.
    fn toolchain(&self) -> &str;

    /// The picture `formula` draws.
    fn make(&self, formula: &str) -> BoxFuture<'_, Result<Svg, Unmade>>;
}

/// The installed TeX, as the check found it, on the service it was found on.
///
/// Only [`available`] returns one:
///
/// ```compile_fail
/// use std::sync::Arc;
/// use crucible_app::math::{MathPictures, TexEngine};
///
/// let engine = TexEngine::new(Arc::new(crucible_sandbox_local::LocalSandbox::new()));
/// let _ = engine.make("x^2");
/// ```
pub struct TexEngine {
    service: Arc<dyn SandboxService>,
    handle: Handle,
    toolchain: Box<str>,
}

impl fmt::Debug for TexEngine {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("TexEngine")
            .field("toolchain", &self.toolchain)
            .finish_non_exhaustive()
    }
}

impl MathPictures for TexEngine {
    fn toolchain(&self) -> &str {
        &self.toolchain
    }

    fn make(&self, formula: &str) -> BoxFuture<'_, Result<Svg, Unmade>> {
        let formula: Box<[u8]> = formula.as_bytes().into();
        Box::pin(async move {
            if formula.len() > FORMULA_BYTES {
                return Err(Unmade::Rejected);
            }
            let Ok(run) = Run::formula(formula) else {
                return Err(Unmade::NotReached);
            };
            drawn(drive(&self.service, &self.handle, run).await.outcome)
        })
    }
}

/// The session's check: whether math pictures can be made here, and the
/// engine that makes them if they can.
///
/// It looks for the TeX tree on the host before preparing anything, then
/// makes two version runs on `service`, under the policy every formula gets:
/// `latex --version`, then `dvisvgm --version`, sharing `budget` as their
/// command time. Each run is prepared, checked for confinement and
/// materialized on its own.
///
/// # Errors
///
/// The one reason they cannot. [`Unavailable::decides_nothing`] says when the
/// check is worth making again.
pub async fn available(
    service: Arc<dyn SandboxService>,
    budget: Duration,
) -> Result<TexEngine, Unavailable> {
    checked(service, budget, &|path| path.is_dir()).await
}

/// [`available`], with what says whether a host directory exists.
async fn checked(
    service: Arc<dyn SandboxService>,
    budget: Duration,
    exists: &(dyn Fn(&Path) -> bool + Sync),
) -> Result<TexEngine, Unavailable> {
    if !exists(Path::new(TEXMF_CONFIG)) || !exists(Path::new(TEXMF_VAR)) {
        return Err(Unavailable::ProgramMissing);
    }
    let Ok(handle) = Handle::try_current() else {
        return Err(Unavailable::NotReached);
    };
    if budget < FLOOR {
        return Err(Unavailable::TimedOut);
    }
    let first = Run::version("latex", budget).map_err(|_| Unavailable::SandboxRefused)?;
    let latex = drive(&service, &handle, first).await;
    let latex_line = versioned(latex.outcome)?;
    let left = budget.saturating_sub(latex.spent);
    if left < FLOOR {
        return Err(Unavailable::TimedOut);
    }
    let second = Run::version("dvisvgm", left).map_err(|_| Unavailable::SandboxRefused)?;
    let dvisvgm_line = versioned(drive(&service, &handle, second).await.outcome)?;
    Ok(TexEngine {
        service,
        handle,
        toolchain: format!("{latex_line}; {dvisvgm_line}").into(),
    })
}

/// One run to make: what is prepared, what is started, what it is told.
struct Run {
    request: SandboxRequest,
    command: SandboxCommand,
    input: Option<Box<[u8]>>,
    command_time: Duration,
}

impl Run {
    /// `program --version`, under the formula policy with `command_time`.
    fn version(program: &str, command_time: Duration) -> Result<Self, SandboxError> {
        Ok(Self {
            request: request(command_time)?,
            command: SandboxCommand::new(
                "/bin/sh",
                ["-c".into(), format!("exec {program} --version").into()],
                environment()?,
            )?,
            input: None,
            command_time,
        })
    }

    /// The wrapper, told `formula`.
    fn formula(formula: Box<[u8]>) -> Result<Self, SandboxError> {
        Ok(Self {
            request: request(COMMAND_TIME)?,
            command: SandboxCommand::new("/bin/sh", ["-c".into(), WRAPPER.into()], environment()?)?
                .spoken_to(),
            input: Some(formula),
            command_time: COMMAND_TIME,
        })
    }
}

/// The request every run is prepared from: the policy with `command_time`, an
/// empty manifest, a fresh identity, a root ancestry and the fixed tool.
fn request(command_time: Duration) -> Result<SandboxRequest, SandboxError> {
    let policy = policy(command_time).map_err(|_| SandboxError::InvalidCommand)?;
    Ok(SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("math-picture"),
        policy,
        SandboxManifest::empty(),
    ))
}

/// The policy every run is held to: confined, the TeX tree read-only and
/// nothing of the host writable, the network closed, and its limits.
pub(crate) fn policy(
    command_time: Duration,
) -> Result<SandboxPolicy, crucible_sandbox::SandboxPolicyError> {
    let read_only = |path: &str| {
        SandboxFilesystemRule::new(
            path,
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime,
        )
    };
    SandboxPolicy::new(
        true,
        [read_only(TEXMF_CONFIG)?, read_only(TEXMF_VAR)?],
        TEXMF_CONFIG,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits {
            memory_bytes: Some(MEMORY_BYTES),
            processes: Some(PROCESSES),
            disk_bytes: Some(DISK_BYTES),
            output_bytes: u64::try_from(STDOUT_BYTES + STDERR_BYTES).ok(),
            command_time: Some(command_time),
            ..SandboxResourceLimits::confining()
        },
    )
}

/// How a run's staging ended: its first message.
enum Staging {
    /// The launch is staged and the thread goes on.
    Staged,
    /// The sandbox refused, somewhere from `prepare` to `stage`.
    Refused(SandboxError),
    /// The session would not confine the run.
    Unconfined,
}

/// How a staged run ended: its second message.
enum Ended {
    /// `release` refused.
    Released(SandboxError),
    /// The formula could not be written to the run's input.
    Unwritten,
    /// It ran.
    Finished(Finished),
}

/// What a run that ran came to.
struct Finished {
    /// The limit the backend says the run crossed, read after its stop.
    violation: Option<SandboxViolation>,
    /// Whether its output passed what is read of it.
    over: bool,
    /// Whether the sandbox reported an error while the run was waited on or
    /// its output read.
    fault: bool,
    /// How its leader exited, if it was seen to.
    status: Option<ExitStatus>,
    /// Its standard output, at most [`STDOUT_BYTES`].
    stdout: Vec<u8>,
}

/// What became of one run, as the asker saw it.
enum Outcome {
    /// It never reached its first message: no thread, a late thread before
    /// it, no message in time, or a channel closed with none.
    Unreached,
    /// Its staging ended without a launch.
    Unstaged(Staging),
    /// It staged, and its last word did not come in time, or never came.
    Lost,
    /// It staged and ended.
    Ended(Ended),
}

/// A run's outcome, and how long it took from "staged" to that outcome.
struct Driven {
    outcome: Outcome,
    spent: Duration,
}

/// The run whose thread the asker stopped waiting on, for each service with
/// one: until it has returned, nothing else runs on that service.
static LATE: Mutex<Vec<(usize, JoinHandle<()>)>> = Mutex::new(Vec::new());

/// Which service `service` is, as [`LATE`] keys it. The late thread holds a
/// clone of the service, so the address is not reused while it lives.
fn keyed(service: &Arc<dyn SandboxService>) -> usize {
    Arc::as_ptr(service).cast::<()>().addr()
}

/// Whether a late thread on `service` is still alive.
fn late(service: &Arc<dyn SandboxService>) -> bool {
    let key = keyed(service);
    let mut late = LATE.lock().unwrap_or_else(PoisonError::into_inner);
    late.retain(|(_, thread)| !thread.is_finished());
    late.iter().any(|(held, _)| *held == key)
}

/// Keeps `thread`, which the asker stopped waiting on, as `service`'s late
/// thread.
fn behind(service: &Arc<dyn SandboxService>, thread: JoinHandle<()>) {
    LATE.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((keyed(service), thread));
}

/// Does `run` on a thread of its own, and waits for it as the module
/// documentation says.
async fn drive(service: &Arc<dyn SandboxService>, handle: &Handle, run: Run) -> Driven {
    let unreached = Driven {
        outcome: Outcome::Unreached,
        spent: Duration::ZERO,
    };
    if late(service) {
        return unreached;
    }
    let started = Instant::now();
    let (staging, mut first) = oneshot::channel();
    let (ending, second) = oneshot::channel();
    let command_time = run.command_time;
    let thread = {
        let held = Arc::clone(service);
        let handle = handle.clone();
        spawned(service, move || ran(&held, &handle, run, staging, ending))
    };
    let Ok(thread) = thread else {
        return unreached;
    };
    let staged = match tokio::time::timeout_at((started + STAGING).into(), &mut first).await {
        Ok(Ok(message)) => message,
        Ok(Err(_)) => {
            behind(service, thread);
            return unreached;
        }
        Err(_) => {
            first.close();
            if let Ok(message) = first.try_recv() {
                message
            } else {
                behind(service, thread);
                return unreached;
            }
        }
    };
    if !matches!(staged, Staging::Staged) {
        let _ = thread.join();
        return Driven {
            outcome: Outcome::Unstaged(staged),
            spent: Duration::ZERO,
        };
    }
    let staged_at = Instant::now();
    let ended = tokio::time::timeout_at((staged_at + command_time + MARGIN).into(), second).await;
    let spent = staged_at.elapsed();
    let outcome = if let Ok(Ok(ended)) = ended {
        let _ = thread.join();
        Outcome::Ended(ended)
    } else {
        behind(service, thread);
        Outcome::Lost
    };
    Driven { outcome, spent }
}

/// Starts a run's thread for `service`.
fn spawned(
    service: &Arc<dyn SandboxService>,
    body: impl FnOnce() + Send + 'static,
) -> std::io::Result<JoinHandle<()>> {
    #[cfg(test)]
    if tests::unstartable(keyed(service)) {
        return Err(std::io::Error::other("a test refused the thread"));
    }
    #[cfg(not(test))]
    let _ = service;
    std::thread::Builder::new()
        .name("crucible-math".into())
        .spawn(body)
}

/// The whole of one run, on its own thread.
fn ran(
    service: &Arc<dyn SandboxService>,
    handle: &Handle,
    run: Run,
    staging: oneshot::Sender<Staging>,
    ending: oneshot::Sender<Ended>,
) {
    let Run {
        request,
        command,
        input,
        command_time,
    } = run;
    handle.block_on(async move {
        let launch = match staged(&**service, request, command).await {
            Ok(launch) => launch,
            Err(why) => {
                let _ = staging.send(why);
                return;
            }
        };
        if staging.send(Staging::Staged).is_err() {
            drop(launch);
            return;
        }
        let ended = released(launch, input.as_deref(), command_time).await;
        let _ = ending.send(ended);
    });
}

/// Prepares, checks, materializes and stages. Whatever it held is dropped
/// before it answers.
async fn staged(
    service: &dyn SandboxService,
    request: SandboxRequest,
    command: SandboxCommand,
) -> Result<Box<dyn SandboxLaunch>, Staging> {
    let mut session = service.prepare(request).await.map_err(Staging::Refused)?;
    if !session.inspection().confined() {
        return Err(Staging::Unconfined);
    }
    session.materialize().await.map_err(Staging::Refused)?;
    session.stage(command).await.map_err(Staging::Refused)
}

/// Releases `launch`, tells it `input`, and reads and waits on it until it
/// ends. Whatever it held is dropped before it answers.
async fn released(
    launch: Box<dyn SandboxLaunch>,
    input: Option<&[u8]>,
    command_time: Duration,
) -> Ended {
    let mut process = match launch.release().await {
        Ok(process) => process,
        Err(why) => return Ended::Released(why),
    };
    if let Some(input) = input {
        let written = process
            .take_stdin()
            .is_some_and(|mut stdin| stdin.write_all(input).is_ok() && stdin.flush().is_ok());
        if !written {
            let _ = process.stop().await;
            return Ended::Unwritten;
        }
    }
    Ended::Finished(finished(process.as_mut(), command_time).await)
}

/// One stream of a run's output, read to its ceiling.
struct Stream {
    output: Option<Box<dyn SandboxOutput>>,
    ceiling: usize,
    kept: Option<Vec<u8>>,
    seen: usize,
}

impl Stream {
    /// Reads what is ready. `Ok(true)` when it read something.
    fn read(&mut self, buffer: &mut [u8]) -> Result<bool, Stop> {
        let Some(output) = self.output.as_mut() else {
            return Ok(false);
        };
        let read = match output.read_ready(buffer).map_err(|_| Stop::Fault)? {
            SandboxRead::Bytes(read) => read,
            SandboxRead::Limited { .. } => return Err(Stop::Over),
            SandboxRead::Pending => return Ok(false),
            SandboxRead::End => {
                self.output = None;
                return Ok(false);
            }
        };
        self.seen = self.seen.saturating_add(read);
        if self.seen > self.ceiling {
            return Err(Stop::Over);
        }
        if let (Some(kept), Some(bytes)) = (self.kept.as_mut(), buffer.get(..read)) {
            kept.extend_from_slice(bytes);
        }
        Ok(read > 0)
    }
}

/// Why a run's reading stopped early.
enum Stop {
    /// Its output passed what is read of it.
    Over,
    /// The sandbox reported an error reading it.
    Fault,
}

/// Reads `process`'s output and waits on its leader, then stops it.
async fn finished(process: &mut dyn SandboxProcess, command_time: Duration) -> Finished {
    let until = Instant::now() + command_time + OVERRUN;
    let mut streams = [
        Stream {
            output: process.take_stdout(),
            ceiling: STDOUT_BYTES,
            kept: Some(Vec::new()),
            seen: 0,
        },
        Stream {
            output: process.take_stderr(),
            ceiling: STDERR_BYTES,
            kept: None,
            seen: 0,
        },
    ];
    let mut buffer = vec![0_u8; 8 * 1024];
    let mut over = false;
    let mut fault = false;
    let mut status = None;
    'ran: while Instant::now() < until {
        let mut read = false;
        for stream in &mut streams {
            match stream.read(&mut buffer) {
                Ok(more) => read |= more,
                Err(Stop::Over) => {
                    over = true;
                    break 'ran;
                }
                Err(Stop::Fault) => {
                    fault = true;
                    break 'ran;
                }
            }
        }
        if streams.iter().all(|stream| stream.output.is_none()) {
            match process.try_wait() {
                Ok(Some(exited)) => {
                    status = Some(exited);
                    break;
                }
                Ok(None) => {}
                Err(_) => {
                    fault = true;
                    break;
                }
            }
        }
        if !read {
            tokio::time::sleep(POLL).await;
        }
    }
    let _ = process.stop().await;
    let [stdout, _] = streams;
    Finished {
        violation: process.violation(),
        over,
        fault,
        status,
        stdout: stdout.kept.unwrap_or_default(),
    }
}

/// A version run's outcome, read as the check reads it: its version line, or
/// why there is none.
fn versioned(outcome: Outcome) -> Result<String, Unavailable> {
    let finished = match outcome {
        Outcome::Unreached | Outcome::Ended(Ended::Unwritten) => {
            return Err(Unavailable::NotReached);
        }
        Outcome::Unstaged(Staging::Refused(why)) | Outcome::Ended(Ended::Released(why)) => {
            return Err(refused(&why));
        }
        Outcome::Unstaged(Staging::Unconfined) => return Err(Unavailable::Unconfined),
        Outcome::Unstaged(Staging::Staged) | Outcome::Lost => return Err(Unavailable::TimedOut),
        Outcome::Ended(Ended::Finished(finished)) => finished,
    };
    match finished.violation {
        Some(SandboxViolation::CommandTime) => return Err(Unavailable::TimedOut),
        Some(SandboxViolation::Output) => return Err(Unavailable::SandboxRefused),
        None => {}
    }
    if finished.over {
        return Err(Unavailable::SandboxRefused);
    }
    if finished.fault {
        return Err(Unavailable::NotReached);
    }
    match finished.status {
        None => Err(Unavailable::TimedOut),
        Some(status) if !status.success() => Err(Unavailable::ProgramMissing),
        Some(_) => line(&finished.stdout).ok_or(Unavailable::ProgramMissing),
    }
}

/// The check's reading of a sandbox refusal, by its variant alone.
const fn refused(why: &SandboxError) -> Unavailable {
    match why {
        SandboxError::Concurrency | SandboxError::Spawn(_) => Unavailable::NotReached,
        SandboxError::Unsupported { feature } => Unavailable::Refused(*feature),
        SandboxError::BackendUnavailable { .. } => Unavailable::NoBackend,
        SandboxError::Audit(_)
        | SandboxError::InvalidCommand
        | SandboxError::InvalidEnvironment
        | SandboxError::InvalidInspection
        | SandboxError::Guardrail
        | SandboxError::Materialization { .. }
        | SandboxError::Lifecycle(_)
        | SandboxError::LaunchRefused { .. } => Unavailable::SandboxRefused,
    }
}

/// The first line of `stdout`, as a version: printable, bounded, and holding
/// a number.
fn line(stdout: &[u8]) -> Option<String> {
    let first = stdout.split(|byte| *byte == b'\n').next()?;
    let printable: String = String::from_utf8_lossy(first)
        .chars()
        .filter(|character| !character.is_control())
        .take(VERSION_BYTES)
        .collect();
    let trimmed = printable.trim();
    trimmed
        .chars()
        .any(|character| character.is_ascii_digit())
        .then(|| trimmed.to_owned())
}

/// A formula run's outcome, read as a picture or why there is none.
fn drawn(outcome: Outcome) -> Result<Svg, Unmade> {
    let finished = match outcome {
        Outcome::Unreached
        | Outcome::Unstaged(Staging::Refused(_) | Staging::Unconfined)
        | Outcome::Ended(Ended::Released(_) | Ended::Unwritten) => return Err(Unmade::NotReached),
        Outcome::Unstaged(Staging::Staged) | Outcome::Lost => return Err(Unmade::Timeout),
        Outcome::Ended(Ended::Finished(finished)) => finished,
    };
    match finished.violation {
        Some(SandboxViolation::CommandTime) => return Err(Unmade::Timeout),
        Some(violation) => return Err(Unmade::Limit(violation)),
        None => {}
    }
    if finished.over {
        return Err(Unmade::Limit(SandboxViolation::Output));
    }
    if finished.fault {
        return Err(Unmade::NotReached);
    }
    match finished.status {
        None => Err(Unmade::Timeout),
        Some(status) if !status.success() => Err(Unmade::Rejected),
        Some(_) if extentless(&finished.stdout) => Err(Unmade::Empty),
        Some(_) => Ok(Svg::new(finished.stdout)),
    }
}

/// Whether `svg` is nothing, or a picture with no width or no height.
fn extentless(svg: &[u8]) -> bool {
    let text = String::from_utf8_lossy(svg);
    let Some(start) = text.find("<svg") else {
        return true;
    };
    let Some(root) = text.get(start..).and_then(|rest| rest.split('>').next()) else {
        return true;
    };
    ["width", "height"]
        .iter()
        .any(|name| measured(root, name).is_some_and(|extent| extent <= 0.0))
}

/// The number `root`'s attribute `name` starts with.
fn measured(root: &str, name: &str) -> Option<f64> {
    let at = root.find(&format!(" {name}="))?;
    let value = root.get(at + name.len() + 2..)?;
    let quote = value.chars().next()?;
    let number: String = value
        .chars()
        .skip(1)
        .take_while(|character| *character != quote)
        .take_while(|character| character.is_ascii_digit() || matches!(character, '.' | '-'))
        .collect();
    number.parse().ok()
}
