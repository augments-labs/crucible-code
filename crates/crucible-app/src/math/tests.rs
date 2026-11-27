//! The session's check against a stand-in sandbox that runs nothing: each
//! row of what a refusal, a hang or a lost run means, where it is injected,
//! and how long the asker waits for it, on the real monotonic clock.
//!
//! A request names `/etc/texmf`, which is absolute only on Unix, so every row
//! that reaches `prepare` runs there; the rows that end before it run on
//! every platform.

// The stand-in and its helpers serve the rows that run on Unix alone.
#![cfg_attr(not(unix), allow(dead_code, unused_imports))]

use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCapability, SandboxCleanup, SandboxInspection, SandboxSession, SandboxUsage,
    confined_inspection, inspection,
};

use super::*;

/// Services whose run threads a test refuses to start, by [`keyed`].
static UNSTARTABLE: Mutex<Vec<usize>> = Mutex::new(Vec::new());

/// Whether a test refused the run threads of the service keyed `key`.
pub(super) fn unstartable(key: usize) -> bool {
    UNSTARTABLE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .contains(&key)
}

/// The longest any stand-in step holds before it is let go regardless.
const HELD: Duration = Duration::from_mins(1);

/// A version line as `latex --version` prints it here.
const LATEX: &str =
    "pdfTeX 3.141592653-2.6-1.40.27 (TeX Live 2025/Debian)\nkpathsea version 6.4.1\n";

/// A version line as `dvisvgm --version` prints it here.
const DVISVGM: &str = "dvisvgm 3.6\n";

/// Where in a run a stand-in does something other than succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Prepare,
    Materialize,
    Stage,
    Release,
}

/// A refusal a stand-in answers with.
#[derive(Debug, Clone, Copy)]
enum Refusal {
    Unsupported(SandboxFeature),
    Backend,
    Materialization,
    LaunchRefused,
    Lifecycle,
    Audit,
    Spawn,
    Concurrency,
}

impl Refusal {
    fn error(self) -> SandboxError {
        match self {
            Self::Unsupported(feature) => SandboxError::Unsupported { feature },
            Self::Backend => SandboxError::BackendUnavailable {
                reason: "a stand-in with no backend".into(),
            },
            Self::Materialization => SandboxError::Materialization {
                problem: "a stand-in refusing".into(),
                source: None,
            },
            Self::LaunchRefused => SandboxError::LaunchRefused {
                said: "bwrap: a stand-in refusing".into(),
                source: io::Error::other("ended before ready"),
            },
            Self::Lifecycle => SandboxError::Lifecycle(io::Error::other("killed before ready")),
            Self::Audit => SandboxError::Audit(crucible_sandbox::SandboxAuditError::Full),
            Self::Spawn => SandboxError::Spawn(io::Error::other("no launcher")),
            Self::Concurrency => SandboxError::Concurrency,
        }
    }
}

/// What one run of a stand-in does.
#[derive(Debug, Clone, Default)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "each flag is one independent fault a row injects, and rows set them one or two at a time"
)]
struct Act {
    refuse: Option<(At, Refusal)>,
    unwind: Option<At>,
    block: Option<At>,
    slow: Option<(At, Duration)>,
    unconfined: bool,
    /// How the leader exits once it has run for `runs_for`; `None` runs until
    /// stopped.
    exit: Option<u32>,
    runs_for: Duration,
    stdout: &'static str,
    wait_error: bool,
    read_error: bool,
    violation: Option<SandboxViolation>,
    stop_blocks: bool,
    /// Writing the formula to standard input fails.
    stdin_error: bool,
    /// Dropping the session, or the process, holds until let go.
    drop_blocks: bool,
    /// `stop` cannot confirm cleanup, and the slot stays taken.
    quarantines: bool,
}

impl Act {
    fn version(stdout: &'static str) -> Self {
        Self {
            exit: Some(0),
            stdout,
            ..Self::default()
        }
    }

    fn refusing(at: At, refusal: Refusal) -> Self {
        Self {
            refuse: Some((at, refusal)),
            ..Self::default()
        }
    }
}

/// What a stand-in saw.
#[derive(Default)]
struct Log {
    prepared: AtomicUsize,
    staged: Mutex<Vec<Instant>>,
    let_go: AtomicBool,
    quarantined: AtomicBool,
    dropped: AtomicUsize,
}

impl Log {
    fn prepared(&self) -> usize {
        self.prepared.load(Ordering::SeqCst)
    }

    /// When the latest run was staged.
    fn staged(&self) -> Instant {
        *self
            .staged
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .last()
            .expect("a run was staged")
    }

    /// Lets go of every held step.
    fn release(&self) {
        self.let_go.store(true, Ordering::SeqCst);
    }

    /// Holds until let go, or [`HELD`].
    async fn hold(&self) {
        let until = Instant::now() + HELD;
        while !self.let_go.load(Ordering::SeqCst) && Instant::now() < until {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

/// A sandbox that runs nothing and does, at each step, what its acts say.
struct Stand {
    acts: Vec<Act>,
    log: Arc<Log>,
}

impl Stand {
    fn serving(acts: impl IntoIterator<Item = Act>) -> (Arc<dyn SandboxService>, Arc<Log>) {
        let log = Arc::new(Log::default());
        let service: Arc<dyn SandboxService> = Arc::new(Self {
            acts: acts.into_iter().collect(),
            log: Arc::clone(&log),
        });
        (service, log)
    }
}

/// Does what `act` says at `at`, and refuses if it says to.
async fn step(act: &Act, log: &Log, at: At) -> Result<(), SandboxError> {
    if let Some((slow, by)) = act.slow
        && slow == at
    {
        tokio::time::sleep(by).await;
    }
    if act.block == Some(at) {
        log.hold().await;
    }
    assert!(act.unwind != Some(at), "the stand-in unwinds at {at:?}");
    match act.refuse {
        Some((refused, refusal)) if refused == at => Err(refusal.error()),
        _ => Ok(()),
    }
}

fn identity() -> SandboxBackendIdentity {
    SandboxBackendIdentity::new(
        SandboxBackendId::new("stand-in").expect("a valid backend id"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("a valid identity")
}

/// Every boundary and limit, enforced.
fn enforcing() -> SandboxCapabilities {
    use SandboxFeature as F;
    [
        F::Filesystem,
        F::NetworkDeny,
        F::DescriptorIsolation,
        F::ProcessIsolation,
        F::KernelSurface,
        F::PrivilegeIsolation,
        F::Materialization,
        F::CpuLimit,
        F::MemoryLimit,
        F::DiskLimit,
        F::ProcessLimit,
        F::OpenFileLimit,
        F::CommandTimeLimit,
        F::OutputLimit,
        F::ConcurrencyLimit,
        F::Audit,
    ]
    .into_iter()
    .fold(SandboxCapabilities::none(), |claims, feature| {
        claims.with(feature, SandboxCapability::Enforced)
    })
}

/// What a session is reported as when confinement is turned off.
fn unconfined(request: &SandboxRequest) -> SandboxInspection {
    let off = SandboxPolicy::new(
        false,
        [SandboxFilesystemRule::new(
            TEXMF_CONFIG,
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::Runtime,
        )
        .expect("a valid rule")],
        TEXMF_CONFIG,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("a valid policy");
    inspection(
        request.id(),
        identity(),
        SandboxCapabilities::none(),
        &off,
        request.manifest(),
        false,
        Some("sandbox disabled by effective policy"),
        SandboxCleanup::Pending,
    )
    .expect("a valid unconfined inspection")
}

impl SandboxService for Stand {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async move { Ok((identity(), enforcing())) })
    }

    fn prepare(
        &self,
        request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let index = self.log.prepared.fetch_add(1, Ordering::SeqCst);
            let act = self
                .acts
                .get(index)
                .or(self.acts.last())
                .cloned()
                .unwrap_or_default();
            if self.log.quarantined.load(Ordering::SeqCst) {
                return Err(SandboxError::Concurrency);
            }
            step(&act, &self.log, At::Prepare).await?;
            let inspection = if act.unconfined {
                unconfined(&request)
            } else {
                confined_inspection(identity(), enforcing(), &request)?
            };
            let hold = Hold {
                log: Arc::clone(&self.log),
                blocks: act.drop_blocks,
            };
            let session: Box<dyn SandboxSession> = Box::new(Session {
                act,
                log: Arc::clone(&self.log),
                inspection,
                hold,
            });
            Ok(session)
        })
    }
}

struct Session {
    act: Act,
    log: Arc<Log>,
    inspection: SandboxInspection,
    hold: Hold,
}

/// What a session or a process holds until it is dropped, and, when told
/// to, holds its drop on until let go.
struct Hold {
    log: Arc<Log>,
    blocks: bool,
}

impl Drop for Hold {
    fn drop(&mut self) {
        let until = Instant::now() + HELD;
        while self.blocks && !self.log.let_go.load(Ordering::SeqCst) && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        self.log.dropped.fetch_add(1, Ordering::SeqCst);
    }
}

impl SandboxSession for Session {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move { step(&self.act, &self.log, At::Materialize).await })
    }

    fn stage<'a>(
        self: Box<Self>,
        _command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            step(&self.act, &self.log, At::Stage).await?;
            self.log
                .staged
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Instant::now());
            let launch: Box<dyn SandboxLaunch> = self;
            Ok(launch)
        })
    }
}

impl SandboxLaunch for Session {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            step(&self.act, &self.log, At::Release).await?;
            let Self {
                act,
                log,
                inspection,
                hold,
            } = *self;
            let stdout: Box<dyn SandboxOutput> = Box::new(Said {
                bytes: Some(act.stdout.as_bytes()),
                error: act.read_error,
            });
            let process: Box<dyn SandboxProcess> = Box::new(Process {
                act,
                log,
                inspection,
                started: Instant::now(),
                stopped: false,
                stdout: Some(stdout),
                _hold: hold,
            });
            Ok(process)
        })
    }
}

/// A stream that says its bytes once, then ends.
struct Said {
    bytes: Option<&'static [u8]>,
    error: bool,
}

impl SandboxOutput for Said {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if self.error {
            return Err(io::Error::other("a stand-in read error"));
        }
        let Some(bytes) = self.bytes.take() else {
            return Ok(SandboxRead::End);
        };
        let read = bytes.len().min(buffer.len());
        let (now, rest) = bytes.split_at(read);
        if let Some(into) = buffer.get_mut(..read) {
            into.copy_from_slice(now);
        }
        if !rest.is_empty() {
            self.bytes = Some(rest);
        }
        Ok(SandboxRead::Bytes(read))
    }
}

/// How a stand-in's leader exits with `code`.
fn exited(code: u32) -> ExitStatus {
    #[cfg(unix)]
    {
        std::os::unix::process::ExitStatusExt::from_raw(
            i32::try_from(code << 8).expect("a small code"),
        )
    }
    #[cfg(windows)]
    {
        std::os::windows::process::ExitStatusExt::from_raw(code)
    }
}

struct Process {
    act: Act,
    log: Arc<Log>,
    inspection: SandboxInspection,
    started: Instant,
    stopped: bool,
    stdout: Option<Box<dyn SandboxOutput>>,
    /// Held only to be dropped with the process.
    _hold: Hold,
}

/// An input that refuses every byte.
struct Refusing;

impl io::Write for Refusing {
    fn write(&mut self, _: &[u8]) -> io::Result<usize> {
        Err(io::Error::from(io::ErrorKind::BrokenPipe))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl SandboxProcess for Process {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        if self.act.stdin_error {
            Some(Box::new(Refusing))
        } else {
            Some(Box::new(io::sink()))
        }
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        Some(Box::new(Said {
            bytes: None,
            error: false,
        }))
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.act.wait_error {
            return Err(io::Error::other("a stand-in wait error"));
        }
        if self.stopped {
            return Ok(Some(exited(137)));
        }
        Ok(self
            .act
            .exit
            .filter(|_| self.started.elapsed() >= self.act.runs_for)
            .map(exited))
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            if self.act.stop_blocks {
                self.log.hold().await;
            }
            self.stopped = true;
            if self.act.quarantines {
                self.log.quarantined.store(true, Ordering::SeqCst);
                return Err(io::Error::other("cleanup not confirmed"));
            }
            Ok(())
        })
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage::default()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.act.violation
    }
}

/// A runtime of one worker, so an asker that blocked it would starve every
/// other task.
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_time()
        .build()
        .expect("a runtime")
}

fn on<F: Future>(work: F) -> F::Output {
    runtime().block_on(work)
}

/// The check on `service`, with the TeX tree present.
async fn check(
    service: &Arc<dyn SandboxService>,
    budget: Duration,
) -> Result<TexEngine, Unavailable> {
    checked(Arc::clone(service), budget, &|_| true).await
}

/// The check's refusal, asserting it and whether it decides anything.
fn refused_as(outcome: Result<TexEngine, Unavailable>, expected: Unavailable) {
    match outcome {
        Ok(engine) => panic!("expected {expected:?}, got {engine:?}"),
        Err(why) => {
            assert_eq!(why, expected);
            assert_eq!(why.decides_nothing(), expected == Unavailable::NotReached);
        }
    }
}

/// Waits, at most 5 s, for `service`'s late thread to return.
async fn returned(service: &Arc<dyn SandboxService>) {
    let until = Instant::now() + Duration::from_secs(5);
    while late(service) && Instant::now() < until {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(!late(service), "the late thread returned");
}

/// Under half of `deadline`: "well before" it.
fn well_before(elapsed: Duration, deadline: Duration) {
    assert!(
        elapsed < deadline / 2,
        "{elapsed:?} is not well before {deadline:?}"
    );
}

/// The check's second deadline for a run of `command_time`, from "staged".
fn second(command_time: Duration) -> Duration {
    command_time + MARGIN
}

// Row: the TeX tree absent on the host, injected before any stage.
#[test]
fn a_missing_tex_tree_is_program_missing_and_nothing_is_prepared() {
    for missing in [TEXMF_CONFIG, TEXMF_VAR] {
        let (service, log) = Stand::serving([Act::version(LATEX), Act::version(DVISVGM)]);
        let outcome = on(checked(service, COMMAND_TIME, &|path| {
            path != Path::new(missing)
        }));
        refused_as(outcome, Unavailable::ProgramMissing);
        assert_eq!(log.prepared(), 0, "{missing} absent prepares nothing");
    }
}

#[cfg(unix)]
mod reached {
    use super::*;

    // Row: a limit the backend cannot hold, injected at `prepare`.
    #[test]
    fn an_unsupported_limit_at_prepare_is_refused_with_that_feature() {
        for feature in [SandboxFeature::ProcessLimit, SandboxFeature::DiskLimit] {
            let (service, log) =
                Stand::serving([Act::refusing(At::Prepare, Refusal::Unsupported(feature))]);
            refused_as(
                on(check(&service, COMMAND_TIME)),
                Unavailable::Refused(feature),
            );
            assert_eq!(log.prepared(), 1, "no second version run");
        }
    }

    // Row: no backend, injected at `prepare`.
    #[test]
    fn no_backend_at_prepare_is_no_backend() {
        let (service, _) = Stand::serving([Act::refusing(At::Prepare, Refusal::Backend)]);
        refused_as(on(check(&service, COMMAND_TIME)), Unavailable::NoBackend);
    }

    // Row: the backend's own refusals, injected at `materialize`, `stage` and
    // `release`.
    #[test]
    fn a_refusal_after_prepare_is_sandbox_refused() {
        for (at, refusal) in [
            (At::Materialize, Refusal::Materialization),
            (At::Stage, Refusal::LaunchRefused),
            (At::Release, Refusal::LaunchRefused),
            (At::Release, Refusal::Lifecycle),
            (At::Release, Refusal::Audit),
        ] {
            let (service, _) = Stand::serving([Act::refusing(at, refusal)]);
            refused_as(
                on(check(&service, COMMAND_TIME)),
                Unavailable::SandboxRefused,
            );
        }
    }

    // Row: confinement turned off, injected at `prepare`'s report.
    #[test]
    fn an_unconfined_session_is_unconfined_and_never_staged() {
        let (service, log) = Stand::serving([Act {
            unconfined: true,
            ..Act::version(LATEX)
        }]);
        refused_as(on(check(&service, COMMAND_TIME)), Unavailable::Unconfined);
        assert!(
            log.staged
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .is_empty(),
            "an unconfined session stages nothing"
        );
    }

    // Row: a version run that finds no program, injected at its exit and its
    // output.
    #[test]
    fn a_version_run_without_its_program_is_program_missing() {
        for act in [
            Act {
                exit: Some(127),
                ..Act::version("")
            },
            Act::version("no version here\n"),
            Act::version(""),
        ] {
            let (service, _) = Stand::serving([act]);
            refused_as(
                on(check(&service, COMMAND_TIME)),
                Unavailable::ProgramMissing,
            );
        }
    }

    // Row: the output limit, injected as the violation the run reports,
    // alone and beside a wait error and a read error.
    #[test]
    fn an_output_violation_is_sandbox_refused() {
        let output = Act {
            violation: Some(SandboxViolation::Output),
            ..Act::version(LATEX)
        };
        for act in [
            output.clone(),
            Act {
                wait_error: true,
                ..output.clone()
            },
            Act {
                read_error: true,
                ..output
            },
        ] {
            let (service, _) = Stand::serving([act]);
            refused_as(
                on(check(&service, COMMAND_TIME)),
                Unavailable::SandboxRefused,
            );
        }
    }

    // Row: the command time, injected as the violation a hanging run
    // reports, alone and beside a wait error and a read error.
    #[test]
    fn a_command_time_violation_is_timed_out_well_before_the_second_deadline() {
        let budget = Duration::from_secs(1);
        let hung = Act {
            violation: Some(SandboxViolation::CommandTime),
            exit: Some(137),
            runs_for: Duration::from_millis(300),
            ..Act::version("")
        };
        for act in [
            hung.clone(),
            Act {
                wait_error: true,
                ..hung.clone()
            },
            Act {
                read_error: true,
                ..hung
            },
        ] {
            let (service, log) = Stand::serving([act]);
            refused_as(on(check(&service, budget)), Unavailable::TimedOut);
            well_before(log.staged().elapsed(), second(budget));
        }
    }

    // Row: a run's thread that never sends its second message, injected as a
    // `stop` that holds.
    #[test]
    fn a_thread_that_never_finishes_times_out_at_the_second_deadline_and_holds_the_service() {
        let budget = Duration::from_secs(1);
        let (service, log) = Stand::serving([
            Act {
                stop_blocks: true,
                ..Act::default()
            },
            Act::version(LATEX),
            Act::version(DVISVGM),
        ]);
        on(async {
            refused_as(check(&service, budget).await, Unavailable::TimedOut);
            let waited = log.staged().elapsed();
            assert!(waited >= second(budget), "waited {waited:?}");
            assert!(
                waited < second(budget) + Duration::from_millis(500),
                "waited {waited:?}"
            );

            refused_as(check(&service, budget).await, Unavailable::NotReached);
            assert_eq!(log.prepared(), 1, "no run starts while the thread lives");

            log.release();
            returned(&service).await;
            let engine = check(&service, COMMAND_TIME).await.expect("the engine");
            assert_eq!(log.prepared(), 3);
            assert_eq!(
                engine.toolchain(),
                "pdfTeX 3.141592653-2.6-1.40.27 (TeX Live 2025/Debian); dvisvgm 3.6"
            );
        });
    }

    // Row: a thread that unwinds in `release`, after "staged".
    #[test]
    fn a_thread_that_unwinds_in_release_times_out_well_before_the_second_deadline() {
        let budget = Duration::from_secs(1);
        let (service, log) = Stand::serving([Act {
            unwind: Some(At::Release),
            ..Act::version(LATEX)
        }]);
        refused_as(on(check(&service, budget)), Unavailable::TimedOut);
        well_before(log.staged().elapsed(), second(budget));
    }

    // Row: a first version run that leaves under the floor, injected as its
    // run time.
    #[test]
    fn a_first_run_leaving_under_a_second_times_out_and_prepares_no_second() {
        let (service, log) = Stand::serving([
            Act {
                runs_for: Duration::from_millis(1_500),
                ..Act::version(LATEX)
            },
            Act::version(DVISVGM),
        ]);
        refused_as(
            on(check(&service, Duration::from_secs(2))),
            Unavailable::TimedOut,
        );
        assert_eq!(log.prepared(), 1, "the second run is not prepared");
    }

    // Row: a kill before READY, which `release` reports.
    #[test]
    fn a_kill_before_ready_is_sandbox_refused() {
        for refusal in [Refusal::Lifecycle, Refusal::LaunchRefused] {
            let (service, _) = Stand::serving([Act::refusing(At::Release, refusal)]);
            refused_as(
                on(check(&service, COMMAND_TIME)),
                Unavailable::SandboxRefused,
            );
        }
    }

    // Row: a `release` that holds, after a `stage` that takes 2 s.
    #[test]
    fn a_release_that_holds_times_out_at_the_second_deadline_with_the_asker_responsive() {
        let budget = Duration::from_secs(1);
        let (service, log) = Stand::serving([Act {
            slow: Some((At::Stage, Duration::from_secs(2))),
            block: Some(At::Release),
            ..Act::version(LATEX)
        }]);
        on(async {
            let gap = Arc::new(Mutex::new(Duration::ZERO));
            let ticking = Arc::new(AtomicBool::new(true));
            let ticker = {
                let gap = Arc::clone(&gap);
                let ticking = Arc::clone(&ticking);
                tokio::spawn(async move {
                    let mut last = Instant::now();
                    while ticking.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        let mut widest = gap.lock().unwrap_or_else(PoisonError::into_inner);
                        *widest = (*widest).max(last.elapsed());
                        last = Instant::now();
                    }
                })
            };
            refused_as(check(&service, budget).await, Unavailable::TimedOut);
            let waited = log.staged().elapsed();
            ticking.store(false, Ordering::SeqCst);
            ticker.await.expect("the ticker ends");
            log.release();
            assert!(waited >= second(budget), "waited {waited:?}");
            assert!(
                waited < second(budget) + Duration::from_millis(500),
                "waited {waited:?}"
            );
            let widest = *gap.lock().unwrap_or_else(PoisonError::into_inner);
            assert!(
                widest < Duration::from_millis(250),
                "the asker's runtime stalled for {widest:?}"
            );
        });
    }

    // Row: what never reaches a version run's end, injected at `prepare`,
    // at the second run's `prepare`, at the wait, at the read, at the
    // thread's start and as an unwind in `stage`.
    #[test]
    fn what_never_reaches_a_version_run_is_not_reached() {
        let refused = [
            vec![Act::refusing(At::Prepare, Refusal::Spawn)],
            vec![
                Act::version(LATEX),
                Act::refusing(At::Prepare, Refusal::Concurrency),
            ],
            vec![Act {
                wait_error: true,
                ..Act::version(LATEX)
            }],
            vec![Act {
                read_error: true,
                ..Act::version(LATEX)
            }],
        ];
        for acts in refused {
            let (service, _) = Stand::serving(acts);
            refused_as(on(check(&service, COMMAND_TIME)), Unavailable::NotReached);
        }

        let (service, log) = Stand::serving([Act::version(LATEX)]);
        UNSTARTABLE
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(keyed(&service));
        refused_as(on(check(&service, COMMAND_TIME)), Unavailable::NotReached);
        assert_eq!(log.prepared(), 0, "no thread, nothing prepared");

        let (service, log) = Stand::serving([Act {
            unwind: Some(At::Stage),
            ..Act::version(LATEX)
        }]);
        let started = Instant::now();
        refused_as(on(check(&service, COMMAND_TIME)), Unavailable::NotReached);
        well_before(started.elapsed(), STAGING);
        assert_eq!(log.prepared(), 1, "prepare took the request");
    }

    // Row: a `prepare` and a `stage` that hold past the first deadline.
    #[test]
    fn a_step_that_holds_before_staged_is_not_reached_at_the_first_deadline() {
        for at in [At::Prepare, At::Stage] {
            let (service, log) = Stand::serving([
                Act {
                    block: Some(at),
                    ..Act::default()
                },
                Act::version(LATEX),
                Act::version(DVISVGM),
            ]);
            on(async {
                let started = Instant::now();
                refused_as(check(&service, COMMAND_TIME).await, Unavailable::NotReached);
                let waited = started.elapsed();
                assert!(waited >= STAGING, "{at:?}: waited {waited:?}");
                assert!(
                    waited < STAGING + Duration::from_millis(500),
                    "{at:?}: waited {waited:?}"
                );

                refused_as(check(&service, COMMAND_TIME).await, Unavailable::NotReached);
                assert_eq!(log.prepared(), 1, "{at:?}: nothing starts behind it");

                log.release();
                returned(&service).await;
                check(&service, COMMAND_TIME).await.expect("the engine");
                assert_eq!(log.prepared(), 3, "{at:?}: the check is made again");
            });
        }
    }

    // Row: slow, but in time: a preparation taking most of the first deadline
    // and version runs taking most of the budget.
    #[test]
    fn a_slow_check_in_time_is_the_engine() {
        let slow = Some((At::Prepare, Duration::from_secs(3)));
        let (service, _) = Stand::serving([
            Act {
                slow,
                runs_for: Duration::from_secs(6),
                ..Act::version(LATEX)
            },
            Act {
                slow,
                runs_for: Duration::from_millis(500),
                ..Act::version(DVISVGM)
            },
        ]);
        let engine = on(check(&service, COMMAND_TIME)).expect("the engine");
        assert_eq!(
            engine.toolchain(),
            "pdfTeX 3.141592653-2.6-1.40.27 (TeX Live 2025/Debian); dvisvgm 3.6"
        );
    }

    // A check refused as busy decides nothing, and the next one finds TeX.
    #[test]
    fn a_check_refused_as_busy_is_made_again_and_yields_the_engine() {
        let (service, log) = Stand::serving([
            Act::refusing(At::Prepare, Refusal::Concurrency),
            Act::version(LATEX),
            Act::version(DVISVGM),
        ]);
        on(async {
            let busy = check(&service, COMMAND_TIME).await;
            refused_as(busy, Unavailable::NotReached);
            check(&service, COMMAND_TIME).await.expect("the engine");
            assert_eq!(log.prepared(), 3);
        });
    }
}

/// A picture as `dvisvgm` writes one.
const PICTURE: &str = "<?xml version='1.0' encoding='UTF-8'?>\n<svg version='1.1' xmlns='http://www.w3.org/2000/svg' width='22.5pt' height='9.8pt' viewBox='0 0 22.5 9.8'>\n<path d='M0 0h1v1z'/>\n</svg>\n";

/// Formulas made by an engine on a stand-in sandbox: what each way a run
/// can end makes of the formula.
#[cfg(unix)]
mod made {
    use super::*;

    /// The engine on `service`, as the check would have returned it.
    fn engine(service: &Arc<dyn SandboxService>) -> TexEngine {
        TexEngine {
            service: Arc::clone(service),
            handle: Handle::current(),
            toolchain: "stand-in".into(),
        }
    }

    fn unmade(outcome: Result<Svg, Unmade>) -> Unmade {
        match outcome {
            Ok(svg) => panic!("expected no picture, got {svg:?}"),
            Err(why) => why,
        }
    }

    #[test]
    fn a_formula_tex_draws_is_its_picture() {
        let (service, _) = Stand::serving([Act::version(PICTURE)]);
        let svg = on(async { engine(&service).make("x^2").await }).expect("a picture");
        assert_eq!(svg.bytes(), PICTURE.as_bytes());
    }

    #[test]
    fn a_formula_longer_than_a_run_is_given_is_rejected_unrun() {
        let (service, log) = Stand::serving([Act::version(PICTURE)]);
        let formula = "x".repeat(FORMULA_BYTES + 1);
        let why = on(async { unmade(engine(&service).make(&formula).await) });
        assert_eq!(why, Unmade::Rejected);
        assert_eq!(log.prepared(), 0);
    }

    #[test]
    fn what_a_run_ends_in_is_what_the_formula_is() {
        let output = Act {
            violation: Some(SandboxViolation::Output),
            ..Act::version(PICTURE)
        };
        let rows = [
            (
                "TeX refused it",
                Act {
                    exit: Some(1),
                    ..Act::version("")
                },
                Unmade::Rejected,
            ),
            ("nothing written", Act::version(""), Unmade::Empty),
            (
                "no width",
                Act::version(
                    "<svg xmlns='http://www.w3.org/2000/svg' width='0pt' height='9pt'></svg>",
                ),
                Unmade::Empty,
            ),
            (
                "an output violation",
                output.clone(),
                Unmade::Limit(SandboxViolation::Output),
            ),
            (
                "a wait error beside an output violation",
                Act {
                    wait_error: true,
                    ..output.clone()
                },
                Unmade::Limit(SandboxViolation::Output),
            ),
            (
                "a read error beside an output violation",
                Act {
                    read_error: true,
                    ..output
                },
                Unmade::Limit(SandboxViolation::Output),
            ),
            (
                "a write to its input refused",
                Act {
                    stdin_error: true,
                    ..Act::version(PICTURE)
                },
                Unmade::NotReached,
            ),
        ];
        for (row, act, expected) in rows {
            let (service, log) = Stand::serving([act]);
            let why = on(async { unmade(engine(&service).make("x").await) });
            assert_eq!(why, expected, "{row}");
            assert_eq!(
                log.staged
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .len(),
                1,
                "{row}"
            );
        }
    }

    #[test]
    fn a_command_time_violation_beside_an_error_is_a_timeout_well_before_the_second_deadline() {
        let hung = Act {
            violation: Some(SandboxViolation::CommandTime),
            exit: Some(137),
            ..Act::version("")
        };
        for act in [
            Act {
                wait_error: true,
                ..hung.clone()
            },
            Act {
                read_error: true,
                ..hung
            },
        ] {
            let (service, log) = Stand::serving([act]);
            let why = on(async { unmade(engine(&service).make("x").await) });
            assert_eq!(why, Unmade::Timeout);
            well_before(log.staged().elapsed(), second(COMMAND_TIME));
        }
    }

    #[test]
    fn an_unwinding_thread_is_answered_from_its_closed_channel() {
        let (service, log) = Stand::serving([Act {
            unwind: Some(At::Stage),
            ..Act::version(PICTURE)
        }]);
        let started = Instant::now();
        let why = on(async { unmade(engine(&service).make("x").await) });
        assert_eq!(why, Unmade::NotReached);
        well_before(started.elapsed(), STAGING);
        assert_eq!(log.prepared(), 1);

        let (service, log) = Stand::serving([Act {
            unwind: Some(At::Release),
            ..Act::version(PICTURE)
        }]);
        let why = on(async { unmade(engine(&service).make("x").await) });
        assert_eq!(why, Unmade::Timeout);
        well_before(log.staged().elapsed(), second(COMMAND_TIME));
    }

    #[test]
    fn a_run_not_ended_past_its_time_is_a_timeout_at_the_second_deadline_and_holds_the_service() {
        let (service, log) = Stand::serving([
            Act {
                slow: Some((At::Stage, Duration::from_secs(2))),
                stop_blocks: true,
                ..Act::default()
            },
            Act::version(PICTURE),
        ]);
        on(async {
            let engine = engine(&service);
            assert_eq!(unmade(engine.make("x").await), Unmade::Timeout);
            let waited = log.staged().elapsed();
            assert!(waited >= second(COMMAND_TIME), "waited {waited:?}");
            assert!(
                waited < second(COMMAND_TIME) + Duration::from_millis(500),
                "waited {waited:?}"
            );

            assert_eq!(unmade(engine.make("y").await), Unmade::NotReached);
            assert_eq!(
                log.prepared(),
                1,
                "nothing is prepared while the thread lives"
            );

            log.release();
            returned(&service).await;
            engine.make("y").await.expect("the next formula is made");
        });
    }

    /// Whether `work` is still unanswered 300 ms after it starts, and then,
    /// once the stand-in is let go, its answer.
    fn held_until_let_go<T: Send + 'static>(
        log: &Arc<Log>,
        work: impl Future<Output = T> + Send + 'static,
    ) -> T {
        on(async {
            let answer = tokio::spawn(work);
            tokio::time::sleep(Duration::from_millis(300)).await;
            assert!(!answer.is_finished(), "an outcome came before the drop");
            log.release();
            answer.await.expect("the work ends")
        })
    }

    #[test]
    fn no_outcome_comes_before_the_run_has_dropped_what_it_held() {
        let (service, log) = Stand::serving([Act {
            drop_blocks: true,
            ..Act::version(PICTURE)
        }]);
        let made = held_until_let_go(&log, {
            let service = Arc::clone(&service);
            async move {
                let engine = engine(&service);
                (engine.make("x").await, engine.make("y").await)
            }
        });
        made.0.expect("the first is made");
        made.1.expect("the second is made");

        for act in [
            Act {
                drop_blocks: true,
                ..Act::refusing(At::Materialize, Refusal::Materialization)
            },
            Act {
                drop_blocks: true,
                unconfined: true,
                ..Act::version(PICTURE)
            },
        ] {
            let (service, log) = Stand::serving([act]);
            let made = held_until_let_go(&log, {
                let service = Arc::clone(&service);
                async move { engine(&service).make("x").await }
            });
            assert_eq!(unmade(made), Unmade::NotReached);
            assert_eq!(log.dropped.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn a_run_whose_cleanup_is_unconfirmed_keeps_its_slot_and_the_next_is_not_reached() {
        let (service, _) = Stand::serving([Act {
            violation: Some(SandboxViolation::CommandTime),
            exit: Some(137),
            quarantines: true,
            ..Act::version("")
        }]);
        on(async {
            let engine = engine(&service);
            assert_eq!(unmade(engine.make("x").await), Unmade::Timeout);
            assert_eq!(unmade(engine.make("y").await), Unmade::NotReached);
        });
    }
}

// The margin past a run's command time is the Linux backend's own bounds on
// a stop, and their values when it was set, so a change to any of them is
// seen here first.
#[cfg(target_os = "linux")]
#[test]
fn the_margin_is_the_backends_stop_bounds_and_750_ms() {
    use crucible_sandbox_local::{CANCEL_GRACE, CANCELLED, REAP};

    assert_eq!(CANCEL_GRACE, Duration::from_secs(5));
    assert_eq!(REAP, Duration::from_secs(1));
    assert_eq!(CANCELLED, Duration::from_millis(250));
    assert_eq!(
        MARGIN,
        CANCEL_GRACE + REAP + CANCELLED + Duration::from_millis(750)
    );
}
