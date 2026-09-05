//! What hosting selected MCP servers beside the built-in roster has to
//! guarantee.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crucible_core::{
    Ancestry, Approved, Ask, Cancel, Mode, Permission, Remember, Revealed, Rules, SandboxBackendId,
    SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities, SandboxCommand,
    SandboxEnvironment, SandboxError, SandboxFilesystemAccess, SandboxFilesystemProvenance,
    SandboxFilesystemRule, SandboxId, SandboxInspection, SandboxLaunch, SandboxManifest,
    SandboxMode, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead,
    SandboxRequest, SandboxResourceLimits, SandboxService, SandboxSession, SandboxUsage,
    SandboxViolation, Sensitivity, Settled, Summary, Tool, ToolArgs, ToolCall, ToolContext,
    ToolDescriptor, ToolError, ToolId, ToolOutput, ToolProvenance, ToolSourceKind, Toolset,
    ToolsetContext, ToolsetError, Verdict,
};
use crucible_runner::Tools;
use serde_json::{Value, json};

use super::{Chosen, Hosting};

mod audit;
mod cleanup;

/// How long a test lets one silence run before it gives up on a server.
///
/// Long enough that a loaded machine does not fail a test about composition,
/// short enough that a test which does wait it out is still a test.
const PATIENCE: Duration = Duration::from_millis(500);

/// How long a stopped server is given to go on its own.
const GRACE: Duration = Duration::from_millis(50);

/// An absolute path spelled the way the running platform's path type accepts.
#[cfg(unix)]
const ROOT: &str = "/workspace";
#[cfg(windows)]
const ROOT: &str = r"C:\workspace";

/// An absolute program, which is the only kind a sandbox command accepts.
#[cfg(unix)]
const PROGRAM: &str = "/usr/bin/docs-server";
#[cfg(windows)]
const PROGRAM: &str = r"C:\docs-server.exe";

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

/// The policy every server here is started under.
fn policy() -> SandboxPolicy {
    SandboxPolicy::new(
        SandboxMode::Required,
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
    .expect("a policy whose rules are all rooted")
}

/// A redacted inspection, which every process has to be able to show.
fn inspection() -> SandboxInspection {
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("mcp"),
        policy(),
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

/// A scripted stream that goes quiet forever once its script runs out.
struct Says {
    frames: VecDeque<String>,
    /// A frame it thinks about before saying, and for how long.
    ///
    /// Real rather than simulated, because what it has to exercise is a
    /// patience measured against a clock: a fake clock here would prove that
    /// the fake was consulted and nothing about the wait.
    slow: Option<(usize, Duration)>,
    /// When the frame it is thinking about is due.
    due: Option<Instant>,
    /// How many it has said.
    at: usize,
}

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        // Quiet rather than blocked: a server thinking about an answer leaves
        // its reader with nothing to read, and a fake that slept inside the
        // read would hand the reader its bytes the instant it woke — which is
        // a reader that never saw a silence to be impatient about.
        if let Some((nth, held)) = self.slow
            && nth == self.at
        {
            let due = *self.due.get_or_insert_with(|| Instant::now() + held);
            if Instant::now() < due {
                return Ok(SandboxRead::Pending);
            }
            self.slow = None;
            self.due = None;
        }
        let Some(frame) = self.frames.pop_front() else {
            return Ok(SandboxRead::Pending);
        };
        self.at += 1;
        let said = format!("{frame}\n");
        let bytes = said.as_bytes();
        let taken = bytes.len().min(buffer.len());
        if let Some((into, from)) = buffer.get_mut(..taken).zip(bytes.get(..taken)) {
            into.copy_from_slice(from);
        }
        Ok(SandboxRead::Bytes(taken))
    }
}

/// Everything a test wants to know about one server after the run has had it.
#[derive(Default)]
struct Watched {
    /// What crucible said to it.
    said: Mutex<Vec<u8>>,
    /// How many times it was asked to stop.
    /// Whether crucible has let go of the server's input, which is the only
    /// way this protocol says a conversation is over.
    closed: AtomicBool,
    ended: AtomicUsize,
    /// Whether the process behind this one has gone, so that writing to it
    /// finds nothing on the other end.
    departed: AtomicBool,
    /// Whether the process behind this one has stopped reading, so that a write
    /// runs out of patience with the bytes already gone from crucible's hands.
    deafened: AtomicBool,
    /// Optional lifecycle evidence for the audit transport fixture.
    audit: Mutex<Option<(SandboxId, crucible_core::SandboxAudit)>>,
    /// A backend that cannot confirm the process scope ended.
    cleanup_refused: AtomicBool,
    stop_attempts: AtomicUsize,
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

    /// How many times this process was brought to an end.
    ///
    /// Counted rather than asserted as a flag because the thing worth proving
    /// is that a second disposal reaches nothing: a count says that, and a flag
    /// that is already set says nothing at all.
    fn stops(&self) -> usize {
        self.ended.load(Ordering::Relaxed)
    }

    /// Makes this server's process gone, the way a program that fell over
    /// between two calls is gone.
    ///
    /// The next frame crucible tries to write finds nothing reading, which is
    /// the one failure that leaves the far end exactly as it was: the call it
    /// belonged to was never seen by anybody.
    fn departs(&self) {
        self.departed.store(true, Ordering::Relaxed);
    }

    /// Makes this server stop reading its input, the way a program still
    /// running but no longer listening does.
    ///
    /// The opposite ending to [`Self::departs`] and the reason the two are
    /// separate: crucible spends a patience and gives up, but the bytes were
    /// handed to the thread that owns the pipe before that wait began, so the
    /// far end may read the call the moment after crucible stopped waiting for
    /// it to.
    fn deafens(&self) {
        self.deafened.store(true, Ordering::Relaxed);
    }
}

/// The writing end of a server's input, as a test can read it back.
struct Input(Arc<Watched>);

impl Drop for Input {
    /// Letting go of it is what tells the server to finish.
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Relaxed);
    }
}

impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if self.0.departed.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "the server this was written to has gone",
            ));
        }
        if self.0.deafened.load(Ordering::Relaxed) {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "the server this was written to stopped reading",
            ));
        }
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

/// A server the sandbox might have started, doing only what a test needs.
struct Fake {
    stdout: Option<Says>,
    watched: Arc<Watched>,
    inspection: SandboxInspection,
    /// Whether this one's ending has been counted, so that a second look at a
    /// process that already finished is not a second ending.
    reaped: bool,
}

impl Fake {
    /// Records this process finishing, once however often it is asked.
    fn end(&mut self) {
        if !self.reaped {
            self.reaped = true;
            self.watched.ended.fetch_add(1, Ordering::Relaxed);
            if let Some((id, audit)) = self.watched.audit.lock().unwrap().take() {
                audit
                    .record(
                        id,
                        crucible_core::SandboxFactKind::Cleanup(
                            crucible_core::SandboxCleanup::Complete,
                        ),
                    )
                    .unwrap();
            }
        }
    }
}

impl SandboxProcess for Fake {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        Some(Box::new(Input(Arc::clone(&self.watched))))
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        self.stdout
            .take()
            .map(|says| Box::new(says) as Box<dyn SandboxOutput>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.watched.cleanup_refused.load(Ordering::Relaxed) {
            return Err(io::Error::other("injected cleanup observation failure"));
        }
        // Running until it is told otherwise, which is what a server does. A
        // process that reported an exit before anything asked it to finish
        // would let a disposal that stopped nothing look like one that worked.
        if !self.watched.closed.load(Ordering::Relaxed) {
            return Ok(None);
        }
        self.end();
        Ok(Some(exited()))
    }

    fn stop(&mut self) -> io::Result<()> {
        self.watched.stop_attempts.fetch_add(1, Ordering::Relaxed);
        if self.watched.cleanup_refused.load(Ordering::Relaxed) {
            return Err(io::Error::other("injected unconfirmed cleanup"));
        }
        self.end();
        Ok(())
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

/// A staged command that has not been let go yet.
struct Held {
    frames: Vec<String>,
    /// The frame this one dawdles before, and for how long.
    slow: Option<(usize, Duration)>,
    watched: Arc<Watched>,
    inspection: SandboxInspection,
}

impl SandboxLaunch for Held {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn release(self: Box<Self>) -> Result<Box<dyn SandboxProcess>, SandboxError> {
        Ok(Box::new(Fake {
            stdout: Some(Says {
                frames: self.frames.into_iter().collect(),
                slow: self.slow,
                due: None,
                at: 0,
            }),
            watched: self.watched,
            inspection: self.inspection,
            reaped: false,
        }))
    }
}

/// A prepared session, which here only remembers what its server will say.
struct Prepared {
    frames: Vec<String>,
    /// The frame this one dawdles before, and for how long.
    slow: Option<(usize, Duration)>,
    watched: Arc<Watched>,
    inspection: SandboxInspection,
    /// Every command staged through it, so a test can see what was started.
    staged: Arc<Mutex<Vec<Vec<OsString>>>>,
}

impl SandboxSession for Prepared {
    fn inspection(&self) -> &SandboxInspection {
        &self.inspection
    }

    fn materialize(&mut self) -> Result<(), SandboxError> {
        Ok(())
    }

    fn stage(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> Result<Box<dyn SandboxLaunch>, SandboxError> {
        let mut image = vec![OsString::from(command.program())];
        image.extend(command.arguments().iter().cloned());
        self.staged.lock().expect("what was staged").push(image);
        Ok(Box::new(Held {
            frames: self.frames,
            slow: self.slow,
            watched: self.watched,
            inspection: self.inspection,
        }))
    }
}

/// What one selected server does when crucible tries to start it.
enum Answers {
    /// It starts, and says these frames in order.
    Says(Vec<Value>),
    /// It talks normally but its process scope cannot be confirmed stopped.
    Unreapable(Vec<Value>),
    /// The same, but it thinks for a while before the `n`th of them.
    Slowly(Vec<Value>, usize, Duration),
    /// It refuses to start at all.
    Refuses,
}

/// A sandbox that starts whatever a test scripted, and nothing else.
#[derive(Default)]
struct Pretend {
    /// One script per preparation, in the order they are asked for.
    scripts: Mutex<VecDeque<Answers>>,
    /// What every started server was watched doing, in start order.
    watched: Mutex<Vec<Arc<Watched>>>,
    /// The image of every command staged, in start order.
    staged: Arc<Mutex<Vec<Vec<OsString>>>>,
}

impl Pretend {
    /// A sandbox that will start these servers, in this order.
    fn new(scripts: impl IntoIterator<Item = Answers>) -> Arc<Self> {
        Arc::new(Self {
            scripts: Mutex::new(scripts.into_iter().collect()),
            ..Self::default()
        })
    }

    /// The `n`th server it started.
    fn server(&self, n: usize) -> Arc<Watched> {
        let watched = self.watched.lock().expect("the servers it started");
        watched
            .get(n)
            .map_or_else(|| panic!("no server {n} was started"), Arc::clone)
    }

    /// How many servers it started.
    fn started(&self) -> usize {
        self.watched.lock().expect("the servers it started").len()
    }
}

impl SandboxService for Pretend {
    fn probe(&self) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
        Ok((
            SandboxBackendIdentity::new(
                SandboxBackendId::new("test").expect("a backend name"),
                "1",
                SandboxBackendProvenance::Compatibility,
                None,
            )
            .expect("a backend identity"),
            SandboxCapabilities::none(),
        ))
    }

    fn prepare(&self, _request: SandboxRequest) -> Result<Box<dyn SandboxSession>, SandboxError> {
        let script = self
            .scripts
            .lock()
            .expect("the scripts a test wrote")
            .pop_front();
        let (frames, slow, cleanup_refused) = match script {
            None | Some(Answers::Refuses) => {
                return Err(SandboxError::Lifecycle(io::Error::other(
                    "this machine has no such program",
                )));
            }
            Some(Answers::Says(frames)) => (frames, None, false),
            Some(Answers::Unreapable(frames)) => (frames, None, true),
            Some(Answers::Slowly(frames, nth, held)) => (frames, Some((nth, held)), false),
        };

        let watched = Arc::new(Watched {
            cleanup_refused: AtomicBool::new(cleanup_refused),
            ..Watched::default()
        });
        self.watched
            .lock()
            .expect("the servers it started")
            .push(Arc::clone(&watched));

        Ok(Box::new(Prepared {
            frames: frames.iter().map(ToString::to_string).collect(),
            slow,
            watched,
            inspection: inspection(),
            staged: Arc::clone(&self.staged),
        }))
    }
}

/// The handshake and catalogue a scripted server opens with.
fn opening(named: &str, tools: &Value) -> Vec<Value> {
    vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": {
                "protocolVersion": "2025-06-18",
                "capabilities": { "tools": {} },
                "serverInfo": { "name": named, "version": "1" },
            },
        }),
        json!({ "jsonrpc": "2.0", "id": 2, "result": { "tools": tools } }),
    ]
}

/// One tool a catalogue offers.
fn offers(named: &str) -> Value {
    json!({ "name": named, "inputSchema": { "type": "object" } })
}

/// A `tools/call` answer carrying one line of text.
fn produced(said: &str, failed: bool) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 3,
        "result": {
            "content": [{ "type": "text", "text": said }],
            "isError": failed,
        },
    })
}

/// A tool the binary compiled in, which reaches nothing.
struct Quiet(&'static str);

impl Tool for Quiet {
    fn validate(&self, _args: &ToolArgs) -> Result<(), ToolError> {
        Ok(())
    }

    fn sensitivity(&self, _args: &ToolArgs) -> Sensitivity {
        Sensitivity::ReadOnly {
            target: crucible_core::Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new(self.0)
    }

    fn run(
        &self,
        _approved: Approved,
        _context: &ToolContext<'_>,
    ) -> Result<ToolOutput, ToolError> {
        Ok(ToolOutput::ok("nothing"))
    }
}

/// A built-in roster offering these names and nothing else.
fn builtin(names: &[&'static str]) -> Tools {
    let mut tools = Tools::new();
    for name in names {
        let descriptor = ToolDescriptor::new(
            *name,
            r#"{"type":"object"}"#,
            ToolProvenance::builtin(name).expect("a built-in name fits its own identity"),
        )
        .expect("a descriptor a test wrote");
        tools
            .add(descriptor, Arc::new(Quiet(name)))
            .expect("no two names here are the same");
    }
    tools
}

/// The narrow lifecycle context a toolset is prepared under.
fn lifecycle() -> ToolsetContext {
    ToolsetContext::new(Ancestry::new(), Cancel::new(), None)
}

/// One selected server, named `name` and started from the same program.
fn chosen(name: &str) -> Chosen {
    Chosen::new(name, PROGRAM, [], policy())
        .given(SandboxEnvironment::new([]).expect("an empty environment"))
        .waiting(PATIENCE, PATIENCE, GRACE)
        .required(true)
}

/// One a run selected and can do without.
fn optional(name: &str) -> Chosen {
    chosen(name).required(false)
}

/// Decides one call the only way a call can be decided.
fn allowed(tool: &dyn Tool, name: &str, args: &str) -> Approved {
    struct Yes;

    impl Ask for Yes {
        fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
            (Verdict::Allow, Remember::Never)
        }
    }

    let call = ToolCall {
        id: ToolId::new("test"),
        name: name.into(),
        args: ToolArgs::new(args),
    };
    let sensitivity = tool.sensitivity(&call.args);
    match Permission::with(Mode::default(), Rules::new()).decide(&call, &sensitivity, &mut Yes) {
        Settled::Approved(approved) => approved,
        Settled::Forbidden | Settled::Refused => panic!("the answer above is yes"),
    }
}

#[test]
fn a_run_that_selected_no_server_hosts_nothing_and_offers_the_builtin_roster_itself() {
    let sandbox = Pretend::new([]);
    let hosting = Hosting::new(
        builtin(&["read"]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        Vec::new(),
    );
    let context = lifecycle();

    hosting.prepare(&context).expect("nothing to prepare");
    let snapshot = hosting.snapshot(&context).expect("the built-in roster");

    assert_eq!(sandbox.started(), 0, "no process may be started");
    assert_eq!(snapshot.entries().len(), 1);
    assert!(snapshot.find("read").is_some());
    hosting.dispose(&context).expect("nothing to dispose");
}

#[test]
fn a_selected_server_is_started_and_what_it_offered_is_named_under_it() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        builtin(&["read"]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();

    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");

    assert_eq!(sandbox.started(), 1);
    let entry = snapshot
        .find("mcp:docs/search")
        .expect("the tool the server offered");
    assert_eq!(
        entry.descriptor().provenance().kind(),
        ToolSourceKind::Mcp,
        "a tool from a server says so"
    );
    assert!(
        snapshot.find("read").is_some(),
        "the built-ins are still here"
    );
    hosting.dispose(&context).expect("the server stopped");
}

#[test]
fn a_server_naming_a_tool_the_builtin_roster_owns_takes_nothing_over() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("read")])))]);
    let hosting = Hosting::new(
        builtin(&["read"]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();

    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");

    assert!(
        snapshot.find("read").is_some(),
        "the built-in keeps its name"
    );
    assert!(
        snapshot.find("mcp:docs/read").is_some(),
        "and the server is still offered"
    );
    hosting.dispose(&context).expect("the server stopped");
}

#[test]
fn a_server_offering_two_names_a_rule_cannot_tell_apart_is_never_started() {
    // `search` and `Search` are two tools to the server and one name to every
    // permission rule that could be written about them, because a rule reads a
    // name without case. A verdict given for the first would be spent on the
    // second, so the catalogue refuses the pair where it reads them and the
    // server never starts — the roster is not reached, let alone published.
    let sandbox = Pretend::new([Answers::Says(opening(
        "docs",
        &json!([offers("search"), offers("Search")]),
    ))]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();

    let refused = hosting
        .prepare(&context)
        .expect_err("two names one rule cannot tell apart");

    assert!(
        refused.to_string().contains("Search"),
        "the refusal names the pair: {refused}"
    );
    hosting.dispose(&context).expect("nothing to stop");
}

#[test]
fn a_tool_the_catalogue_offered_is_called_over_the_conversation_that_read_it() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(frames)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let output = entry
        .tool()
        .run(
            allowed(entry.tool(), "mcp:docs/search", r#"{"query":"crates"}"#),
            &crucible_core::ToolContext::new(
                Ancestry::new(),
                ToolId::new("test"),
                &Cancel::new(),
                None,
                &Nothing,
            ),
        )
        .expect("the server answered");

    assert!(output.text().contains("two pages"));
    let sent = sandbox.server(0).sent();
    let call = sent.last().expect("crucible said something last");
    assert_eq!(
        call.get("method").and_then(Value::as_str),
        Some("tools/call")
    );
    assert_eq!(
        call.pointer("/params/name").and_then(Value::as_str),
        Some("search"),
        "the server is asked for the name it knows, not the one the model used"
    );
    assert_eq!(
        call.pointer("/params/arguments/query")
            .and_then(Value::as_str),
        Some("crates")
    );
    hosting.dispose(&context).expect("the server stopped");
}

#[test]
fn a_server_that_will_not_start_fails_the_turn_and_names_which_one() {
    let sandbox = Pretend::new([Answers::Refuses]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();

    let refused = hosting.prepare(&context).expect_err("nothing started");

    assert!(
        matches!(&refused, ToolsetError::Source { id, .. } if id.as_ref() == "docs"),
        "a selected server that never started is named: {refused}"
    );
}

#[test]
fn a_server_answers_its_catalogue_under_the_request_wait_rather_than_the_handshake() {
    // Two frames: the greeting, then the catalogue. The pause is before the
    // second, so the handshake is met and the catalogue is not — under the
    // handshake's patience. A conversation held at one number cannot pass this
    // and refuse a handshake that takes just as long.
    let dawdling = Duration::from_millis(120);
    let sandbox = Pretend::new([Answers::Slowly(
        opening("docs", &json!([offers("search")])),
        1,
        dawdling,
    )]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![
            Chosen::new("docs", PROGRAM, [], policy())
                .given(SandboxEnvironment::new([]).expect("an empty environment"))
                .waiting(Duration::from_millis(40), PATIENCE, GRACE)
                .required(true),
        ],
    );
    let context = lifecycle();

    hosting
        .prepare(&context)
        .expect("the greeting was prompt and the catalogue is a request");

    let named: Vec<_> = hosting
        .snapshot(&context)
        .expect("one generation")
        .entries()
        .iter()
        .map(|entry| entry.descriptor().name().to_owned())
        .collect();
    assert_eq!(named, vec!["mcp:docs/search".to_owned()]);
    hosting.dispose(&context).expect("it stops");
}

#[test]
fn a_server_too_slow_to_greet_is_refused_before_any_request_wait_applies() {
    // The same wait, spent before the *first* frame. Nothing has agreed a
    // version yet, so the generous request budget has not begun to apply.
    let sandbox = Pretend::new([Answers::Slowly(
        opening("docs", &json!([offers("search")])),
        0,
        Duration::from_millis(120),
    )]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![
            Chosen::new("docs", PROGRAM, [], policy())
                .given(SandboxEnvironment::new([]).expect("an empty environment"))
                .waiting(Duration::from_millis(40), PATIENCE, GRACE)
                .required(true),
        ],
    );

    let refused = hosting
        .prepare(&lifecycle())
        .expect_err("it never agreed a version");

    assert!(
        refused.to_string().contains("docs"),
        "which server would not greet: {refused}"
    );
}

#[test]
fn a_server_the_run_can_do_without_is_left_out_rather_than_fatal() {
    let sandbox = Pretend::new([
        Answers::Says(opening("docs", &json!([offers("search")]))),
        Answers::Refuses,
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs"), optional("notes")],
    );
    let context = lifecycle();

    hosting
        .prepare(&context)
        .expect("a server nobody required cannot fail the turn");

    let named: Vec<_> = hosting
        .snapshot(&context)
        .expect("one generation")
        .entries()
        .iter()
        .map(|entry| entry.descriptor().name().to_owned())
        .collect();
    assert_eq!(
        named,
        vec!["mcp:docs/search".to_owned()],
        "the run carries on with the tools it does have"
    );
    hosting
        .dispose(&context)
        .expect("and stops the one that ran");
}

#[test]
fn a_start_that_fails_partway_stops_the_servers_that_already_ran() {
    let sandbox = Pretend::new([
        Answers::Says(opening("docs", &json!([offers("search")]))),
        Answers::Refuses,
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs"), chosen("notes")],
    );
    let context = lifecycle();

    hosting
        .prepare(&context)
        .expect_err("the second server refused, so the lifecycle has none");

    // Nothing will ever dispose these: the lifecycle they belong to did not
    // begin, so preparation is the only thing that still knows they exist.
    assert_eq!(sandbox.started(), 1);
    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "a server left running is one nothing can reach to stop"
    );
}

#[test]
fn refreshing_republishes_the_committed_generation_rather_than_reading_again() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        builtin(&["read"]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");

    let first = hosting.snapshot(&context).expect("one generation");
    let after = sandbox.server(0).sent().len();
    let second = hosting.refresh(&context).expect("the same generation");

    assert_eq!(
        first.generation().context_id(),
        second.generation().context_id(),
        "an admission from the pass before has to resolve through this one"
    );
    assert_eq!(
        sandbox.server(0).sent().len(),
        after,
        "a refresh must not go back to the server"
    );
    hosting.dispose(&context).expect("the server stopped");
}

#[test]
fn disposal_stops_every_server_it_started() {
    let sandbox = Pretend::new([
        Answers::Says(opening("docs", &json!([offers("search")]))),
        Answers::Says(opening("notes", &json!([offers("find")]))),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs"), chosen("notes")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("both started");

    hosting.dispose(&context).expect("both stopped");

    assert_eq!(sandbox.server(0).stops(), 1);
    assert_eq!(sandbox.server(1).stops(), 1);
}

#[test]
fn disposing_twice_stops_nothing_a_second_time() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");

    hosting.dispose(&context).expect("the server stopped");
    hosting.dispose(&context).expect("and stays stopped");

    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "a second disposal repeats no effect"
    );
}

#[test]
fn a_second_turn_starts_its_servers_again_over_the_same_hosting() {
    let sandbox = Pretend::new([
        Answers::Says(opening("docs", &json!([offers("search")]))),
        Answers::Says(opening("docs", &json!([offers("search")]))),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();

    // The runner prepares and disposes once per turn over one toolset, so the
    // second turn of an ordinary conversation is this exact path. A disposal
    // that left its dead servers behind would make it a turn with no tools
    // whose handles all refuse.
    hosting
        .prepare(&context)
        .expect("the first turn started it");
    hosting.dispose(&context).expect("and stopped it");
    hosting
        .prepare(&context)
        .expect("the second turn started it again");

    assert_eq!(sandbox.started(), 2, "each turn hosts a server of its own");
    let named: Vec<_> = hosting
        .snapshot(&context)
        .expect("one generation")
        .entries()
        .iter()
        .map(|entry| entry.descriptor().name().to_owned())
        .collect();
    assert_eq!(named, vec!["mcp:docs/search".to_owned()]);
    assert_eq!(
        sandbox.server(1).stops(),
        0,
        "the second one is still running"
    );

    hosting.dispose(&context).expect("and it stops too");
    assert_eq!(sandbox.server(1).stops(), 1);
}

#[test]
fn a_handle_from_a_disposed_lifecycle_refuses_rather_than_speaking() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(frames)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let held = snapshot
        .find("mcp:docs/search")
        .expect("the offered tool")
        .shared_tool();

    hosting.dispose(&context).expect("the server stopped");
    let refused = held
        .run(
            allowed(held.as_ref(), "mcp:docs/search", "{}"),
            &crucible_core::ToolContext::new(
                Ancestry::new(),
                ToolId::new("test"),
                &Cancel::new(),
                None,
                &Nothing,
            ),
        )
        .expect_err("the lifecycle it belonged to is over");

    assert!(
        matches!(&refused, ToolError::StaleGeneration { tool } if tool.as_ref() == "mcp:docs/search"),
        "a handle outliving its lifecycle refuses: {refused}"
    );
    let sent = sandbox.server(0).sent();
    assert!(
        !sent
            .iter()
            .any(|frame| frame.get("method").and_then(Value::as_str) == Some("tools/call")),
        "and nothing was said into a pipe that belongs to nothing"
    );
}

#[test]
fn a_tool_that_ran_and_failed_is_a_result_rather_than_a_broken_turn() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("no such index", true));
    let sandbox = Pretend::new([Answers::Says(frames)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let output = entry
        .tool()
        .run(
            allowed(entry.tool(), "mcp:docs/search", "{}"),
            &crucible_core::ToolContext::new(
                Ancestry::new(),
                ToolId::new("test"),
                &Cancel::new(),
                None,
                &Nothing,
            ),
        )
        .expect("a failed tool is still an answer");

    assert!(
        output.is_failed(),
        "what the server said failed is a failed result"
    );
    assert!(output.text().contains("no such index"));
    hosting.dispose(&context).expect("the server stopped");
}

#[test]
fn arguments_that_are_not_an_object_are_refused_before_anything_is_sent() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let refused = entry
        .tool()
        .validate(&ToolArgs::new("[1,2,3]"))
        .expect_err("a list is not one call's arguments");

    assert!(matches!(refused, ToolError::Arguments { .. }));
    let sent = sandbox.server(0).sent();
    assert!(
        !sent
            .iter()
            .any(|frame| frame.get("method").and_then(Value::as_str) == Some("tools/call")),
        "a call crucible refused reaches no server"
    );
    hosting.dispose(&context).expect("the server stopped");
}

/// A watcher that keeps nothing, for the calls whose progress no test reads.
struct Nothing;

impl crucible_core::Watch for Nothing {
    fn wrote(&self, _text: crucible_core::Wrote) {}
}

#[test]
fn the_generation_names_the_servers_in_selection_order_and_each_catalogue_in_its_own() {
    let sandbox = Pretend::new([
        Answers::Says(opening(
            "docs",
            &json!([offers("search"), offers("browse")]),
        )),
        Answers::Says(opening("notes", &json!([offers("find")]))),
    ]);
    let hosting = Hosting::new(
        builtin(&["read"]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs"), chosen("notes")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("both started");

    let snapshot = hosting.snapshot(&context).expect("one generation");
    let named: Vec<&str> = snapshot
        .entries()
        .iter()
        .map(|entry| entry.descriptor().name())
        .collect();

    // The order a provider is told about tools in is the order it caches
    // against, so a generation that reordered itself between two identical
    // runs would cost a cache hit for no reason a reader could name.
    assert_eq!(
        named,
        [
            "read",
            "mcp:docs/search",
            "mcp:docs/browse",
            "mcp:notes/find"
        ]
    );
    assert_eq!(
        snapshot
            .find("mcp:notes/find")
            .expect("the second server's tool")
            .descriptor()
            .provenance()
            .id(),
        "mcp:notes",
        "a tool says which server answered for it, not merely that one did"
    );
    hosting.dispose(&context).expect("both stopped");
}

#[test]
fn a_generation_rebuilt_under_a_moved_roster_keeps_every_name_source_and_approval() {
    let revealed = Revealed::new();
    let mut tools = Tools::looking_up(revealed.clone());
    for (name, held) in [("read", false), ("grep", true)] {
        let descriptor = ToolDescriptor::new(
            name,
            r#"{"type":"object"}"#,
            ToolProvenance::builtin(name).expect("a built-in name fits its own identity"),
        )
        .expect("a descriptor a test wrote");
        let tool = Arc::new(Quiet(name));
        if held {
            tools.defer(descriptor, tool)
        } else {
            tools.add(descriptor, tool)
        }
        .expect("no two names here are the same");
    }

    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        tools,
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");

    let first = hosting.snapshot(&context).expect("one generation");
    let before = first.find("mcp:docs/search").expect("the server's tool");
    let source = before.descriptor().provenance().id().to_owned();
    let approval = before.tool().sensitivity(&ToolArgs::new("{}"));
    let read = sandbox.server(0).sent().len();

    // What `tool_search` does mid-turn: the built-in roster grows, so the
    // merged generation has to be rebuilt around it.
    revealed.reveal("grep");
    let second = hosting.refresh(&context).expect("the generation after");

    assert_ne!(
        first.generation().context_id(),
        second.generation().context_id(),
        "a roster that moved is a new generation"
    );
    assert!(second.find("grep").is_some(), "the revealed tool arrived");
    let after = second
        .find("mcp:docs/search")
        .expect("the server's tool survived the swap");
    assert_eq!(after.descriptor().provenance().id(), source);
    assert_eq!(
        after.tool().sensitivity(&ToolArgs::new("{}")),
        approval,
        "a swap must not change what a call is approved as"
    );
    assert_eq!(
        sandbox.server(0).sent().len(),
        read,
        "and must not go back to the server for a catalogue it already read"
    );
    hosting.dispose(&context).expect("the server stopped");
}

#[test]
fn a_call_interrupted_before_it_is_sent_reaches_no_server() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(frames)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let cancel = Cancel::new();
    cancel.request();
    let refused = entry
        .tool()
        .run(
            allowed(entry.tool(), "mcp:docs/search", r#"{"query":"crates"}"#),
            &crucible_core::ToolContext::new(
                Ancestry::new(),
                ToolId::new("test"),
                &cancel,
                None,
                &Nothing,
            ),
        )
        .expect_err("the run was interrupted");

    assert!(matches!(refused, ToolError::Cancelled(_)));
    assert!(
        !sandbox
            .server(0)
            .sent()
            .iter()
            .any(|frame| frame.get("method").and_then(Value::as_str) == Some("tools/call")),
        "an interrupted call must not start somebody else's program working"
    );
    hosting.dispose(&context).expect("the server stopped");
}

/// Runs one offered tool the way a turn does, under an interrupt of its own.
fn calls(
    tool: &dyn Tool,
    named: &str,
    args: &str,
    cancel: &Cancel,
) -> Result<ToolOutput, ToolError> {
    tool.run(
        allowed(tool, named, args),
        &crucible_core::ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            cancel,
            None,
            &Nothing,
        ),
    )
}

/// A selection whose calls are given a long patience, so that a test about an
/// interrupt is not quietly a test about a timeout.
fn patient(name: &str, request: Duration) -> Chosen {
    Chosen::new(name, PROGRAM, [], policy())
        .given(SandboxEnvironment::new([]).expect("an empty environment"))
        .waiting(PATIENCE, request, GRACE)
        .required(true)
}

#[test]
fn a_call_interrupted_after_the_frame_went_ends_at_the_press_rather_than_at_the_patience() {
    // The server takes the call and thinks about it far longer than anybody
    // waits. Without the interrupt reaching into the wait, the only ending
    // available is the request patience, so the number below is the difference
    // between a press that works and one that is merely recorded.
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Slowly(frames, 2, Duration::from_secs(5))]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![patient("docs", Duration::from_secs(5))],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let cancel = Cancel::new();
    let pressing = cancel.clone();
    let presser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        pressing.request();
    });
    let began = Instant::now();
    let refused = calls(entry.tool(), "mcp:docs/search", "{}", &cancel)
        .expect_err("the wait was interrupted");
    let waited = began.elapsed();
    presser.join().expect("the press happened");

    assert!(
        matches!(&refused, ToolError::Cancelled(tool) if tool.as_ref() == "mcp:docs/search"),
        "an interrupted call is cancelled rather than broken: {refused}"
    );
    assert!(
        waited < Duration::from_secs(2),
        "the press ends the wait, not the patience: waited {waited:?}"
    );
    // The call went, so the server may be doing it. Reading its answer as the
    // reply to some later question is exactly what must not happen.
    let after = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("the server was finished with");
    assert!(
        matches!(&after, ToolError::StaleGeneration { tool } if tool.as_ref() == "mcp:docs/search"),
        "an interrupted server is not asked a second question: {after}"
    );
    assert_eq!(
        sandbox.started(),
        1,
        "and a call whose fate is unknown buys no restart"
    );
    hosting.dispose(&context).expect("nothing left to stop");
}

#[test]
fn a_server_that_died_before_the_frame_went_is_started_again_and_the_call_answered() {
    let mut first = opening("docs", &json!([offers("search")]));
    first.push(produced("never said", false));
    let mut second = opening("docs", &json!([offers("search")]));
    second.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(first), Answers::Says(second)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    // The process goes between reading the catalogue and taking the call, which
    // is the ending a restart is for: the frame never left crucible, so nothing
    // over there has half-happened and sending it once more sends it once.
    sandbox.server(0).departs();
    let output = calls(
        entry.tool(),
        "mcp:docs/search",
        r#"{"query":"crates"}"#,
        &Cancel::new(),
    )
    .expect("the server that replaced it answered");

    assert!(output.text().contains("two pages"));
    assert_eq!(sandbox.started(), 2, "it was started again, once");
    let sent = sandbox.server(1).sent();
    let call = sent.last().expect("crucible said something last");
    assert_eq!(
        call.pointer("/params/name").and_then(Value::as_str),
        Some("search"),
        "and the call the model made is the one that was retried"
    );
    assert_eq!(
        call.pointer("/params/arguments/query")
            .and_then(Value::as_str),
        Some("crates"),
        "with the arguments it was written with"
    );
    hosting.dispose(&context).expect("the replacement stopped");
}

#[test]
fn a_call_whose_frame_ran_out_of_patience_is_never_sent_a_second_time() {
    let mut first = opening("docs", &json!([offers("search")]));
    first.push(produced("never said", false));
    let mut second = opening("docs", &json!([offers("search")]));
    second.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(first), Answers::Says(second)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    // The server stops reading rather than going. Crucible spends its patience
    // and gives up on the write, but by then the bytes are with the thread that
    // owns the pipe: the far end may read that call a moment later, and there
    // is a restart in the budget for a crucible that believed otherwise.
    sandbox.server(0).deafens();
    let refused = calls(
        entry.tool(),
        "mcp:docs/search",
        r#"{"query":"crates"}"#,
        &Cancel::new(),
    )
    .expect_err("a write nobody took is not an answer");

    assert!(
        !matches!(refused, ToolError::Cancelled { .. }),
        "nobody pressed anything; the pipe simply stopped taking bytes: {refused}"
    );
    assert_eq!(
        sandbox.started(),
        1,
        "a call that may already have been read is not one to send again, so \
         the restart in the budget is left unspent"
    );
}

#[test]
fn a_server_selected_with_no_restarts_is_not_started_again_and_the_answer_says_why() {
    let mut frames = opening("docs", &json!([offers("search")]));
    frames.push(produced("never said", false));
    let sandbox = Pretend::new([Answers::Says(frames)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    sandbox.server(0).departs();
    let refused = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("nothing was allowed to replace it");

    assert_eq!(
        sandbox.started(),
        1,
        "a default of none starts nothing again"
    );
    let said = refused.to_string();
    assert!(
        said.contains("will not be asked again"),
        "the model is told the tool is gone rather than left to retry it: {said}"
    );
    hosting.dispose(&context).expect("nothing left to stop");
}

#[test]
fn a_restarted_server_offering_the_tool_under_another_schema_is_refused_and_retired() {
    let mut first = opening("docs", &json!([offers("search")]));
    first.push(produced("never said", false));
    // The same name, a different promise. The arguments the model wrote were
    // checked against the catalogue this run published, and this is not it.
    let moved = json!([{
        "name": "search",
        "inputSchema": { "type": "object", "required": ["corpus"] },
    }]);
    let mut second = opening("docs", &moved);
    second.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(first), Answers::Says(second)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    sandbox.server(0).departs();
    let refused = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("what came back does not describe the tool that was published");

    assert_eq!(sandbox.started(), 2, "it was started again to find out");
    assert!(
        !sandbox
            .server(1)
            .sent()
            .iter()
            .any(|frame| frame.get("method").and_then(Value::as_str) == Some("tools/call")),
        "and was told nothing once it had said so"
    );
    assert_eq!(sandbox.server(1).stops(), 1, "the replacement is stopped");
    assert!(
        refused.to_string().contains("will not be asked again"),
        "and the tool is finished with: {refused}"
    );
    hosting.dispose(&context).expect("nothing left to stop");
}

#[test]
fn a_restarted_server_that_reshaped_a_tool_nobody_called_is_refused_just_the_same() {
    // The call in hand comes back describable and its neighbour does not. The
    // model holds both descriptors and may call the second next, so a restart
    // judged on the first alone would leave the published roster promising a
    // schema the server no longer offers.
    let mut first = opening("docs", &json!([offers("search"), offers("fetch")]));
    first.push(produced("never said", false));
    let moved = json!([
        offers("search"),
        {
            "name": "fetch",
            "inputSchema": { "type": "object", "required": ["url"] },
        },
    ]);
    let mut second = opening("docs", &moved);
    second.push(produced("two pages", false));
    let sandbox = Pretend::new([Answers::Says(first), Answers::Says(second)]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    sandbox.server(0).departs();
    let refused = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("what came back does not describe everything that was published");

    assert_eq!(sandbox.started(), 2, "it was started again to find out");
    assert_eq!(sandbox.server(1).stops(), 1, "the replacement is stopped");
    assert!(
        refused.to_string().contains("fetch"),
        "and the refusal names the tool that moved: {refused}"
    );
    hosting.dispose(&context).expect("nothing left to stop");
}

#[test]
fn a_call_still_outstanding_when_the_server_went_quiet_is_never_repeated() {
    // A budget with plenty left, spent on nothing: the frame went, so what the
    // far end did with it cannot be seen from here, and a second copy of a call
    // that may already have run is not a recovery.
    let sandbox = Pretend::new([
        Answers::Says(opening("docs", &json!([offers("search")]))),
        Answers::Says(opening("docs", &json!([offers("search")]))),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(3)],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let refused = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("the server took the call and said nothing");

    assert_eq!(
        sandbox.started(),
        1,
        "a ceiling is not permission to repeat a call whose effect is unknown"
    );
    assert!(
        refused.to_string().contains("will not be asked again"),
        "and the server is finished with rather than asked again: {refused}"
    );
    let after = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("there is nothing left to ask");
    assert!(
        matches!(&after, ToolError::StaleGeneration { tool } if tool.as_ref() == "mcp:docs/search"),
        "the conversation ended with the call it lost: {after}"
    );
    hosting.dispose(&context).expect("nothing left to stop");
}

#[test]
fn a_ceiling_of_one_restart_is_spent_once_and_the_next_ending_is_the_last() {
    // Three deaths of the kind a restart is allowed for, against a document
    // that permitted one. A budget that were merely a flag would answer the
    // second the way it answered the first, and a server that cannot stay up
    // would be started again for every call the run ever makes.
    let scripts: Vec<Answers> = (0..3)
        .map(|_| {
            let mut frames = opening("docs", &json!([offers("search")]));
            frames.push(produced("two pages", false));
            Answers::Says(frames)
        })
        .collect();
    let sandbox = Pretend::new(scripts);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
    );
    let context = lifecycle();
    hosting.prepare(&context).expect("the server started");
    let snapshot = hosting.snapshot(&context).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    sandbox.server(0).departs();
    calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect("the one restart the document allowed");
    assert_eq!(sandbox.started(), 2);

    sandbox.server(1).departs();
    let refused = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("the budget is spent");

    assert_eq!(
        sandbox.started(),
        2,
        "a ceiling of one permits one restart, not one per call"
    );
    let said = refused.to_string();
    assert!(
        said.contains("will not be asked again"),
        "and the run is told the tool is finished with: {said}"
    );
    hosting.dispose(&context).expect("nothing left to stop");
}
