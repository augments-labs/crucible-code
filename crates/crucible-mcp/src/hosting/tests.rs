//! What hosting selected MCP servers beside the built-in roster has to
//! guarantee.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crucible_runtime::{BoxFuture, Cancel};
use crucible_sandbox::{
    SandboxAudit, SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance,
    SandboxCapabilities, SandboxCleanup, SandboxCommand, SandboxEnvironment, SandboxError,
    SandboxFactKind, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxInspection, SandboxLaunch, SandboxLifecycle, SandboxManifest, SandboxNetworkPolicy,
    SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead, SandboxRequest,
    SandboxResourceLimits, SandboxService, SandboxSession, SandboxUsage, SandboxViolation,
};
use crucible_tools::{
    Approved, Ask, Mode, Permission, Remember, Rules, Sensitivity, Settled, Summary, Target, Tool,
    ToolContext, ToolDescriptor, ToolEntry, ToolError, ToolOutput, ToolProvenance, ToolSnapshot,
    ToolSourceKind, Toolset, ToolsetContext, ToolsetError, Verdict, Watch, Wrote,
};
use crucible_types::{Ancestry, SandboxId, ToolArgs, ToolCall, ToolId};
use serde_json::{Value, json};

use super::{Chosen, Hosting};

mod audit;
mod cleanup;
mod given_up;
mod startup;
mod withheld;

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
    audit: Mutex<Option<(SandboxId, SandboxAudit)>>,
    /// A backend that cannot confirm the process scope ended.
    cleanup_refused: AtomicBool,
    /// A process whose ending, once it came, went wrong: what it wrote was
    /// refused.
    ending_refused: AtomicBool,
    stop_attempts: AtomicUsize,
    missing_input: bool,
    missing_output: bool,
    /// A process that does not finish when its input closes, and what stopping
    /// it does.
    unfinished: Option<Stop>,
    /// The ending published while a stop was joining it.
    published: AtomicBool,
    /// A stop that failed after an ending completed.
    stop_fails: AtomicBool,
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
    /// handed to the task that owns the pipe before that wait began, so the
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
                    .record(id, SandboxFactKind::Cleanup(SandboxCleanup::Complete))
                    .unwrap();
            }
        }
    }
}

impl SandboxProcess for Fake {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        (!self.watched.missing_input)
            .then(|| Box::new(Input(Arc::clone(&self.watched))) as Box<dyn Write + Send>)
    }

    fn take_stdout(&mut self) -> Option<Box<dyn SandboxOutput>> {
        if self.watched.missing_output {
            return None;
        }
        self.stdout
            .take()
            .map(|says| Box::new(says) as Box<dyn SandboxOutput>)
    }

    fn take_stderr(&mut self) -> Option<Box<dyn SandboxOutput>> {
        None
    }

    fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if self.watched.unfinished.is_some() {
            return Ok(None);
        }
        if self.watched.cleanup_refused.load(Ordering::Relaxed) {
            return Err(io::Error::other("injected cleanup observation failure"));
        }
        // Running until it is told otherwise, which is what a server does. A
        // process that reported an exit before anything asked it to finish
        // would let a disposal that stopped nothing look like one that worked.
        if !self.watched.closed.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if self.watched.ending_refused.load(Ordering::Relaxed) {
            return Err(io::Error::other(
                "writable root changed after the command started",
            ));
        }
        self.end();
        Ok(Some(exited()))
    }

    fn ended(&mut self) -> bool {
        match self.watched.unfinished {
            // It went when its input closed, and its ending never completes.
            Some(Stop::UnansweredAfterEnding | Stop::PublishedAfterEnding) => {
                self.watched.closed.load(Ordering::Relaxed)
            }
            Some(Stop::Unanswered | Stop::Fails) => false,
            // A process whose status cannot be read has not been seen to end.
            None => {
                !self.watched.cleanup_refused.load(Ordering::Relaxed)
                    && self.watched.closed.load(Ordering::Relaxed)
            }
        }
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            self.watched.stop_attempts.fetch_add(1, Ordering::Relaxed);
            match self.watched.unfinished {
                Some(Stop::Unanswered | Stop::UnansweredAfterEnding) => {
                    std::future::pending::<()>().await;
                }
                Some(Stop::PublishedAfterEnding) => {
                    if self.watched.closed.load(Ordering::Relaxed) {
                        self.watched.published.store(true, Ordering::Release);
                    }
                    if self.watched.stop_fails.load(Ordering::Relaxed) {
                        return Err(io::Error::other("the scope could not be reaped"));
                    }
                }
                Some(Stop::Fails) => {
                    // Words that say what went wrong and not that cleanup is
                    // unconfirmed, as a backend's failure need not say it.
                    return Err(io::Error::other("the scope could not be reaped"));
                }
                None => {}
            }
            if self.watched.cleanup_refused.load(Ordering::Relaxed) {
                return Err(io::Error::other("injected unconfirmed cleanup"));
            }
            self.end();
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

    fn publication_outcome(&self) -> Option<SandboxLifecycle> {
        self.watched
            .published
            .load(Ordering::Acquire)
            .then_some(SandboxLifecycle::Published)
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

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
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
            }) as Box<dyn SandboxProcess>)
        })
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

    fn materialize(&mut self) -> BoxFuture<'_, Result<(), SandboxError>> {
        Box::pin(async move { Ok(()) })
    }

    fn stage<'a>(
        self: Box<Self>,
        command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let mut image = vec![OsString::from(command.program())];
            image.extend(command.arguments().iter().cloned());
            self.staged.lock().expect("what was staged").push(image);
            Ok(Box::new(Held {
                frames: self.frames,
                slow: self.slow,
                watched: self.watched,
                inspection: self.inspection,
            }) as Box<dyn SandboxLaunch>)
        })
    }
}

/// What one selected server does when crucible tries to start it.
enum Answers {
    /// It starts, and says these frames in order.
    Says(Vec<Value>),
    /// It talks normally but its process scope cannot be confirmed stopped.
    Unreapable(Vec<Value>),
    /// It talks normally, goes when its input closes, and what it wrote is
    /// refused.
    Refused(Vec<Value>),
    /// Construction lacks a pipe and stopping the process also fails.
    MissingInput,
    MissingOutput,
    /// The same, but it thinks for a while before the `n`th of them.
    Slowly(Vec<Value>, usize, Duration),
    /// It refuses to start at all.
    Refuses,
    /// Preparing its sandbox never answers.
    Hangs,
    /// It talks normally, does not finish when its input closes, and stopping
    /// it does what the second says.
    Unfinished(Vec<Value>, Stop),
}

/// What stopping a server that did not finish when its input closed does.
#[derive(Debug, Clone, Copy)]
enum Stop {
    /// It keeps running, and a stop never answers, so an awaited caller gives
    /// up on it at the bound on a stop that does not answer.
    Unanswered,
    /// It went, its ending never completes, and a stop never answers: the wait
    /// for what it wrote runs to its ceiling before the stop is tried.
    UnansweredAfterEnding,
    /// It went, and the stop joins a publication that completes there.
    PublishedAfterEnding,
    /// It keeps running, and a stop fails, in words that say only what went
    /// wrong.
    Fails,
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
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async move {
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
        })
    }

    fn prepare(
        &self,
        _request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let script = self
                .scripts
                .lock()
                .expect("the scripts a test wrote")
                .pop_front();
            let missing_input = matches!(script, Some(Answers::MissingInput));
            let missing_output = matches!(script, Some(Answers::MissingOutput));
            let ending_refused = matches!(script, Some(Answers::Refused(_)));
            let unfinished = match &script {
                Some(Answers::Unfinished(_, stop)) => Some(*stop),
                _ => None,
            };
            let publication_stop_failed = matches!(unfinished, Some(Stop::PublishedAfterEnding));
            let (frames, slow, cleanup_refused) = match script {
                Some(Answers::Hangs) => return std::future::pending().await,
                None | Some(Answers::Refuses) => {
                    return Err(SandboxError::Lifecycle(io::Error::other(
                        "this machine has no such program",
                    )));
                }
                Some(
                    Answers::Says(frames)
                    | Answers::Refused(frames)
                    | Answers::Unfinished(frames, _),
                ) => (frames, None, false),
                Some(Answers::Unreapable(frames)) => (frames, None, true),
                Some(Answers::MissingInput | Answers::MissingOutput) => (Vec::new(), None, true),
                Some(Answers::Slowly(frames, nth, held)) => (frames, Some((nth, held)), false),
            };

            let watched = Arc::new(Watched {
                cleanup_refused: AtomicBool::new(cleanup_refused),
                ending_refused: AtomicBool::new(ending_refused),
                stop_fails: AtomicBool::new(publication_stop_failed),
                missing_input,
                missing_output,
                unfinished,
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
            }) as Box<dyn SandboxSession>)
        })
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
            target: Target::unresolved(),
        }
    }

    fn summary(&self, _args: &ToolArgs) -> Summary {
        Summary::new(self.0)
    }

    fn run<'a>(
        &'a self,
        _approved: Approved,
        _context: &'a ToolContext<'_>,
    ) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move { Ok(ToolOutput::ok("nothing")) })
    }
}

/// A built-in roster, as this composition is allowed to see one.
///
/// What a run actually hands over is the roster the binary compiled in, which
/// belongs to whatever drives turns. All this composition ever asks of it is
/// the trait, so what a test hands it is the trait — with the one thing the
/// merge has to survive kept: a roster that moves between one pass and the
/// next, the way revealing a held-back tool moves the real one mid-turn.
#[derive(Debug, Default)]
struct Roster {
    /// The entries it offers, and what they were last materialized as.
    ///
    /// Cached rather than rebuilt, because the real roster caches: a fresh
    /// generation on every pass would mint an identity that a call admitted in
    /// the pass before could not resolve through.
    held: Mutex<(Vec<ToolEntry>, Option<ToolSnapshot>)>,
}

impl Roster {
    /// A roster offering these names and nothing else.
    fn of(names: &[&'static str]) -> Arc<Self> {
        let roster = Arc::new(Self::default());
        for name in names {
            roster.offer(name);
        }
        roster
    }

    /// Offers one more name, which is what revealing a held-back tool does.
    fn offer(&self, name: &'static str) {
        let descriptor = ToolDescriptor::new(
            name,
            r#"{"type":"object"}"#,
            ToolProvenance::builtin(name).expect("a built-in name fits its own identity"),
        )
        .expect("a descriptor a test wrote");
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        held.0
            .push(ToolEntry::new(descriptor, Arc::new(Quiet(name))));
        held.1 = None;
    }

    /// The generation it publishes, which moves only when its names do.
    fn generation(&self) -> Result<ToolSnapshot, ToolsetError> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(snapshot) = held.1.as_ref() {
            return Ok(snapshot.clone());
        }
        let snapshot = ToolSnapshot::new(held.0.iter().cloned())?;
        held.1 = Some(snapshot.clone());
        Ok(snapshot)
    }
}

impl Toolset for Roster {
    fn prepare<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move { Ok(()) })
    }

    fn snapshot<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(async move { self.generation() })
    }

    fn refresh<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<ToolSnapshot, ToolsetError>> {
        Box::pin(async move { self.generation() })
    }

    fn dispose<'a>(
        &'a self,
        _context: &'a ToolsetContext,
    ) -> BoxFuture<'a, Result<(), ToolsetError>> {
        Box::pin(async move { Ok(()) })
    }

    fn registered(&self, name: &str) -> Option<ToolEntry> {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .0
            .iter()
            .find(|entry| entry.descriptor().name() == name)
            .cloned()
    }
}

/// Awaits `work` on the tests' runtime until it answers.
///
/// Every lifecycle step and every call here is awaited the way a turn awaits
/// it: a server's greeting, catalogue and calls wait on its streams, so a step
/// that is asked once and dropped would be a test of something no turn does.
fn awaited<F: Future>(work: F) -> F::Output {
    crate::testing::runtime().block_on(work)
}

/// A built-in roster offering these names and nothing else.
fn builtin(names: &[&'static str]) -> Arc<dyn Toolset> {
    Roster::of(names)
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
        fn ask<'a>(
            &'a mut self,
            _call: &'a ToolCall,
            _sensitivity: &'a Sensitivity,
        ) -> crucible_runtime::BoxFuture<'a, (Verdict, Remember)> {
            Box::pin(async { (Verdict::Allow, Remember::Never) })
        }
    }

    let call = ToolCall {
        id: ToolId::new("test"),
        name: name.into(),
        args: ToolArgs::new(args),
    };
    let sensitivity = tool.sensitivity(&call.args);
    match awaited(Permission::with(Mode::default(), Rules::new()).decide(
        &call,
        &sensitivity,
        &mut Yes,
    )) {
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
        crate::testing::runtime(),
    );
    let context = lifecycle();

    awaited(hosting.prepare(&context)).expect("nothing to prepare");
    let snapshot = awaited(hosting.snapshot(&context)).expect("the built-in roster");

    assert_eq!(sandbox.started(), 0, "no process may be started");
    assert_eq!(snapshot.entries().len(), 1);
    assert!(snapshot.find("read").is_some());
    awaited(hosting.dispose(&context)).expect("nothing to dispose");
}

#[test]
fn a_selected_server_is_started_and_what_it_offered_is_named_under_it() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        builtin(&["read"]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
        crate::testing::runtime(),
    );
    let context = lifecycle();

    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");

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
    awaited(hosting.dispose(&context)).expect("the server stopped");
}

#[test]
fn a_server_naming_a_tool_the_builtin_roster_owns_takes_nothing_over() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("read")])))]);
    let hosting = Hosting::new(
        builtin(&["read"]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
        crate::testing::runtime(),
    );
    let context = lifecycle();

    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");

    assert!(
        snapshot.find("read").is_some(),
        "the built-in keeps its name"
    );
    assert!(
        snapshot.find("mcp:docs/read").is_some(),
        "and the server is still offered"
    );
    awaited(hosting.dispose(&context)).expect("the server stopped");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();

    let refused =
        awaited(hosting.prepare(&context)).expect_err("two names one rule cannot tell apart");

    assert!(
        refused.to_string().contains("Search"),
        "the refusal names the pair: {refused}"
    );
    awaited(hosting.dispose(&context)).expect("nothing to stop");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let output = awaited(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", r#"{"query":"crates"}"#),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &Cancel::new(),
            None,
            &Nothing,
        ),
    ))
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
    awaited(hosting.dispose(&context)).expect("the server stopped");
}

#[test]
fn a_server_that_will_not_start_fails_the_turn_and_names_which_one() {
    let sandbox = Pretend::new([Answers::Refuses]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
        crate::testing::runtime(),
    );
    let context = lifecycle();

    let refused = awaited(hosting.prepare(&context)).expect_err("nothing started");

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
        crate::testing::runtime(),
    );
    let context = lifecycle();

    awaited(hosting.prepare(&context))
        .expect("the greeting was prompt and the catalogue is a request");

    let named: Vec<_> = awaited(hosting.snapshot(&context))
        .expect("one generation")
        .entries()
        .iter()
        .map(|entry| entry.descriptor().name().to_owned())
        .collect();
    assert_eq!(named, vec!["mcp:docs/search".to_owned()]);
    awaited(hosting.dispose(&context)).expect("it stops");
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
        crate::testing::runtime(),
    );

    let refused = awaited(hosting.prepare(&lifecycle())).expect_err("it never agreed a version");

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
        crate::testing::runtime(),
    );
    let context = lifecycle();

    awaited(hosting.prepare(&context)).expect("a server nobody required cannot fail the turn");

    let named: Vec<_> = awaited(hosting.snapshot(&context))
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
    awaited(hosting.dispose(&context)).expect("and stops the one that ran");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();

    awaited(hosting.prepare(&context))
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");

    let first = awaited(hosting.snapshot(&context)).expect("one generation");
    let after = sandbox.server(0).sent().len();
    let second = awaited(hosting.refresh(&context)).expect("the same generation");

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
    awaited(hosting.dispose(&context)).expect("the server stopped");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("both started");

    awaited(hosting.dispose(&context)).expect("both stopped");

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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");

    awaited(hosting.dispose(&context)).expect("the server stopped");
    awaited(hosting.dispose(&context)).expect("and stays stopped");

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
        crate::testing::runtime(),
    );
    let context = lifecycle();

    // The runner prepares and disposes once per turn over one toolset, so the
    // second turn of an ordinary conversation is this exact path. A disposal
    // that left its dead servers behind would make it a turn with no tools
    // whose handles all refuse.
    awaited(hosting.prepare(&context)).expect("the first turn started it");
    awaited(hosting.dispose(&context)).expect("and stopped it");
    awaited(hosting.prepare(&context)).expect("the second turn started it again");

    assert_eq!(sandbox.started(), 2, "each turn hosts a server of its own");
    let named: Vec<_> = awaited(hosting.snapshot(&context))
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

    awaited(hosting.dispose(&context)).expect("and it stops too");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let held = snapshot
        .find("mcp:docs/search")
        .expect("the offered tool")
        .shared_tool();

    awaited(hosting.dispose(&context)).expect("the server stopped");
    let refused = awaited(held.run(
        allowed(held.as_ref(), "mcp:docs/search", "{}"),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &Cancel::new(),
            None,
            &Nothing,
        ),
    ))
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let output = awaited(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", "{}"),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &Cancel::new(),
            None,
            &Nothing,
        ),
    ))
    .expect("a failed tool is still an answer");

    assert!(
        output.is_failed(),
        "what the server said failed is a failed result"
    );
    assert!(output.text().contains("no such index"));
    awaited(hosting.dispose(&context)).expect("the server stopped");
}

#[test]
fn arguments_that_are_not_an_object_are_refused_before_anything_is_sent() {
    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        builtin(&[]),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("the server stopped");
}

/// A watcher that keeps nothing, for the calls whose progress no test reads.
struct Nothing;

impl Watch for Nothing {
    fn wrote(&self, _text: Wrote) {}
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("both started");

    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("both stopped");
}

#[test]
fn a_generation_rebuilt_under_a_moved_roster_keeps_every_name_source_and_approval() {
    let roster = Roster::of(&["read"]);

    let sandbox = Pretend::new([Answers::Says(opening("docs", &json!([offers("search")])))]);
    let hosting = Hosting::new(
        Arc::clone(&roster) as Arc<dyn Toolset>,
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");

    let first = awaited(hosting.snapshot(&context)).expect("one generation");
    let before = first.find("mcp:docs/search").expect("the server's tool");
    let source = before.descriptor().provenance().id().to_owned();
    let approval = before.tool().sensitivity(&ToolArgs::new("{}"));
    let read = sandbox.server(0).sent().len();

    // What `tool_search` does mid-turn: the built-in roster grows, so the
    // merged generation has to be rebuilt around it.
    roster.offer("grep");
    let second = awaited(hosting.refresh(&context)).expect("the generation after");

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
    awaited(hosting.dispose(&context)).expect("the server stopped");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    let cancel = Cancel::new();
    cancel.request();
    let refused = awaited(entry.tool().run(
        allowed(entry.tool(), "mcp:docs/search", r#"{"query":"crates"}"#),
        &ToolContext::new(
            Ancestry::new(),
            ToolId::new("test"),
            &cancel,
            None,
            &Nothing,
        ),
    ))
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
    awaited(hosting.dispose(&context)).expect("the server stopped");
}

/// Runs one offered tool the way a turn does, under an interrupt of its own.
fn calls(
    tool: &dyn Tool,
    named: &str,
    args: &str,
    cancel: &Cancel,
) -> Result<ToolOutput, ToolError> {
    awaited(tool.run(
        allowed(tool, named, args),
        &ToolContext::new(Ancestry::new(), ToolId::new("test"), cancel, None, &Nothing),
    ))
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("nothing left to stop");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("the replacement stopped");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
    let entry = snapshot.find("mcp:docs/search").expect("the offered tool");

    // The server stops reading rather than going. Crucible spends its patience
    // and gives up on the write, but by then the bytes are with the task that
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("nothing left to stop");
}

#[test]
fn a_refused_restart_is_said_about_the_server_it_was_refused_to() {
    // Both refusals the budget can give, read as the model and the person
    // reading over its shoulder are shown them. The budget is shared with
    // everything else crucible hosts, and a sentence about some other kind of
    // program sends whoever reads it looking for something they never set up.
    let spent = {
        let mut frames = opening("docs", &json!([offers("search")]));
        frames.push(produced("never said", false));
        let sandbox = Pretend::new([Answers::Says(frames)]);
        (sandbox, chosen("docs"), true)
    };
    let unsettled = {
        let frames = opening("docs", &json!([offers("search")]));
        let sandbox = Pretend::new([Answers::Says(frames)]);
        (sandbox, chosen("docs").restarting(3), false)
    };

    for (sandbox, chosen, departs) in [spent, unsettled] {
        let hosting = Hosting::new(
            builtin(&[]),
            Arc::clone(&sandbox) as Arc<dyn SandboxService>,
            vec![chosen],
            crate::testing::runtime(),
        );
        let context = lifecycle();
        awaited(hosting.prepare(&context)).expect("the server started");
        let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
        let entry = snapshot.find("mcp:docs/search").expect("the offered tool");
        if departs {
            sandbox.server(0).departs();
        }

        let said = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
            .expect_err("no restart was permitted")
            .to_string();

        assert!(
            said.contains("will not be asked again: the server "),
            "the refusal is not about the server: {said}"
        );
        assert!(
            !said.contains("extension"),
            "an MCP server was called an extension: {said}"
        );
        awaited(hosting.dispose(&context)).expect("nothing left to stop");
    }
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("nothing left to stop");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("nothing left to stop");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("nothing left to stop");
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
        crate::testing::runtime(),
    );
    let context = lifecycle();
    awaited(hosting.prepare(&context)).expect("the server started");
    let snapshot = awaited(hosting.snapshot(&context)).expect("one generation");
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
    awaited(hosting.dispose(&context)).expect("nothing left to stop");
}

#[test]
fn a_selection_never_shows_the_arguments_its_server_is_started_with() {
    // An argument is where a server is handed a token or a connection string,
    // so a selection written into a log line says how many there are and not
    // what they say.
    let canary = "--token=canary-7f3a";
    let chosen = Chosen::new(
        "docs",
        PROGRAM,
        [std::ffi::OsString::from(canary), "--quiet".into()],
        policy(),
    );

    let shown = format!("{chosen:?}");

    assert!(!shown.contains("canary"), "{shown}");
    assert!(!shown.contains("--quiet"), "{shown}");
    assert!(shown.contains("docs"), "{shown}");
    assert!(shown.contains("arguments: 2"), "{shown}");
}
