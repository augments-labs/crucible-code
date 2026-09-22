//! What the built-in roster and a hosted MCP server have to guarantee together.
//!
//! Composition is the only place the two meet: the roster a turn drives belongs
//! to the runner, the hosting that puts a server's tools beside it belongs to
//! the protocol crate, and neither may name the other. So the agreement between
//! them — that a roster which moved mid-turn is a new generation, and that the
//! merged generation is rebuilt around it without going back to the server —
//! can only be held from here, where both are already in reach.
//!
//! The server is scripted rather than started. What is under test is the seam,
//! and a real program would put this machine's process table between the two
//! things being checked.

// Test-only helpers fail the owning case when its controlled fixture is invalid.
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_core::{
    Ancestry, Approved, Cancel, Revealed, SandboxBackendId, SandboxBackendIdentity,
    SandboxBackendProvenance, SandboxCapabilities, SandboxCommand, SandboxEnvironment,
    SandboxError, SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule,
    SandboxId, SandboxInspection, SandboxLaunch, SandboxManifest, SandboxNetworkPolicy,
    SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead, SandboxRequest,
    SandboxResourceLimits, SandboxService, SandboxSession, SandboxUsage, SandboxViolation,
    Sensitivity, Summary, Target, Tool, ToolArgs, ToolContext, ToolDescriptor, ToolError, ToolId,
    ToolOutput, ToolProvenance, Toolset, ToolsetContext,
};
use crucible_mcp::{Chosen, Hosting};
use crucible_runner::Tools;
use crucible_runtime::BoxFuture;
use serde_json::{Value, json};

/// How long a test lets one silence run before it gives up on a server.
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

/// The policy the scripted server is started under.
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

/// The handshake and the catalogue one server answers with.
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

/// Everything this test wants to know about the server afterwards.
#[derive(Default)]
struct Watched {
    /// What crucible said to it, so a test can count the frames it cost.
    said: Mutex<Vec<u8>>,
    /// Whether crucible has let go of its input, which ends the conversation.
    closed: AtomicBool,
}

impl Watched {
    /// How many frames crucible has sent it.
    fn frames(&self) -> usize {
        let said = self.said.lock().expect("what crucible said");
        String::from_utf8_lossy(&said)
            .lines()
            .filter(|line| !line.is_empty())
            .count()
    }
}

/// The writing end of the server's input, as a test can read it back.
struct Input(Arc<Watched>);

impl Drop for Input {
    /// Letting go of it is what tells the server to finish.
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Relaxed);
    }
}

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

/// A scripted stream that goes quiet forever once its script runs out.
struct Says(VecDeque<String>);

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        let Some(frame) = self.0.pop_front() else {
            return Ok(SandboxRead::Pending);
        };
        let said = format!("{frame}\n");
        let bytes = said.as_bytes();
        let taken = bytes.len().min(buffer.len());
        if let Some((into, from)) = buffer.get_mut(..taken).zip(bytes.get(..taken)) {
            into.copy_from_slice(from);
        }
        Ok(SandboxRead::Bytes(taken))
    }
}

/// The server the sandbox pretends to have started.
struct Fake {
    stdout: Option<Says>,
    watched: Arc<Watched>,
    inspection: SandboxInspection,
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
        // Running until it is told otherwise, which is what a server does.
        if self.watched.closed.load(Ordering::Relaxed) {
            return Ok(Some(exited()));
        }
        Ok(None)
    }

    fn ended(&mut self) -> bool {
        self.watched.closed.load(Ordering::Relaxed)
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            self.watched.closed.store(true, Ordering::Relaxed);
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

/// A staged command that has not been let go yet.
struct Held {
    frames: Vec<String>,
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
                stdout: Some(Says(self.frames.into_iter().collect())),
                watched: self.watched,
                inspection: self.inspection,
            }) as Box<dyn SandboxProcess>)
        })
    }
}

/// A prepared session, which here only remembers what its server will say.
struct Prepared {
    frames: Vec<String>,
    watched: Arc<Watched>,
    inspection: SandboxInspection,
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
        _command: SandboxCommand,
    ) -> BoxFuture<'a, Result<Box<dyn SandboxLaunch>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            Ok(Box::new(Held {
                frames: self.frames,
                watched: self.watched,
                inspection: self.inspection,
            }) as Box<dyn SandboxLaunch>)
        })
    }
}

/// A sandbox that starts the one server this test scripted, and nothing else.
struct Pretend {
    says: Mutex<Option<Vec<Value>>>,
    watched: Arc<Watched>,
}

impl Pretend {
    /// A sandbox whose one server answers with `frames`.
    fn saying(frames: Vec<Value>) -> Arc<Self> {
        Arc::new(Self {
            says: Mutex::new(Some(frames)),
            watched: Arc::new(Watched::default()),
        })
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
            let said = self
                .says
                .lock()
                .expect("the script this test wrote")
                .take()
                .ok_or_else(|| {
                    SandboxError::Lifecycle(io::Error::other("only one server was scripted"))
                })?;
            Ok(Box::new(Prepared {
                frames: said.iter().map(ToString::to_string).collect(),
                watched: Arc::clone(&self.watched),
                inspection: inspection(),
            }) as Box<dyn SandboxSession>)
        })
    }
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

/// The context a lifecycle call is made under.
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

/// The roster the binary compiles in, with `grep` held back the way a deferred
/// tool is until something in the turn asks for it.
fn roster(revealed: &Revealed) -> Tools {
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
    tools
}

/// Revealing a held-back built-in mid-turn moves the roster, and the merged
/// generation has to be rebuilt around it without asking the server again.
///
/// This is the one agreement neither crate can hold alone. `Hosting` keys its
/// merged generation on the built-in generation's own identity, and the thing
/// that makes the key move is the runner minting a new generation when a
/// deferred tool is revealed. A double can only mimic that; the real roster is
/// what decides it.
#[test]
fn a_revealed_builtin_moves_the_generation_the_hosted_server_was_merged_into() {
    let revealed = Revealed::new();
    let sandbox = Pretend::saying(opening("docs", &json!([offers("search")])));
    let hosting = Hosting::new(
        Arc::new(roster(&revealed)),
        Arc::clone(&sandbox) as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let context = lifecycle();
    crucible_runtime::answered!(hosting.prepare(&context)).expect("the server started");

    let first = crucible_runtime::answered!(hosting.snapshot(&context)).expect("one generation");
    let before = first.find("mcp:docs/search").expect("the server's tool");
    let source = before.descriptor().provenance().id().to_owned();
    let approval = before.tool().sensitivity(&ToolArgs::new("{}"));
    let read = sandbox.watched.frames();

    // What `tool_search` does mid-turn: the built-in roster grows, so the
    // merged generation has to be rebuilt around it.
    revealed.reveal("grep");
    let second =
        crucible_runtime::answered!(hosting.refresh(&context)).expect("the generation after");

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
        sandbox.watched.frames(),
        read,
        "and must not go back to the server for a catalogue it already read"
    );
    crucible_runtime::answered!(hosting.dispose(&context)).expect("the server stopped");
}
