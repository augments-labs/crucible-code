//! One spawned command and its complete local cleanup scope.
//!
//! Failed cleanup remains retryable while this owner exists. A stage and its
//! admission slot are released only after process cleanup is confirmed. If Drop
//! still cannot finish, the stage is retained and the slot stays consumed until
//! the service restarts. This bounds new admissions without claiming that an
//! unconfirmed workload died or retaining an unbounded cleanup thread.
//!
//! **A status task of its own.** Each command is watched by a task on the
//! runtime its service was handed, which the process owns and its stop
//! aborts. Every [`SUPERVISE`] it enforces the command-time limit, starts the
//! cancel of a violation, and looks at the leader, reaping it and keeping its
//! status once it has exited. A look never waits, so neither the task nor a
//! caller asking for the status holds up a runtime thread or the other.
//!
//! The task shares the runtime's worker threads with whatever else runs
//! there. A worker held inside other work delays its next pass, and with it a
//! deadline or output-limit kill, so work spawned onto that runtime has to
//! leave a worker free for it.
//!
//! **Nothing waits behind a cancel.** The leader, its scope and what has been
//! seen of them share one lock, held only to look at the leader or signal its
//! scope, and by a stop through its bounded reap. A backend's cancel, which may
//! wait for the leader within a budget of its own, runs on a thread of its own
//! without that lock; the kill that follows it takes the lock only to make
//! sure the leader has not been reaped, so a signal never reaches a process
//! identity that may have been reused. A status asked for meanwhile answers
//! from a look, and a stop kills and reaps the command itself and then waits a
//! bounded time for the cancel to end.

use std::io::{self, Write as _};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::thread;
use std::time::{Duration, Instant};

use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxAudit, SandboxCleanup, SandboxFactKind, SandboxInspection, SandboxInvocationMode,
    SandboxLifecycle, SandboxOutput, SandboxProcess, SandboxRead, SandboxResourceLimits,
    SandboxUsage, SandboxViolation,
};
use crucible_storage::{CallResultKey, CallResultReceipt};
use crucible_types::SandboxId;

use crate::platform::{self, ReadState, Scope, Stream, Terminator};

/// Absolute ceiling even where a policy omits a smaller one.
pub(super) const MAX_LOCAL_COMMANDS: usize = 16;

/// Bounded reap interval inside synchronous `stop`. Not only Drop reaches it.
const REAP: Duration = Duration::from_millis(250);

/// How often a command's status task looks at it. It bounds deadline
/// overshoot, and how late an exit nobody asked about is seen, without
/// spinning.
const SUPERVISE: Duration = Duration::from_millis(5);

/// How long a stop waits for a violation's cancel to end once it has killed
/// and reaped the command. A cancel waits for the leader's exit, which that
/// kill has just brought about, so it ends within a poll of its own; one that
/// has not ended by then is reported as failed cleanup and left for a retry.
const CANCELLED: Duration = Duration::from_millis(250);

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
    ) -> Result<Self, crucible_sandbox::SandboxError> {
        let maximum = maximum.min(MAX_LOCAL_COMMANDS);
        let reserved = active.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < maximum).then_some(current.saturating_add(1))
        });
        if reserved.is_err() {
            return Err(crucible_sandbox::SandboxError::Concurrency);
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

#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
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
/// arrives before the kill. It runs on a thread of its own, holding nothing a
/// status or a stop waits on.
pub(super) type Canceller = Box<dyn Fn(u32) -> io::Result<()> + Send + Sync>;

/// Spawns `command` inside a platform process-tree scope.
pub(super) struct SpawnPlan {
    pub(super) network: Option<super::network::Mediator>,
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
    /// A cooperative stop the status task tries before the group kill when a
    /// limit is broken. Killing the Linux launcher alone leaves its PID
    /// namespace running, so the Linux backend asks the broker to end the
    /// workload and report its wait status; the kill stays as the backstop.
    pub(super) canceller: Option<Canceller>,
    /// The runtime the command's status task runs on. Without one no command
    /// is started.
    pub(super) runtime: Option<tokio::runtime::Handle>,
    /// Whether crucible keeps the writing end of standard input. Decided with
    /// the command, because a pipe cannot be attached after a spawn.
    pub(super) speech: crucible_sandbox::SandboxSpeech,
    /// Trusted bytes written after containment starts and before caller input
    /// can take the same pipe. Native brokers consume a bounded launch frame
    /// and leave any later command-protocol bytes for the workload.
    pub(super) startup_input: Option<Vec<u8>>,
    /// The exact bytes of every credential value the command's environment
    /// carries, from [`credential_values`], masked on standard error, and on
    /// standard output unless the command is spoken to.
    pub(super) credentials: Vec<Vec<u8>>,
}

/// What of `environment` is masked in the command's output: the encoded bytes
/// of every credential value, each of which is non-empty.
pub(super) fn credential_values(
    environment: &crucible_sandbox::SandboxEnvironment,
) -> Vec<Vec<u8>> {
    environment
        .credential_values()
        .map(|value| value.as_encoded_bytes().to_vec())
        .collect()
}

/// Cleans preparation owners that never reached a child process.
///
/// A cleanup failure retains both the stage and its bounded admission slot so
/// a later command cannot reuse authority whose disposal was not confirmed.
#[cfg(any(
    target_os = "macos",
    target_os = "windows",
    all(test, target_os = "linux")
))]
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
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(super) fn cleanup_unspawned(mut plan: SpawnPlan) -> SandboxCleanup {
    let mut reservation = Some(plan.reservation);
    let network = plan
        .network
        .as_mut()
        .map_or(Ok(()), super::network::Mediator::stop);
    if network.is_err() {
        if let Some(stage) = &mut plan.stage {
            stage.retain();
        }
        if let Some(reservation) = reservation.take() {
            reservation.quarantine();
        }
        return SandboxCleanup::Failed;
    }
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
) -> Result<Box<dyn SandboxProcess>, crucible_sandbox::SandboxError> {
    spawn_local(command, plan).map(|process| Box::new(process) as Box<dyn SandboxProcess>)
}

/// [`spawn`], with the mark a stop made from outside the process sets on it.
#[cfg(target_os = "linux")]
pub(super) fn spawn_marked(
    command: Command,
    plan: SpawnPlan,
) -> Result<(Box<dyn SandboxProcess>, StopMark), crucible_sandbox::SandboxError> {
    spawn_local(command, plan).map(|process| {
        let mark = StopMark(Arc::clone(&process.control));
        (Box::new(process) as Box<dyn SandboxProcess>, mark)
    })
}

/// Tells a command's output streams that crucible is stopping it, for an
/// owner that can end the command's output before [`SandboxProcess::stop`]
/// reaches the process: the Linux projection cancels through its broker.
#[cfg(target_os = "linux")]
pub(super) struct StopMark(Arc<Control>);

#[cfg(target_os = "linux")]
impl StopMark {
    /// Marks the command cut, unless it has already been seen to exit. Called
    /// before anything that can end its output.
    pub(super) fn stopping(&self) {
        self.0.stopping();
    }
}

fn spawn_local(
    command: Command,
    plan: SpawnPlan,
) -> Result<LocalProcess, crucible_sandbox::SandboxError> {
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
) -> Result<LocalProcess, crucible_sandbox::SandboxError> {
    #[cfg(test)]
    let stop_scope = test_stop;
    let SpawnPlan {
        network,
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
        runtime,
        speech,
        startup_input,
        credentials,
    } = plan;
    let Some(runtime) = runtime else {
        return Err(failed_before_spawn(
            crucible_sandbox::SandboxError::Spawn(io::Error::other(
                "this sandbox service was given no runtime to watch its commands on",
            )),
            stage,
            reservation,
            network,
        ));
    };
    command
        .stdin(match (speech, startup_input.is_some()) {
            // A step that reads gets end-of-file, which is an answer. A peer
            // gets a pipe crucible keeps, because it is going to be spoken to.
            (crucible_sandbox::SandboxSpeech::Closed, false) => Stdio::null(),
            (crucible_sandbox::SandboxSpeech::Closed, true)
            | (crucible_sandbox::SandboxSpeech::Held, _) => Stdio::piped(),
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    let scope = Scope::new(&mut command);
    #[cfg(windows)]
    let scope = match Scope::new(&mut command, limits) {
        Ok(scope) => scope,
        Err(source) => {
            return Err(failed_before_spawn(
                crucible_sandbox::SandboxError::Spawn(source),
                stage,
                reservation,
                network,
            ));
        }
    };

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(source) => {
            return Err(failed_before_spawn(
                crucible_sandbox::SandboxError::Spawn(source),
                stage,
                reservation,
                network,
            ));
        }
    };
    let started = Instant::now();
    let stdin = child.stdin.take();
    let control = Arc::new(Control::new(limits.output_bytes, audit, sandbox));
    // Own every resource before the first fallible initialization operation.
    // No caller can observe this private, unfinished process. Stop and Drop
    // use the scope and child directly, without needing a borrowed terminator.
    let mut process = LocalProcess {
        watched: Arc::new(Mutex::new(Watched {
            child,
            scope,
            status: None,
            scope_stopped: false,
            cancel: None,
        })),
        stdin,
        input_thread: platform::InputThread::default(),
        terminator: None,
        stdout: None,
        stderr: None,
        inspection,
        reservation: Some(reservation),
        stage,
        network,
        control,
        watch: None,
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
    match process.initialize(
        audit_started,
        StartupInput {
            bytes: startup_input,
            speech,
        },
    ) {
        Ok(terminator) => {
            process.terminator = Some(terminator);
            process.protect_outputs(credentials, speech);
            process.watch(&runtime, terminator, limits, canceller);
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
    startup: crucible_sandbox::SandboxError,
    mut stage: Option<Stage>,
    reservation: Reservation,
    mut network: Option<super::network::Mediator>,
) -> crucible_sandbox::SandboxError {
    let stopped = network
        .as_mut()
        .map_or(Ok(()), super::network::Mediator::stop);
    match stopped.and_then(|()| stage.as_mut().map_or(Ok(()), Stage::cleanup)) {
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
    startup: crucible_sandbox::SandboxError,
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
    startup: crucible_sandbox::SandboxError,
    cleanup: io::Error,
) -> crucible_sandbox::SandboxError {
    crucible_sandbox::SandboxError::Lifecycle(io::Error::new(
        cleanup.kind(),
        StartupCleanupFailure { startup, cleanup },
    ))
}

/// A stop's scope cleanup and its input thread's end, as one answer: the
/// scope's failure where it failed, carrying the thread's beside it where both
/// did, so neither is lost to a caller deciding whether to retry.
fn stopped_with_input(scope: io::Result<()>, input: io::Result<()>) -> io::Result<()> {
    match (scope, input) {
        (Err(scope), Err(input)) => Err(io::Error::new(
            scope.kind(),
            InputAlsoFailed { scope, input },
        )),
        (scope, input) => scope.and(input),
    }
}

/// A stop whose scope cleanup failed, and whose input thread's end failed too.
#[derive(Debug)]
struct InputAlsoFailed {
    scope: io::Error,
    input: io::Error,
}

impl std::fmt::Display for InputAlsoFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "sandbox process scope cleanup failed ({:?}); the thread writing its input did not end either ({:?})",
            self.scope.kind(),
            self.input.kind(),
        )
    }
}

impl std::error::Error for InputAlsoFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.scope)
    }
}

/// Shared hard-limit state used by both output streams and the status task.
struct Control {
    /// Set once the leader has been seen to exit, or once a stop has begun.
    done: AtomicBool,
    violation: AtomicU8,
    /// Set when crucible stops a command it has not seen exit.
    stopped_running: AtomicBool,
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
            done: AtomicBool::new(false),
            violation: AtomicU8::new(NO_VIOLATION),
            stopped_running: AtomicBool::new(false),
            output_remaining: output_limit.map(AtomicU64::new),
            output_bytes: AtomicU64::new(0),
            failure: Mutex::new(None),
            audit,
            sandbox,
        }
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

    /// Marks a stop of a command not yet seen to exit, which is what ends it.
    /// `done` is set once an exit is seen, or once a stop has begun, and that
    /// stop has already made this decision. A stop that lands after the
    /// command exited, but before crucible saw it, masks what was held back,
    /// and the stop is then what crucible reports; on Linux that window opens
    /// at the workload's exit and lasts until the broker has cleaned up and
    /// exited.
    fn stopping(&self) {
        if !self.done.load(Ordering::Acquire) {
            self.stopped_running.store(true, Ordering::Release);
        }
    }

    /// Whether crucible cut the command short: by its output or command-time
    /// limit, or by a stop while it was still running. Either way, its output
    /// ended early.
    fn interrupted(&self) -> bool {
        self.violation().is_some() || self.stopped_running.load(Ordering::Acquire)
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

    fn audit(&self, kind: SandboxFactKind) -> Result<(), crucible_sandbox::SandboxAuditError> {
        self.audit.record(self.sandbox, kind)
    }
}

fn atomic_saturating_add(value: &AtomicU64, increment: u64) {
    let _ = value.fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
        Some(current.saturating_add(increment))
    });
}

/// The leader and its scope, and what has been seen of them.
///
/// Shared, behind one lock, by the process, its status task and the thread a
/// violation's cancel runs on. The lock is held only to look at the leader or
/// to signal its scope, neither of which waits, except by a stop, which holds
/// it through its bounded kill and reap and which the status task steps round.
/// It is what keeps a signal from reaching the scope after the leader has been
/// reaped, once its numeric identity may name another process.
struct Watched {
    child: Child,
    scope: Scope,
    /// The leader's status, once it has been reaped.
    status: Option<ExitStatus>,
    /// Whether the whole scope is known to have been stopped.
    scope_stopped: bool,
    /// The thread a violation's cancel runs on, until a stop joins it.
    cancel: Option<thread::JoinHandle<()>>,
}

impl Watched {
    /// Looks at the leader once without waiting, reaping it and keeping its
    /// status if it has exited, after its scope has been stopped.
    fn look(
        &mut self,
        terminator: Terminator,
        control: &Control,
    ) -> io::Result<Option<ExitStatus>> {
        if self.status.is_some() {
            return Ok(self.status);
        }
        let status = self.scope.try_wait(&mut self.child, terminator)?;
        if status.is_some() {
            self.status = status;
            self.scope_stopped = true;
            control.done.store(true, Ordering::Release);
        }
        Ok(status)
    }
}

/// Locks `watched`, answering a poisoned lock as an error.
fn watched(watched: &Mutex<Watched>) -> io::Result<MutexGuard<'_, Watched>> {
    watched.lock().map_err(|_| poisoned())
}

fn poisoned() -> io::Error {
    io::Error::other("sandbox process status lock was poisoned")
}

/// The status task's work: the command-time limit enforced, a violation's
/// cancel and kill started, and the leader looked at, every [`SUPERVISE`]
/// until it has been reaped or a stop has begun.
struct Watch {
    watched: Arc<Mutex<Watched>>,
    control: Arc<Control>,
    terminator: Terminator,
    deadline: Option<Instant>,
    /// Taken when a violation is first seen, so it is tried once.
    canceller: Option<Canceller>,
    /// Whether a violation has been acted on.
    cut: bool,
    /// Whether the watch ended the way it ends, rather than being dropped
    /// before the command did: by a panic, or a runtime shut down under it.
    ended: bool,
}

impl Watch {
    async fn run(mut self) {
        loop {
            tokio::time::sleep(SUPERVISE).await;
            if !self.look() {
                self.ended = true;
                return;
            }
        }
    }

    /// One pass, which never waits: a lock held by someone else is left for
    /// the next. Whether the command still needs watching.
    fn look(&mut self) -> bool {
        // The leader has been seen to exit, or a stop has begun and does the
        // rest.
        if self.control.done.load(Ordering::Acquire) {
            return false;
        }
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.control.mark(SandboxViolation::CommandTime);
        }
        let shared = Arc::clone(&self.watched);
        let mut watched = match shared.try_lock() {
            Ok(watched) => watched,
            Err(TryLockError::WouldBlock) => return true,
            Err(TryLockError::Poisoned(_)) => {
                self.control.record_failure(&poisoned());
                return false;
            }
        };
        // Checked again under the lock. A stop says it has begun before it
        // takes this lock, so a cancel is started here only by a pass that
        // held the lock first, and that stop then finds the cancel and joins
        // it; once the stop has the lock, no cancel is started.
        if watched.status.is_some() || self.control.done.load(Ordering::Acquire) {
            return false;
        }
        if !self.cut && self.control.violation().is_some() {
            self.cut = true;
            self.cut(&mut watched);
        }
        // What went wrong is the caller's to hear, from a look of its own.
        !matches!(watched.look(self.terminator, &self.control), Ok(Some(_)))
    }

    /// Ends a command that broke a limit: through the backend's cancel where
    /// it has one, on a thread of its own, then the kill.
    fn cut(&mut self, watched: &mut Watched) {
        let Some(cancel) = self.canceller.take() else {
            if let Err(problem) = self.terminator.stop() {
                self.control.record_failure(&problem);
            }
            return;
        };
        let shared = Arc::clone(&self.watched);
        let control = Arc::clone(&self.control);
        let terminator = self.terminator;
        let leader = watched.child.id();
        let started = thread::Builder::new()
            .name("crucible-sandbox-cancel".into())
            .spawn(move || {
                // A cancellation the backend cannot deliver leaves the kill.
                let _ = cancel(leader);
                killed_unless_reaped(&shared, terminator, &control);
            });
        match started {
            Ok(thread) => watched.cancel = Some(thread),
            // Killed without the cancel it needed first, which is kept as the
            // failure it is.
            Err(problem) => {
                self.control.record_failure(&problem);
                if let Err(problem) = self.terminator.stop() {
                    self.control.record_failure(&problem);
                }
            }
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        if !self.ended && !self.control.done.load(Ordering::Acquire) {
            self.control.record_failure(&io::Error::other(
                "the task watching a sandbox process ended before the process did",
            ));
        }
    }
}

/// The kill after a cancel, sent only while the leader is unreaped, so its
/// process group is still the command's.
fn killed_unless_reaped(watched: &Mutex<Watched>, terminator: Terminator, control: &Control) {
    match self::watched(watched) {
        Ok(watched) if watched.status.is_none() => {
            if let Err(problem) = terminator.stop() {
                control.record_failure(&problem);
            }
        }
        Ok(_) => {}
        Err(problem) => control.record_failure(&problem),
    }
}

/// A pipe put into non-blocking mode before the process handle escapes, and
/// counted against the command's output budget as it is read.
struct PreparedOutput {
    inner: Box<dyn Stream>,
    control: Arc<Control>,
}

impl PreparedOutput {
    const fn new(inner: Box<dyn Stream>, control: Arc<Control>) -> Self {
        Self { inner, control }
    }

    /// What a read of the pipe found, counted against the output budget.
    fn counted(&self, read: ReadState) -> SandboxRead {
        match read {
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
        }
    }
}

impl SandboxOutput for PreparedOutput {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if buffer.is_empty() {
            return Ok(SandboxRead::Pending);
        }
        let read = self.inner.read_ready(buffer)?;
        Ok(self.counted(read))
    }

    /// Waits on the pipe as the platform waits on one (see
    /// [`crate::platform`]), and counts what it read as a read without waiting
    /// is counted.
    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<SandboxRead>> {
        Box::pin(async move {
            if buffer.is_empty() {
                return Ok(SandboxRead::Bytes(0));
            }
            let read = self.inner.read(buffer).await?;
            Ok(self.counted(read))
        })
    }
}

/// Masks `patterns` in one output stream of the command `control` governs,
/// which says whether crucible cut that command short.
fn protect_output(
    output: Box<dyn SandboxOutput>,
    patterns: Vec<Vec<u8>>,
    control: &Arc<Control>,
) -> Box<dyn SandboxOutput> {
    let control = Arc::clone(control);
    Box::new(
        super::redaction::ProtectedOutput::new(output, patterns)
            .interrupted_by(Box::new(move || control.interrupted())),
    )
}

/// The process, its process-tree scope, streams, stage, and reservation.
struct LocalProcess {
    /// The leader and its scope, shared with the status task.
    watched: Arc<Mutex<Watched>>,
    /// Present only after initialization has fully succeeded.
    terminator: Option<Terminator>,
    /// The writing end of a peer's input, until somebody takes it. Dropping it
    /// unread is what closes the far end's stdin.
    stdin: Option<ChildStdin>,
    /// The thread, where the platform starts one, that the input taken
    /// asynchronously is written on; `stop` ends and joins it.
    input_thread: platform::InputThread,
    stdout: Option<Box<dyn SandboxOutput>>,
    stderr: Option<Box<dyn SandboxOutput>>,
    inspection: SandboxInspection,
    reservation: Option<Reservation>,
    stage: Option<Stage>,
    network: Option<super::network::Mediator>,
    control: Arc<Control>,
    /// The status task, which `stop` aborts. It never blocks on the shared
    /// lock and between passes only sleeps, so it ends at its next poll.
    watch: Option<tokio::task::JoinHandle<()>>,
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
        audit_started: bool,
        startup: StartupInput,
    ) -> Result<Terminator, crucible_sandbox::SandboxError> {
        let mut watched =
            self::watched(&self.watched).map_err(crucible_sandbox::SandboxError::Spawn)?;
        let Watched { child, scope, .. } = &mut *watched;
        #[cfg(windows)]
        scope
            .attach(child)
            .map_err(crucible_sandbox::SandboxError::Spawn)?;
        let terminator = scope
            .terminator(child)
            .map_err(crucible_sandbox::SandboxError::Spawn)?;
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        drop(watched);
        if let Some(startup_input) = startup.bytes {
            let input = self.stdin.as_mut().ok_or_else(|| {
                crucible_sandbox::SandboxError::Spawn(io::Error::other(
                    "sandbox launcher startup pipe is unavailable",
                ))
            })?;
            input
                .write_all(&startup_input)
                .and_then(|()| input.flush())
                .map_err(crucible_sandbox::SandboxError::Spawn)?;
            if startup.speech == crucible_sandbox::SandboxSpeech::Closed {
                self.stdin.take();
            }
        }
        self.stdout = stdout
            .map(platform::stream)
            .transpose()
            .map_err(crucible_sandbox::SandboxError::Spawn)?
            .map(|pipe| {
                Box::new(PreparedOutput::new(pipe, Arc::clone(&self.control)))
                    as Box<dyn SandboxOutput>
            });
        self.stderr = stderr
            .map(platform::stream)
            .transpose()
            .map_err(crucible_sandbox::SandboxError::Spawn)?
            .map(|pipe| {
                Box::new(PreparedOutput::new(pipe, Arc::clone(&self.control)))
                    as Box<dyn SandboxOutput>
            });
        if audit_started {
            self.control
                .audit(SandboxFactKind::Lifecycle(SandboxLifecycle::CommandStarted))?;
        }
        Ok(terminator)
    }

    /// Starts the command's status task on `runtime`, with the command-time
    /// limit counted from the spawn.
    fn watch(
        &mut self,
        runtime: &tokio::runtime::Handle,
        terminator: Terminator,
        limits: SandboxResourceLimits,
        canceller: Option<Canceller>,
    ) {
        self.watch = Some(
            runtime.spawn(
                Watch {
                    watched: Arc::clone(&self.watched),
                    control: Arc::clone(&self.control),
                    terminator,
                    deadline: limits
                        .command_time
                        .map(|allowed| self.started.checked_add(allowed).unwrap_or(self.started)),
                    canceller,
                    cut: false,
                    ended: false,
                }
                .run(),
            ),
        );
    }

    /// Masks what the command could print that it was given as a secret: its
    /// proxy's credential where it has one, on both output streams, and
    /// `credentials`, every credential its environment carries, on standard
    /// error and on standard output unless the command is spoken to.
    ///
    /// A spoken-to command's standard output is a protocol. There a value is
    /// escaped as the protocol escapes it, so its own bytes need not appear,
    /// and a short one masked in place would rewrite the frames around it; the
    /// peer decodes each frame and masks what it keeps. A proxy credential is
    /// crucible's own and long, and masked there as everywhere.
    ///
    /// Each stream is wrapped once, and not at all where there is nothing to
    /// mask. Nothing reads either stream before the process is handed back,
    /// so wrapping them once initialization has succeeded loses no byte.
    fn protect_outputs(
        &mut self,
        credentials: Vec<Vec<u8>>,
        speech: crucible_sandbox::SandboxSpeech,
    ) {
        let proxy = self
            .network
            .as_ref()
            .map(super::network::Mediator::masked)
            .unwrap_or_default();
        let mut printed = proxy.clone();
        if speech == crucible_sandbox::SandboxSpeech::Closed {
            printed.extend(credentials.iter().cloned());
        }
        let mut muttered = proxy;
        muttered.extend(credentials);
        if !printed.is_empty() {
            self.stdout = self
                .stdout
                .take()
                .map(|output| protect_output(output, printed, &self.control));
        }
        if !muttered.is_empty() {
            self.stderr = self
                .stderr
                .take()
                .map(|output| protect_output(output, muttered, &self.control));
        }
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

    /// What [`SandboxProcess::stop`] does, synchronously, so `Drop` and a
    /// failed startup can stop the process without a future to drive.
    fn stop(&mut self) -> io::Result<()> {
        #[cfg(test)]
        let stop_scope = self.test_stop;
        #[cfg(test)]
        let reap = self.test_reap;
        if self.stopped {
            return self.control.failure().map_or(Ok(()), Err);
        }

        // Before the kill: a command not yet seen to exit is being cut short.
        self.control.stopping();
        self.control.done.store(true, Ordering::Release);
        // The status task never waits, so it ends at its next poll, and a look
        // it is in the middle of finishes under the lock first.
        if let Some(watch) = &self.watch {
            watch.abort();
        }
        let (cleanup, cancel) = match watched(&self.watched) {
            Ok(mut watched) => {
                let Watched {
                    child,
                    scope,
                    status,
                    scope_stopped,
                    cancel,
                } = &mut *watched;
                let signaled = if *scope_stopped {
                    Ok(())
                } else {
                    stop_scope(scope, child)
                };
                if signaled.is_ok() {
                    *scope_stopped = true;
                }
                // Keep the leader unreaped while its scope remains uncertain:
                // a cancel's kill and a retry still borrow its numeric identity.
                (signaled.and_then(|()| reap(child, status)), cancel.take())
            }
            Err(problem) => (Err(problem), None),
        };
        let joined = self.joined(cancel);
        let supervised = self.control.failure().map_or(Ok(()), Err);

        self.stdout.take();
        self.stderr.take();
        let network = self
            .network
            .as_mut()
            .map_or(Ok(()), super::network::Mediator::stop);
        // After the scope's stop, which closed the pipe's other end where it
        // succeeded, so a write the thread was parked in has failed or is
        // abandoned here. Ended whether or not it did: a thread left unjoined
        // for a stop to retry is one more thing the retry has to find.
        let input = self.input_thread.end();
        let scope_confirmed = cleanup.is_ok() && joined.is_ok() && network.is_ok() && input.is_ok();
        let cleanup = stopped_with_input(cleanup, input);
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
        let mut result = cleanup.and(joined).and(network).and(staged).and(supervised);
        if scope_confirmed && self.reaped() && self.terminator.is_some() {
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

    /// Whether the leader has been reaped.
    fn reaped(&self) -> bool {
        watched(&self.watched).is_ok_and(|watched| watched.status.is_some())
    }

    /// Joins the thread a violation's cancel runs on, giving it [`CANCELLED`]
    /// to end. One still running is kept for a later stop to join.
    fn joined(&self, cancel: Option<thread::JoinHandle<()>>) -> io::Result<()> {
        let Some(cancel) = cancel else {
            return Ok(());
        };
        let deadline = Instant::now() + CANCELLED;
        while !cancel.is_finished() {
            if Instant::now() >= deadline {
                if let Ok(mut watched) = watched(&self.watched) {
                    watched.cancel = Some(cancel);
                }
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the cancel of a sandbox violation had not ended",
                ));
            }
            thread::sleep(SUPERVISE);
        }
        cancel.join().map_err(|_| {
            // The join has been consumed. Keep its failure, so a later stop,
            // finding nothing to join, cannot report cleanup as confirmed.
            let problem = io::Error::other("sandbox violation cancel panicked");
            self.control.record_failure(&problem);
            problem
        })
    }

    /// What [`SandboxProcess::begin_background_acceptance`] does, synchronously.
    fn begin_background_acceptance(
        &mut self,
        key: CallResultKey,
    ) -> Result<(), crucible_sandbox::SandboxError> {
        if self.invocation == SandboxInvocationMode::Foreground
            || self.call_result_key.is_none()
            || self.call_result_key != Some(key)
            || self.background_acceptance != BackgroundAcceptance::None
        {
            return Err(crucible_sandbox::SandboxError::Lifecycle(io::Error::other(
                "sandbox background result identity is invalid",
            )));
        }
        self.background_acceptance = BackgroundAcceptance::Pending;
        Ok(())
    }

    /// What [`SandboxProcess::complete_background_acceptance`] does,
    /// synchronously.
    fn complete_background_acceptance(
        &mut self,
        _receipt: CallResultReceipt,
    ) -> Result<(), crucible_sandbox::SandboxError> {
        if self.background_acceptance != BackgroundAcceptance::Pending {
            return Err(crucible_sandbox::SandboxError::Lifecycle(io::Error::other(
                "sandbox background result intent is unavailable",
            )));
        }
        self.background_acceptance = BackgroundAcceptance::Accepted;
        Ok(())
    }
}

struct StartupInput {
    bytes: Option<Vec<u8>>,
    speech: crucible_sandbox::SandboxSpeech,
}

impl SandboxProcess for LocalProcess {
    fn take_stdin(&mut self) -> Option<Box<dyn std::io::Write + Send>> {
        self.stdin
            .take()
            .map(|input| Box::new(input) as Box<dyn std::io::Write + Send>)
    }

    /// The pipe written as the platform writes one asynchronously (see
    /// [`crate::platform`]). A thread the platform starts for it belongs to
    /// this process, and `stop` ends and joins it.
    fn take_async_stdin(&mut self) -> Option<Box<dyn crucible_sandbox::SandboxInput>> {
        self.stdin
            .take()
            .map(|pipe| platform::input(pipe, &self.input_thread))
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
        // The lock is only ever held to look or to signal, never across a
        // cancel, so this answers at once whatever a cancel is doing.
        let status = {
            let mut watched = watched(&self.watched)?;
            if let Some(status) = watched.status {
                if !watched.scope_stopped {
                    return Err(io::Error::other(
                        "sandbox process scope cleanup is unconfirmed",
                    ));
                }
                Some(status)
            } else {
                let terminator = self.terminator.ok_or_else(|| {
                    io::Error::other("sandbox process initialization is incomplete")
                })?;
                watched.look(terminator, &self.control)?
            }
        };
        if status.is_some() {
            self.audit_finished()?;
        }
        if let Some(problem) = self.control.failure() {
            return Err(problem);
        }
        Ok(status)
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move { self.stop() })
    }

    /// The same stop the future above drives, for the owners that have no
    /// future to drive. What bounds it is what bounds that body: the scope's
    /// own reap bound, the cancel's join bound, and the network proxy's stop
    /// bound, each reported as failed cleanup where it gives out.
    fn stop_sync(&mut self) -> io::Result<()> {
        LocalProcess::stop(self)
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
    ) -> BoxFuture<'_, Result<(), crucible_sandbox::SandboxError>> {
        Box::pin(async move { self.begin_background_acceptance(key) })
    }

    fn complete_background_acceptance(
        &mut self,
        receipt: CallResultReceipt,
    ) -> BoxFuture<'_, Result<(), crucible_sandbox::SandboxError>> {
        Box::pin(async move { self.complete_background_acceptance(receipt) })
    }
}

impl std::fmt::Debug for LocalProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProcess")
            .field("inspection", &self.inspection)
            .field("running", &!self.stopped)
            .field("reservation", &self.reservation)
            .field("stage", &self.stage)
            .field("network", &self.network)
            .finish_non_exhaustive()
    }
}

impl Drop for LocalProcess {
    fn drop(&mut self) {
        let _ = self.stop();
        // Ordinary field destruction must not clean an uncertain workload's
        // files or advertise room for a replacement process: the service
        // retains only its already-bounded counter slot. Every thread this
        // process started has been joined by a stop that succeeded, and its
        // status task aborted. A thread a stop could not end — an input thread
        // whose write nothing reached in its bound, or a violation's cancel
        // still inside its own budget — was reported by that stop as failed
        // cleanup, and goes with this quarantined process unjoined until its
        // pipe closes or its budget ends. The scope itself is shared with the
        // status task, so where a stop failed, the Windows job's
        // kill-on-close fires when the last owner of the scope drops it: this
        // process or the aborted status task, whichever is later.
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
#[cfg(all(test, target_os = "linux"))]
fn unconfined_child(
    command: Command,
    speech: crucible_sandbox::SandboxSpeech,
) -> Result<Box<dyn SandboxProcess>, crucible_sandbox::SandboxError> {
    testing_local(command, speech, None).map(|process| Box::new(process) as Box<dyn SandboxProcess>)
}

#[cfg(all(test, any(target_os = "linux", windows)))]
fn testing_local(
    command: Command,
    speech: crucible_sandbox::SandboxSpeech,
    stage: Option<Stage>,
) -> Result<LocalProcess, crucible_sandbox::SandboxError> {
    spawn_local(command, testing_plan(speech, stage)?)
}

#[cfg(test)]
pub(super) fn testing_plan(
    speech: crucible_sandbox::SandboxSpeech,
    stage: Option<Stage>,
) -> Result<SpawnPlan, crucible_sandbox::SandboxError> {
    use crucible_sandbox::{
        SandboxAudit, SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance,
        SandboxCapabilities, SandboxCleanup, SandboxFilesystemAccess, SandboxFilesystemProvenance,
        SandboxFilesystemRule, SandboxManifest, SandboxNetworkPolicy, SandboxPolicy,
    };
    use crucible_types::{Ancestry, SandboxId, ToolId};

    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("test-process")
            .map_err(|_| crucible_sandbox::SandboxError::InvalidInspection)?,
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .map_err(|_| crucible_sandbox::SandboxError::InvalidInspection)?;
    let root = std::env::current_dir()
        .and_then(|path| path.canonicalize())
        .map_err(crucible_sandbox::SandboxError::Spawn)?;
    let rule = SandboxFilesystemRule::new(
        &root,
        SandboxFilesystemAccess::ReadWrite,
        SandboxFilesystemProvenance::Workspace,
    )
    .map_err(|_| crucible_sandbox::SandboxError::InvalidInspection)?;
    let policy = SandboxPolicy::new(
        false,
        [rule],
        root,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .map_err(|_| crucible_sandbox::SandboxError::InvalidInspection)?;
    let manifest = SandboxManifest::empty();
    let inspection = crucible_sandbox::inspection(
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
        network: None,
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
        runtime: Some(crate::sample::runtime().map_err(crucible_sandbox::SandboxError::Spawn)?),
        speech,
        startup_input: None,
        credentials: Vec::new(),
    })
}

// What watches a command's status, on every platform that runs one.
#[cfg(test)]
#[path = "process/tests/watching.rs"]
mod watching;

// Windows gives each input written asynchronously a thread of its own, which
// only there has something for the process to join.
#[cfg(all(test, windows))]
#[path = "process/tests/windows_input.rs"]
mod windows_input;

#[cfg(all(test, target_os = "linux"))]
pub(crate) mod tests {
    mod cleanup;
    mod credential;
    pub(crate) mod pipes;
    mod startup;

    use super::Stage;

    #[test]
    fn proxy_credentials_are_masked_on_both_real_process_outputs() {
        use crucible_sandbox::{
            SandboxDomainPolicy, SandboxNetworkProvenance, SandboxRead, SandboxSpeech,
        };
        use crucible_types::SandboxId;
        let policy =
            SandboxDomainPolicy::new([], [], false, [], SandboxNetworkProvenance::User).unwrap();
        let proxy = super::super::network::Mediator::tcp(
            policy,
            SandboxId::new(),
            Some(std::time::Duration::from_secs(5)),
        )
        .unwrap();
        let mut command = std::process::Command::new("/bin/sh");
        command.args([
            "-c",
            "printf '%s\\n%s\\n' \"$HTTP_PROXY\" \"$CRUCIBLE_TEST_AUTH\"; printf '%s\\n%s\\n' \"$HTTP_PROXY\" \"$CRUCIBLE_TEST_AUTH\" >&2",
        ]);
        command.envs(proxy.environment(proxy.address()));
        command.env("CRUCIBLE_TEST_AUTH", proxy.authorization());
        let expected = format!(
            "http://crucible:{}@{}\nBasic {}\n",
            "*".repeat(64),
            proxy.address(),
            "*".repeat(100),
        );
        let mut plan = super::testing_plan(SandboxSpeech::Closed, None).unwrap();
        plan.network = Some(proxy);
        let mut process = super::spawn(command, plan).unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        for mut reader in [
            process.take_stdout().unwrap(),
            process.take_stderr().unwrap(),
        ] {
            let mut received = Vec::new();
            let mut bytes = [0; 17];
            loop {
                assert!(
                    std::time::Instant::now() < deadline,
                    "output did not finish"
                );
                match reader.read_ready(&mut bytes).unwrap() {
                    SandboxRead::Bytes(count) => {
                        received
                            .extend_from_slice(bytes.get(..count).expect("bounded output read"));
                    }
                    SandboxRead::Pending => std::thread::sleep(std::time::Duration::from_millis(1)),
                    SandboxRead::End => break,
                    SandboxRead::Limited { .. } => panic!("small output exceeded its budget"),
                }
            }
            assert!(
                received == expected.as_bytes(),
                "process output was not safely masked"
            );
        }
        crucible_runtime::answered!(process.stop()).unwrap();
    }

    #[test]
    fn command_owns_the_network_listener_until_cleanup() {
        use crucible_sandbox::{SandboxDomainPolicy, SandboxNetworkProvenance, SandboxSpeech};
        use crucible_types::SandboxId;
        let policy =
            SandboxDomainPolicy::new([], [], false, [], SandboxNetworkProvenance::User).unwrap();
        let proxy = super::super::network::Mediator::tcp(
            policy,
            SandboxId::new(),
            Some(std::time::Duration::from_secs(5)),
        )
        .unwrap();
        let address = proxy.address();
        let mut plan = super::testing_plan(SandboxSpeech::Held, None).unwrap();
        plan.network = Some(proxy);
        let mut process = super::spawn(echoing(), plan).unwrap();
        assert!(
            std::net::TcpStream::connect(address).is_ok(),
            "live command lost its network owner"
        );
        crucible_runtime::answered!(process.stop()).unwrap();
        // Another test may be between fork and exec with a transient copy of
        // the CLOEXEC listener. Require closure within a fixed bound rather
        // than confusing that short window with a retained network owner.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while std::net::TcpStream::connect(address).is_ok() {
            assert!(
                std::time::Instant::now() < deadline,
                "listener survived command cleanup"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(
            process.inspection().cleanup(),
            crucible_sandbox::SandboxCleanup::Complete
        );
    }

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
        let mut process =
            super::unconfined_child(echoing(), crucible_sandbox::SandboxSpeech::Closed)
                .expect("a confined step");

        assert!(
            process.take_stdin().is_none(),
            "a command built Closed must not hand back a writer"
        );

        crucible_runtime::answered!(process.stop()).expect("cleanup");
    }

    /// A peer is spoken to, and what it says back proves the bytes arrived
    /// rather than that a handle was returned.
    #[cfg(unix)]
    #[test]
    fn a_command_crucible_speaks_to_hears_what_it_said() {
        let mut process = super::unconfined_child(echoing(), crucible_sandbox::SandboxSpeech::Held)
            .expect("a confined peer");
        let mut stdin = process
            .take_stdin()
            .expect("a command built Held hands back a writer");

        stdin.write_all(b"a kettle\n").expect("crucible speaks");
        stdin.flush().expect("nothing is left in a buffer");
        drop(stdin);

        let said = drained(&mut process).expect("what the peer said back");
        assert_eq!(said.trim_end(), "heard a kettle");

        crucible_runtime::answered!(process.stop()).expect("cleanup");
    }

    /// A trusted launcher prefix is written before the caller receives the
    /// same pipe for command input. The Windows broker uses this to consume a
    /// bounded launch frame and leave later peer input untouched.
    #[cfg(unix)]
    #[test]
    fn startup_bytes_precede_held_command_input() {
        let mut command = std::process::Command::new("/bin/sh");
        command.args([
            "-c",
            "IFS= read -r setup; IFS= read -r input; printf '%s|%s\\n' \"$setup\" \"$input\"",
        ]);
        let mut plan = super::testing_plan(crucible_sandbox::SandboxSpeech::Held, None)
            .expect("a launch plan");
        plan.startup_input = Some(b"trusted launch frame\n".to_vec());
        let mut process = super::spawn(command, plan).expect("a launched peer");
        let mut stdin = process.take_stdin().expect("peer input remains held");
        stdin.write_all(b"caller input\n").expect("caller input");
        stdin.flush().expect("caller input flush");
        drop(stdin);

        let said = drained(&mut process).expect("what the peer received");
        assert_eq!(said.trim_end(), "trusted launch frame|caller input");
        crucible_runtime::answered!(process.stop()).expect("cleanup");
    }

    /// A one-shot command gets the trusted prefix and then end-of-file. Keeping
    /// the launch pipe open would turn an ordinary Windows command into a hang.
    #[cfg(unix)]
    #[test]
    fn startup_bytes_are_followed_by_eof_for_a_closed_command() {
        let mut command = std::process::Command::new("/bin/sh");
        command.args([
            "-c",
            "IFS= read -r setup; if IFS= read -r extra; then exit 9; fi; printf '%s|eof\\n' \"$setup\"",
        ]);
        let mut plan = super::testing_plan(crucible_sandbox::SandboxSpeech::Closed, None)
            .expect("a launch plan");
        plan.startup_input = Some(b"trusted launch frame\n".to_vec());
        let mut process = super::spawn(command, plan).expect("a launched step");
        assert!(process.take_stdin().is_none());

        let said = drained(&mut process).expect("what the step received");
        assert_eq!(said.trim_end(), "trusted launch frame|eof");
        crucible_runtime::answered!(process.stop()).expect("cleanup");
    }

    /// Standard input is handed over once. A second holder would be two writers
    /// interleaving frames into one stream the far end reads as one.
    #[cfg(unix)]
    #[test]
    fn standard_input_is_handed_over_once() {
        let mut process = super::unconfined_child(echoing(), crucible_sandbox::SandboxSpeech::Held)
            .expect("a confined peer");

        let first = process.take_stdin();
        let second = process.take_stdin();

        assert!(first.is_some(), "the first take hands back the writer");
        assert!(second.is_none(), "the second take hands back nothing");

        drop(first);
        crucible_runtime::answered!(process.stop()).expect("cleanup");
    }

    /// Reads stdout until the far end closes it.
    ///
    /// Written without a panicking path because it is a helper rather than a
    /// test: the exemption the workspace grants covers `#[test]` bodies, and a
    /// helper that panics reports its own failure instead of the caller's.
    #[cfg(unix)]
    fn drained(process: &mut Box<dyn crucible_sandbox::SandboxProcess>) -> std::io::Result<String> {
        let mut output = process
            .take_stdout()
            .ok_or_else(|| std::io::Error::other("the process has no stdout"))?;
        let mut said = Vec::new();
        let mut buffer = [0_u8; 256];
        loop {
            let taken = match output.read_ready(&mut buffer)? {
                crucible_sandbox::SandboxRead::Bytes(read) => read,
                crucible_sandbox::SandboxRead::Limited { retained, .. } => retained,
                crucible_sandbox::SandboxRead::Pending => {
                    std::thread::yield_now();
                    continue;
                }
                crucible_sandbox::SandboxRead::End => break,
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

        assert_eq!(cleanup, crucible_sandbox::SandboxCleanup::Complete);
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

        assert_eq!(cleanup, crucible_sandbox::SandboxCleanup::Failed);
        assert!(stage.as_ref().is_some_and(Stage::retained));
        assert!(reservation.is_none());
        assert!(root.exists());
        assert_eq!(active.load(std::sync::atomic::Ordering::Acquire), 1);
        std::fs::remove_dir(&root).expect("remove quarantined test stage");
    }
}
