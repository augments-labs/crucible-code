#![allow(
    missing_docs,
    reason = "the target-gated integration crate is empty when checked on another host"
)]
#![cfg(target_os = "windows")]
#![allow(
    clippy::expect_used,
    reason = "native integration tests fail immediately with fixture context"
)]

//! Native behavior checks for the dedicated-account Windows backend.

use std::ffi::OsString;
use std::fs;
use std::io::Write as _;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command as HostCommand;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_core::{
    Ancestry, SandboxCommand, SandboxEnvironment, SandboxError, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId, SandboxManifest, SandboxMode,
    SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead,
    SandboxRequest, SandboxService, ToolId, Workspace,
};
use crucible_tools::LocalSandbox;

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

fn powershell() -> PathBuf {
    let root = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"));
    root.join("System32/WindowsPowerShell/v1.0/powershell.exe")
        .canonicalize()
        .expect("PowerShell")
}

fn command(script: &str, arguments: impl IntoIterator<Item = OsString>) -> SandboxCommand {
    let arguments = [
        OsString::from("-NoLogo"),
        OsString::from("-NoProfile"),
        OsString::from("-NonInteractive"),
        OsString::from("-Command"),
        OsString::from(script),
    ]
    .into_iter()
    .chain(arguments);
    SandboxCommand::new(powershell(), arguments, SandboxEnvironment::empty()).expect("command")
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
    let deadline = Instant::now() + Duration::from_secs(20);
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
fn windows_writes_only_the_workspace_and_protects_repository_metadata() {
    let fixture = Fixture::new("filesystem");
    let protected = fixture.workspace.join(".git/config");
    let protected_directory = fixture.workspace.join(".git");
    let renamed_directory = fixture.workspace.join("renamed-control");
    let linked_file = fixture.workspace.join("config-alias");
    fs::write(&protected, "protected").expect("protected file");
    let allowed = fixture.workspace.join("allowed.txt");
    let denied = fixture.outside.join("denied.txt");
    let script = "$ErrorActionPreference='Stop'; \
        [IO.File]::WriteAllText($args[0], 'allowed'); \
        try { [IO.File]::WriteAllText($args[1], 'denied'); exit 71 } catch {} \
        try { [IO.File]::WriteAllText($args[2], 'denied'); exit 72 } catch {} \
        try { Move-Item -LiteralPath $args[3] -Destination $args[4] -ErrorAction Stop; exit 73 } catch {} \
        try { New-Item -ItemType HardLink -Path $args[5] -Target $args[2] -ErrorAction Stop | Out-Null; exit 74 } catch {} \
        [Console]::Out.Write([Environment]::UserName)";
    let (status, output, errors) = finish(start(
        fixture.request("windows-filesystem"),
        command(
            script,
            [
                allowed.clone().into_os_string(),
                denied.clone().into_os_string(),
                protected.clone().into_os_string(),
                protected_directory.clone().into_os_string(),
                renamed_directory.clone().into_os_string(),
                linked_file.clone().into_os_string(),
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
    assert!(
        String::from_utf8_lossy(&output)
            .to_ascii_lowercase()
            .starts_with("cruciblesbx-"),
        "unexpected sandbox identity: {}",
        String::from_utf8_lossy(&output)
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
        SandboxMode::Required,
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
    let script = "$client=[Net.Sockets.TcpClient]::new(); \
        try { $client.Connect('127.0.0.1', [int]$args[0]); exit 71 } catch { exit 0 }";
    let (status, _, errors) = finish(start(
        fixture.request("windows-network"),
        command(script, [OsString::from(port.to_string())]),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert!(matches!(
        listener.accept(),
        Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn windows_forwards_input_after_its_private_launch_frame() {
    let fixture = Fixture::new("input");
    let script = "[Console]::Out.Write([Console]::In.ReadToEnd())";
    let mut process = start(
        fixture.request("windows-input"),
        command(script, []).spoken_to(),
    );
    let mut input = process.take_stdin().expect("held input");
    input.write_all(b"caller input").expect("write input");
    input.flush().expect("flush input");
    drop(input);
    let (status, output, errors) = finish(process);

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(output, b"caller input");
}

#[test]
fn windows_preserves_the_native_workload_exit_status() {
    let fixture = Fixture::new("exit-status");
    let (status, _, errors) = finish(start(
        fixture.request("windows-exit-status"),
        command("exit 4660", []),
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
    let executable = powershell();
    let script = "$child=Start-Process -FilePath $args[0] \
        -ArgumentList @('-NoProfile','-NonInteractive','-Command','Start-Sleep -Seconds 30') \
        -PassThru; [Console]::Out.Write($child.Id)";
    let (status, output, errors) = finish(start(
        fixture.request("windows-process-cleanup"),
        command(script, [executable.clone().into_os_string()]),
    ));
    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    let pid = String::from_utf8(output)
        .expect("UTF-8 pid")
        .parse::<u32>()
        .expect("background pid");
    let deadline = Instant::now() + Duration::from_secs(5);
    while process_exists(&executable, pid) {
        assert!(
            Instant::now() < deadline,
            "background descendant survived Job cleanup"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn process_exists(powershell: &Path, pid: u32) -> bool {
    HostCommand::new(powershell)
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "if (Get-Process -Id {pid} -ErrorAction SilentlyContinue) {{ exit 0 }}; exit 1"
            ),
        ])
        .status()
        .is_ok_and(|status| status.success())
}
