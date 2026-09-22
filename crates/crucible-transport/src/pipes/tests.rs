//! What taking a hosted program's streams has to guarantee.

use std::io;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection,
    SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRead, SandboxRequest, SandboxResourceLimits, SandboxUsage, SandboxViolation,
};
use crucible_types::{Ancestry, SandboxId, ToolId};

use crate::{Frames, Written};

use super::{Absent, Pipes};

/// How long one silence is sat through here.
///
/// Nothing in this module waits one out: every stream a test hands over has
/// already said whatever it is going to.
const PATIENCE: std::time::Duration = std::time::Duration::from_millis(50);

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

    let mut pipes = Pipes::taken(&mut process, PATIENCE).expect("every stream was there");
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

    let unspoken = Pipes::taken(&mut process, PATIENCE).expect_err(
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

    let unspoken = Pipes::taken(&mut process, PATIENCE)
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

    let unspoken = Pipes::taken(&mut process, PATIENCE).expect_err("the process was refused");

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

    let pipes = Pipes::taken(&mut process, PATIENCE).expect("both conversation streams were there");

    assert_eq!(
        pipes.muttered.text(),
        "",
        "a program the sandbox gave no standard error says nothing beside the \
         conversation, rather than making every host carry an absence"
    );
}
