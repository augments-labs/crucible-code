//! One spawned command and its complete local cleanup scope.

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

/// A generated staging tree removed on every session/process drop path.
pub(super) struct Stage {
    root: std::path::PathBuf,
    retained: bool,
}

impl Stage {
    pub(super) fn new(root: std::path::PathBuf) -> Self {
        Self {
            root,
            retained: false,
        }
    }

    pub(super) fn manifest(&self) -> std::path::PathBuf {
        self.root.join("manifest")
    }

    pub(super) fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Keeps a quarantined tree for bounded startup recovery and inspection.
    pub(super) fn retain(&mut self) {
        self.retained = true;
    }

    pub(super) fn retained(&self) -> bool {
        self.retained
    }

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
}

/// Starts one command under an already negotiated process plan.
pub(super) fn spawn(
    mut command: Command,
    plan: SpawnPlan,
) -> Result<Box<dyn SandboxProcess>, crucible_core::SandboxError> {
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
    } = plan;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    let scope = Scope::new(&mut command);
    #[cfg(windows)]
    let scope = Scope::new(&mut command).map_err(crucible_core::SandboxError::Spawn)?;

    let mut child = command
        .spawn()
        .map_err(crucible_core::SandboxError::Spawn)?;
    let started = Instant::now();

    #[cfg(windows)]
    if let Err(source) = scope.attach(&child) {
        let _ = scope.stop(&mut child);
        let _ = child.wait();
        return Err(crucible_core::SandboxError::Spawn(source));
    }

    let terminator = match scope.terminator(&child) {
        Ok(terminator) => terminator,
        Err(source) => {
            let _ = stop_scope(&scope, &mut child);
            let _ = child.wait();
            return Err(crucible_core::SandboxError::Spawn(source));
        }
    };

    let control = Arc::new(Control::new(limits.output_bytes, audit, sandbox));

    let stdout = match child
        .stdout
        .take()
        .map(|output| PreparedOutput::new(output, Arc::clone(&control)))
        .transpose()
    {
        Ok(stdout) => stdout,
        Err(source) => {
            let _ = stop_scope(&scope, &mut child);
            let _ = child.wait();
            return Err(crucible_core::SandboxError::Spawn(source));
        }
    };
    let stderr = match child
        .stderr
        .take()
        .map(|output| PreparedOutput::new(output, Arc::clone(&control)))
        .transpose()
    {
        Ok(stderr) => stderr,
        Err(source) => {
            let _ = stop_scope(&scope, &mut child);
            let _ = child.wait();
            return Err(crucible_core::SandboxError::Spawn(source));
        }
    };

    let supervisor = if limits.command_time.is_some() || limits.output_bytes.is_some() {
        match Supervisor::start(
            Arc::clone(&control),
            terminator,
            limits
                .command_time
                .map(|allowed| started.checked_add(allowed).unwrap_or(started)),
        ) {
            Ok(supervisor) => Some(supervisor),
            Err(source) => {
                let _ = stop_scope(&scope, &mut child);
                let _ = child.wait();
                return Err(crucible_core::SandboxError::Spawn(source));
            }
        }
    } else {
        None
    };

    if audit_started
        && let Err(source) =
            control.audit(SandboxFactKind::Lifecycle(SandboxLifecycle::CommandStarted))
    {
        control.done.store(true, Ordering::Release);
        let _ = stop_scope(&scope, &mut child);
        let _ = child.wait();
        return Err(crucible_core::SandboxError::Audit(source));
    }

    Ok(Box::new(LocalProcess {
        child,
        scope,
        terminator,
        stdout: stdout.map(|pipe| Box::new(pipe) as Box<dyn SandboxOutput>),
        stderr: stderr.map(|pipe| Box::new(pipe) as Box<dyn SandboxOutput>),
        inspection,
        reservation: Some(reservation),
        stage,
        control,
        supervisor,
        status: None,
        scope_stopped: false,
        started,
        stopped: false,
        audit_state: AuditState::default(),
        audit_cleanup,
        invocation,
        call_result_key,
        background_acceptance: BackgroundAcceptance::None,
    }))
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
    terminator: Terminator,
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
    cleanup: bool,
}

impl LocalProcess {
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
        if self.audit_state.cleanup {
            return Ok(());
        }
        self.control
            .audit(SandboxFactKind::Cleanup(cleanup))
            .map_err(io::Error::other)?;
        self.audit_state.cleanup = true;
        Ok(())
    }
}

impl SandboxProcess for LocalProcess {
    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stderr.take()
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(status) = self.status {
            self.audit_finished()?;
            return Ok(Some(status));
        }
        if let Some(problem) = self.control.failure() {
            return Err(problem);
        }

        let status = {
            let _lifecycle = self.control.lifecycle()?;
            let status = self.scope.try_wait(&mut self.child, self.terminator)?;
            if status.is_some() {
                self.control.done.store(true, Ordering::Release);
            }
            status
        };
        if let Some(status) = status {
            self.status = Some(status);
            self.scope_stopped = true;
            if let Some(supervisor) = &mut self.supervisor {
                supervisor.finish()?;
            }
            self.audit_finished()?;
        }
        if let Some(problem) = self.control.failure() {
            return Err(problem);
        }
        Ok(status)
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.stopped {
            return self.control.failure().map_or(Ok(()), Err);
        }

        let cleanup = {
            let _lifecycle = self.control.lifecycle()?;
            self.control.done.store(true, Ordering::Release);
            let signaled = if self.scope_stopped {
                Ok(())
            } else {
                stop_scope(&self.scope, &mut self.child)
            };
            if signaled.is_ok() {
                self.scope_stopped = true;
            }
            let reaped = reap(&mut self.child, &mut self.status);
            signaled.and(reaped)
        };
        let joined = self.supervisor.as_mut().map_or(Ok(()), Supervisor::finish);
        let supervised = self.control.failure().map_or(Ok(()), Err);

        self.stdout.take();
        self.stderr.take();
        let staged = self.stage.as_mut().map_or(Ok(()), Stage::cleanup);
        if staged.is_ok() {
            self.stage.take();
        }
        self.reservation.take();
        self.stopped = true;
        let cleanup_succeeded = cleanup.is_ok() && joined.is_ok() && staged.is_ok();
        let cleanup_state = if cleanup_succeeded {
            SandboxCleanup::Complete
        } else {
            SandboxCleanup::Failed
        };
        self.inspection = self.inspection.clone().cleaned(cleanup_state);
        let mut result = cleanup.and(joined).and(staged).and(supervised);
        if self.status.is_some() {
            let audited = self.audit_finished();
            result = result.and(audited);
        }
        if self.audit_cleanup {
            let audited = self.audit_cleanup(cleanup_state);
            result = result.and(audited);
        }
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
#[cfg(test)]
pub(crate) fn testing(
    command: Command,
) -> Result<Box<dyn SandboxProcess>, crucible_core::SandboxError> {
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
    spawn(
        command,
        SpawnPlan {
            inspection,
            reservation,
            stage: None,
            limits: SandboxResourceLimits::default(),
            audit: SandboxAudit::new(Ancestry::new(), ToolId::new("test-process")),
            sandbox,
            audit_started: true,
            audit_cleanup: true,
            invocation: SandboxInvocationMode::Foreground,
            call_result_key: None,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::Stage;

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
}
