//! What `crucible doctor` answers on this machine, and the exit each answer
//! ends with.
//!
//! The built binary is run in a directory and a home of the test's own, with
//! an environment cleared down to what it needs and every proxy pointed at a
//! loopback listener of the test's own, so nothing the machine running this
//! keeps is read, nothing is written anywhere a test did not make, and any
//! attempt to reach out lands where the test can hear it.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::io::ErrorKind;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crucible_client_api::doctor::{KIND, Report};

/// A directory under the system temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-doctor-{probe}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("work")).expect("a temporary directory");
        fs::create_dir_all(path.join("home")).expect("a temporary directory");
        Self(path)
    }

    fn work(&self) -> PathBuf {
        self.0.join("work")
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A loopback listener nothing should ever connect to.
struct Sentinel(TcpListener);

impl Sentinel {
    fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback listener");
        listener
            .set_nonblocking(true)
            .expect("a listener that can be polled");
        Self(listener)
    }

    fn url(&self) -> String {
        format!(
            "http://{}",
            self.0.local_addr().expect("the listener's address")
        )
    }

    /// Whether anything connected since it was made.
    fn heard(&self) -> bool {
        match self.0.accept() {
            Ok(_) => true,
            Err(error) if error.kind() == ErrorKind::WouldBlock => false,
            Err(error) => panic!("the listener failed: {error}"),
        }
    }
}

/// What the built binary answers to `args`, run in `scratch` with every proxy
/// pointed at `sentinel`, with crucible's home named outright unless
/// `homeless`.
fn asked(scratch: &Scratch, sentinel: &Sentinel, args: &[&str], homeless: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crucible"));
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .current_dir(scratch.work())
        .stdin(Stdio::null());
    for proxy in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
    ] {
        command.env(proxy, sentinel.url());
    }
    if !homeless {
        command
            .env("HOME", scratch.home())
            .env("CRUCIBLE_CODE_HOME", scratch.home().join(".crucible"));
    }
    command.output().expect("the built binary runs")
}

/// Every entry under `root`, with its bytes where it is a file.
fn tree(root: &Path) -> BTreeMap<PathBuf, Option<Vec<u8>>> {
    let mut seen = BTreeMap::new();
    let mut left = vec![root.to_path_buf()];
    while let Some(directory) = left.pop() {
        for entry in fs::read_dir(&directory).expect("a directory this test made") {
            let at = entry.expect("an entry").path();
            if at.is_dir() {
                seen.insert(at.clone(), None);
                left.push(at);
            } else {
                seen.insert(at.clone(), Some(fs::read(&at).expect("its bytes")));
            }
        }
    }
    seen
}

/// The report `answered` wrote as one JSON line, checked against its exit.
fn reported(answered: &Output) -> Report {
    let line = answered
        .stdout
        .strip_suffix(b"\n")
        .expect("one document, ending in a newline");
    assert!(!line.contains(&b'\n'), "more than one line");
    let written = String::from_utf8_lossy(line);
    assert!(
        written.contains(&format!(r#""kind":"{KIND}""#)),
        "{written}"
    );
    let report = Report::decode(&answered.stdout).expect("a document that reads back");
    assert_eq!(
        answered.status.code(),
        Some(i32::from(report.exit())),
        "{written}"
    );
    report
}

#[test]
fn both_forms_report_every_check_and_exit_as_the_report_says_writing_nothing() {
    let scratch = Scratch::new("report");
    let sentinel = Sentinel::new();
    let before = tree(&scratch.0);

    let json = asked(&scratch, &sentinel, &["doctor", "--json"], false);
    assert!(json.stderr.is_empty(), "{json:?}");
    let report = reported(&json);
    let ids: Vec<&str> = report
        .checks
        .iter()
        .map(|check| check.id.as_str())
        .collect();
    assert!(ids.contains(&"config"), "{ids:?}");
    assert!(ids.contains(&"credentials"), "{ids:?}");
    assert!(ids.contains(&"sandbox-backend"), "{ids:?}");
    assert!(ids.contains(&"mcp"), "{ids:?}");

    let text = asked(&scratch, &sentinel, &["doctor"], false);
    assert!(text.stderr.is_empty(), "{text:?}");
    assert_eq!(text.status.code(), json.status.code(), "{text:?}");
    let said = String::from_utf8_lossy(&text.stdout);
    assert!(said.starts_with("crucible doctor: "), "{said}");
    for id in &ids {
        assert!(said.contains(&format!(" {id}: ")), "{id}: {said}");
    }

    // No path reaches either form, not even the directory asked about.
    let under = scratch.0.to_string_lossy();
    for written in [String::from_utf8_lossy(&json.stdout), said] {
        assert!(!written.contains(&*under), "{written}");
    }
    // Nothing was written, and nothing reached out.
    assert_eq!(tree(&scratch.0), before);
    assert!(!sentinel.heard());
}

#[test]
fn a_configuration_that_does_not_read_is_a_failure_reported_not_a_run_that_failed() {
    let scratch = Scratch::new("unreadable-config");
    let sentinel = Sentinel::new();
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    fs::write(
        scratch.work().join(".crucible/config.json"),
        r#"{"not_a_setting":true}"#,
    )
    .expect("a project file");

    let json = asked(&scratch, &sentinel, &["doctor", "--json"], false);
    assert!(json.stderr.is_empty(), "{json:?}");
    let report = reported(&json);
    assert_eq!(report.exit(), 2);
    let status = |id: &str| {
        report
            .checks
            .iter()
            .find(|check| check.id.as_str() == id)
            .map(|check| check.status.as_str())
    };
    assert_eq!(status("config"), Some("failed"));
    assert_eq!(status("provider"), Some("unavailable"));
    // What needs no configuration was still looked at.
    assert_eq!(status("home"), Some("ok"));

    let text = asked(&scratch, &sentinel, &["doctor"], false);
    assert_eq!(text.status.code(), Some(2), "{text:?}");
    assert!(text.stderr.is_empty(), "{text:?}");
    assert!(!sentinel.heard());
}

#[test]
fn with_no_home_the_report_still_comes_and_says_so() {
    let scratch = Scratch::new("homeless");
    let sentinel = Sentinel::new();
    let json = asked(&scratch, &sentinel, &["doctor", "--json"], true);
    assert!(json.stderr.is_empty(), "{json:?}");
    let report = reported(&json);
    assert_eq!(report.exit(), 2);
    let home = report
        .checks
        .iter()
        .find(|check| check.id.as_str() == "home")
        .expect("the home check");
    assert_eq!(home.status.as_str(), "failed");
    assert!(!sentinel.heard());
}

#[test]
fn a_command_line_that_does_not_parse_is_a_usage_error() {
    let scratch = Scratch::new("usage");
    let sentinel = Sentinel::new();
    for args in [
        &["doctor", "--bogus"][..],
        &["doctor", "--json", "extra"],
        &["doctor", "doctor"],
    ] {
        let answered = asked(&scratch, &sentinel, args, false);
        assert_eq!(answered.status.code(), Some(2), "{args:?}: {answered:?}");
        assert!(answered.stdout.is_empty(), "{answered:?}");
    }
}
