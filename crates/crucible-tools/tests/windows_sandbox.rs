#![allow(
    missing_docs,
    reason = "the target-gated integration crate is empty when checked on another host"
)]
#![cfg(target_os = "windows")]
#![allow(
    clippy::exit,
    clippy::expect_used,
    clippy::panic,
    clippy::zombie_processes,
    reason = "the native fixture reports exact failures and deliberately leaves a descendant for job cleanup"
)]

//! Native behavior checks for the dedicated-account Windows backend.

use std::ffi::OsString;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command as HostCommand;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_core::{
    Ancestry, SandboxCommand, SandboxEnvironment, SandboxError, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId, SandboxManifest,
    SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead,
    SandboxRequest, SandboxService, ToolId, Workspace,
};
use crucible_tools::LocalSandbox;

const ACTION: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_ACTION";
const ALLOWED: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_ALLOWED";
const CONNECTED: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_CONNECTED";
const DENIED: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_DENIED";
const IDENTITY: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_IDENTITY";
const LINKED: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_LINKED";
const PORT: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_PORT";
const PROTECTED: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_PROTECTED";
const PROTECTED_DIRECTORY: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_PROTECTED_DIRECTORY";
const RENAMED_DIRECTORY: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_RENAMED_DIRECTORY";
const RESULT: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_RESULT";
const SENTINEL: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_SENTINEL";
const WHOAMI: &str = "CRUCIBLE_WINDOWS_SANDBOX_TEST_WHOAMI";

// Native launches may queue behind the broker's 30-second machine-wide setup
// bound. Keep this outer deadline longer so a broker timeout is reported as
// the result instead of being hidden by the integration harness.
const COMMAND_WAIT: Duration = Duration::from_secs(45);

struct Fixture {
    parent: PathBuf,
    workspace: PathBuf,
    outside: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_nanos());
        let parent = std::env::temp_dir().join(format!(
            "crucible-windows-{name}-{unique}-{}",
            std::process::id()
        ));
        let workspace = parent.join("workspace");
        let outside = parent.join("outside");
        fs::create_dir_all(workspace.join(".git")).expect("workspace");
        fs::create_dir(&outside).expect("outside");
        crucible_privacy::directory(&outside).expect("protected outside directory");
        Self {
            parent: parent.canonicalize().expect("canonical fixture"),
            workspace: workspace.canonicalize().expect("canonical workspace"),
            outside: outside.canonicalize().expect("canonical outside"),
        }
    }

    fn request(&self, name: &str) -> SandboxRequest {
        let workspace = Workspace::open(&self.workspace).expect("workspace");
        let policy = SandboxPolicy::standard(&workspace).expect("policy");
        SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new(name),
            policy,
            SandboxManifest::empty(),
        )
    }

    fn request_with_policy(name: &str, policy: SandboxPolicy) -> SandboxRequest {
        SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new(name),
            policy,
            SandboxManifest::empty(),
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.parent);
    }
}

fn system_program(name: &str) -> PathBuf {
    let root = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"));
    root.join("System32").join(name).canonicalize().expect(name)
}

fn command(
    action: &'static str,
    variables: impl IntoIterator<Item = (&'static str, OsString)>,
) -> SandboxCommand {
    let variables: Vec<_> = std::iter::once((ACTION, OsString::from(action)))
        .chain(variables)
        .collect();
    let environment = SandboxEnvironment::new(
        variables
            .iter()
            .map(|(name, value)| (*name, value.as_os_str())),
    )
    .expect("environment");
    SandboxCommand::new(
        std::env::current_exe().expect("current test executable"),
        [
            OsString::from("--exact"),
            OsString::from("windows_sandbox_workload"),
            OsString::from("--test-threads=1"),
        ],
        environment,
    )
    .expect("command")
}

#[test]
fn windows_sandbox_workload() {
    let Some(action) = std::env::var_os(ACTION) else {
        return;
    };
    match action.to_string_lossy().as_ref() {
        "filesystem" => workload_filesystem(),
        "input" => workload_input(),
        "linger" => workload_linger(),
        "network" => workload_network(),
        "spawn-descendant" => workload_spawn_descendant(),
        "status" => std::process::exit(4660),
        "temporary-directory" => workload_temporary_directory(),
        unknown => panic!("unknown native sandbox workload {unknown}"),
    }
}

fn workload_filesystem() {
    fs::write(required_path(ALLOWED), "allowed").expect("allowed write");
    assert!(fs::write(required_path(DENIED), "denied").is_err());
    assert!(fs::write(required_path(PROTECTED), "denied").is_err());
    assert!(
        fs::rename(
            required_path(PROTECTED_DIRECTORY),
            required_path(RENAMED_DIRECTORY)
        )
        .is_err()
    );
    assert!(fs::hard_link(required_path(PROTECTED), required_path(LINKED)).is_err());
    let identity = HostCommand::new(required_path(WHOAMI))
        .output()
        .expect("sandbox identity");
    assert!(identity.status.success());
    fs::write(required_path(IDENTITY), identity.stdout).expect("identity result");
}

fn workload_input() {
    let mut input = Vec::new();
    std::io::stdin()
        .read_to_end(&mut input)
        .expect("sandbox input");
    fs::write(required_path(RESULT), input).expect("input result");
}

fn workload_network() {
    let port = required(PORT).parse::<u16>().expect("network port");
    let address = SocketAddr::from(([127, 0, 0, 1], port));
    if TcpStream::connect_timeout(&address, Duration::from_secs(2)).is_ok() {
        fs::write(required_path(CONNECTED), "connected").expect("connection result");
    }
}

fn workload_spawn_descendant() {
    let child = HostCommand::new(std::env::current_exe().expect("current test executable"))
        .args(["--exact", "windows_sandbox_workload", "--test-threads=1"])
        .env(ACTION, "linger")
        .spawn()
        .expect("background descendant");
    fs::write(required_path(RESULT), child.id().to_string()).expect("descendant pid");
}

fn workload_linger() {
    thread::sleep(Duration::from_secs(30));
    fs::write(required_path(SENTINEL), "survived").expect("descendant sentinel");
}

fn workload_temporary_directory() {
    let temporary = required_path("TEMP");
    assert_eq!(temporary, required_path("TMP"));
    fs::write(temporary.join("created.txt"), "temporary").expect("temporary write");
    fs::write(
        required_path(RESULT),
        temporary.to_string_lossy().as_bytes(),
    )
    .expect("temporary result");
}

fn required(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("missing workload variable {name}"))
}

fn required_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_else(|| panic!("missing workload path {name}")))
}

fn start(request: SandboxRequest, command: SandboxCommand) -> Box<dyn SandboxProcess> {
    let service = LocalSandbox::new();
    let mut session = service.prepare(request).expect("prepared sandbox");
    session.materialize().expect("materialized sandbox");
    session.start(command).expect("started command")
}

fn finish(mut process: Box<dyn SandboxProcess>) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    let mut stdout = process.take_stdout();
    let mut stderr = process.take_stderr();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut status = None;
    let deadline = Instant::now() + COMMAND_WAIT;
    while stdout.is_some() || stderr.is_some() || status.is_none() {
        read(&mut stdout, &mut output);
        read(&mut stderr, &mut errors);
        status = status.or_else(|| process.try_wait().expect("wait"));
        assert!(Instant::now() < deadline, "sandbox command timed out");
        thread::sleep(Duration::from_millis(10));
    }
    process.stop().expect("cleanup");
    (status.expect("status"), output, errors)
}

fn read(stream: &mut Option<Box<dyn SandboxOutput>>, retained: &mut Vec<u8>) {
    let Some(output) = stream else {
        return;
    };
    let mut buffer = [0_u8; 512];
    match output.read_ready(&mut buffer).expect("read output") {
        SandboxRead::Bytes(read) | SandboxRead::Limited { retained: read, .. } => {
            retained.extend_from_slice(buffer.get(..read).expect("reported bytes"));
        }
        SandboxRead::Pending => {}
        SandboxRead::End => *stream = None,
    }
}

#[test]
fn windows_writes_the_workspace_and_protects_private_and_repository_data() {
    let fixture = Fixture::new("filesystem");
    let protected = fixture.workspace.join(".git/config");
    let protected_directory = fixture.workspace.join(".git");
    let renamed_directory = fixture.workspace.join("renamed-control");
    let linked_file = fixture.workspace.join("config-alias");
    fs::write(&protected, "protected").expect("protected file");
    let allowed = fixture.workspace.join("allowed.txt");
    let denied = fixture.outside.join("denied.txt");
    let identity = fixture.workspace.join("identity.txt");
    let (status, _, errors) = finish(start(
        fixture.request("windows-filesystem"),
        command(
            "filesystem",
            [
                (ALLOWED, allowed.clone().into_os_string()),
                (DENIED, denied.clone().into_os_string()),
                (PROTECTED, protected.clone().into_os_string()),
                (
                    PROTECTED_DIRECTORY,
                    protected_directory.clone().into_os_string(),
                ),
                (
                    RENAMED_DIRECTORY,
                    renamed_directory.clone().into_os_string(),
                ),
                (LINKED, linked_file.clone().into_os_string()),
                (IDENTITY, identity.clone().into_os_string()),
                (WHOAMI, system_program("whoami.exe").into_os_string()),
            ],
        ),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(
        fs::read_to_string(allowed).expect("allowed write"),
        "allowed"
    );
    assert!(!denied.exists());
    assert_eq!(
        fs::read_to_string(protected).expect("protected read"),
        "protected"
    );
    assert!(protected_directory.is_dir());
    assert!(!renamed_directory.exists());
    assert!(!linked_file.exists());
    let identity = fs::read_to_string(identity).expect("sandbox identity");
    assert!(
        identity
            .trim()
            .rsplit_once('\\')
            .map_or(identity.trim(), |(_, account)| account)
            .to_ascii_lowercase()
            .starts_with("cruciblesbx-"),
        "unexpected sandbox identity: {identity}"
    );
}

#[test]
fn windows_refuses_an_explicit_unreadable_root_before_preparation() {
    let fixture = Fixture::new("unreadable-refusal");
    let hidden = fixture.workspace.join("hidden.txt");
    fs::write(&hidden, "hidden").expect("hidden file");
    let workspace = Workspace::open(&fixture.workspace).expect("workspace");
    let base = SandboxPolicy::standard(&workspace).expect("base policy");
    let unreadable = SandboxFilesystemRule::new(
        hidden,
        SandboxFilesystemAccess::Unreadable,
        SandboxFilesystemProvenance::Descendant,
    )
    .expect("unreadable root");
    let policy = SandboxPolicy::new(
        true,
        base.filesystem().iter().cloned().chain([unreadable]),
        &fixture.workspace,
        SandboxNetworkPolicy::Closed,
        base.limits(),
    )
    .expect("unreadable policy");
    let result = LocalSandbox::new().prepare(Fixture::request_with_policy(
        "windows-unreadable-refusal",
        policy,
    ));

    assert!(matches!(
        result,
        Err(SandboxError::Unsupported {
            feature: crucible_core::SandboxFeature::Filesystem
        })
    ));
}

#[test]
fn windows_denies_loopback_network_connections() {
    let fixture = Fixture::new("network");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let port = listener.local_addr().expect("listener address").port();
    let connected = fixture.workspace.join("connected.txt");
    let (status, _, errors) = finish(start(
        fixture.request("windows-network"),
        command(
            "network",
            [
                (PORT, OsString::from(port.to_string())),
                (CONNECTED, connected.clone().into_os_string()),
            ],
        ),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert!(!connected.exists());
    assert!(matches!(
        listener.accept(),
        Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn windows_forwards_input_after_its_private_launch_frame() {
    let fixture = Fixture::new("input");
    let result = fixture.workspace.join("input.txt");
    let mut process = start(
        fixture.request("windows-input"),
        command("input", [(RESULT, result.clone().into_os_string())]).spoken_to(),
    );
    let mut input = process.take_stdin().expect("held input");
    input.write_all(b"caller input").expect("write input");
    input.flush().expect("flush input");
    drop(input);
    let (status, _, errors) = finish(process);

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(fs::read(result).expect("input result"), b"caller input");
}

#[test]
fn windows_supplies_and_removes_a_private_temporary_directory() {
    let fixture = Fixture::new("temporary-directory");
    let result = fixture.workspace.join("temporary-directory.txt");
    let (status, _, errors) = finish(start(
        fixture.request("windows-temporary-directory"),
        command(
            "temporary-directory",
            [(RESULT, result.clone().into_os_string())],
        ),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    let temporary = PathBuf::from(fs::read_to_string(result).expect("temporary path"));
    assert_ne!(temporary, std::env::temp_dir());
    assert!(
        !temporary.exists(),
        "private temporary directory survived command cleanup"
    );
}

#[test]
fn windows_preserves_the_native_workload_exit_status() {
    let fixture = Fixture::new("exit-status");
    let (status, _, errors) = finish(start(
        fixture.request("windows-exit-status"),
        command("status", std::iter::empty::<(&'static str, OsString)>()),
    ));

    assert_eq!(
        status.code(),
        Some(4660),
        "{}",
        String::from_utf8_lossy(&errors)
    );
}

#[test]
fn windows_job_cleanup_stops_a_background_descendant() {
    let fixture = Fixture::new("process-cleanup");
    let result = fixture.workspace.join("descendant-pid.txt");
    let sentinel = fixture.workspace.join("descendant-survived.txt");
    let (status, _, errors) = finish(start(
        fixture.request("windows-process-cleanup"),
        command(
            "spawn-descendant",
            [
                (RESULT, result.clone().into_os_string()),
                (SENTINEL, sentinel.clone().into_os_string()),
            ],
        ),
    ));
    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    let pid = fs::read_to_string(result)
        .expect("descendant pid")
        .parse::<u32>()
        .expect("background pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(pid) {
        assert!(
            Instant::now() < deadline,
            "background descendant survived Job cleanup"
        );
        thread::sleep(Duration::from_millis(25));
    }
    assert!(!sentinel.exists());
}

fn process_exists(pid: u32) -> bool {
    let Ok(output) = HostCommand::new(system_program("tasklist.exe"))
        .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
        .output()
    else {
        return true;
    };
    String::from_utf8_lossy(&output.stdout).lines().any(|line| {
        line.split(',')
            .nth(1)
            .is_some_and(|field| field.trim_matches('"') == pid.to_string())
    })
}
