//! What hosting an MCP server over a confined process has to guarantee.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Ancestry, Cancel, Finish, SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance,
    SandboxCapabilities, SandboxFilesystemAccess, SandboxFilesystemProvenance,
    SandboxFilesystemRule, SandboxId, SandboxInspection, SandboxManifest, SandboxNetworkPolicy,
    SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead, SandboxRequest,
    SandboxResourceLimits, SandboxUsage, SandboxViolation, ToolId,
};
use serde_json::{Value, json};

use super::{Hosted, Unstarted};
use crate::VERSIONS;
use crate::catalogue::Offered;

/// How long a test sits through one silence.
///
/// Long enough that a loaded machine does not fail a test about protocol, short
/// enough that the one test which does wait it out is still a test.
const PATIENCE: Duration = Duration::from_millis(500);

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
        .expect("a rule over an absolute root")],
        ROOT,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::default(),
    )
    .expect("a policy whose rules are all rooted");
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("mcp"),
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
    SandboxInspection::unconfined_for_request(
        backend,
        SandboxCapabilities::none(),
        &request,
        "a test, which confines nothing",
    )
    .expect("an inspection of a request a test built")
}

/// One thing a stream does when it is asked what it has.
enum Step {
    /// It has a frame, and the newline that ends one.
    Says(String),
    /// It has nothing yet, and its writer is still there.
    Waits,
    /// It waits a moment, and then has a byte that finishes nothing.
    ///
    /// A silence is measured between bytes, so a run of these is never quiet
    /// for long enough to be given up on — and never says a whole frame
    /// either. It is the shape a patience-per-silence cannot bound.
    Trickles,
}

/// How long a [`Step::Trickles`] holds its byte back.
///
/// Under any patience a test using it sets, so the silence between two of them
/// never runs out.
const TRICKLE: Duration = Duration::from_millis(20);

/// A scripted stream that goes quiet forever once its script runs out.
struct Says(VecDeque<Step>);

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        match self.0.pop_front() {
            None | Some(Step::Waits) => Ok(SandboxRead::Pending),
            Some(Step::Trickles) => {
                thread::sleep(TRICKLE);
                match buffer.first_mut() {
                    Some(into) => {
                        *into = b' ';
                        Ok(SandboxRead::Bytes(1))
                    }
                    None => Ok(SandboxRead::Pending),
                }
            }
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

/// Everything a test wants to know about a process after the session has had it.
#[derive(Default)]
struct Watched {
    /// What crucible said to it.
    said: Mutex<Vec<u8>>,
    /// Whether its input has been closed.
    closed: AtomicBool,
    /// How many times it was asked to stop.
    stopped: AtomicUsize,
}

impl Watched {
    /// Every message crucible sent, in order.
    fn sent(&self) -> Vec<Value> {
        let said = self.said.lock().expect("what crucible said");
        String::from_utf8_lossy(&said)
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_str(line).expect("crucible sends whole JSON frames"))
            .collect()
    }
}

/// The writing end of a process's input, as a test can read it back.
struct Input(Arc<Watched>);

impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .said
            .lock()
            .expect("what crucible said")
            .extend_from_slice(bytes);
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
    /// It exits once its input closes, which is what a server does.
    OnEof,
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
    /// What the sandbox stopped it for, where anything.
    violation: Option<SandboxViolation>,
    /// What a test reads back afterwards.
    watched: Arc<Watched>,
    /// Its redacted report.
    inspection: SandboxInspection,
}

impl Fake {
    /// A process that says `steps` and then finishes the way `ending` says.
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
            Ending::OnEof if self.watched.closed.load(Ordering::Relaxed) => Ok(Some(exited())),
            Ending::Stubborn | Ending::Unreapable | Ending::OnEof => Ok(None),
        }
    }

    fn stop(&mut self) -> io::Result<()> {
        self.watched.stopped.fetch_add(1, Ordering::Relaxed);
        match self.ending {
            Ending::Unreapable => Err(io::Error::other("the scope could not be reaped")),
            Ending::Exited | Ending::Stubborn | Ending::OnEof => Ok(()),
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
    }
}

/// The newest version crucible speaks, which is what it offers first.
fn newest() -> &'static str {
    VERSIONS.first().expect("crucible speaks a version")
}

/// An `initialize` answer agreeing `version` and declaring a tool capability.
fn greeted(version: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "protocolVersion": version,
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "docs", "version": "1"},
        },
    })
    .to_string()
}

/// A `tools/call` answer carrying one line of text.
fn produced(id: u64, said: &str) -> String {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {"content": [{"type": "text", "text": said}]},
    })
    .to_string()
}

/// A `tools/list` answer offering exactly `names`, with no further page.
fn listed(id: u64, names: &[&str]) -> String {
    let tools: Vec<Value> = names
        .iter()
        .map(|name| json!({"name": name, "inputSchema": {"type": "object"}}))
        .collect();
    json!({"jsonrpc": "2.0", "id": id, "result": {"tools": tools}}).to_string()
}

#[test]
fn a_process_crucible_cannot_speak_to_is_stopped_rather_than_hosted() {
    let (mut fake, watched) = Fake::new([], Ending::Stubborn);
    fake.speaks = false;

    let refused = Hosted::over(fake, PATIENCE).expect_err("a server with no input is no server");

    assert!(matches!(refused, Unstarted::Unspeakable));
    assert_eq!(
        watched.stopped.load(Ordering::Relaxed),
        1,
        "a process crucible will not host is a process it has to stop"
    );
}

#[test]
fn a_process_crucible_cannot_hear_is_stopped_rather_than_hosted() {
    let (mut fake, watched) = Fake::new([], Ending::Stubborn);
    fake.stdout = None;

    let refused = Hosted::over(fake, PATIENCE).expect_err("a server with no output is no server");

    assert!(matches!(refused, Unstarted::Unheard));
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
}

#[test]
fn a_handshake_and_a_catalogue_travel_over_the_process_streams() {
    let (fake, watched) = Fake::new(
        [
            Step::Says(greeted(newest())),
            Step::Says(listed(2, &["search", "fetch"])),
        ],
        Ending::Exited,
    );

    let mut hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let greeting = hosted
        .greet(None)
        .expect("a server offering a version crucible speaks");
    let offered = hosted
        .catalogue(&greeting, None)
        .expect("a catalogue within every bound");

    assert_eq!(greeting.version(), newest());
    assert_eq!(
        offered.iter().map(Offered::name).collect::<Vec<_>>(),
        ["search", "fetch"]
    );
    let sent = watched.sent();
    assert_eq!(
        sent.iter()
            .filter_map(|message| message.get("method").and_then(Value::as_str))
            .collect::<Vec<_>>(),
        ["initialize", "notifications/initialized", "tools/list"],
        "the handshake is finished before a catalogue is asked for"
    );
}

#[test]
fn a_tool_the_catalogue_offered_can_then_be_called_over_the_same_streams() {
    let (fake, watched) = Fake::new(
        [
            Step::Says(greeted(newest())),
            Step::Says(listed(2, &["search"])),
            Step::Says(produced(3, "one match")),
        ],
        Ending::Exited,
    );

    let mut hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let greeting = hosted.greet(None).expect("an agreeable server");
    let offered = hosted
        .catalogue(&greeting, None)
        .expect("a catalogue within bounds");
    let tool = offered.first().expect("the server offered one tool");
    let answered = hosted
        .call(tool, &json!({"query": "sandbox"}), None)
        .expect("the server answered the call");

    assert_eq!(answered.text(), "one match");
    assert!(!answered.failed());

    let called = watched
        .sent()
        .into_iter()
        .find(|message| message.get("method") == Some(&json!("tools/call")))
        .expect("crucible sent the call over the process's own input");
    assert_eq!(
        called.pointer("/params/name"),
        Some(&json!("search")),
        "the name that goes across is the one this server's catalogue carried"
    );
    assert_eq!(
        called.pointer("/params/arguments"),
        Some(&json!({"query": "sandbox"}))
    );
}

#[test]
fn a_server_that_says_nothing_is_given_up_on_rather_than_waited_out_forever() {
    let (fake, _watched) = Fake::new([Step::Waits], Ending::Stubborn);

    let mut hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let refused = hosted.greet(None).expect_err("a server that never answers");

    assert!(
        refused.to_string().contains("said nothing"),
        "the reason has to say the server went quiet, not merely that something failed: {refused}"
    );
}

/// Whether the server's input reaches the state of being closed.
///
/// The pipe lives on the thread that writes to it, so ending the conversation
/// closes it by leaving that thread nothing further to receive, and the close
/// itself happens over there. Waiting for it is the only way to ask the
/// question without asserting an ordering between two threads that nothing
/// guarantees; a session that never let go of the pipe waits this out.
fn closes(watched: &Watched) -> bool {
    let until = Instant::now() + PATIENCE;
    while Instant::now() < until {
        if watched.closed.load(Ordering::Relaxed) {
            return true;
        }
        thread::sleep(Duration::from_millis(1));
    }
    false
}

#[test]
fn ending_a_session_closes_crucibles_end_before_the_process_is_stopped() {
    let (fake, watched) = Fake::new([Step::Says(greeted(newest()))], Ending::Stubborn);

    let hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let ended = hosted.stop(Duration::ZERO);

    assert!(
        closes(&watched),
        "a server is told there is nothing further by having its input closed"
    );
    assert!(matches!(ended.finish, Finish::Stopped));
    assert_eq!(watched.stopped.load(Ordering::Relaxed), 1);
}

#[test]
fn a_server_that_goes_when_its_input_closes_is_given_the_chance_to() {
    let (fake, watched) = Fake::new([Step::Waits], Ending::OnEof);

    let hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let ended = hosted.stop(PATIENCE);

    assert!(
        matches!(ended.finish, Finish::Exited(_)),
        "the grace is the chance to act on a closed input, so the closing has to \
         come first for there to be anything to act on: {:?}",
        ended.finish
    );
    assert_eq!(
        watched.stopped.load(Ordering::Relaxed),
        0,
        "a server that took the chance does not also need killing"
    );
}

#[test]
fn a_process_that_went_on_its_own_is_reaped_rather_than_stopped() {
    let (fake, watched) = Fake::new([], Ending::Exited);

    let hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let ended = hosted.stop(Duration::ZERO);

    assert!(matches!(ended.finish, Finish::Exited(_)));
    assert_eq!(
        watched.stopped.load(Ordering::Relaxed),
        0,
        "a process that already finished has nothing left to stop"
    );
}

#[test]
fn a_process_that_cannot_be_reaped_says_so_rather_than_reporting_a_clean_ending() {
    let (fake, _watched) = Fake::new([], Ending::Unreapable);

    let hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let ended = hosted.stop(Duration::ZERO);

    assert!(matches!(ended.finish, Finish::Unreaped(_)));
}

#[test]
fn confinement_that_killed_a_server_is_what_the_ending_carries() {
    let (mut fake, _watched) = Fake::new([Step::Waits], Ending::Exited);
    fake.violation = Some(SandboxViolation::CommandTime);

    let hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    let ended = hosted.stop(Duration::ZERO);

    assert_eq!(
        ended.violation,
        Some(SandboxViolation::CommandTime),
        "a server crucible's own deadline killed says nothing on the way out, so \
         the ending is the only place that can explain it"
    );
}

#[test]
fn what_a_server_complained_about_survives_the_ending_that_it_explains() {
    let (mut fake, _watched) = Fake::new([Step::Waits], Ending::Stubborn);
    fake.stderr = Some(Says(
        [Step::Says("docs-mcp: no index at /srv/docs".to_owned())]
            .into_iter()
            .collect(),
    ));

    let hosted = Hosted::over(fake, PATIENCE).expect("a process with both pipes");
    // The drain runs on a thread of its own, so the complaint is read while
    // crucible waits out the silence rather than before it starts.
    let mut hosted = hosted;
    drop(hosted.greet(None));
    let ended = hosted.stop(Duration::ZERO);

    assert!(
        ended.muttered.text().contains("no index"),
        "standard error is usually the only thing that says why a server ended: {:?}",
        ended.muttered.text()
    );
}

#[test]
fn a_handshake_nobody_is_waiting_for_any_more_ends_at_the_press() {
    // A server that never answers the greeting, and a patience nobody sits
    // through. Start-up is where a press matters most: nothing has been asked
    // of the far end yet, so giving up leaves nothing behind to wonder about.
    let (fake, _watched) = Fake::new([Step::Waits], Ending::Exited);
    let cancel = Cancel::new();
    cancel.request();
    let mut hosted =
        Hosted::over(fake, Duration::from_secs(30)).expect("a process with both pipes");

    let began = Instant::now();
    let rebuffed = hosted
        .greet(Some(&cancel))
        .expect_err("a handshake that was called off is not a greeting");
    let waited = began.elapsed();

    assert!(
        waited < Duration::from_secs(5),
        "a handshake carries the press the same way a call does, or a server that \
         says nothing holds the whole start-up open: {waited:?} against {rebuffed}"
    );
}

#[test]
fn a_server_that_speaks_just_often_enough_to_never_fall_silent_is_still_given_up_on() {
    // `requestSeconds` is spent on one quiet stretch and handed back the moment
    // anything moves, so a server saying a byte just short of it and then going
    // quiet again is never silent long enough to be given up on. It would hold
    // the exchange open for as long as it cared to. A deadline over the whole
    // exchange counts the time rather than the gaps in it.
    let patience = Duration::from_millis(200);
    let dribbling = TRICKLE * 100;
    let (fake, _watched) = Fake::new(
        std::iter::repeat_with(|| Step::Trickles).take(100),
        Ending::Exited,
    );
    let mut hosted = Hosted::over(fake, patience).expect("a process with both pipes");

    let began = Instant::now();
    let refused = hosted
        .greet(None)
        .expect_err("a server that never finishes a frame is not a greeting");
    let waited = began.elapsed();

    assert!(
        waited < dribbling / 2,
        "the exchange ran to the end of what the server was willing to dribble \
         rather than to its own deadline: {waited:?} of a possible {dribbling:?}, \
         against {refused}"
    );
}

#[test]
fn missing_input_retains_failed_cleanup() {
    let (mut process, watched) = Fake::new([], Ending::Unreapable);
    process.speaks = false;
    let refused = Hosted::over(process, PATIENCE).unwrap_err();
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
    let refused = Hosted::over(process, PATIENCE).unwrap_err();
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
