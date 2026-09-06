//! What hosting an extension over a confined process has to guarantee.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Ancestry, Outcome, Over, SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance,
    SandboxCapabilities, SandboxFilesystemAccess, SandboxFilesystemProvenance,
    SandboxFilesystemRule, SandboxId, SandboxInspection, SandboxManifest, SandboxNetworkPolicy,
    SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead, SandboxRequest,
    SandboxResourceLimits, SandboxUsage, SandboxViolation, ToolId, Turn,
};
use serde_json::json;

use super::{Finish, Hosted, Unstarted};

/// How long a test sits through one silence.
const PATIENCE: Duration = Duration::from_millis(500);

/// How long a test waits for something it does not drive itself.
const LATEST: Duration = Duration::from_secs(2);

/// An exit status a test can compare against, without a process to get one from.
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

/// An absolute path spelled the way the running platform's path type accepts.
///
/// A POSIX root is not absolute on Windows, and a policy refuses a path that is
/// not. The host these tests are about has no platform in it, so the fixture
/// takes the local spelling rather than the tests taking a `cfg` that would
/// leave ten of them unrun on one of the platforms crucible ships to.
#[cfg(unix)]
const ROOT: &str = "/workspace";
#[cfg(windows)]
const ROOT: &str = r"C:\workspace";

/// A redacted inspection, which every process has to be able to show.
fn inspection() -> SandboxInspection {
    let policy = SandboxPolicy::new(
        false,
        [SandboxFilesystemRule::new(
            ROOT,
            SandboxFilesystemAccess::ReadWrite,
            SandboxFilesystemProvenance::Workspace,
        )
        .expect("rule")],
        ROOT,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("policy");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("extension"),
        policy,
        SandboxManifest::empty(),
    );
    let backend = SandboxBackendIdentity::new(
        SandboxBackendId::new("test").expect("id"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("identity");
    SandboxInspection::unconfined_for_request(
        backend,
        SandboxCapabilities::none(),
        &request,
        "a test, which confines nothing",
    )
    .expect("inspection")
}

/// One thing a stream does when it is asked what it has.
enum Step {
    /// It has a frame, and the newline that ends one.
    Says(&'static str),
    /// It has nothing yet, and its writer is still there.
    Waits,
}

/// A scripted stream that goes quiet forever once its script runs out.
struct Says(VecDeque<Step>);

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        match self.0.pop_front() {
            None | Some(Step::Waits) => Ok(SandboxRead::Pending),
            Some(Step::Says(frame)) => {
                let said = format!("{frame}\n");
                let bytes = said.as_bytes();
                let taken = bytes.len().min(buffer.len());
                if let Some((into, from)) = buffer.get_mut(..taken).zip(bytes.get(..taken)) {
                    into.copy_from_slice(from);
                }
                Ok(SandboxRead::Bytes(taken))
            }
        }
    }
}

/// Everything a test wants to know about a process after the host has had it.
#[derive(Default)]
struct Watched {
    /// What crucible said to it.
    said: Mutex<Vec<u8>>,
    /// Whether its input has been closed.
    closed: AtomicBool,
    /// How many times it was asked to stop.
    stopped: AtomicUsize,
}

/// The writing end of a process's input, as a test can read it back.
struct Input(Arc<Watched>);

impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.said.lock().expect("said").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Relaxed);
    }
}

/// How a fake process behaves when it is asked to finish.
enum Ending {
    /// It has already exited.
    Exited,
    /// It never exits, and stops when it is told to.
    Stubborn,
    /// It never exits, and cannot be reaped.
    Unreapable,
}

/// A process the sandbox might have started, doing only what a test needs.
struct Fake {
    /// What it says, where it was given an output at all.
    stdout: Option<Says>,
    /// What it complains about, where anything.
    stderr: Option<Says>,
    /// Whether it kept an input for crucible to speak over.
    speaks: bool,
    /// What happens when it is asked to finish.
    ending: Ending,
    /// What the sandbox stopped it for, where anything. Reported only once it
    /// has been stopped, the way a supervisor records one at the moment it acts
    /// on it rather than while the command is still running.
    violation: Option<SandboxViolation>,
    /// What a test reads back afterwards.
    watched: Arc<Watched>,
    /// Its redacted report.
    inspection: SandboxInspection,
}

impl Fake {
    fn new(steps: impl IntoIterator<Item = Step>, ending: Ending) -> (Box<Self>, Arc<Watched>) {
        let watched = Arc::new(Watched::default());
        (
            Box::new(Self {
                stdout: Some(Says(steps.into_iter().collect())),
                stderr: None,
                speaks: true,
                ending,
                violation: None,
                watched: Arc::clone(&watched),
                inspection: inspection(),
            }),
            watched,
        )
    }

    /// The same process, but one crucible's confinement stopped for `violation`.
    fn violating(
        steps: impl IntoIterator<Item = Step>,
        ending: Ending,
        violation: SandboxViolation,
    ) -> (Box<Self>, Arc<Watched>) {
        let (mut fake, watched) = Self::new(steps, ending);
        fake.violation = Some(violation);
        (fake, watched)
    }
}

impl SandboxProcess for Fake {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        self.speaks
            .then(|| Box::new(Input(Arc::clone(&self.watched))) as Box<dyn Write + Send>)
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stdout
            .take()
            .map(|says| Box::new(says) as Box<dyn SandboxOutput>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stderr
            .take()
            .map(|says| Box::new(says) as Box<dyn SandboxOutput>)
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        match self.ending {
            Ending::Exited => Ok(Some(exited())),
            Ending::Stubborn | Ending::Unreapable => Ok(None),
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        self.watched.stopped.fetch_add(1, Ordering::Relaxed);
        match self.ending {
            Ending::Unreapable => Err(io::Error::other("the scope could not be reaped")),
            Ending::Exited | Ending::Stubborn => Ok(()),
        }
    }

    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn usage(&self) -> SandboxUsage {
        SandboxUsage::default()
    }

    fn violation(&self) -> Option<SandboxViolation> {
        self.violation
            .filter(|_| self.watched.stopped.load(Ordering::Relaxed) > 0)
    }
}

/// Waits for `settled` to hold, so a test never races a thread it started.
fn until(mut settled: impl FnMut() -> bool) -> bool {
    let began = Instant::now();
    while began.elapsed() < LATEST {
        if settled() {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    settled()
}

#[test]
fn a_process_crucible_cannot_answer_is_refused_and_stopped() {
    let (mut process, watched) = Fake::new([], Ending::Stubborn);
    process.speaks = false;

    let refused = Hosted::<()>::over(process, PATIENCE).expect_err("no input, no conversation");

    assert!(matches!(refused, Unstarted::Unspeakable));
    assert_eq!(
        watched.stopped.load(Ordering::Relaxed),
        1,
        "a process that will not be hosted is not left running"
    );
}

#[test]
fn a_process_crucible_cannot_hear_is_refused_and_stopped() {
    let (mut process, watched) = Fake::new([], Ending::Stubborn);
    process.stdout = None;

    let refused = Hosted::<()>::over(process, PATIENCE).expect_err("no output, nothing to host");

    assert!(matches!(refused, Unstarted::Unheard));
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
    assert!(
        watched.closed.load(Ordering::Relaxed),
        "the input taken before the refusal is closed rather than leaked"
    );
}

#[test]
fn what_the_extension_says_arrives_as_a_turn() {
    let (process, _) = Fake::new(
        [Step::Says(
            r#"{"jsonrpc":"2.0","method":"ready","params":{"version":"1"}}"#,
        )],
        Ending::Exited,
    );
    let mut hosted = Hosted::<()>::over(process, PATIENCE).expect("hosted");

    let turn = hosted.turn().expect("a turn");

    match turn {
        Turn::Told { method, params } => {
            assert_eq!(&*method, "ready");
            assert_eq!(params, json!({"version": "1"}));
        }
        other => panic!("expected something told, got {other:?}"),
    }
}

#[test]
fn what_crucible_asks_reaches_the_process() {
    let (process, watched) = Fake::new([Step::Waits], Ending::Exited);
    let mut hosted = Hosted::<&str>::over(process, PATIENCE).expect("hosted");

    hosted
        .ask("tools/list", json!({}), "why crucible asked")
        .expect("asked");

    let said = String::from_utf8(watched.said.lock().expect("said").clone()).expect("utf-8");
    assert!(
        said.contains(r#""method":"tools/list""#),
        "the call should be on the wire: {said:?}"
    );
    assert!(said.ends_with('\n'), "a frame ends: {said:?}");
}

#[test]
fn stopping_closes_the_input_first_and_reports_a_quiet_ending() {
    let (process, watched) = Fake::new([Step::Waits], Ending::Exited);
    let mut hosted = Hosted::<&str>::over(process, PATIENCE).expect("hosted");
    hosted
        .ask("tools/list", json!({}), "unanswered")
        .expect("asked");

    let ended = hosted.stop(PATIENCE);

    assert!(
        matches!(ended.finish, Finish::Exited(status) if status == exited()),
        "a process that finished on its own is not stopped: {:?}",
        ended.finish
    );
    assert_eq!(
        watched.stopped.load(Ordering::Relaxed),
        0,
        "nothing is killed that ended within its grace"
    );
    let [(_, why)] = ended.waiting.as_slice() else {
        panic!(
            "the call nothing will answer comes back: {:?}",
            ended.waiting
        );
    };
    assert_eq!(*why, "unanswered");
    assert_eq!(
        ended.violation, None,
        "an extension that was not stopped for anything has nothing to report"
    );
    assert!(
        until(|| watched.closed.load(Ordering::Relaxed)),
        "crucible's end of the input is closed"
    );
}

#[test]
fn a_process_that_will_not_finish_is_stopped() {
    let (process, watched) = Fake::new([Step::Waits], Ending::Stubborn);
    let hosted = Hosted::<()>::over(process, PATIENCE).expect("hosted");

    let ended = hosted.stop(Duration::from_millis(20));

    assert!(
        matches!(ended.finish, Finish::Stopped),
        "a process that outlasts its grace is stopped: {:?}",
        ended.finish
    );
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
}

#[test]
fn a_scope_that_cannot_be_reaped_says_so() {
    let (process, _) = Fake::new([Step::Waits], Ending::Unreapable);
    let hosted = Hosted::<()>::over(process, PATIENCE).expect("hosted");

    let ended = hosted.stop(Duration::from_millis(20));

    match ended.finish {
        Finish::Unreaped(source) => {
            assert!(source.to_string().contains("could not be reaped"));
        }
        other => panic!("a failed reap is not an ordinary ending: {other:?}"),
    }
}

#[test]
fn a_silent_extension_ends_the_conversation_rather_than_waiting_forever() {
    let (process, _) = Fake::new([Step::Waits], Ending::Exited);
    let mut hosted = Hosted::<()>::over(process, Duration::from_millis(20)).expect("hosted");

    let over = hosted.turn().expect_err("nothing is coming");

    assert!(
        matches!(over, Over::Unreadable { .. }),
        "a peer that stopped talking is an ending, not a hang: {over:?}"
    );
}

#[test]
fn what_the_extension_complains_about_is_drained_and_kept() {
    let (mut process, _) = Fake::new([Step::Waits], Ending::Exited);
    process.stderr = Some(Says(
        [Step::Says("loader: libfoo.so not found")]
            .into_iter()
            .collect(),
    ));
    let hosted = Hosted::<()>::over(process, PATIENCE).expect("hosted");

    assert!(
        until(|| hosted.muttered().text().contains("libfoo.so")),
        "standard error is read rather than left to fill: {:?}",
        hosted.muttered().text()
    );
}

#[test]
fn a_call_the_extension_makes_is_answered_on_the_wire() {
    let (process, watched) = Fake::new(
        [Step::Says(
            r#"{"jsonrpc":"2.0","id":7,"method":"workspace/root","params":{}}"#,
        )],
        Ending::Exited,
    );
    let mut hosted = Hosted::<()>::over(process, PATIENCE).expect("hosted");

    let Turn::Asked { id, method, .. } = hosted.turn().expect("a turn") else {
        panic!("a request is something to answer");
    };
    assert_eq!(&*method, "workspace/root");

    hosted
        .answer(id, Outcome::Worked(json!({"root": "/workspace"})))
        .expect("answered");

    let said = String::from_utf8(watched.said.lock().expect("said").clone()).expect("utf-8");
    assert!(
        said.contains(r#""id":7"#) && said.contains("/workspace"),
        "the answer names the call it settles: {said:?}"
    );
}

/// A call the host has stopped waiting on is answered right then, and is not
/// handed back a second time when the extension goes away.
#[test]
fn a_call_the_host_gave_up_on_is_not_owed_again_at_the_end() {
    let (process, _) = Fake::new([Step::Waits], Ending::Exited);
    let mut hosted = Hosted::<&str>::over(process, PATIENCE).expect("hosted");
    let kept = hosted
        .ask("tools/list", json!({}), "still wanted")
        .expect("asked");
    let given_up = hosted
        .ask("tools/call", json!({}), "no longer wanted")
        .expect("asked again");

    assert_eq!(
        hosted.give_up(given_up).expect("giving up on it"),
        "no longer wanted"
    );

    let ended = hosted.stop(PATIENCE);
    let [(id, why)] = ended.waiting.as_slice() else {
        panic!("only the call still wanted comes back: {:?}", ended.waiting);
    };
    assert_eq!(*id, kept);
    assert_eq!(*why, "still wanted");
}

/// An extension crucible's own confinement stopped explains itself from
/// nowhere else: it was killed mid-sentence, so it wrote no complaint and its
/// conversation simply stopped. The reason has to survive the ending or the
/// host has nothing to say about it.
#[test]
fn an_extension_the_sandbox_stopped_says_what_it_was_stopped_for() {
    let (process, _) = Fake::violating(
        [Step::Waits],
        Ending::Stubborn,
        SandboxViolation::CommandTime,
    );
    let hosted = Hosted::<()>::over(process, PATIENCE).expect("hosted");

    let ended = hosted.stop(Duration::from_millis(20));

    assert_eq!(ended.violation, Some(SandboxViolation::CommandTime));
    assert!(
        matches!(ended.finish, Finish::Stopped),
        "it outlasted its grace: {:?}",
        ended.finish
    );
}

#[test]
fn missing_input_retains_failed_cleanup() {
    let (mut process, watched) = Fake::new([], Ending::Unreapable);
    process.speaks = false;
    let refused = Hosted::<()>::over(process, PATIENCE).unwrap_err();
    let message = refused.to_string();
    assert!(message.contains("input"), "{message}");
    assert!(
        message.contains("the scope could not be reaped"),
        "{message}"
    );
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
    let Unstarted::Unreaped { cause, cleanup } = refused else {
        panic!("cleanup uncertainty must be typed");
    };
    assert!(matches!(*cause, Unstarted::Unspeakable));
    assert_eq!(cleanup.kind(), io::ErrorKind::Other);
}

#[test]
fn missing_output_retains_failed_cleanup() {
    let (mut process, watched) = Fake::new([], Ending::Unreapable);
    process.stdout = None;
    let refused = Hosted::<()>::over(process, PATIENCE).unwrap_err();
    let message = refused.to_string();
    assert!(message.contains("output"), "{message}");
    assert!(
        message.contains("the scope could not be reaped"),
        "{message}"
    );
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
    let Unstarted::Unreaped { cause, cleanup } = refused else {
        panic!("cleanup uncertainty must be typed");
    };
    assert!(matches!(*cause, Unstarted::Unheard));
    assert_eq!(cleanup.kind(), io::ErrorKind::Other);
}
