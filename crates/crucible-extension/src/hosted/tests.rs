//! What hosting an extension over a confined process has to guarantee.

use std::collections::VecDeque;
use std::future::Future;
use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection,
    SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRead, SandboxRequest, SandboxResourceLimits, SandboxUsage, SandboxViolation,
};
use crucible_types::{Ancestry, SandboxId, ToolId};
use serde_json::{Value, json};

use crate::calls::Generation;
use crate::{Asking, CallError, Outcome, Over, Trouble, Turn};

use super::{Finish, Hosted, Unstarted};

/// How long a test sits through one silence.
const PATIENCE: Duration = Duration::from_millis(500);

/// How long a test waits for something it does not drive itself.
const LATEST: Duration = Duration::from_secs(2);

/// Drives one wait of the host to its answer.
fn on<F: Future>(work: F) -> F::Output {
    crate::testing::runtime().block_on(work)
}

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
    /// It has nothing until this moment, and then a frame.
    SaysFrom(Instant, &'static str),
}

/// A scripted stream that goes quiet forever once its script runs out.
struct Says(VecDeque<Step>);

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if let Some(Step::SaysFrom(from, _)) = self.0.front()
            && Instant::now() < *from
        {
            return Ok(SandboxRead::Pending);
        }
        match self.0.pop_front() {
            None | Some(Step::Waits) => Ok(SandboxRead::Pending),
            Some(Step::Says(frame) | Step::SaysFrom(_, frame)) => {
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
    /// It never exits, and a stop never answers, so an awaited caller gives up
    /// on it at the bound on a stop that does not answer.
    Unanswering,
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
            Ending::Stubborn | Ending::Unreapable | Ending::Unanswering => Ok(None),
        }
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            self.watched.stopped.fetch_add(1, Ordering::Relaxed);
            match self.ending {
                Ending::Unreapable => Err(io::Error::other("the scope could not be reaped")),
                Ending::Unanswering => std::future::pending().await,
                Ending::Exited | Ending::Stubborn => Ok(()),
            }
        })
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

    let refused = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect_err("no input, no conversation");

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

    let refused = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect_err("no output, nothing to host");

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
    let mut hosted = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    let turn = on(hosted.turn()).expect("a turn");

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
    let mut hosted = on(Hosted::<&str>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    on(hosted.ask("tools/list", json!({}), "why crucible asked")).expect("asked");

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
    let mut hosted = on(Hosted::<&str>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");
    on(hosted.ask("tools/list", json!({}), "unanswered")).expect("asked");

    let ended = on(hosted.stop(PATIENCE));

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
    let hosted = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    let ended = on(hosted.stop(Duration::from_millis(20)));

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
    let hosted = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    let ended = on(hosted.stop(Duration::from_millis(20)));

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
    let mut hosted = on(Hosted::<()>::over(
        process,
        Duration::from_millis(20),
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    let over = on(hosted.turn()).expect_err("nothing is coming");

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
    let hosted = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

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
    let mut hosted = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    let Turn::Asked { id, method, .. } = on(hosted.turn()).expect("a turn") else {
        panic!("a request is something to answer");
    };
    assert_eq!(&*method, "workspace/root");

    on(hosted.answer(id, Outcome::Worked(json!({"root": "/workspace"})))).expect("answered");

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
    let mut hosted = on(Hosted::<&str>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");
    let kept = on(hosted.ask("tools/list", json!({}), "still wanted")).expect("asked");
    let given_up =
        on(hosted.ask("tools/call", json!({}), "no longer wanted")).expect("asked again");

    assert_eq!(
        hosted.give_up(given_up).expect("giving up on it"),
        "no longer wanted"
    );

    let ended = on(hosted.stop(PATIENCE));
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
    let hosted = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    let ended = on(hosted.stop(Duration::from_millis(20)));

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
    let refused = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .unwrap_err();
    let message = refused.to_string();
    assert!(message.contains("input"), "{message}");
    assert!(
        message.contains("the scope could not be reaped"),
        "{message}"
    );
    // A failed stop's words do not say that cleanup is unconfirmed, so the
    // message says it, once.
    assert_eq!(
        message,
        "the extension was started without crucible keeping its input, so there is \
         no way to answer it; process cleanup remains unconfirmed: the scope could \
         not be reaped"
    );
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
    let Unstarted::Unreaped { cause, cleanup } = refused else {
        panic!("cleanup uncertainty must be typed");
    };
    assert!(matches!(*cause, Unstarted::Unspeakable));
    assert_eq!(cleanup.kind(), io::ErrorKind::Other);
}

#[test]
fn missing_input_retains_a_stop_that_never_answered() {
    // The stop is awaited up to its bound and then given up on, and the
    // timeout it was given up with already says that what it began is
    // unconfirmed: the message does not say it a second time.
    let (mut process, watched) = Fake::new([], Ending::Unanswering);
    process.speaks = false;
    let refused = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .unwrap_err();
    assert_eq!(
        refused.to_string(),
        "the extension was started without crucible keeping its input, so there is \
         no way to answer it; process cleanup: stopping a hosted program did not \
         answer within 10s, so whatever it began is unconfirmed"
    );
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
    let Unstarted::Unreaped { cause, cleanup } = refused else {
        panic!("cleanup uncertainty must be typed");
    };
    assert!(matches!(*cause, Unstarted::Unspeakable));
    assert_eq!(cleanup.kind(), io::ErrorKind::TimedOut);
    assert!(
        matches!(
            cleanup.get_ref(),
            Some(held) if held.is::<crucible_transport::Unanswered>()
        ),
        "the timeout is carried as itself, not as its words: {cleanup:?}"
    );
}

#[test]
fn missing_output_retains_failed_cleanup() {
    let (mut process, watched) = Fake::new([], Ending::Unreapable);
    process.stdout = None;
    let refused = on(Hosted::<()>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .unwrap_err();
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

/// Asking the extension something begins an exchange of its own. A turn given
/// up on while the extension was quiet leaves that silence behind it, and once
/// crucible has asked something new, the wait for the answer sits through a
/// silence of its own rather than the remainder of the last one.
#[test]
fn a_new_request_sits_through_a_silence_of_its_own() {
    let patience = Duration::from_secs(1);
    let began = Instant::now();
    let (process, _) = Fake::new(
        [Step::SaysFrom(
            began + Duration::from_millis(1300),
            r#"{"jsonrpc":"2.0","id":0,"result":null}"#,
        )],
        Ending::Exited,
    );
    let mut hosted = on(Hosted::<&str>::over(
        process,
        patience,
        &crate::testing::runtime(),
    ))
    .expect("hosted");

    let given_up =
        on(async { tokio::time::timeout(Duration::from_millis(900), hosted.turn()).await });
    assert!(given_up.is_err(), "nothing was said yet: {given_up:?}");
    on(hosted.ask("work", json!({}), "the new request")).expect("asked");

    // The silence the first turn began would have ended a full patience after
    // it began, three hundred milliseconds before the extension answers.
    let turn = on(hosted.turn()).expect("the answer waits a patience of its own");
    assert_eq!(
        turn,
        Turn::Answer {
            waiting: "the new request",
            outcome: Outcome::Worked(Value::Null),
        }
    );
}

/// What the extension says when it asks crucible for the workspace root.
const ASKS_FOR_THE_ROOT: &str = r#"{"jsonrpc":"2.0","id":7,"method":"workspace/root","params":{}}"#;

/// Once a replacement is hosted, a call the replaced extension made, or that
/// crucible made to it, is refused by a typed outcome and never reaches the
/// replacement, even where the replacement has calls open under the same
/// numbers.
#[test]
fn a_call_on_a_replaced_generation_is_refused() {
    let runtime = crate::testing::runtime();
    let (old, replaced) = Fake::new([Step::Says(ASKS_FOR_THE_ROOT)], Ending::Stubborn);
    let mut hosted = on(Hosted::<&str>::over(old, PATIENCE, &runtime)).expect("hosted");
    let Turn::Asked { id: theirs, .. } = on(hosted.turn()).expect("a turn") else {
        panic!("a request is something to answer");
    };
    let ours = on(hosted.ask("tools/list", json!({}), "the replaced one's")).expect("asked");

    let (new, watched) = Fake::new([Step::Says(ASKS_FOR_THE_ROOT)], Ending::Exited);
    let ended =
        on(hosted.replace(new, PATIENCE, &runtime, Duration::from_millis(20))).expect("replaced");
    assert_eq!(
        ended.waiting,
        vec![(ours, "the replaced one's")],
        "the replaced extension's unanswered calls come back with it"
    );
    assert!(
        matches!(ended.finish, Finish::Stopped),
        "the replaced extension outlasted its grace and was stopped: {:?}",
        ended.finish
    );
    assert_eq!(replaced.stopped.load(Ordering::Relaxed), 1);
    assert!(
        replaced.closed.load(Ordering::Relaxed),
        "the replaced extension's input is closed"
    );
    let Turn::Asked { id: current, .. } = on(hosted.turn()).expect("the replacement's turn") else {
        panic!("a request is something to answer");
    };
    let fresh = on(hosted.ask("tools/list", json!({}), "the replacement's")).expect("asked");

    let refused = on(hosted.answer(theirs, Outcome::Worked(json!("meant for the first"))))
        .expect_err("a replaced generation's call is not answered");
    assert!(
        matches!(refused, Asking::Refused(CallError::Elsewhere { call }) if call == theirs),
        "{refused:?}"
    );
    let refused = hosted
        .give_up(ours)
        .expect_err("a replaced generation's call is not given up on");
    assert!(
        matches!(refused, Asking::Refused(CallError::Elsewhere { call }) if call == ours),
        "{refused:?}"
    );
    let said = String::from_utf8(watched.said.lock().expect("said").clone()).expect("utf-8");
    assert!(
        !said.contains("meant for the first"),
        "nothing meant for the replaced extension reaches its replacement: {said:?}"
    );

    on(hosted.answer(current, Outcome::Worked(json!("for the second"))))
        .expect("the replacement's own call is answered");
    let ended = on(hosted.stop(PATIENCE));
    assert_eq!(
        ended.waiting,
        vec![(fresh, "the replacement's")],
        "the replacement's own call is still waiting"
    );
}

/// A replacement crucible cannot speak to never started, so the extension it
/// would have replaced is still the one being spoken to.
#[test]
fn a_replacement_that_cannot_be_hosted_leaves_the_extension_as_it_was() {
    let runtime = crate::testing::runtime();
    let (old, _) = Fake::new([Step::Says(ASKS_FOR_THE_ROOT)], Ending::Exited);
    let mut hosted = on(Hosted::<()>::over(old, PATIENCE, &runtime)).expect("hosted");
    let Turn::Asked { id, .. } = on(hosted.turn()).expect("a turn") else {
        panic!("a request is something to answer");
    };
    let (mut unspeakable, watched) = Fake::new([], Ending::Stubborn);
    unspeakable.speaks = false;

    let refused = on(hosted.replace(unspeakable, PATIENCE, &runtime, PATIENCE))
        .expect_err("nothing to speak over");

    assert!(matches!(refused, Unstarted::Unspeakable), "{refused:?}");
    assert_eq!(
        watched.stopped.load(Ordering::Relaxed),
        1,
        "a replacement that will not be hosted is not left running"
    );
    on(hosted.answer(id, Outcome::Worked(Value::Null)))
        .expect("the extension that was not replaced is still spoken to");
}

/// A process offered once every generation has been handed out cannot be
/// numbered, and a number used again would be one an earlier call still
/// carries. It is stopped rather than hosted.
#[test]
fn a_process_offered_when_generations_have_run_out_is_stopped_rather_than_hosted() {
    let spent = AtomicU64::new(u64::MAX);
    let (process, watched) = Fake::new([Step::Waits], Ending::Stubborn);

    let refused = on(Hosted::<()>::hosting(
        process,
        PATIENCE,
        &crate::testing::runtime(),
        Generation::drawn_from(&spent),
    ))
    .expect_err("there is no generation left to give it");

    assert!(
        matches!(
            refused,
            Unstarted::Spent {
                finish: Finish::Stopped
            }
        ),
        "{refused:?}"
    );
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
}

/// Answers that arrive in another order than the calls went out each come back
/// with what was remembered against the call they answer, and with their own
/// outcome: the extension decides the order, and the number is what matches.
#[test]
fn out_of_order_answers_keep_their_outcomes() {
    let (process, _) = Fake::new(
        [
            Step::Says(r#"{"jsonrpc":"2.0","id":2,"result":"third"}"#),
            Step::Says(r#"{"jsonrpc":"2.0","id":0,"error":"first failed"}"#),
            Step::Says(r#"{"jsonrpc":"2.0","id":1,"result":["second"]}"#),
        ],
        Ending::Exited,
    );
    let mut hosted = on(Hosted::<&str>::over(
        process,
        PATIENCE,
        &crate::testing::runtime(),
    ))
    .expect("hosted");
    for about in ["the first", "the second", "the third"] {
        on(hosted.ask("work", json!({}), about)).expect("asked");
    }

    let answers: Vec<Turn<&str>> = (0..3)
        .map(|_| on(hosted.turn()).expect("an answer"))
        .collect();

    assert_eq!(
        answers,
        vec![
            Turn::Answer {
                waiting: "the third",
                outcome: Outcome::Worked(json!("third")),
            },
            Turn::Answer {
                waiting: "the first",
                outcome: Outcome::Failed(Trouble::new("first failed").expect("words")),
            },
            Turn::Answer {
                waiting: "the second",
                outcome: Outcome::Worked(json!(["second"])),
            },
        ]
    );
}

/// A call is tied to the process it was made with, whichever host holds it: a
/// host started afresh for a new process refuses a call an earlier host's
/// process made, even where the new process has a call open under the same
/// number.
#[test]
fn a_call_from_another_host_is_refused() {
    let runtime = crate::testing::runtime();
    let (old, _) = Fake::new([Step::Says(ASKS_FOR_THE_ROOT)], Ending::Exited);
    let mut first = on(Hosted::<()>::over(old, PATIENCE, &runtime)).expect("hosted");
    let Turn::Asked { id: stale, .. } = on(first.turn()).expect("a turn") else {
        panic!("a request is something to answer");
    };
    let _ = on(first.stop(PATIENCE));

    let (new, watched) = Fake::new([Step::Says(ASKS_FOR_THE_ROOT)], Ending::Exited);
    let mut second = on(Hosted::<()>::over(new, PATIENCE, &runtime)).expect("hosted");
    let Turn::Asked { .. } = on(second.turn()).expect("a turn") else {
        panic!("a request is something to answer");
    };

    let refused = on(second.answer(stale, Outcome::Worked(json!("meant for the first"))))
        .expect_err("another host's call is not answered");

    assert!(
        matches!(refused, Asking::Refused(CallError::Elsewhere { call }) if call == stale),
        "{refused:?}"
    );
    let said = String::from_utf8(watched.said.lock().expect("said").clone()).expect("utf-8");
    assert!(
        !said.contains("meant for the first"),
        "nothing meant for the first process reaches the second: {said:?}"
    );
}

/// A turn given up on and taken up again is still waiting through the same
/// silence. A host that steps away from a quiet extension and comes back more
/// often than the patience still has it ended once the patience is out.
#[test]
fn a_turn_taken_up_again_keeps_its_silence() {
    let (process, _) = Fake::new([Step::Waits], Ending::Exited);
    let mut hosted = on(Hosted::<()>::over(
        process,
        Duration::from_millis(500),
        &crate::testing::runtime(),
    ))
    .expect("hosted");
    let began = Instant::now();

    let ended = on(async {
        for _ in 0..10 {
            if let Ok(ended) = tokio::time::timeout(Duration::from_millis(300), hosted.turn()).await
            {
                return Some(ended);
            }
        }
        None
    });

    assert!(
        matches!(ended, Some(Err(Over::Unreadable { .. }))),
        "a silent extension is ended however often its turn is taken up again: \
         {ended:?} after {:?}",
        began.elapsed()
    );
}

/// Answering the extension begins an exchange too: once crucible has answered,
/// the extension is the one that owes something next. A silence the host began
/// while it was still working out its answer does not carry across that
/// answer, so an extension that replies promptly is not ended for the time
/// crucible spent.
#[test]
fn an_answer_sent_sits_through_a_silence_of_its_own() {
    let patience = Duration::from_secs(1);
    let began = Instant::now();
    let (process, _) = Fake::new(
        [
            Step::Says(ASKS_FOR_THE_ROOT),
            Step::SaysFrom(
                began + Duration::from_millis(1300),
                r#"{"jsonrpc":"2.0","method":"ready","params":null}"#,
            ),
        ],
        Ending::Exited,
    );
    let mut hosted = on(Hosted::<()>::over(
        process,
        patience,
        &crate::testing::runtime(),
    ))
    .expect("hosted");
    let Turn::Asked { id, .. } = on(hosted.turn()).expect("a turn") else {
        panic!("a request is something to answer");
    };

    // The host steps away from a turn while it works the answer out.
    let given_up =
        on(async { tokio::time::timeout(Duration::from_millis(700), hosted.turn()).await });
    assert!(given_up.is_err(), "nothing was said yet: {given_up:?}");
    thread::sleep(Duration::from_millis(250));
    on(hosted.answer(id, Outcome::Worked(json!({"root": "/workspace"})))).expect("answered");

    // The silence the stepped-away turn began would have ended a full patience
    // after it began, three hundred milliseconds before the extension speaks.
    let turn = on(hosted.turn()).expect("the reply waits a patience of its own");
    assert!(
        matches!(turn, Turn::Told { ref method, .. } if &**method == "ready"),
        "{turn:?}"
    );
}
