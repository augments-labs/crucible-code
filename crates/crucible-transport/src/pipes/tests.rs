//! What taking a hosted program's streams has to guarantee.

use std::io;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInput,
    SandboxInspection, SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy,
    SandboxProcess, SandboxRead, SandboxRequest, SandboxResourceLimits, SandboxUsage,
    SandboxViolation,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

use crate::testing::runtime;
use crate::{FrameError, Frames, Written};

use super::{Absent, Pipes, Unspoken};

/// Drives [`Pipes::taken`] to its answer, for a test that does not itself
/// await it.
fn taken(
    process: &mut dyn SandboxProcess,
    patience: std::time::Duration,
    on: &tokio::runtime::Handle,
) -> Result<Pipes, Unspoken> {
    runtime().block_on(Pipes::taken(process, patience, on))
}

/// How long one silence is sat through here.
///
/// Nothing in this module waits one out: every stream a test hands over has
/// already said whatever it is going to. Long all the same, because what it
/// says is read by a task on the runtime and handed across, and a loaded
/// machine can put a while between a task being woken and it running.
const PATIENCE: std::time::Duration = std::time::Duration::from_secs(5);

/// An absolute path spelled the way the running platform's path type accepts.
#[cfg(unix)]
const ROOT: &str = "/workspace";
#[cfg(windows)]
const ROOT: &str = r"C:\workspace";

/// Which of a process's streams the sandbox kept back, and what stopping it did.
#[derive(Clone, Copy)]
struct Withheld {
    /// Standard input was not kept.
    input: bool,
    /// Standard output was not kept.
    output: bool,
    /// Stopping it cannot confirm that its scope ended.
    unstoppable: bool,
}

impl Withheld {
    /// A process with every stream in hand, which stops when it is told to.
    const fn none() -> Self {
        Self {
            input: false,
            output: false,
            unstoppable: false,
        }
    }
}

/// What a test's process is holding, so a test can read it back afterwards.
#[derive(Default)]
struct Held {
    /// What crucible wrote to its standard input.
    written: Arc<Mutex<Vec<u8>>>,
    /// How many times it was asked to stop.
    stops: AtomicUsize,
}

struct Process {
    withheld: Withheld,
    held: Arc<Held>,
    /// What it says on standard output, once.
    says: Option<&'static str>,
    /// What it mutters on standard error, once.
    mutters: Option<&'static str>,
    inspection: SandboxInspection,
}

/// A writer that keeps what it was given where the test can read it.
struct Kept(Arc<Mutex<Vec<u8>>>);

impl io::Write for Kept {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut held = self.0.lock().map_err(|_| io::Error::other("poisoned"))?;
        held.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A stream that says one thing and then ends.
struct Once(Option<&'static str>);

impl SandboxOutput for Once {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        let Some(said) = self.0.take() else {
            return Ok(SandboxRead::End);
        };
        let bytes = said.as_bytes();
        let taken = bytes.len().min(buffer.len());
        if let Some((into, from)) = buffer.get_mut(..taken).zip(bytes.get(..taken)) {
            into.copy_from_slice(from);
        }
        Ok(SandboxRead::Bytes(taken))
    }
}

impl SandboxProcess for Process {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        (!self.withheld.input)
            .then(|| Box::new(Kept(Arc::clone(&self.held.written))) as Box<dyn io::Write + Send>)
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        (!self.withheld.output).then(|| Box::new(Once(self.says)) as Box<dyn SandboxOutput>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.mutters
            .map(|said| Box::new(Once(Some(said))) as Box<dyn SandboxOutput>)
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(None)
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            self.held.stops.fetch_add(1, Ordering::Relaxed);
            if self.withheld.unstoppable {
                return Err(io::Error::other("scope termination could not be confirmed"));
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
        None
    }
}

/// One process a test speaks to, and what it is holding.
fn process(withheld: Withheld) -> (Process, Arc<Held>) {
    let policy = SandboxPolicy::new(
        false,
        [SandboxFilesystemRule::new(
            ROOT,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("a rule over an absolute root")],
        ROOT,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("a policy whose rules are all rooted");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("pipes"),
        policy,
        SandboxManifest::empty(),
    );
    let backend = SandboxBackendIdentity::new(
        SandboxBackendId::new("test").expect("a backend name"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("a backend identity");
    let inspection = SandboxInspection::unconfined_for_request(
        backend,
        SandboxCapabilities::none(),
        &request,
        "a test, which confines nothing",
    )
    .expect("an inspection of a request a test built");
    let held = Arc::new(Held::default());
    (
        Process {
            withheld,
            held: Arc::clone(&held),
            says: Some("what it said\n"),
            mutters: Some("what went wrong"),
            inspection,
        },
        held,
    )
}

#[test]
fn a_process_with_every_stream_is_spoken_to_heard_and_drained() {
    let (mut process, held) = process(Withheld::none());

    let mut pipes =
        taken(&mut process, PATIENCE, crate::testing::runtime()).expect("every stream was there");
    Written::new(&mut pipes.said)
        .send("asked")
        .expect("the pipe took it");
    let arrived = Frames::new(&mut pipes.heard).next_frame();

    assert!(
        matches!(arrived, Some(Ok(ref said)) if said == "what it said"),
        "what the program said has to reach the reader the host was handed: {arrived:?}"
    );
    drop(pipes.said);
    assert_eq!(
        String::from_utf8(
            held.written
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        )
        .unwrap_or_default(),
        "asked\n",
        "and what the host says has to reach the program's own input"
    );
    assert_eq!(
        held.stops.load(Ordering::Relaxed),
        0,
        "a process that was hosted must not be stopped on the way in"
    );
}

#[test]
fn a_process_whose_input_was_not_kept_is_refused_and_stopped() {
    let (mut process, held) = process(Withheld {
        input: true,
        ..Withheld::none()
    });

    let unspoken = taken(&mut process, PATIENCE, crate::testing::runtime()).expect_err(
        "a process crucible cannot speak to is not a peer, whatever else it \
         handed back",
    );

    assert_eq!(unspoken.absent, Absent::Input);
    assert!(
        unspoken.cleanup.is_none(),
        "a stop the backend confirmed is not an uncertainty to report: {:?}",
        unspoken.cleanup
    );
    assert_eq!(
        held.stops.load(Ordering::Relaxed),
        1,
        "a process that will not be hosted is still running, and leaving it \
         would leave a scope nothing reaps"
    );
}

#[test]
fn a_process_whose_output_was_not_kept_is_refused_after_its_input_was_taken() {
    let (mut process, held) = process(Withheld {
        output: true,
        ..Withheld::none()
    });

    let unspoken = taken(&mut process, PATIENCE, crate::testing::runtime())
        .expect_err("a process with nothing to read is not a peer either");

    assert_eq!(unspoken.absent, Absent::Output);
    assert_eq!(held.stops.load(Ordering::Relaxed), 1);
}

#[test]
fn a_refusal_whose_stop_was_not_confirmed_carries_the_cleanup_beside_it() {
    // Both facts, because they have different remedies. A caller told only that
    // a pipe was missing would retire a process scope nothing confirmed the end
    // of; one told only that cleanup failed would look for a conversation that
    // never began.
    let (mut process, _held) = process(Withheld {
        input: true,
        unstoppable: true,
        ..Withheld::none()
    });

    let unspoken = taken(&mut process, PATIENCE, crate::testing::runtime())
        .expect_err("the process was refused");

    assert_eq!(unspoken.absent, Absent::Input);
    assert!(
        unspoken
            .cleanup
            .is_some_and(|cleanup| cleanup.to_string().contains("could not be confirmed")),
        "an unconfirmed stop has to survive the refusal that caused it"
    );
}

#[test]
fn a_process_with_no_standard_error_is_given_one_that_never_says_anything() {
    let (mut process, _held) = process(Withheld::none());
    process.mutters = None;

    let pipes = taken(&mut process, PATIENCE, crate::testing::runtime())
        .expect("both conversation streams were there");

    assert_eq!(
        pipes.muttered.text(),
        "",
        "a program the sandbox gave no standard error says nothing beside the \
         conversation, rather than making every host carry an absence"
    );
}

/// How many of a dead program's streams have been let go of.
type Released = Arc<AtomicUsize>;

/// An output of a program that has died: what it managed to say, then its end.
struct Last {
    said: Option<&'static str>,
    released: Released,
}

impl SandboxOutput for Last {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        Once(self.said.take()).read_ready(buffer)
    }
}

impl Drop for Last {
    fn drop(&mut self) {
        self.released.fetch_add(1, Ordering::Relaxed);
    }
}

/// The input of a program that has died: nobody is left to read it.
struct Unread(Released);

impl SandboxInput for Unread {
    fn write<'a>(&'a mut self, _bytes: &'a [u8]) -> BoxFuture<'a, io::Result<usize>> {
        Box::pin(async {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the program is no longer reading",
            ))
        })
    }
}

impl Drop for Unread {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::Relaxed);
    }
}

/// A process that died after saying one thing on each of its outputs.
struct Died {
    released: Released,
    inspection: SandboxInspection,
}

impl SandboxProcess for Died {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        None
    }

    fn take_async_stdin(&mut self) -> Option<Box<dyn SandboxInput>> {
        Some(Box::new(Unread(Arc::clone(&self.released))))
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        Some(Box::new(Last {
            said: Some("its last words\n"),
            released: Arc::clone(&self.released),
        }))
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        Some(Box::new(Last {
            said: Some("segmentation fault\n"),
            released: Arc::clone(&self.released),
        }))
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(Some(exited()))
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage::default()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        None
    }
}

/// Waits for `settled` to hold, so a test never races a task it started.
fn until(settled: impl Fn() -> bool) -> bool {
    let began = std::time::Instant::now();
    while began.elapsed() < std::time::Duration::from_secs(2) {
        if settled() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    settled()
}

/// A host that dies ends each stream the way it always has — what it said is
/// heard and then the stream finishes, what crucible says next is refused as
/// never having reached it, and what it muttered on the way out is kept — and
/// the work on each stream ends with the stream rather than with whoever holds
/// the conversation, which may be nobody for a while yet.
#[test]
fn a_host_that_dies_ends_every_stream_with_the_outcome_it_always_had() {
    let released = Released::default();
    let (process, _held) = process(Withheld::none());
    let mut died = Died {
        released: Arc::clone(&released),
        inspection: process.inspection,
    };

    let Pipes {
        heard,
        said,
        muttered,
    } = taken(&mut died, PATIENCE, runtime()).expect("every stream was there");
    let mut frames = Frames::new(heard);
    let mut written = Written::new(said);

    let last = frames.next_frame();
    let after = frames.next_frame();
    let sent = written.send("asked");

    assert!(
        matches!(last, Some(Ok(ref said)) if said == "its last words"),
        "what it said before it died is heard: {last:?}"
    );
    assert!(
        after.is_none(),
        "and then its output finishes, the ordinary ending: {after:?}"
    );
    assert!(
        matches!(&sent, Err(refused @ FrameError::Unreadable { source })
            if source.kind() == io::ErrorKind::BrokenPipe && refused.never_left()),
        "a frame for a program that is gone is refused as never having reached it: {sent:?}"
    );
    assert!(
        until(|| muttered.text() == "segmentation fault\n"),
        "what it muttered on the way out is kept: {:?}",
        muttered.text()
    );
    assert!(
        until(|| released.load(Ordering::Relaxed) == 3),
        "each stream is let go of once it has ended, while the conversation is still \
         held: {} of 3",
        released.load(Ordering::Relaxed)
    );
}

/// How a process that ended on its own reports it.
fn exited() -> ExitStatus {
    #[cfg(unix)]
    {
        std::os::unix::process::ExitStatusExt::from_raw(0)
    }
    #[cfg(windows)]
    {
        std::os::windows::process::ExitStatusExt::from_raw(0)
    }
}

/// The same death, met by a host that awaits: what it said is heard and then
/// the stream finishes, what crucible says next is refused as never having
/// reached it, what it muttered is kept, and how it ended is learned from the
/// finish that awaits it.
#[tokio::test]
async fn a_host_that_dies_is_met_the_same_way_by_a_host_that_awaits() {
    let released = Released::default();
    let (process, _held) = process(Withheld::none());
    let mut died = Died {
        released: Arc::clone(&released),
        inspection: process.inspection,
    };

    let Pipes {
        heard,
        said,
        muttered,
    } = Pipes::taken(&mut died, PATIENCE, &tokio::runtime::Handle::current())
        .await
        .expect("every stream was there");
    let mut frames = Frames::new(heard);
    let mut written = Written::new(said);

    let last = frames.next_frame_async().await;
    let after = frames.next_frame_async().await;
    let sent = written.send_async("asked").await;
    let finish = crate::Finish::after_async(&mut died, std::time::Duration::from_secs(1)).await;

    assert!(
        matches!(last, Some(Ok(ref said)) if said == "its last words"),
        "what it said before it died is heard: {last:?}"
    );
    assert!(after.is_none(), "and then its output finishes: {after:?}");
    assert!(
        matches!(&sent, Err(refused @ FrameError::Unreadable { source })
            if source.kind() == io::ErrorKind::BrokenPipe && refused.never_left()),
        "a frame for a program that is gone is refused as never having reached it: {sent:?}"
    );
    assert!(
        matches!(finish, crate::Finish::Exited(status) if status.success()),
        "and how it ended is learned: {finish:?}"
    );
    for _ in 0..200 {
        if muttered.text() == "segmentation fault\n" && released.load(Ordering::Relaxed) == 3 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(muttered.text(), "segmentation fault\n");
    assert_eq!(
        released.load(Ordering::Relaxed),
        3,
        "each stream is let go of once it has ended"
    );
}

/// A standard error that never stops talking, counting what it has said.
struct Flooding(Arc<AtomicUsize>);

impl SandboxOutput for Flooding {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        buffer.fill(b'x');
        self.0.fetch_add(buffer.len(), Ordering::Relaxed);
        Ok(SandboxRead::Bytes(buffer.len()))
    }
}

/// An output that answers one frame after a moment, then stays open.
struct Answering(Option<&'static str>);

impl SandboxOutput for Answering {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        match self.0.take() {
            Some(said) => Once(Some(said)).read_ready(buffer),
            None => Ok(SandboxRead::Pending),
        }
    }

    fn read<'a>(&'a mut self, buffer: &'a mut [u8]) -> BoxFuture<'a, io::Result<SandboxRead>> {
        Box::pin(async move {
            if self.0.is_none() {
                return std::future::pending().await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            self.read_ready(buffer)
        })
    }
}

/// A process that answers one frame while flooding its standard error.
struct Talkative {
    flooded: Arc<AtomicUsize>,
    inspection: SandboxInspection,
}

impl SandboxProcess for Talkative {
    fn take_stdin(&mut self) -> Option<Box<dyn io::Write + Send>> {
        Some(Box::new(Kept(Arc::default())))
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        Some(Box::new(Answering(Some("the answer\n"))))
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        Some(Box::new(Flooding(Arc::clone(&self.flooded))))
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        Ok(None)
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async { Ok(()) })
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage::default()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        None
    }
}

/// A runtime of one worker that is let go of without a wait when a test
/// unwinds past it, so a task that never yields its worker fails the test
/// rather than hanging it.
struct OneWorker(Option<tokio::runtime::Runtime>);

impl OneWorker {
    fn new() -> Self {
        Self(Some(
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .expect("a runtime of one worker"),
        ))
    }

    fn handle(&self) -> &tokio::runtime::Handle {
        self.0.as_ref().expect("a runtime until dropped").handle()
    }
}

impl Drop for OneWorker {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.take() {
            runtime.shutdown_background();
        }
    }
}

/// A flood on standard error beside a conversation on standard output: the
/// frame still arrives, what is kept of the flood stays at its bound, and the
/// rest is only counted. On one worker, so a drain that held its worker for as
/// long as the flood lasted would be a conversation nobody ever heard.
#[test]
fn a_flood_on_standard_error_does_not_hold_up_the_conversation_beside_it() {
    let runtime = OneWorker::new();
    let flooded = Arc::new(AtomicUsize::new(0));
    let (process, _held) = process(Withheld::none());
    let mut talkative = Talkative {
        flooded: Arc::clone(&flooded),
        inspection: process.inspection,
    };

    let pipes = taken(&mut talkative, PATIENCE, runtime.handle()).expect("every stream was there");
    let mut frames = Frames::new(pipes.heard);
    let answer = frames.next_frame();
    assert!(
        until(|| flooded.load(Ordering::Relaxed) > 64 * 1024),
        "the flood went on beside the conversation"
    );
    let muttered = pipes.muttered.text();

    assert!(
        matches!(answer, Some(Ok(ref said)) if said == "the answer"),
        "the frame arrives however loudly the program mutters: {answer:?}"
    );
    assert_eq!(
        muttered.split('\n').next().map(str::len),
        Some(8 * 1024),
        "what is kept of the flood stays at its bound"
    );
    assert!(
        muttered.contains("further bytes were dropped"),
        "and the rest is counted, not kept"
    );
}
