//! One spawned command and its complete local cleanup scope.
//!
//! Failed cleanup remains retryable while this owner exists. A stage and its
//! admission slot are released only after process cleanup is confirmed. If Drop
//! still cannot finish, the stage is retained and the slot stays consumed until
//! the service restarts. This bounds new admissions without claiming that an
//! unconfirmed workload died or retaining an unbounded cleanup thread.

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    CallResultKey, CallResultReceipt, SandboxAudit, SandboxCleanup, SandboxFactKind, SandboxId,
    SandboxInspection, SandboxInvocationMode, SandboxLifecycle, SandboxOutput, SandboxProcess,
    SandboxRead, SandboxResourceLimits, SandboxUsage, SandboxViolation,
};

use crate::bash::platform::{Output as PlatformOutput, ReadState, Scope, Terminator};

/// Absolute ceiling even where a policy omits a smaller one.
pub(super) const MAX_LOCAL_COMMANDS: usize = 16;

/// Bounded reap interval used by destructors.
const REAP: Duration = Duration::from_millis(250);

/// Supervisor polling interval. It bounds deadline overshoot without spinning.
const SUPERVISE: Duration = Duration::from_millis(5);

const NO_VIOLATION: u8 = 0;
const COMMAND_TIME_VIOLATION: u8 = 1;
const OUTPUT_VIOLATION: u8 = 2;

/// One concurrency reservation transferred from prepare through process drop.
pub(super) struct Reservation {
    active: Arc<AtomicUsize>,
    held: bool,
}

impl Reservation {
    /// Reserves one slot without temporarily exceeding `maximum`.
    pub(super) fn take(
        active: Arc<AtomicUsize>,
        maximum: usize,
    ) -> Result<Self, crucible_core::SandboxError> {
        let maximum = maximum.min(MAX_LOCAL_COMMANDS);
        let reserved = active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < maximum).then_some(current.saturating_add(1))
        });
        if reserved.is_err() {
            return Err(crucible_core::SandboxError::Concurrency);
        }
        Ok(Self { active, held: true })
    }

    /// Keeps the service's counter slot consumed after cleanup ownership is lost.
    /// The Arc itself is released normally; only the bounded count is retained.
    fn quarantine(mut self) {
        self.held = false;
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Reservation")
            .field("held", &self.held)
            .finish_non_exhaustive()
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if self.held {
            self.active.fetch_sub(1, Ordering::AcqRel);
            self.held = false;
        }
    }
}

/// A generated staging tree, retained when cleanup cannot safely be confirmed.
pub(super) struct Stage {
    root: std::path::PathBuf,
    retained: bool,
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Stage {
    pub(super) fn new(root: std::path::PathBuf) -> Self {
        Self {
            root,
            retained: false,
        }
    }

    pub(super) fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Keeps a quarantined tree for bounded startup recovery and inspection.
    pub(super) fn retain(&mut self) {
        self.retained = true;
    }

    #[cfg(target_os = "linux")]
    pub(super) fn retained(&self) -> bool {
        self.retained
    }
}

/// Linux materialization owns additional state below its stage.
#[cfg(target_os = "linux")]
impl Stage {
    pub(super) fn manifest(&self) -> std::path::PathBuf {
        self.root.join("manifest")
    }
}

impl Stage {
    /// Removes the complete stage and proves that its pathname is absent.
    ///
    /// Retained quarantine evidence deliberately fails cleanup instead of
    /// allowing a caller to report that every sandbox resource is gone.
    pub(super) fn cleanup(&mut self) -> io::Result<()> {
        if self.retained {
            return Err(io::Error::other(
                "sandbox stage is retained as quarantine evidence",
            ));
        }
        match std::fs::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(problem) if problem.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(problem) => Err(problem),
        }
    }
}

impl std::fmt::Debug for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Stage")
            .field("root", &"[temporary sandbox path]")
            .finish()
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

/// Asks a backend to end its workload before the uncatchable group kill.
///
/// The argument is the process id of the spawned leader. The call may wait,
/// within a budget of its own, for that leader to exit so the backend's report
/// arrives before the kill.
pub(super) type Canceller = Box<dyn Fn(u32) -> io::Result<()> + Send + Sync>;

/// Spawns `command` inside a platform process-tree scope.
pub(super) struct SpawnPlan {
    pub(super) inspection: SandboxInspection,
    pub(super) reservation: Reservation,
    pub(super) stage: Option<Stage>,
    pub(super) limits: SandboxResourceLimits,
    pub(super) audit: SandboxAudit,
    pub(super) sandbox: SandboxId,
    pub(super) audit_started: bool,
    /// Whether this process owns the terminal cleanup fact. Linux transfers
    /// that responsibility to the projection wrapper that owns more state.
    pub(super) audit_cleanup: bool,
    pub(super) invocation: SandboxInvocationMode,
    pub(super) call_result_key: Option<CallResultKey>,
    /// A cooperative stop the supervisor tries before the group kill. Killing
    /// the Linux launcher alone leaves its PID namespace running, so the Linux
    /// backend asks the broker to end the workload and report its wait status;
    /// the kill stays as the backstop.
    pub(super) canceller: Option<Canceller>,
    /// Whether crucible keeps the writing end of standard input. Decided with
    /// the command, because a pipe cannot be attached after a spawn.
    pub(super) speech: crucible_core::SandboxSpeech,
}

/// Cleans preparation owners that never reached a child process.
///
/// A cleanup failure retains both the stage and its bounded admission slot so
/// a later command cannot reuse authority whose disposal was not confirmed.
#[cfg(any(target_os = "macos", test))]
pub(super) fn cleanup_prepared_owners(
    stage: &mut Option<Stage>,
    reservation: &mut Option<Reservation>,
) -> SandboxCleanup {
    if stage.as_mut().map_or(Ok(()), Stage::cleanup).is_ok() {
        stage.take();
        reservation.take();
        SandboxCleanup::Complete
    } else {
        if let Some(stage) = stage {
            stage.retain();
        }
        if let Some(reservation) = reservation.take() {
            reservation.quarantine();
        }
        SandboxCleanup::Failed
    }
}

/// Disposes a launch plan that was abandoned before spawn.
#[cfg(target_os = "macos")]
pub(super) fn cleanup_unspawned(mut plan: SpawnPlan) -> SandboxCleanup {
    let mut reservation = Some(plan.reservation);
    cleanup_prepared_owners(&mut plan.stage, &mut reservation)
}

/// Starts one command under an already negotiated process plan.
///
/// An unconfirmed startup cleanup returns Lifecycle; Spawn and Audit errors
/// retain their original category only after cleanup has been proved. Callers
/// use that distinction to retain their separately owned projection evidence.
pub(super) fn spawn(
    command: Command,
    plan: SpawnPlan,
) -> Result<Box<dyn SandboxProcess>, crucible_core::SandboxError> {
    spawn_local(command, plan).map(|process| Box::new(process) as Box<dyn SandboxProcess>)
}

fn spawn_local(
    command: Command,
    plan: SpawnPlan,
) -> Result<LocalProcess, crucible_core::SandboxError> {
    spawn_inner(
        command,
        plan,
        #[cfg(test)]
        stop_scope,
    )
}

fn spawn_inner(
    mut command: Command,
    plan: SpawnPlan,
    #[cfg(test)] test_stop: fn(&Scope, &mut Child) -> io::Result<()>,
) -> Result<LocalProcess, crucible_core::SandboxError> {
    #[cfg(test)]
    let stop_scope = test_stop;
    let SpawnPlan {
        inspection,
        reservation,
        stage,
        limits,
        audit,
        sandbox,
        audit_started,
        audit_cleanup,
        invocation,
        call_result_key,
        canceller,
        speech,
    } = plan;
    command
        .stdin(match speech {
            // A step that reads gets end-of-file, which is an answer. A peer
            // gets a pipe crucible keeps, because it is going to be spoken to.
            crucible_core::SandboxSpeech::Closed => Stdio::null(),
            crucible_core::SandboxSpeech::Held => Stdio::piped(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    let scope = Scope::new(&mut command);
    #[cfg(windows)]
    let scope = match Scope::new(&mut command) {
        Ok(scope) => scope,
        Err(source) => {
            return Err(failed_before_spawn(
                crucible_core::SandboxError::Spawn(source),
                stage,
                reservation,
            ));
        }
    };

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(source) => {
            return Err(failed_before_spawn(
                crucible_core::SandboxError::Spawn(source),
                stage,
                reservation,
            ));
        }
    };
    let started = Instant::now();
    let stdin = child
        .stdin
        .take()
        .map(|input| Box::new(input) as Box<dyn std::io::Write + Send>);
    let control = Arc::new(Control::new(limits.output_bytes, audit, sandbox));
    // Own every resource before the first fallible initialization operation.
    // No caller can observe this private, unfinished process. Stop and Drop
    // use the scope and child directly, without needing a borrowed terminator.
    let mut process = LocalProcess {
        child,
        scope,
        stdin,
        terminator: None,
        stdout: None,
        stderr: None,
        inspection,
        reservation: Some(reservation),
        stage,
        control,
        supervisor: None,
        status: None,
        scope_stopped: false,
        started,
        stopped: false,
        audit_state: AuditState::default(),
        audit_cleanup,
        invocation,
        call_result_key,
        background_acceptance: BackgroundAcceptance::None,
        #[cfg(test)]
        test_stop: stop_scope,
        #[cfg(test)]
        test_reap: reap,
    };
    match process.initialize(limits, canceller, audit_started) {
        Ok(terminator) => {
            process.terminator = Some(terminator);
            Ok(process)
        }
        Err(startup) => match process.stop() {
            Ok(()) => Err(startup),
            Err(cleanup) => Err(startup_cleanup_failed(startup, cleanup)),
        },
    }
}

/// No child exists on these paths, but failed staging cleanup still keeps its
/// reservation consumed and its filesystem evidence intact.
fn failed_before_spawn(
    startup: crucible_core::SandboxError,
    mut stage: Option<Stage>,
    reservation: Reservation,
) -> crucible_core::SandboxError {
    match stage.as_mut().map_or(Ok(()), Stage::cleanup) {
        Ok(()) => startup,
        Err(cleanup) => {
            if let Some(stage) = &mut stage {
                stage.retained = true;
            }
            reservation.quarantine();
            startup_cleanup_failed(startup, cleanup)
        }
    }
}

/// Preserves both causes while keeping the public diagnostic bounded and free
/// of command text and temporary paths. Lifecycle means spawn cleanup failed;
/// the original Spawn or Audit error is returned only after proved cleanup.
#[derive(Debug)]
struct StartupCleanupFailure {
    startup: crucible_core::SandboxError,
    cleanup: io::Error,
}

impl std::fmt::Display for StartupCleanupFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "sandbox startup failed ({:?}); cleanup is unconfirmed ({:?})",
            self.startup.failure_kind(),
            self.cleanup.kind(),
        )
    }
}

impl std::error::Error for StartupCleanupFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.startup)
    }
}

fn startup_cleanup_failed(
    startup: crucible_core::SandboxError,
    cleanup: io::Error,
) -> crucible_core::SandboxError {
    crucible_core::SandboxError::Lifecycle(io::Error::new(
        cleanup.kind(),
        StartupCleanupFailure { startup, cleanup },
    ))
}

/// Shared hard-limit state used by both output streams and the supervisor.
struct Control {
    lifecycle: Mutex<()>,
    done: AtomicBool,
    violation: AtomicU8,
    output_remaining: Option<AtomicU64>,
    output_bytes: AtomicU64,
    failure: Mutex<Option<Failure>>,
    audit: SandboxAudit,
    sandbox: SandboxId,
}

#[derive(Clone, Copy)]
struct Failure {
    kind: io::ErrorKind,
    raw: Option<i32>,
}

impl Control {
    fn new(output_limit: Option<u64>, audit: SandboxAudit, sandbox: SandboxId) -> Self {
        Self {
            lifecycle: Mutex::new(()),
            done: AtomicBool::new(false),
            violation: AtomicU8::new(NO_VIOLATION),
            output_remaining: output_limit.map(AtomicU64::new),
            output_bytes: AtomicU64::new(0),
            failure: Mutex::new(None),
            audit,
            sandbox,
        }
    }

    fn lifecycle(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.lifecycle
            .lock()
            .map_err(|_| io::Error::other("sandbox lifecycle supervisor lock was poisoned"))
    }

    fn record_output(&self, bytes: usize) -> (usize, usize) {
        let bytes_u64 = u64::try_from(bytes).unwrap_or(u64::MAX);
        atomic_saturating_add(&self.output_bytes, bytes_u64);

        let Some(remaining) = &self.output_remaining else {
            return (bytes, 0);
        };
        let previous = remaining
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                Some(current.saturating_sub(bytes_u64))
            })
            .unwrap_or_else(|current| current);
        let retained_u64 = previous.min(bytes_u64);
        let retained = usize::try_from(retained_u64).unwrap_or(bytes);
        let discarded = bytes.saturating_sub(retained);
        if discarded > 0 {
            self.mark(SandboxViolation::Output);
        }
        (retained, discarded)
    }

    fn mark(&self, violation: SandboxViolation) {
        let code = match violation {
            SandboxViolation::CommandTime => COMMAND_TIME_VIOLATION,
            SandboxViolation::Output => OUTPUT_VIOLATION,
        };
        if self
            .violation
            .compare_exchange(NO_VIOLATION, code, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
            && let Err(problem) = self.audit(SandboxFactKind::Violation(violation))
        {
            self.record_failure(&io::Error::other(problem));
        }
    }

    fn violation(&self) -> Option<SandboxViolation> {
        match self.violation.load(Ordering::Acquire) {
            COMMAND_TIME_VIOLATION => Some(SandboxViolation::CommandTime),
            OUTPUT_VIOLATION => Some(SandboxViolation::Output),
            _ => None,
        }
    }

    fn record_failure(&self, problem: &io::Error) {
        let Ok(mut failure) = self.failure.lock() else {
            return;
        };
        if failure.is_none() {
            *failure = Some(Failure {
                kind: problem.kind(),
                raw: problem.raw_os_error(),
            });
        }
    }

    fn failure(&self) -> Option<io::Error> {
        let failure = self.failure.lock().ok()?.as_ref().copied()?;
        Some(failure.raw.map_or_else(
            || {
                io::Error::new(
                    failure.kind,
                    "sandbox supervisor could not stop the process scope",
                )
            },
            io::Error::from_raw_os_error,
        ))
    }

    fn audit(&self, kind: SandboxFactKind) -> Result<(), crucible_core::SandboxAuditError> {
        self.audit.record(self.sandbox, kind)
    }
}

fn atomic_saturating_add(value: &AtomicU64, increment: u64) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_add(increment))
    });
}

/// One bounded thread that owns deadline/output-triggered process-tree stops.
struct Supervisor {
    control: Arc<Control>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Supervisor {
    fn start(
        control: Arc<Control>,
        terminator: Terminator,
        deadline: Option<Instant>,
        canceller: Option<Canceller>,
        leader: u32,
    ) -> io::Result<Self> {
        let supervised = Arc::clone(&control);
        let thread = thread::Builder::new()
            .name("crucible-sandbox-supervisor".into())
            .spawn(move || {
                loop {
                    if supervised.done.load(Ordering::Acquire) {
                        return;
                    }
                    let expired = deadline.is_some_and(|deadline| Instant::now() >= deadline);
                    if expired {
                        supervised.mark(SandboxViolation::CommandTime);
                    }
                    if supervised.violation().is_none() {
                        thread::sleep(SUPERVISE);
                        continue;
                    }

                    let Ok(_lifecycle) = supervised.lifecycle() else {
                        return;
                    };
                    if supervised.done.load(Ordering::Acquire) {
                        return;
                    }
                    if let Some(cancel) = &canceller {
                        // A cancellation the backend cannot deliver leaves the kill.
                        let _ = cancel(leader);
                    }
                    if let Err(problem) = terminator.stop() {
                        supervised.record_failure(&problem);
                    }
                    return;
                }
            })?;
        Ok(Self {
            control,
            thread: Some(thread),
        })
    }

    fn finish(&mut self) -> io::Result<()> {
        self.control.done.store(true, Ordering::Release);
        let Some(thread) = self.thread.take() else {
            return Ok(());
        };
        thread
            .join()
            .map_err(|_| io::Error::other("sandbox lifecycle supervisor panicked"))
    }
}

/// A pipe put into non-blocking mode before the process handle escapes.
struct PreparedOutput {
    inner: Box<dyn PlatformOutput>,
    control: Arc<Control>,
}

impl PreparedOutput {
    fn new(output: impl PlatformOutput, control: Arc<Control>) -> io::Result<Self> {
        output.prepare()?;
        Ok(Self {
            inner: Box::new(output),
            control,
        })
    }
}

impl SandboxOutput for PreparedOutput {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if buffer.is_empty() {
            return Ok(SandboxRead::Pending);
        }
        self.inner.read_ready(buffer).map(|read| match read {
            ReadState::Bytes(bytes) => {
                let (retained, discarded) = self.control.record_output(bytes);
                if discarded == 0 {
                    SandboxRead::Bytes(retained)
                } else {
                    SandboxRead::Limited {
                        retained,
                        discarded,
                    }
                }
            }
            ReadState::Pending => SandboxRead::Pending,
            ReadState::End => SandboxRead::End,
        })
    }
}

/// The process, its process-tree scope, streams, stage, and reservation.
struct LocalProcess {
    child: Child,
    scope: Scope,
    /// Present only after initialization has fully succeeded.
    terminator: Option<Terminator>,
    /// The writing end of a peer's input, until somebody takes it. Dropping it
    /// unread is what closes the far end's stdin.
    stdin: Option<Box<dyn std::io::Write + Send>>,
    stdout: Option<Box<dyn SandboxOutput>>,
    stderr: Option<Box<dyn SandboxOutput>>,
    inspection: SandboxInspection,
    reservation: Option<Reservation>,
    stage: Option<Stage>,
    control: Arc<Control>,
    supervisor: Option<Supervisor>,
    status: Option<ExitStatus>,
    scope_stopped: bool,
    started: Instant,
    stopped: bool,
    audit_state: AuditState,
    audit_cleanup: bool,
    invocation: SandboxInvocationMode,
    call_result_key: Option<CallResultKey>,
    background_acceptance: BackgroundAcceptance,
    /// Per-instance failure injection leaves production builds and other tests unchanged.
    #[cfg(test)]
    test_stop: fn(&Scope, &mut Child) -> io::Result<()>,
    #[cfg(test)]
    test_reap: fn(&mut Child, &mut Option<ExitStatus>) -> io::Result<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BackgroundAcceptance {
    None,
    Pending,
    Accepted,
}

#[derive(Default)]
struct AuditState {
    finished: bool,
    usage: bool,
    cleanup: Option<SandboxCleanup>,
}

impl LocalProcess {
    fn initialize(
        &mut self,
        limits: SandboxResourceLimits,
        canceller: Option<Canceller>,
        audit_started: bool,
    ) -> Result<Terminator, crucible_core::SandboxError> {
        #[cfg(windows)]
        self.scope
            .attach(&self.child)
            .map_err(crucible_core::SandboxError::Spawn)?;
        let terminator = self
            .scope
            .terminator(&self.child)
            .map_err(crucible_core::SandboxError::Spawn)?;
        self.stdout = self
            .child
            .stdout
            .take()
            .map(|pipe| PreparedOutput::new(pipe, Arc::clone(&self.control)))
            .transpose()
            .map_err(crucible_core::SandboxError::Spawn)?
            .map(|pipe| Box::new(pipe) as Box<dyn SandboxOutput>);
        self.stderr = self
            .child
            .stderr
            .take()
            .map(|pipe| PreparedOutput::new(pipe, Arc::clone(&self.control)))
            .transpose()
            .map_err(crucible_core::SandboxError::Spawn)?
            .map(|pipe| Box::new(pipe) as Box<dyn SandboxOutput>);
        if limits.command_time.is_some() || limits.output_bytes.is_some() {
            self.supervisor = Some(
                Supervisor::start(
                    Arc::clone(&self.control),
                    terminator,
                    limits
                        .command_time
                        .map(|allowed| self.started.checked_add(allowed).unwrap_or(self.started)),
                    canceller,
                    self.child.id(),
                )
                .map_err(crucible_core::SandboxError::Spawn)?,
            );
        }
        if audit_started {
            self.control
                .audit(SandboxFactKind::Lifecycle(SandboxLifecycle::CommandStarted))?;
        }
        Ok(terminator)
    }

    fn audit_finished(&mut self) -> io::Result<()> {
        if !self.audit_state.finished {
            self.control
                .audit(SandboxFactKind::Lifecycle(
                    SandboxLifecycle::CommandFinished,
                ))
                .map_err(io::Error::other)?;
            self.audit_state.finished = true;
        }
        if !self.audit_state.usage {
            self.control
                .audit(SandboxFactKind::Usage(self.usage()))
                .map_err(io::Error::other)?;
            self.audit_state.usage = true;
        }
        Ok(())
    }

    fn audit_cleanup(&mut self, cleanup: SandboxCleanup) -> io::Result<()> {
        if self.audit_state.cleanup == Some(cleanup) {
            return Ok(());
        }
        self.control
            .audit(SandboxFactKind::Cleanup(cleanup))
            .map_err(io::Error::other)?;
        self.audit_state.cleanup = Some(cleanup);
        Ok(())
    }
}

impl SandboxProcess for LocalProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.stdin.take()
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stderr.take()
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(problem) = self.control.failure() {
            return Err(problem);
        }
        if let Some(status) = self.status {
            if !self.scope_stopped {
                return Err(io::Error::other(
                    "sandbox process scope cleanup is unconfirmed",
                ));
            }
            self.audit_finished()?;
            return Ok(Some(status));
        }

        let status = {
            let _lifecycle = self.control.lifecycle()?;
            let terminator = self
                .terminator
                .ok_or_else(|| io::Error::other("sandbox process initialization is incomplete"))?;
            let status = self.scope.try_wait(&mut self.child, terminator)?;
            if status.is_some() {
                self.control.done.store(true, Ordering::Release);
            }
            status
        };
        if let Some(status) = status {
            self.status = Some(status);
            self.scope_stopped = true;
            if let Some(supervisor) = &mut self.supervisor
                && let Err(problem) = supervisor.finish()
            {
                self.control.record_failure(&problem);
                return Err(problem);
            }
            self.audit_finished()?;
        }
        if let Some(problem) = self.control.failure() {
            return Err(problem);
        }
        Ok(status)
    }

    fn stop(&mut self) -> io::Result<()> {
        #[cfg(test)]
        let stop_scope = self.test_stop;
        #[cfg(test)]
        let reap = self.test_reap;
        if self.stopped {
            return self.control.failure().map_or(Ok(()), Err);
        }

        self.control.done.store(true, Ordering::Release);
        let cleanup = match self.control.lifecycle() {
            Ok(_lifecycle) => {
                let signaled = if self.scope_stopped {
                    Ok(())
                } else {
                    stop_scope(&self.scope, &mut self.child)
                };
                if signaled.is_ok() {
                    self.scope_stopped = true;
                }
                // Keep the leader unreaped while its scope remains uncertain:
                // Unix supervisors and retries still borrow its numeric identity.
                signaled.and_then(|()| reap(&mut self.child, &mut self.status))
            }
            Err(problem) => Err(problem),
        };
        let joined = self.supervisor.as_mut().map_or(Ok(()), Supervisor::finish);
        if let Err(problem) = &joined {
            // The join handle has been consumed even on panic. Preserve that
            // failure so a subsequent no-op join cannot erase it.
            self.control.record_failure(problem);
        }
        let supervised = self.control.failure().map_or(Ok(()), Err);

        self.stdout.take();
        self.stderr.take();
        let scope_confirmed = cleanup.is_ok() && joined.is_ok();
        let staged = if scope_confirmed {
            let staged = self.stage.as_mut().map_or(Ok(()), Stage::cleanup);
            if staged.is_ok() {
                self.stage.take();
                self.reservation.take();
            }
            staged
        } else {
            Ok(())
        };
        let mut result = cleanup.and(joined).and(staged).and(supervised);
        if scope_confirmed && self.status.is_some() && self.terminator.is_some() {
            let audited = self.audit_finished();
            result = result.and(audited);
        }
        let cleanup_state = if result.is_ok() {
            SandboxCleanup::Complete
        } else {
            SandboxCleanup::Failed
        };
        self.inspection = self.inspection.clone().cleaned(cleanup_state);
        if self.audit_cleanup && self.terminator.is_some() {
            let audited = self.audit_cleanup(cleanup_state);
            result = result.and(audited);
        }
        if result.is_err() {
            self.inspection = self.inspection.clone().cleaned(SandboxCleanup::Failed);
        }
        self.stopped = result.is_ok();
        result
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage {
            wall_time: self.started.elapsed(),
            output_bytes: self.control.output_bytes.load(Ordering::Acquire),
            ..SandboxUsage::default()
        }
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.control.violation()
    }

    fn begin_background_acceptance(
        &mut self,
        key: CallResultKey,
    ) -> Result<(), crucible_core::SandboxError> {
        if self.invocation == SandboxInvocationMode::Foreground
            || self.call_result_key.is_none()
            || self.call_result_key != Some(key)
            || self.background_acceptance != BackgroundAcceptance::None
        {
            return Err(crucible_core::SandboxError::Lifecycle(io::Error::other(
                "sandbox background result identity is invalid",
            )));
        }
        self.background_acceptance = BackgroundAcceptance::Pending;
        Ok(())
    }

    fn complete_background_acceptance(
        &mut self,
        _receipt: CallResultReceipt,
    ) -> Result<(), crucible_core::SandboxError> {
        if self.background_acceptance != BackgroundAcceptance::Pending {
            return Err(crucible_core::SandboxError::Lifecycle(io::Error::other(
                "sandbox background result intent is unavailable",
            )));
        }
        self.background_acceptance = BackgroundAcceptance::Accepted;
        Ok(())
    }
}

impl std::fmt::Debug for LocalProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProcess")
            .field("inspection", &self.inspection)
            .field("running", &!self.stopped)
            .field("reservation", &self.reservation)
            .field("stage", &self.stage)
            .finish_non_exhaustive()
    }
}

impl Drop for LocalProcess {
    fn drop(&mut self) {
        let _ = self.stop();
        // Ordinary field destruction must not clean an uncertain workload's
        // files or advertise room for a replacement process. No thread or Arc
        // is leaked: the service retains only its already-bounded counter slot.
        if let Some(stage) = &mut self.stage {
            stage.retained = true;
        }
        if let Some(reservation) = self.reservation.take() {
            reservation.quarantine();
        }
    }
}

fn reap(child: &mut Child, status: &mut Option<ExitStatus>) -> io::Result<()> {
    if status.is_some() {
        return Ok(());
    }
    let deadline = Instant::now() + REAP;
    loop {
        if let Some(exited) = child.try_wait()? {
            *status = Some(exited);
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox process did not become reapable after termination",
            ));
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[cfg(unix)]
fn stop_scope(_scope: &Scope, child: &mut Child) -> io::Result<()> {
    Scope::stop(child)
}

#[cfg(windows)]
fn stop_scope(scope: &Scope, child: &mut Child) -> io::Result<()> {
    scope.stop(child)
}

/// Production process wrapper with a synthetic unconfined inspection record,
/// for lifetime tests that exercise the wrapper itself.
#[cfg(all(test, unix))]
pub(crate) fn testing(
    command: Command,
    speech: crucible_core::SandboxSpeech,
) -> Result<Box<dyn SandboxProcess>, crucible_core::SandboxError> {
    testing_local(command, speech, None).map(|process| Box::new(process) as Box<dyn SandboxProcess>)
}

#[cfg(all(test, unix))]
fn testing_local(
    command: Command,
    speech: crucible_core::SandboxSpeech,
    stage: Option<Stage>,
) -> Result<LocalProcess, crucible_core::SandboxError> {
    spawn_local(command, testing_plan(speech, stage)?)
}

#[cfg(all(test, unix))]
pub(super) fn testing_plan(
    speech: crucible_core::SandboxSpeech,
    stage: Option<Stage>,
) -> Result<SpawnPlan, crucible_core::SandboxError> {
    use crucible_core::{
        Ancestry, SandboxAudit, SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance,
        SandboxCapabilities, SandboxCleanup, SandboxFilesystemAccess, SandboxFilesystemProvenance,
        SandboxFilesystemRule, SandboxId, SandboxManifest, SandboxMode, SandboxNetworkPolicy,
        SandboxPolicy, ToolId,
    };

    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("test-process")
            .map_err(|_| crucible_core::SandboxError::InvalidInspection)?,
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .map_err(|_| crucible_core::SandboxError::InvalidInspection)?;
    let root = std::env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(crucible_core::SandboxError::Spawn)?;
    let rule = SandboxFilesystemRule::new(
        &root,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .map_err(|_| crucible_core::SandboxError::InvalidInspection)?;
    let policy = SandboxPolicy::new(
        SandboxMode::Off,
        [rule],
        root,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .map_err(|_| crucible_core::SandboxError::InvalidInspection)?;
    let manifest = SandboxManifest::empty();
    let inspection = SandboxInspection::new(
        SandboxId::new(),
        identity,
        SandboxCapabilities::none(),
        &policy,
        &manifest,
        false,
        Some("test-only unconfined process"),
        SandboxCleanup::Pending,
    )?;
    let active = Arc::new(AtomicUsize::new(0));
    let reservation = Reservation::take(active, 1)?;
    let sandbox = inspection.id();
    Ok(SpawnPlan {
        inspection,
        reservation,
        stage,
        limits: SandboxResourceLimits::default(),
        audit: SandboxAudit::new(Ancestry::new(), ToolId::new("test-process")),
        sandbox,
        audit_started: true,
        audit_cleanup: true,
        invocation: SandboxInvocationMode::Foreground,
        call_result_key: None,
        canceller: None,
        speech,
    })
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    mod cleanup;
    mod startup;

    use super::Stage;

    #[cfg(unix)]
    use std::io::Write as _;

    /// A command whose whole job is to say back what it was told, so a test can
    /// prove crucible was heard rather than only that a pipe existed.
    #[cfg(unix)]
    fn echoing() -> std::process::Command {
        let mut command = std::process::Command::new("/bin/sh");
        command.args(["-c", "read line; printf '%s\\n' \"heard $line\""]);
        command
    }

    /// The everyday case, and the one that must not change: a step reads
    /// end-of-file rather than waiting on a crucible that has nothing to say.
    #[cfg(unix)]
    #[test]
    fn a_command_nobody_speaks_to_has_no_standard_input_to_take() {
        let mut process = super::testing(echoing(), crucible_core::SandboxSpeech::Closed)
            .expect("a confined step");

        assert!(
            process.take_stdin().is_none(),
            "a command built Closed must not hand back a writer"
        );

        process.stop().expect("cleanup");
    }

    /// A peer is spoken to, and what it says back proves the bytes arrived
    /// rather than that a handle was returned.
    #[cfg(unix)]
    #[test]
    fn a_command_crucible_speaks_to_hears_what_it_said() {
        let mut process =
            super::testing(echoing(), crucible_core::SandboxSpeech::Held).expect("a confined peer");
        let mut stdin = process
            .take_stdin()
            .expect("a command built Held hands back a writer");

        stdin.write_all(b"a kettle\n").expect("crucible speaks");
        stdin.flush().expect("nothing is left in a buffer");
        drop(stdin);

        let said = drained(&mut process).expect("what the peer said back");
        assert_eq!(said.trim_end(), "heard a kettle");

        process.stop().expect("cleanup");
    }

    /// Standard input is handed over once. A second holder would be two writers
    /// interleaving frames into one stream the far end reads as one.
    #[cfg(unix)]
    #[test]
    fn standard_input_is_handed_over_once() {
        let mut process =
            super::testing(echoing(), crucible_core::SandboxSpeech::Held).expect("a confined peer");

        let first = process.take_stdin();
        let second = process.take_stdin();

        assert!(first.is_some(), "the first take hands back the writer");
        assert!(second.is_none(), "the second take hands back nothing");

        drop(first);
        process.stop().expect("cleanup");
    }

    /// Reads stdout until the far end closes it.
    ///
    /// Written without a panicking path because it is a helper rather than a
    /// test: the exemption the workspace grants covers `#[test]` bodies, and a
    /// helper that panics reports its own failure instead of the caller's.
    #[cfg(unix)]
    fn drained(process: &mut Box<dyn crucible_core::SandboxProcess>) -> std::io::Result<String> {
        let mut output = process
            .take_stdout()
            .ok_or_else(|| std::io::Error::other("the process has no stdout"))?;
        let mut said = Vec::new();
        let mut buffer = [0_u8; 256];
        loop {
            let taken = match output.read_ready(&mut buffer)? {
                crucible_core::SandboxRead::Bytes(read) => read,
                crucible_core::SandboxRead::Limited { retained, .. } => retained,
                crucible_core::SandboxRead::Pending => {
                    std::thread::yield_now();
                    continue;
                }
                crucible_core::SandboxRead::End => break,
            };
            let arrived = buffer
                .get(..taken)
                .ok_or_else(|| std::io::Error::other("more bytes were reported than read"))?;
            said.extend_from_slice(arrived);
        }
        String::from_utf8(said).map_err(std::io::Error::other)
    }

    #[test]
    fn stage_cleanup_is_explicit_and_idempotent() {
        let sample = crate::sample::Sample::new("sandbox-stage-cleanup");
        let root = sample.root().join("stage");
        std::fs::create_dir(&root).expect("stage fixture");
        std::fs::write(root.join("payload"), "temporary\n").expect("stage payload");
        let mut stage = Stage::new(root.clone());

        stage.cleanup().expect("first cleanup");
        stage.cleanup().expect("idempotent cleanup");

        assert!(!root.exists());
    }

    #[test]
    fn abandoned_preparation_releases_its_stage_and_reservation_together() {
        let sample = crate::sample::Sample::new("sandbox-prepared-cleanup");
        let root = sample.root().join("stage");
        std::fs::create_dir(&root).expect("stage fixture");
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut stage = Some(Stage::new(root.clone()));
        let mut reservation =
            Some(super::Reservation::take(std::sync::Arc::clone(&active), 1).expect("reservation"));

        let cleanup = super::cleanup_prepared_owners(&mut stage, &mut reservation);

        assert_eq!(cleanup, crucible_core::SandboxCleanup::Complete);
        assert!(stage.is_none());
        assert!(reservation.is_none());
        assert!(!root.exists());
        assert_eq!(active.load(std::sync::atomic::Ordering::Acquire), 0);
    }

    #[test]
    fn uncertain_preparation_cleanup_quarantines_its_reservation() {
        let sample = crate::sample::Sample::new("sandbox-prepared-quarantine");
        let root = sample.root().join("stage");
        std::fs::create_dir(&root).expect("stage fixture");
        let active = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut retained = Stage::new(root.clone());
        retained.retain();
        let mut stage = Some(retained);
        let mut reservation =
            Some(super::Reservation::take(std::sync::Arc::clone(&active), 1).expect("reservation"));

        let cleanup = super::cleanup_prepared_owners(&mut stage, &mut reservation);

        assert_eq!(cleanup, crucible_core::SandboxCleanup::Failed);
        assert!(stage.as_ref().is_some_and(Stage::retained));
        assert!(reservation.is_none());
        assert!(root.exists());
        assert_eq!(active.load(std::sync::atomic::Ordering::Acquire), 1);
        std::fs::remove_dir(&root).expect("remove quarantined test stage");
    }
}
