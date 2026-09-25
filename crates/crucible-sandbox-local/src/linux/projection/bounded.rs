//! Where a command's ending is written, and how many are written at once.
//!
//! An ending journals the broker's scan and then either discards what the
//! command wrote or asks for the publication lock and publishes it: work that
//! syncs the journal after every record and the roots after every change, and
//! so waits on the disk for as long as the disk takes. It runs on a thread of
//! its own rather than on whoever asked how the command ended, which may be a
//! runtime worker or the thread that draws.
//!
//! **Sixteen at once.** One service runs at most [`IN_FLIGHT`] endings at a
//! time. A seventeenth is handed back unstarted and asked for again on a later
//! look, so it waits without holding a thread. A command has one ending at a
//! time, and a service holds at most as many commands as there are slots, so
//! the bound caps threads rather than queuing commands; it is what keeps a
//! quarantined command, whose slot outlives its cleanup, from being counted
//! twice.
//!
//! **Owned, not detached.** The thread belongs to the command it ends, which
//! joins it. A look joins only a thread that has already ended. A stop, and a
//! background result's acceptance, join one still running: it holds the
//! command's journal and roots, and cutting it short would leave a publication
//! half made. That join waits for at most one attempt at the lock or one
//! publication, each bounded by the entry, byte and depth ceilings of what it
//! writes, which is the wait the same work made on the asking thread before it
//! moved here.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Condvar, Mutex};
use std::thread::{self, JoinHandle};

use crucible_runtime::BoxFuture;
use crucible_sandbox::{SandboxOutput, SandboxRead};

/// How many endings one service writes at once.
pub(crate) const IN_FLIGHT: usize = 16;

#[cfg(test)]
pub(crate) type ScanGate = (
    crucible_types::SandboxId,
    std::sync::mpsc::SyncSender<()>,
    std::sync::mpsc::Receiver<()>,
);

#[cfg(test)]
static SCAN_GATE: std::sync::Mutex<Option<ScanGate>> = std::sync::Mutex::new(None);

#[cfg(test)]
static SCAN_GATE_TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
fn scan_gate_mutex() -> std::sync::MutexGuard<'static, Option<ScanGate>> {
    SCAN_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) fn install_scan_gate(
    sandbox: crucible_types::SandboxId,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    let mut gate = scan_gate_mutex();
    assert!(gate.is_none(), "another scan gate is already installed");
    SCAN_GATE_TAKEN.store(false, std::sync::atomic::Ordering::Release);
    *gate = Some((sandbox, reached, release));
}

#[cfg(test)]
pub(crate) fn clear_scan_gate(sandbox: crucible_types::SandboxId) {
    let mut gate = scan_gate_mutex();
    if gate
        .as_ref()
        .is_some_and(|(installed, _, _)| *installed == sandbox)
    {
        *gate = None;
    }
}

#[cfg(test)]
pub(crate) fn scan_gate_taken() -> bool {
    SCAN_GATE_TAKEN.load(std::sync::atomic::Ordering::Acquire)
}

#[cfg(test)]
pub(crate) fn take_scan_gate(sandbox: crucible_types::SandboxId) -> Option<ScanGate> {
    let mut gate = scan_gate_mutex();
    if gate
        .as_ref()
        .is_some_and(|(installed, _, _)| *installed == sandbox)
    {
        SCAN_GATE_TAKEN.store(true, std::sync::atomic::Ordering::Release);
        gate.take()
    } else {
        None
    }
}

#[cfg(test)]
struct FinalCheckGate {
    sandbox: crucible_types::SandboxId,
    reached: Arc<std::sync::mpsc::SyncSender<()>>,
    release: Mutex<std::sync::mpsc::Receiver<()>>,
}

#[cfg(test)]
static FINAL_CHECK_GATE: std::sync::Mutex<Option<Arc<FinalCheckGate>>> =
    std::sync::Mutex::new(None);

#[cfg(test)]
static FINAL_CHECK_GATE_TAKEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

#[cfg(test)]
fn final_check_gate_mutex() -> std::sync::MutexGuard<'static, Option<Arc<FinalCheckGate>>> {
    FINAL_CHECK_GATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
pub(crate) fn install_final_check_gate(
    sandbox: crucible_types::SandboxId,
    reached: std::sync::mpsc::SyncSender<()>,
    release: std::sync::mpsc::Receiver<()>,
) {
    let mut gate = final_check_gate_mutex();
    assert!(
        gate.is_none(),
        "another final-check gate is already installed"
    );
    FINAL_CHECK_GATE_TAKEN.store(false, Ordering::Release);
    *gate = Some(Arc::new(FinalCheckGate {
        sandbox,
        reached: Arc::new(reached),
        release: Mutex::new(release),
    }));
}

#[cfg(test)]
pub(crate) fn clear_final_check_gate(sandbox: crucible_types::SandboxId) {
    let mut gate = final_check_gate_mutex();
    if gate
        .as_ref()
        .is_some_and(|installed| installed.sandbox == sandbox)
    {
        *gate = None;
    }
}

#[cfg(test)]
pub(crate) fn final_check_gate_taken() -> bool {
    FINAL_CHECK_GATE_TAKEN.load(Ordering::Acquire)
}

#[cfg(test)]
pub(crate) fn hold_final_check(sandbox: crucible_types::SandboxId) {
    let gate = final_check_gate_mutex()
        .as_ref()
        .filter(|gate| gate.sandbox == sandbox)
        .map(Arc::clone);
    let Some(gate) = gate else {
        return;
    };
    FINAL_CHECK_GATE_TAKEN.store(true, Ordering::Release);
    let _ = gate.reached.send(());
    let _ = gate
        .release
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .recv();
}

/// The collector gives an ended command's output readers this long to reach
/// their pipes' ends. The publication boundary uses the same bound, so a
/// reader that cannot be sealed cannot hold publication open indefinitely.
const OUTPUT_READER_SEAL: std::time::Duration = std::time::Duration::from_millis(200);

/// The output-read boundary shared by a command's readers and its ending.
///
/// A read is counted while it holds a permit. Sealing first refuses new reads,
/// then waits for the permits already in flight. The output reader can record
/// the hard-limit result before its permit is released; after the seal no
/// reader can record a violation before publication begins. The underlying
/// stream remains the one that accounts bytes; this boundary shares its
/// `Limited` result with the ending so the two cannot disagree at publication.
#[derive(Default)]
pub(super) struct OutputBoundary {
    state: Mutex<OutputBoundaryState>,
    idle: Condvar,
}

#[derive(Default)]
struct OutputBoundaryState {
    sealed: bool,
    readers: usize,
    limited: bool,
}

struct OutputPermit {
    boundary: Arc<OutputBoundary>,
}

impl OutputBoundary {
    fn begin_read(self: &Arc<Self>) -> Option<OutputPermit> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.sealed {
            return None;
        }
        state.readers = state.readers.saturating_add(1);
        Some(OutputPermit {
            boundary: Arc::clone(self),
        })
    }

    fn record(&self, read: SandboxRead) -> SandboxRead {
        if matches!(read, SandboxRead::Limited { .. }) {
            self.state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .limited = true;
        }
        read
    }

    fn finish_read(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.readers = state.readers.saturating_sub(1);
        if state.readers == 0 {
            self.idle.notify_all();
        }
    }

    pub(super) fn seal(&self) -> Result<bool, io::Error> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        state.sealed = true;
        let deadline = std::time::Instant::now() + OUTPUT_READER_SEAL;
        while state.readers != 0 {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::other(
                    "command output readers did not seal within the publication bound",
                ));
            }
            let (next, waited) = self
                .idle
                .wait_timeout(state, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            if waited.timed_out() && state.readers != 0 {
                return Err(io::Error::other(
                    "command output readers did not seal within the publication bound",
                ));
            }
        }
        Ok(state.limited)
    }
}

impl Drop for OutputPermit {
    fn drop(&mut self) {
        self.boundary.finish_read();
    }
}

pub(super) struct PublicationOutput {
    inner: Box<dyn SandboxOutput>,
    boundary: Arc<OutputBoundary>,
}

impl PublicationOutput {
    pub(super) fn new(inner: Box<dyn SandboxOutput>, boundary: Arc<OutputBoundary>) -> Self {
        Self { inner, boundary }
    }
}

pub(super) fn wrap_output(
    output: Option<Box<dyn SandboxOutput>>,
    boundary: Arc<OutputBoundary>,
) -> Option<Box<dyn SandboxOutput>> {
    output
        .map(|output| Box::new(PublicationOutput::new(output, boundary)) as Box<dyn SandboxOutput>)
}

impl SandboxOutput for PublicationOutput {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if buffer.is_empty() {
            return Ok(SandboxRead::Pending);
        }
        let Some(_permit) = self.boundary.begin_read() else {
            return Ok(SandboxRead::End);
        };
        let read = self.inner.read_ready(buffer)?;
        Ok(self.boundary.record(read))
    }

    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<SandboxRead>> {
        Box::pin(async move {
            if buffer.is_empty() {
                return Ok(SandboxRead::Bytes(0));
            }
            let Some(_permit) = self.boundary.begin_read() else {
                return Ok(SandboxRead::End);
            };
            let read = self.inner.read(buffer).await?;
            Ok(self.boundary.record(read))
        })
    }
}

/// A service's endings in flight, and the bound on them.
#[derive(Clone, Default)]
pub(crate) struct BoundedPublication {
    in_flight: Arc<AtomicUsize>,
}

impl std::fmt::Debug for BoundedPublication {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BoundedPublication")
            .field("in_flight", &self.in_flight.load(Ordering::Acquire))
            .finish()
    }
}

/// One ending on its thread, which its owner joins.
pub(super) struct Running<R> {
    thread: JoinHandle<Option<R>>,
}

impl BoundedPublication {
    /// Starts `run` over `work` on a thread of its own, or hands `work` back
    /// while [`IN_FLIGHT`] are running or no thread could be started.
    ///
    /// The work reaches the thread only once it has started, so work handed
    /// back has been neither run nor dropped.
    pub(super) fn start<W, R>(
        &self,
        work: W,
        run: impl FnOnce(W) -> R + Send + 'static,
    ) -> Result<Running<R>, W>
    where
        W: Send + 'static,
        R: Send + 'static,
    {
        let Some(slot) = Slot::take(&self.in_flight) else {
            return Err(work);
        };
        let (give, take) = mpsc::sync_channel::<W>(1);
        // The slot is let go as the thread ends, after what it answers is
        // ready, or with the closure where the thread never started.
        let started = thread::Builder::new()
            .name("crucible-publication".into())
            .spawn(move || {
                let _slot = slot;
                take.recv().ok().map(run)
            });
        let Ok(thread) = started else {
            return Err(work);
        };
        match give.send(work) {
            Ok(()) => Ok(Running { thread }),
            Err(mpsc::SendError(work)) => {
                let _ = thread.join();
                Err(work)
            }
        }
    }
}

/// One of [`IN_FLIGHT`], held by an ending's thread for as long as it runs.
struct Slot(Arc<AtomicUsize>);

impl Slot {
    /// A slot, without the count ever passing [`IN_FLIGHT`], or `None` while
    /// every one is held.
    fn take(in_flight: &Arc<AtomicUsize>) -> Option<Self> {
        in_flight
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held < IN_FLIGHT).then_some(held.saturating_add(1))
            })
            .ok()
            .map(|_| Self(Arc::clone(in_flight)))
    }
}

impl Drop for Slot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl<R> Running<R> {
    /// Whether [`Self::finish`] would answer without waiting.
    pub(super) fn finished(&self) -> bool {
        self.thread.is_finished()
    }

    /// What the ending came to, waiting for its thread to end.
    pub(super) fn finish(self) -> io::Result<R> {
        self.thread
            .join()
            .map_err(|_| io::Error::other("the thread writing a command's ending panicked"))?
            .ok_or_else(|| io::Error::other("a command's ending never reached its thread"))
    }
}

/// Under test, every slot of a service held by work that waits until this is
/// dropped, the way sixteen endings writing at once hold them.
#[cfg(test)]
pub(crate) struct Held {
    release: Option<mpsc::Sender<()>>,
    running: Vec<Running<()>>,
}

#[cfg(test)]
impl BoundedPublication {
    /// Holds every slot, once the endings already writing have let theirs go.
    pub(crate) fn hold_every_slot(&self) -> Held {
        let (release, gate) = mpsc::channel::<()>();
        let gate = Arc::new(std::sync::Mutex::new(gate));
        let mut running = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        while running.len() < IN_FLIGHT {
            // Every holder ends when the sender goes: each wakes to a closed
            // channel in turn.
            let held = self.start(Arc::clone(&gate), |gate| {
                let _ = gate.lock().map(|gate| gate.recv());
            });
            if let Ok(held) = held {
                running.push(held);
            } else {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the endings already writing never let their slots go"
                );
                thread::sleep(std::time::Duration::from_millis(1));
            }
        }
        Held {
            release: Some(release),
            running,
        }
    }
}

#[cfg(test)]
impl Drop for Held {
    fn drop(&mut self) {
        drop(self.release.take());
        for held in self.running.drain(..) {
            let _ = held.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::{BoundedPublication, IN_FLIGHT};

    /// How long a thread here gets to end before the test calls it hung.
    const HUNG: Duration = Duration::from_secs(30);

    #[test]
    fn a_seventeenth_ending_waits_until_one_of_sixteen_is_done() {
        let publications = BoundedPublication::default();
        let mut releases = Vec::new();
        let mut running = Vec::new();
        for held in 0..IN_FLIGHT {
            let (release, gate) = mpsc::channel::<()>();
            let started = publications.start(gate, move |gate| {
                let _ = gate.recv();
                held
            });
            running.push(started.expect("an ending starts while a slot is free"));
            releases.push(release);
        }

        let waiting = publications.start(IN_FLIGHT, |held| held);
        let Err(handed_back) = waiting else {
            panic!("a seventeenth ending started while sixteen were running");
        };
        assert_eq!(
            handed_back, IN_FLIGHT,
            "the waiting work came back as it went"
        );

        let first = running.remove(0);
        drop(releases.remove(0));
        assert_eq!(first.finish().expect("the first ending"), 0);
        let deadline = Instant::now() + HUNG;
        let started = loop {
            match publications.start(handed_back, |held| held) {
                Ok(started) => break started,
                Err(_) => assert!(Instant::now() < deadline, "a freed slot was never reused"),
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        assert_eq!(started.finish().expect("the seventeenth ending"), IN_FLIGHT);

        drop(releases);
        for ending in running {
            ending.finish().expect("a held ending");
        }
    }
}
