//! A pretend sandbox whose one program is an MCP server that answers.
//!
//! The server is played here rather than started, as the hosting seam's is:
//! what the trial asks of it is that it be launched once a turn, asked for one
//! tool call, and let go, and a real program would put this machine's process
//! table between the trial and what it is checking.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crucible_mcp::Chosen;
use crucible_runtime::BoxFuture;
use crucible_sandbox::{
    SandboxBackendId, SandboxBackendIdentity, SandboxBackendProvenance, SandboxCapabilities,
    SandboxCommand, SandboxEnvironment, SandboxError, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxInspection, SandboxLaunch,
    SandboxManifest, SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess,
    SandboxRead, SandboxRequest, SandboxResourceLimits, SandboxService, SandboxSession,
    SandboxUsage, SandboxViolation, unconfined_inspection,
};
use crucible_types::{Ancestry, SandboxId, ToolId};
use serde_json::{Value, json};

/// The one tool the server offers, as it knows it.
pub(crate) const TOOL: &str = "search";

/// What the server answers every call to it with.
pub(crate) const FOUND: &str = "found the page";

/// How long the hosting lets one silence run before it gives up on the server.
const PATIENCE: Duration = Duration::from_secs(5);

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

/// An exit status to report once the server has gone.
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

/// The policy the server is started under.
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

fn backend() -> SandboxBackendIdentity {
    SandboxBackendIdentity::new(
        SandboxBackendId::new("trial").expect("a backend name"),
        "1",
        SandboxBackendProvenance::Compatibility,
        None,
    )
    .expect("a backend identity")
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
    unconfined_inspection(
        backend(),
        SandboxCapabilities::none(),
        &request,
        "a trial, which confines nothing",
    )
    .expect("an inspection of a request the trial built")
}

/// The server `name`, as a run selects it.
pub(crate) fn chosen(name: &str) -> Chosen {
    Chosen::new(name, PROGRAM, [], policy())
        .given(SandboxEnvironment::new([]).expect("an empty environment"))
        .waiting(PATIENCE, PATIENCE, GRACE)
        .required(true)
}

/// One launch of the server: what it has to say, and whether it has gone.
#[derive(Default)]
struct Life {
    /// Answers written but not yet read.
    replies: Mutex<VecDeque<Vec<u8>>>,
    /// Whether crucible has let go of its input.
    closed: AtomicBool,
    /// Whether the process has ended.
    ended: AtomicBool,
}

/// What every launch of the server shares, and what the trial reads back.
#[derive(Default)]
pub(crate) struct Ledger {
    /// How many `tools/call` requests reached any launch.
    calls: AtomicUsize,
    /// Every launch so far.
    lives: Mutex<Vec<Arc<Life>>>,
}

impl Ledger {
    /// How many calls reached the server, across every launch.
    pub(crate) fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// How many times it was launched.
    pub(crate) fn launches(&self) -> usize {
        self.lives.lock().expect("the launches").len()
    }

    /// Whether crucible let go of every launch's input.
    pub(crate) fn all_closed(&self) -> bool {
        self.lives
            .lock()
            .expect("the launches")
            .iter()
            .all(|life| life.closed.load(Ordering::SeqCst))
    }

    /// Whether every launch was let go and has ended.
    pub(crate) fn all_ended(&self) -> bool {
        self.lives
            .lock()
            .expect("the launches")
            .iter()
            .all(|life| life.closed.load(Ordering::SeqCst) && life.ended.load(Ordering::SeqCst))
    }
}

/// What the server answers `request` with, if it answers at all.
fn answering(request: &Value, ledger: &Ledger) -> Option<Value> {
    let id = request.get("id")?;
    let method = request.get("method").and_then(Value::as_str)?;
    let result = match method {
        "initialize" => json!({
            "protocolVersion": request
                .pointer("/params/protocolVersion")
                .cloned()
                .unwrap_or_else(|| json!("2025-06-18")),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "docs", "version": "1" },
        }),
        "tools/list" => json!({
            "tools": [{ "name": TOOL, "inputSchema": { "type": "object" } }],
        }),
        "tools/call" => {
            ledger.calls.fetch_add(1, Ordering::SeqCst);
            json!({ "content": [{ "type": "text", "text": FOUND }], "isError": false })
        }
        other => {
            return Some(json!({
                "jsonrpc": "2.0", "id": id,
                "error": { "code": -32601, "message": format!("no {other} here") },
            }));
        }
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

/// The writing end of the server's input: each whole line is a request the
/// server answers at once.
struct Input {
    life: Arc<Life>,
    ledger: Arc<Ledger>,
    line: Vec<u8>,
}

impl Drop for Input {
    /// Letting go of it is what tells the server to finish.
    fn drop(&mut self) {
        self.life.closed.store(true, Ordering::SeqCst);
    }
}

impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for &byte in bytes {
            if byte != b'\n' {
                self.line.push(byte);
                continue;
            }
            let line = std::mem::take(&mut self.line);
            let request: Value = serde_json::from_slice(&line).map_err(io::Error::other)?;
            if let Some(reply) = answering(&request, &self.ledger) {
                let mut frame = reply.to_string().into_bytes();
                frame.push(b'\n');
                self.life
                    .replies
                    .lock()
                    .expect("the replies")
                    .push_back(frame);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// The server's output: whatever it has answered, then its end once it has
/// gone.
struct Says {
    life: Arc<Life>,
    rest: Vec<u8>,
}

impl SandboxOutput for Says {
    fn read_ready(&mut self, buffer: &mut [u8]) -> io::Result<SandboxRead> {
        if self.rest.is_empty() {
            match self.life.replies.lock().expect("the replies").pop_front() {
                Some(frame) => self.rest = frame,
                None if self.life.ended.load(Ordering::SeqCst) => return Ok(SandboxRead::End),
                None => return Ok(SandboxRead::Pending),
            }
        }
        let taken = self.rest.len().min(buffer.len());
        if let Some(into) = buffer.get_mut(..taken) {
            into.copy_from_slice(self.rest.get(..taken).unwrap_or_default());
        }
        self.rest.drain(..taken);
        Ok(SandboxRead::Bytes(taken))
    }
}

/// The server's process.
struct Process {
    life: Arc<Life>,
    ledger: Arc<Ledger>,
    stdout: Option<Says>,
    stubborn: bool,
    inspection: SandboxInspection,
}

impl Process {
    /// Whether it has gone, which a willing server does once its input closes.
    fn gone(&self) -> bool {
        if !self.stubborn && self.life.closed.load(Ordering::SeqCst) {
            self.life.ended.store(true, Ordering::SeqCst);
        }
        self.life.ended.load(Ordering::SeqCst)
    }
}

impl SandboxProcess for Process {
    fn take_stdin(&mut self) -> Option<Box<dyn Write + Send>> {
        Some(Box::new(Input {
            life: Arc::clone(&self.life),
            ledger: Arc::clone(&self.ledger),
            line: Vec::new(),
        }))
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
        Ok(self.gone().then(exited))
    }

    fn ended(&mut self) -> bool {
        self.gone()
    }

    fn stop(&mut self) -> BoxFuture<'_, io::Result<()>> {
        Box::pin(async move {
            if self.stubborn {
                return Err(io::Error::other("the server would not stop"));
            }
            self.life.ended.store(true, Ordering::SeqCst);
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

/// A launch staged and not yet let go.
struct Staged(Process);

impl SandboxLaunch for Staged {
    fn inspection(&self) -> &SandboxInspection {
        &self.0.inspection
    }

    fn release<'a>(self: Box<Self>) -> BoxFuture<'a, Result<Box<dyn SandboxProcess>, SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move { Ok(Box::new(self.0) as Box<dyn SandboxProcess>) })
    }
}

/// A prepared session, which starts the server once it is staged.
struct Prepared(Process);

impl SandboxSession for Prepared {
    fn inspection(&self) -> &SandboxInspection {
        &self.0.inspection
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
        Box::pin(async move { Ok(Box::new(Staged(self.0)) as Box<dyn SandboxLaunch>) })
    }
}

/// A sandbox that starts the server, as often as it is asked to.
pub(crate) struct Docs {
    ledger: Arc<Ledger>,
    stubborn: bool,
}

impl Docs {
    /// A server that goes when it is let go.
    pub(crate) fn willing() -> (Arc<Self>, Arc<Ledger>) {
        Self::built(false)
    }

    /// A server that refuses every stop and never ends.
    pub(crate) fn stubborn() -> (Arc<Self>, Arc<Ledger>) {
        Self::built(true)
    }

    fn built(stubborn: bool) -> (Arc<Self>, Arc<Ledger>) {
        let ledger = Arc::new(Ledger::default());
        let docs = Arc::new(Self {
            ledger: Arc::clone(&ledger),
            stubborn,
        });
        (docs, ledger)
    }
}

impl SandboxService for Docs {
    fn probe(
        &self,
    ) -> BoxFuture<'_, Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError>> {
        Box::pin(async move { Ok((backend(), SandboxCapabilities::none())) })
    }

    fn prepare(
        &self,
        _request: SandboxRequest,
    ) -> BoxFuture<'_, Result<Box<dyn SandboxSession>, SandboxError>> {
        Box::pin(async move {
            let life = Arc::new(Life::default());
            self.ledger
                .lives
                .lock()
                .expect("the launches")
                .push(Arc::clone(&life));
            Ok(Box::new(Prepared(Process {
                stdout: Some(Says {
                    life: Arc::clone(&life),
                    rest: Vec::new(),
                }),
                life,
                ledger: Arc::clone(&self.ledger),
                stubborn: self.stubborn,
                inspection: inspection(),
            })) as Box<dyn SandboxSession>)
        })
    }
}
