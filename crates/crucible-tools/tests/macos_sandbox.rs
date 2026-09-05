#![allow(
    missing_docs,
    reason = "the target-gated integration crate is empty when checked on another host"
)]
#![cfg(target_os = "macos")]
#![allow(
    clippy::expect_used,
    reason = "native integration tests fail immediately with fixture context"
)]

//! Native behavior checks for the system Seatbelt backend.

use std::ffi::OsString;
use std::fs;
use std::net::TcpListener;
use std::os::fd::AsRawFd as _;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::process::Command as HostCommand;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_core::{
    Ancestry, SandboxCommand, SandboxEnvironment, SandboxFilesystemAccess,
    SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId, SandboxManifest, SandboxMode,
    SandboxNetworkPolicy, SandboxOutput, SandboxPolicy, SandboxProcess, SandboxRead,
    SandboxRequest, SandboxResourceLimits, SandboxService, SandboxUnreadablePattern, ToolId,
    Workspace,
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
            "crucible-macos-{name}-{unique}-{}",
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
        Self::request_with_policy(name, policy)
    }

    fn request_with_limits(&self, name: &str, limits: SandboxResourceLimits) -> SandboxRequest {
        let workspace = Workspace::open(&self.workspace).expect("workspace");
        let base = SandboxPolicy::standard(&workspace).expect("base policy");
        let policy = SandboxPolicy::new(
            SandboxMode::Required,
            base.filesystem().iter().cloned(),
            &self.workspace,
            SandboxNetworkPolicy::Closed,
            limits,
        )
        .expect("limited policy");
        Self::request_with_policy(name, policy)
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

fn command(program: &str, arguments: impl IntoIterator<Item = OsString>) -> SandboxCommand {
    SandboxCommand::new(program, arguments, SandboxEnvironment::empty()).expect("command")
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
    let deadline = Instant::now() + Duration::from_secs(5);
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

fn native_matrix_requires_fixture() -> bool {
    std::env::var_os("CRUCIBLE_TEST_REQUIRE_ENFORCING_SANDBOX").is_some()
}

#[test]
fn seatbelt_writes_only_the_workspace_and_protects_repository_metadata() {
    let fixture = Fixture::new("filesystem");
    fs::write(fixture.workspace.join(".git/config"), "protected\n").expect("protected file");
    fs::create_dir_all(fixture.workspace.join("nested/.git")).expect("nested repository");
    let script = "printf 'allowed\\n' > \"$1\" || exit 70; \
                  if printf denied > \"$2\" 2>/dev/null; then exit 71; fi; \
                  if printf denied > \"$3\" 2>/dev/null; then exit 72; fi; \
                  if /bin/mv \"$4\" \"$5\" 2>/dev/null; then exit 73; fi; \
                  if /bin/mv \"$6\" \"$7\" 2>/dev/null; then exit 74; fi; \
                  if /bin/ln \"$3\" \"$8\" 2>/dev/null; then exit 75; fi";
    let arguments = [
        OsString::from("-c"),
        OsString::from(script),
        OsString::from("crucible-test"),
        fixture.workspace.join("allowed.txt").into_os_string(),
        fixture.outside.join("denied.txt").into_os_string(),
        fixture.workspace.join(".git/config").into_os_string(),
        fixture.workspace.join(".git").into_os_string(),
        fixture.workspace.join(".GIT").into_os_string(),
        fixture.workspace.join("nested").into_os_string(),
        fixture.workspace.join("moved").into_os_string(),
        fixture.workspace.join("config-alias").into_os_string(),
    ];
    let (status, _, errors) = finish(start(
        fixture.request("macos-filesystem"),
        command("/bin/sh", arguments),
    ));

    assert!(
        status.success(),
        "{status:?}: {}",
        String::from_utf8_lossy(&errors)
    );
    assert_eq!(
        fs::read_to_string(fixture.workspace.join("allowed.txt")).expect("allowed write"),
        "allowed\n"
    );
    assert!(!fixture.outside.join("denied.txt").exists());
    assert_eq!(
        fs::read_to_string(fixture.workspace.join(".git/config")).expect("protected file"),
        "protected\n"
    );
    assert!(fixture.workspace.join(".git").is_dir());
    let names: Vec<_> = fs::read_dir(&fixture.workspace)
        .expect("workspace entries")
        .map(|entry| entry.expect("workspace entry").file_name())
        .collect();
    assert!(names.contains(&OsString::from(".git")));
    assert!(!names.contains(&OsString::from(".GIT")));
    assert!(fixture.workspace.join("nested/.git").is_dir());
    assert!(!fixture.workspace.join("moved").exists());
    assert!(!fixture.workspace.join("config-alias").exists());
}

#[test]
fn seatbelt_denies_loopback_network_connections() {
    let fixture = Fixture::new("network");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let endpoint = format!(
        "http://127.0.0.1:{}/",
        listener.local_addr().expect("listener address").port()
    );
    let arguments = [
        OsString::from("--silent"),
        OsString::from("--max-time"),
        OsString::from("1"),
        OsString::from(endpoint),
    ];
    let (status, _, _) = finish(start(
        fixture.request("macos-network"),
        command("/usr/bin/curl", arguments),
    ));

    assert!(!status.success(), "a closed network policy connected");
    assert!(matches!(
        listener.accept(),
        Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock
    ));
}

#[test]
fn seatbelt_denies_signals_to_host_processes() {
    let fixture = Fixture::new("host-process");
    let mut bystander = HostCommand::new("/bin/sleep")
        .arg("30")
        .spawn()
        .expect("host bystander");
    let script = "if /bin/kill -0 \"$1\" 2>/dev/null; then exit 71; fi";
    let (status, _, errors) = finish(start(
        fixture.request("macos-host-process"),
        command(
            "/bin/sh",
            [
                OsString::from("-c"),
                OsString::from(script),
                OsString::from("crucible-test"),
                OsString::from(bystander.id().to_string()),
            ],
        ),
    ));
    let _ = bystander.kill();
    let _ = bystander.wait();

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
}

#[test]
fn seatbelt_does_not_inherit_the_host_environment() {
    let fixture = Fixture::new("environment");
    let (status, output, errors) = finish(start(
        fixture.request("macos-environment"),
        command("/usr/bin/env", []),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    let environment = String::from_utf8(output).expect("UTF-8 environment");
    let entries: Vec<_> = environment.lines().collect();
    assert_eq!(entries.len(), 1, "unexpected environment: {environment}");
    assert!(
        entries
            .first()
            .is_some_and(|entry| entry.starts_with("TMPDIR=/")),
        "{environment}"
    );
}

#[test]
fn seatbelt_blocks_the_runners_passwordless_privilege_path() {
    let positive = HostCommand::new("/usr/bin/sudo")
        .args(["-n", "/usr/bin/true"])
        .status()
        .expect("sudo positive control");
    if !positive.success() {
        assert!(
            !native_matrix_requires_fixture(),
            "the required macOS CI runner has no passwordless sudo fixture"
        );
        return;
    }

    let fixture = Fixture::new("privilege");
    let (status, _, _) = finish(start(
        fixture.request("macos-privilege"),
        command(
            "/usr/bin/sudo",
            [OsString::from("-n"), OsString::from("/usr/bin/true")],
        ),
    ));

    assert!(
        !status.success(),
        "a confined command acquired sudo authority"
    );
}

#[test]
fn owned_process_group_cleanup_stops_a_background_descendant() {
    let fixture = Fixture::new("process-cleanup");
    let script = "/bin/sleep 30 </dev/null >/dev/null 2>/dev/null & echo $!";
    let (status, output, errors) = finish(start(
        fixture.request("macos-process-cleanup"),
        command("/bin/sh", [OsString::from("-c"), OsString::from(script)]),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    let pid = String::from_utf8(output)
        .expect("UTF-8 pid")
        .trim()
        .parse::<u32>()
        .expect("background pid");
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let alive = HostCommand::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success());
        if !alive {
            break;
        }
        if Instant::now() >= deadline {
            let _ = HostCommand::new("/bin/kill")
                .args(["-9", &pid.to_string()])
                .status();
            panic!("background descendant survived owned-group cleanup");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn seatbelt_denies_unix_domain_socket_connections() {
    let fixture = Fixture::new("unix-network");
    // Darwin's sockaddr_un path is much shorter than a GitHub runner's
    // canonical temporary directory. Keep this host endpoint outside the
    // workspace while making the test independent of TMPDIR length.
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos());
    let socket = PathBuf::from(format!("/tmp/cru-{}-{unique:x}.sock", std::process::id()));
    let listener = UnixListener::bind(&socket).expect("Unix listener");
    listener
        .set_nonblocking(true)
        .expect("nonblocking listener");
    let script = "import socket,sys\ns=socket.socket(socket.AF_UNIX)\ntry:\n s.connect(sys.argv[1])\nexcept OSError:\n sys.exit(0)\nsys.exit(71)";
    let (status, _, errors) = finish(start(
        fixture.request("macos-unix-network"),
        command(
            "/usr/bin/python3",
            [
                OsString::from("-c"),
                OsString::from(script),
                socket.as_os_str().to_owned(),
            ],
        ),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert!(matches!(
        listener.accept(),
        Err(problem) if problem.kind() == std::io::ErrorKind::WouldBlock
    ));
    drop(listener);
    fs::remove_file(socket).expect("remove Unix listener");
}

#[test]
fn seatbelt_hides_paths_selected_by_unreadable_patterns() {
    let fixture = Fixture::new("unreadable");
    fs::create_dir(fixture.workspace.join("nested")).expect("nested directory");
    fs::write(fixture.workspace.join("nested/secret.pem"), "secret\n").expect("secret");
    fs::write(fixture.workspace.join("nested/visible.txt"), "visible\n").expect("visible");
    let workspace = Workspace::open(&fixture.workspace).expect("workspace");
    let policy = SandboxPolicy::standard(&workspace)
        .expect("policy")
        .with_unreadable_patterns([SandboxUnreadablePattern::new(
            fixture.workspace.join("**/*.pem"),
            SandboxFilesystemProvenance::Descendant,
        )
        .expect("pattern")])
        .expect("unreadable policy");
    let script = "if /bin/cat nested/secret.pem 2>/dev/null; then exit 71; fi; \
                  if printf hidden > nested/new.PEM 2>/dev/null; then exit 72; fi; \
                  if /bin/mv nested/visible.txt nested/renamed.pem 2>/dev/null; then exit 73; fi; \
                  /bin/cat nested/visible.txt";
    let (status, output, errors) = finish(start(
        Fixture::request_with_policy("macos-unreadable", policy),
        command("/bin/sh", [OsString::from("-c"), OsString::from(script)]),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8_lossy(&output), "visible\n");
    assert!(!fixture.workspace.join("nested/new.PEM").exists());
    assert!(!fixture.workspace.join("nested/renamed.pem").exists());
}

#[test]
fn seatbelt_hides_a_private_var_path_through_its_system_alias() {
    let fixture = Fixture::new("private-var-alias");
    let workspace = Workspace::open(&fixture.workspace).expect("workspace");
    let base = SandboxPolicy::standard(&workspace).expect("policy");
    let unreadable = SandboxFilesystemRule::new(
        "/private/var/select",
        SandboxFilesystemAccess::Unreadable,
        SandboxFilesystemProvenance::Descendant,
    )
    .expect("unreadable rule");
    let policy = SandboxPolicy::new(
        SandboxMode::Required,
        base.filesystem().iter().cloned().chain([unreadable]),
        &fixture.workspace,
        SandboxNetworkPolicy::Closed,
        SandboxResourceLimits::confining(),
    )
    .expect("unreadable policy");
    let script = "if /bin/ls /var/select >/dev/null 2>&1; then exit 71; fi";
    let (status, _, errors) = finish(start(
        Fixture::request_with_policy("macos-private-var-alias", policy),
        command("/bin/sh", [OsString::from("-c"), OsString::from(script)]),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
}

#[test]
fn the_pre_seatbelt_launcher_closes_inherited_descriptors() {
    let fixture = Fixture::new("descriptors");
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("listener");
    let descriptor = listener.as_raw_fd();
    let flags = rustix::io::fcntl_getfd(&listener).expect("descriptor flags");
    rustix::io::fcntl_setfd(&listener, rustix::io::FdFlags::empty())
        .expect("make descriptor inheritable");
    let script = format!(
        "import os,sys\ntry:\n os.fstat({descriptor})\nexcept OSError:\n sys.exit(0)\nsys.exit(71)"
    );
    let started = start(
        fixture.request("macos-descriptors"),
        command(
            "/usr/bin/python3",
            [OsString::from("-c"), OsString::from(script)],
        ),
    );
    rustix::io::fcntl_setfd(&listener, flags).expect("restore descriptor flags");
    let (status, _, errors) = finish(started);

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
}

#[test]
fn the_pre_seatbelt_launcher_applies_the_requested_open_file_limit() {
    let fixture = Fixture::new("open-files");
    let request = fixture.request_with_limits(
        "macos-open-files",
        SandboxResourceLimits {
            open_files: Some(32),
            ..SandboxResourceLimits::default()
        },
    );
    let (status, output, errors) = finish(start(
        request,
        command(
            "/bin/sh",
            [OsString::from("-c"), OsString::from("ulimit -n")],
        ),
    ));

    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(String::from_utf8_lossy(&output).trim(), "32");
}
