//! A real program, started by the confinement the application composes and
//! hosted as an MCP server over the application's own runtime.
//!
//! Everything below the conversation is the shipped path: the local backend's
//! pipes, read and written by the transport's tasks on the runtime a run owns,
//! with its I/O driver and its timer. The program is a few lines of shell that
//! answer the three things a server is asked, so what is proven is the path
//! and not a server.
//!
//! Unconfined always, and confined wherever this machine can confine: where a
//! job declares that the confining backend must exist, its absence fails the
//! test rather than skipping it.

#![cfg(unix)]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "a test reports exactly what it expected and did not get"
)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crucible_app::services::serving;
use crucible_mcp::Hosted;
use crucible_sandbox::{
    SandboxCommand, SandboxEnvironment, SandboxError, SandboxManifest, SandboxPolicy,
    SandboxRequest, SandboxService,
};
use crucible_sandbox_local::LocalSandbox;
use crucible_types::{Ancestry, SandboxId, ToolId};
use crucible_workspace::Workspace;
use serde_json::json;
use tokio::runtime::Handle;

/// How long one exchange with the program may take.
const PATIENCE: Duration = Duration::from_secs(20);

/// A server in a few lines: it answers the handshake, the catalogue and one
/// call, mutters once on the way, and then waits for its input to close.
const SERVER: &str = r#"read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"live","version":"1"}}}'
read -r line
read -r line
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","inputSchema":{"type":"object"}}]}}'
read -r line
echo 'a complaint on the way' >&2
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"heard"}],"isError":false}}'
cat > /dev/null
"#;

/// The variable a job sets so that a missing confining backend is a failure.
const REQUIRE_ENFORCING_SANDBOX: &str = "CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX";

/// A directory of the test's own, removed when it drops.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Self {
        static MADE: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "crucible-hosted-server-{}-{}",
            std::process::id(),
            MADE.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).expect("a directory for the test");
        Self(path)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether to host confined as well as unconfined: always unconfined, and
/// confined where the backend can confine here or a job requires it to.
fn confinements(service: &LocalSandbox, runtime: &Handle) -> Vec<bool> {
    match runtime.block_on(service.probe()) {
        Ok(_) => vec![false, true],
        Err(SandboxError::BackendUnavailable { reason }) => {
            assert!(
                std::env::var_os(REQUIRE_ENFORCING_SANDBOX).is_none(),
                "the confining backend is required by this job but unavailable: {reason}"
            );
            vec![false]
        }
        Err(other) => panic!("sandbox probe: {other}"),
    }
}

/// Starts the server under `enabled` confinement and speaks to it through the
/// whole conversation, ending it.
fn hosted_once(service: &LocalSandbox, runtime: &Handle, enabled: bool) {
    let scratch = Scratch::new();
    let workspace = Workspace::open(&scratch.0).expect("a workspace");
    let policy = SandboxPolicy::standard(&workspace)
        .expect("a policy")
        .with_enabled(enabled);
    let request = SandboxRequest::new(
        SandboxId::new(),
        Ancestry::new(),
        ToolId::new("mcp:live"),
        policy,
        SandboxManifest::empty(),
    );
    let mut session = runtime
        .block_on(service.prepare(request))
        .expect("a session");
    runtime
        .block_on(session.materialize())
        .expect("materialized");
    let command = SandboxCommand::new(
        "/bin/sh",
        [OsString::from("-c"), OsString::from(SERVER)],
        SandboxEnvironment::empty(),
    )
    .expect("a command")
    .spoken_to();
    let process = runtime
        .block_on(session.start(command))
        .expect("the server started");

    // Awaited, as hosting speaks to a server.
    let mut hosted = Hosted::over(process, PATIENCE, runtime).expect("both pipes were there");
    let (tool, answered) = runtime.block_on(async {
        let greeting = hosted.greet_async(None).await.expect("the handshake");
        let offered = hosted
            .catalogue_async(&greeting, None)
            .await
            .expect("the catalogue");
        let tool = offered.first().expect("one tool offered").clone();
        let answered = hosted
            .call_async(&tool, &json!({}), None)
            .await
            .expect("the call");
        (tool, answered)
    });
    let ended = hosted.stop(Duration::from_secs(10));

    assert_eq!(
        (tool.name(), answered.text(), answered.failed()),
        ("echo", "heard", false),
        "confined: {enabled}"
    );
    assert!(
        format!("{:?}", ended.finish).starts_with("Exited("),
        "the server went on its own once its input closed (confined: {enabled}): {:?}",
        ended.finish
    );
    assert_eq!(
        ended.muttered.text(),
        "a complaint on the way\n",
        "what it said beside the conversation was kept (confined: {enabled})"
    );
}

#[test]
fn a_real_server_is_spoken_to_over_the_applications_runtime() {
    let ((), shutdown) = serving(|services| {
        let runtime = services
            .runtime()
            .handle()
            .expect("the application's runtime");
        // Built as a run builds it: the commands it starts are watched on the
        // same runtime their streams are read and written on.
        let service = LocalSandbox::new().watching_on(runtime.clone());
        for enabled in confinements(&service, &runtime) {
            hosted_once(&service, &runtime, enabled);
        }
    });

    assert_eq!(
        shutdown,
        Ok(()),
        "every task the servers' streams had was over by the end"
    );
}
