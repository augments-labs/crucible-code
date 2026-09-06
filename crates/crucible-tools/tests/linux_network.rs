#![allow(
    missing_docs,
    reason = "the native integration crate is empty on other targets"
)]
#![cfg(target_os = "linux")]
#![allow(
    clippy::expect_used,
    clippy::panic,
    reason = "native acceptance fixtures report their exact failed observation"
)]

//! Real namespace observations for domain mediation and local socket authority.

use std::ffi::OsStr;
use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpListener;
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crucible_core::{
    Ancestry, SandboxCommand, SandboxDomainPattern, SandboxDomainPolicy, SandboxEnvironment,
    SandboxFilesystemAccess, SandboxFilesystemProvenance, SandboxFilesystemRule, SandboxId,
    SandboxManifest, SandboxNetworkPolicy, SandboxNetworkProvenance, SandboxOutput, SandboxPolicy,
    SandboxProcess, SandboxRead, SandboxRequest, SandboxService, ToolId, Workspace,
};
use crucible_tools::LocalSandbox;

// The native writable projection currently admits one transaction per user.
// Keep independent canaries serialized without changing that product contract.
static NATIVE: std::sync::Mutex<()> = std::sync::Mutex::new(());

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "crucible-linux-network-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture");
        Self(root.canonicalize().expect("canonical fixture"))
    }

    fn start(
        &self,
        network: SandboxDomainPolicy,
        command: SandboxCommand,
    ) -> Box<dyn SandboxProcess> {
        let workspace = Workspace::open(&self.0).expect("workspace");
        let standard = SandboxPolicy::standard(&workspace).expect("standard policy");
        let executable = SandboxFilesystemRule::new(
            command.program(),
            SandboxFilesystemAccess::ReadOnly,
            SandboxFilesystemProvenance::UserConfiguration,
        )
        .expect("executable rule");
        let policy = SandboxPolicy::new(
            true,
            standard.filesystem().iter().cloned().chain([executable]),
            &self.0,
            SandboxNetworkPolicy::Domains(network),
            standard.limits(),
        )
        .expect("network policy");
        let request = SandboxRequest::new(
            SandboxId::new(),
            Ancestry::new(),
            ToolId::new("network-test"),
            policy,
            SandboxManifest::empty(),
        );
        let mut session = LocalSandbox::new()
            .prepare(request)
            .expect("enforcing network preparation");
        session.materialize().expect("materialization");
        session.start(command).expect("network command")
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn domains(allowed: bool, binding: bool, sockets: Vec<PathBuf>) -> SandboxDomainPolicy {
    SandboxDomainPolicy::new(
        if allowed {
            vec![SandboxDomainPattern::new("127.0.0.1").expect("literal grant")]
        } else {
            Vec::new()
        },
        [],
        binding,
        sockets,
        SandboxNetworkProvenance::User,
    )
    .expect("domain policy")
}

fn finish(mut process: Box<dyn SandboxProcess>) -> (std::process::ExitStatus, Vec<u8>, Vec<u8>) {
    let mut stdout = process.take_stdout();
    let mut stderr = process.take_stderr();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    let mut status = None;
    let deadline = Instant::now() + Duration::from_secs(10);
    while stdout.is_some() || stderr.is_some() || status.is_none() {
        for (stream, retained) in [(&mut stdout, &mut output), (&mut stderr, &mut errors)] {
            if let Some(reader) = stream {
                let mut bytes = [0; 4096];
                match reader.read_ready(&mut bytes).expect("output") {
                    SandboxRead::Bytes(count)
                    | SandboxRead::Limited {
                        retained: count, ..
                    } => {
                        retained.extend_from_slice(bytes.get(..count).expect("bounded read"));
                    }
                    SandboxRead::End => *stream = None,
                    SandboxRead::Pending => {}
                }
            }
        }
        status = status.or_else(|| process.try_wait().expect("wait"));
        assert!(
            Instant::now() < deadline,
            "network workload exceeded its fixture deadline"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    process.stop().expect("complete cleanup");
    (status.expect("status"), output, errors)
}

#[test]
fn linux_curl_uses_the_private_proxy_and_a_denied_target_is_never_dialed() {
    let _serial = NATIVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let listener = TcpListener::bind("127.0.0.1:0").expect("origin");
    listener.set_nonblocking(true).expect("nonblocking");
    let address = listener.local_addr().expect("origin address");
    let command = || {
        SandboxCommand::new(
            PathBuf::from("/usr/bin/curl"),
            [
                "--fail".into(),
                "--silent".into(),
                "--show-error".into(),
                "--max-time".into(),
                "3".into(),
                format!("http://{address}/allowed").into(),
            ],
            SandboxEnvironment::new([
                ("NO_PROXY", OsStr::new("*")),
                ("http_proxy", OsStr::new("http://invalid.test:1")),
            ])
            .expect("inherited bypass fixture"),
        )
        .expect("curl command")
    };
    let process = fixture.start(domains(true, false, Vec::new()), command());
    let origin = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match listener.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("authorized request did not arrive: {other:?}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("origin timeout");
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte).expect("origin request");
            request.extend_from_slice(&byte);
            assert!(request.len() < 16_384);
        }
        assert!(request.starts_with(b"GET /allowed HTTP/1.1\r\n"));
        assert!(
            !String::from_utf8_lossy(&request)
                .to_ascii_lowercase()
                .contains("proxy-authorization")
        );
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 7\r\nConnection: close\r\n\r\nallowed")
            .expect("origin reply");
        listener
    });
    let (status, output, errors) = finish(process);
    let listener = origin.join().expect("origin fixture");
    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert_eq!(output, b"allowed");
    let (status, _, _) = finish(fixture.start(domains(false, false, Vec::new()), command()));
    assert!(!status.success(), "denied request succeeded");
    assert_eq!(
        listener
            .accept()
            .expect_err("denied origin must not be contacted")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}

const ACTION: &str = "CRUCIBLE_TEST_NETWORK_ACTION";
const ENDPOINT: &str = "CRUCIBLE_TEST_NETWORK_ENDPOINT";
const DENIED_SOCKET: &str = "CRUCIBLE_TEST_DENIED_SOCKET";

fn workload(action: &str, endpoint: &OsStr, denied: &OsStr) -> SandboxCommand {
    SandboxCommand::new(
        std::env::current_exe().expect("workload binary"),
        [
            "--exact".into(),
            "linux_network_workload".into(),
            "--nocapture".into(),
        ],
        SandboxEnvironment::new([
            (ACTION, OsStr::new(action)),
            (ENDPOINT, endpoint),
            (DENIED_SOCKET, denied),
        ])
        .expect("workload environment"),
    )
    .expect("workload")
}

#[test]
fn linux_network_workload() {
    let Some(action) = std::env::var_os(ACTION) else {
        return;
    };
    let endpoint = std::env::var_os(ENDPOINT).expect("endpoint");
    match action.to_str().expect("action") {
        "bind-denied" | "bind-allowed" => {
            let allowed = action == "bind-allowed";
            let listener = TcpListener::bind("127.0.0.1:0");
            assert_eq!(
                listener.is_ok(),
                allowed,
                "local binding disagrees with policy"
            );
            let host = endpoint
                .to_str()
                .expect("address")
                .parse()
                .expect("address");
            assert!(
                std::net::TcpStream::connect_timeout(&host, Duration::from_millis(500)).is_err(),
                "direct egress reached host loopback"
            );
        }
        "unix" => {
            use std::os::unix::net::UnixStream;
            let mut granted =
                UnixStream::connect(PathBuf::from(endpoint)).expect("authorized Unix endpoint");
            granted
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("read bound");
            granted.write_all(b"ping").expect("Unix request");
            let mut reply = [0; 4];
            granted.read_exact(&mut reply).expect("Unix reply");
            assert_eq!(&reply, b"pong");
            assert!(
                UnixStream::connect(PathBuf::from(
                    std::env::var_os(DENIED_SOCKET).expect("denied socket")
                ))
                .is_err(),
                "ambient Unix socket escaped policy"
            );
        }
        _ => panic!("unknown workload"),
    }
    println!("native network policy observed");
}

#[test]
fn linux_local_binding_is_explicit_and_never_opens_direct_host_egress() {
    let _serial = NATIVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let host = TcpListener::bind("127.0.0.1:0").expect("host canary");
    host.set_nonblocking(true).expect("host nonblocking");
    let endpoint = host.local_addr().expect("host address").to_string();
    for (allowed, action) in [(false, "bind-denied"), (true, "bind-allowed")] {
        let (status, output, errors) = finish(fixture.start(
            domains(false, allowed, Vec::new()),
            workload(action, OsStr::new(&endpoint), OsStr::new("")),
        ));
        assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
        assert!(String::from_utf8_lossy(&output).contains("native network policy observed"));
        assert_eq!(
            host.accept()
                .expect_err("direct host canary untouched")
                .kind(),
            std::io::ErrorKind::WouldBlock
        );
    }
}

#[test]
fn linux_projects_only_the_explicit_unix_socket_from_a_writable_workspace() {
    let _serial = NATIVE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture = Fixture::new();
    let outside = Fixture::new();
    let path = fixture.0.join("granted.sock");
    let denied_path = outside.0.join("denied.sock");
    let host = UnixListener::bind(&path).expect("granted socket");
    host.set_nonblocking(true).expect("nonblocking");
    let denied = UnixListener::bind(&denied_path).expect("ambient socket");
    denied.set_nonblocking(true).expect("nonblocking");
    let origin = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut stream = loop {
            match host.accept() {
                Ok((stream, _)) => break stream,
                Err(error)
                    if error.kind() == std::io::ErrorKind::WouldBlock
                        && Instant::now() < deadline =>
                {
                    std::thread::sleep(Duration::from_millis(5));
                }
                other => panic!("authorized Unix request did not arrive: {other:?}"),
            }
        };
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("timeout");
        let mut bytes = [0; 4];
        stream.read_exact(&mut bytes).expect("Unix request");
        assert_eq!(&bytes, b"ping");
        stream.write_all(b"pong").expect("Unix response");
    });
    let (status, output, errors) = finish(fixture.start(
        domains(false, false, vec![path.clone()]),
        workload("unix", path.as_os_str(), denied_path.as_os_str()),
    ));
    origin.join().expect("Unix canary");
    assert!(status.success(), "{}", String::from_utf8_lossy(&errors));
    assert!(String::from_utf8_lossy(&output).contains("native network policy observed"));
    assert!(path.exists(), "projection removed the host-owned socket");
    assert_eq!(
        denied
            .accept()
            .expect_err("ambient socket untouched")
            .kind(),
        std::io::ErrorKind::WouldBlock
    );
}
