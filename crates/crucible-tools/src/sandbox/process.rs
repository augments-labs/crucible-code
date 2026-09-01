//! One spawned command and its complete local cleanup scope.

use std::io;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    SandboxCleanup, SandboxInspection, SandboxOutput, SandboxProcess, SandboxRead, SandboxUsage,
};

use crate::bash::platform::{Output as PlatformOutput, ReadState, Scope};

/// Absolute ceiling even where a policy omits a smaller one.
pub(super) const MAX_LOCAL_COMMANDS: usize = 16;

/// Bounded reap interval used by destructors.
const REAP: Duration = Duration::from_millis(250);

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

/// Spawns `command` inside a platform process-tree scope.
pub(super) fn spawn(
    mut command: Command,
    inspection: SandboxInspection,
    reservation: Reservation,
) -> Result<Box<dyn SandboxProcess>, crucible_core::SandboxError> {
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

    #[cfg(windows)]
    if let Err(source) = scope.attach(&child) {
        let _ = scope.stop(&mut child);
        let _ = child.wait();
        return Err(crucible_core::SandboxError::Spawn(source));
    }

    let stdout = child
        .stdout
        .take()
        .map(PreparedOutput::new)
        .transpose()
        .map_err(crucible_core::SandboxError::Spawn)?;
    let stderr = child
        .stderr
        .take()
        .map(PreparedOutput::new)
        .transpose()
        .map_err(crucible_core::SandboxError::Spawn)?;

    Ok(Box::new(LocalProcess {
        child,
        scope,
        stdout: stdout.map(|pipe| Box::new(pipe) as Box<dyn SandboxOutput>),
        stderr: stderr.map(|pipe| Box::new(pipe) as Box<dyn SandboxOutput>),
        inspection,
        reservation: Some(reservation),
        started: Instant::now(),
        stopped: false,
    }))
}

/// A pipe put into non-blocking mode before the process handle escapes.
struct PreparedOutput {
    inner: Box<dyn PlatformOutput>,
}

impl PreparedOutput {
    fn new(output: impl PlatformOutput) -> io::Result<Self> {
        output.prepare()?;
        Ok(Self {
            inner: Box::new(output),
        })
    }
}

impl SandboxOutput for PreparedOutput {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        self.inner.read_ready(buffer).map(|read| match read {
            ReadState::Bytes(bytes) => SandboxRead::Bytes(bytes),
            ReadState::Pending => SandboxRead::Pending,
            ReadState::End => SandboxRead::End,
        })
    }
}

/// The process, its process-tree scope, streams, stage, and reservation.
struct LocalProcess {
    child: Child,
    scope: Scope,
    stdout: Option<Box<dyn SandboxOutput>>,
    stderr: Option<Box<dyn SandboxOutput>>,
    inspection: SandboxInspection,
    reservation: Option<Reservation>,
    started: Instant,
    stopped: bool,
}

impl SandboxProcess for LocalProcess {
    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stdout.take()
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stderr.take()
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    fn stop(&mut self) -> io::Result<()> {
        if self.stopped {
            return Ok(());
        }
        stop_scope(&self.scope, &mut self.child)?;
        self.stopped = true;
        self.inspection = self.inspection.clone().cleaned(SandboxCleanup::Complete);
        Ok(())
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage {
            wall_time: self.started.elapsed(),
            ..SandboxUsage::default()
        }
    }
}

impl std::fmt::Debug for LocalProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalProcess")
            .field("inspection", &self.inspection)
            .field("running", &!self.stopped)
            .field("reservation", &self.reservation)
            .finish()
    }
}

impl Drop for LocalProcess {
    fn drop(&mut self) {
        let _ = self.stop();
        let deadline = Instant::now() + REAP;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) | Err(_) => break,
                Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
                Ok(None) => break,
            }
        }
        self.stdout.take();
        self.stderr.take();
        self.reservation.take();
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
        SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
        SandboxCleanup, SandboxId,
    };

    let identity = SandboxBackendIdentity::new(
        SandboxBackendId::new("test-process")
            .map_err(|_| crucible_core::SandboxError::InvalidInspection)?,
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .map_err(|_| crucible_core::SandboxError::InvalidInspection)?;
    let inspection = SandboxInspection::new(
        SandboxId::new(),
        identity,
        SandboxCapabilities::none(),
        [0; 32],
        [0; 32],
        false,
        Some("test-only unconfined process"),
        SandboxCleanup::Pending,
    )?;
    let active = Arc::new(AtomicUsize::new(0));
    let reservation = Reservation::take(active, 1)?;
    spawn(command, inspection, reservation)
}
