//! What `crucible mcp`, `crucible extensions list` and `crucible sessions
//! list` answer, and what they leave alone while answering.
//!
//! Each is a list somebody reads *before* deciding whether to run what is on
//! it, so each is watched for the one thing it must never do: start what it
//! lists, or write to what it reads. The built binary is run in a directory
//! and a home of the test's own, with an environment cleared down to what it
//! needs, so nothing the machine running this keeps is read or written.
//!
//! A server is written down whose whole job is to leave a file behind when it
//! is started, and the lists are run twice: once with no process to spare,
//! where Linux refuses every thread and child the process asks for, so a list
//! that started anything would fail to answer, and once with no limit at all,
//! where a list that started the server would leave its file behind.
//!
//! Sessions are recorded by the session crate itself, asked something no list
//! may repeat, and the home they are kept in is compared byte for byte before
//! and after the list, so a list that resumed, appended to or indexed one would
//! show.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A work directory and a home under the system temporary directory, removed
/// when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crucible-read-only-lists-{probe}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("work")).expect("a temporary directory");
        fs::create_dir_all(path.join("home").join(".crucible")).expect("a temporary directory");
        Self(path)
    }

    fn work(&self) -> PathBuf {
        self.0.join("work")
    }

    fn home(&self) -> PathBuf {
        self.0.join("home")
    }

    /// Crucible's own directory, the one `CRUCIBLE_CODE_HOME` names.
    fn crucible(&self) -> PathBuf {
        self.home().join(".crucible")
    }

    /// The file a started server leaves behind.
    fn marker(&self) -> PathBuf {
        self.0.join("started")
    }

    fn configure(&self, document: &str) {
        fs::write(self.crucible().join("config.json"), document).expect("a home config");
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// What the built binary answers to `args` in `scratch`, with no process to
/// spare where `limited`.
///
/// The shell lowers the limit and then replaces itself with the program, so
/// the limit is in force before the program's first instruction.
fn asked(scratch: &Scratch, args: &[&str], limited: bool) -> Output {
    let crucible = env!("CARGO_BIN_EXE_crucible");
    let mut command = if limited {
        let mut shell = Command::new("bash");
        shell
            .arg("-c")
            .arg(r#"ulimit -u 0 && exec "$0" "$@""#)
            .arg(crucible);
        shell
    } else {
        Command::new(crucible)
    };
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", scratch.home())
        .env("CRUCIBLE_CODE_HOME", scratch.crucible())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .current_dir(scratch.work())
        .stdin(Stdio::null());
    command.output().expect("the built binary runs")
}

/// Fails the calling test unless a process limit of none binds whoever runs
/// it: a child that starts with no limit is refused under one.
fn assert_bound(scratch: &Scratch) {
    let run = |limited: bool| {
        let mut command = if limited {
            let mut shell = Command::new("bash");
            shell.arg("-c").arg("ulimit -u 0 && exec timeout 5 true");
            shell
        } else {
            let mut timeout = Command::new("timeout");
            timeout.args(["5", "true"]);
            timeout
        };
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .current_dir(scratch.work())
            .stdin(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    assert!(
        run(false),
        "`timeout 5 true` failed with no process limit, so a refusal under one would prove nothing"
    );
    assert!(
        !run(true),
        "a child started under a process limit of none, so this user is not bound by it and a \
         list that started a server could not be told from one that did not"
    );
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

fn said(answered: &Output) -> String {
    format!(
        "{}\n--- stdout ---\n{}\n--- stderr ---\n{}",
        answered.status,
        String::from_utf8_lossy(&answered.stdout),
        String::from_utf8_lossy(&answered.stderr)
    )
}

/// A home that writes down one server, which leaves `marker` behind the moment
/// anything starts it.
fn starting(scratch: &Scratch) {
    let marker = scratch.marker();
    scratch.configure(&format!(
        r#"{{"mcp":{{"servers":{{"probe":{{"command":"/bin/sh","args":["-c","touch {}"]}}}}}}}}"#,
        marker.display()
    ));
}

#[test]
fn neither_mcp_list_nor_mcp_get_starts_the_server_it_describes() {
    let scratch = Scratch::new("denied-launch");
    assert_bound(&scratch);
    starting(&scratch);
    let before = tree(&scratch.home());

    for args in [&["mcp", "list"][..], &["mcp", "get", "probe"][..]] {
        for limited in [true, false] {
            let answered = asked(&scratch, args, limited);
            assert!(
                answered.status.success(),
                "`crucible {}` (limited: {limited}) did not answer: {}",
                args.join(" "),
                said(&answered)
            );
            assert!(
                String::from_utf8_lossy(&answered.stdout).contains("probe"),
                "`crucible {}` did not name the server: {}",
                args.join(" "),
                said(&answered)
            );
            assert!(
                !scratch.marker().exists(),
                "`crucible {}` (limited: {limited}) started the server it describes",
                args.join(" ")
            );
        }
    }
    assert_eq!(tree(&scratch.home()), before, "a list wrote to the home");
}

#[test]
fn a_server_nobody_wrote_down_is_a_failure_naming_the_ones_that_were() {
    let scratch = Scratch::new("unknown-server");
    starting(&scratch);

    let answered = asked(&scratch, &["mcp", "get", "nothing-here"], false);

    assert_eq!(answered.status.code(), Some(1), "{}", said(&answered));
    assert!(answered.stdout.is_empty(), "{}", said(&answered));
    assert_eq!(
        String::from_utf8_lossy(&answered.stderr),
        "crucible: no mcp server called nothing-here; this configuration has probe\n"
    );
    assert!(!scratch.marker().exists());
}

/// A secret written in every place a server's record can hold one.
const SECRET: &str = "swordfish-sentinel";

/// A secret with nothing around it to say so.
const TOKEN: &str = "Zq7Sentinel0451Secret9Kx";

#[test]
fn no_secret_a_record_holds_reaches_either_stream() {
    let scratch = Scratch::new("redacted");
    scratch.configure(&format!(
        r#"{{"mcp": {{"servers": {{"docs": {{
            "command": "docs-mcp",
            "args": [
                "--api-key", "{SECRET}",
                "--password={SECRET}",
                "-H", "Authorization: Bearer {SECRET}",
                "https://someone:{SECRET}@mcp.example.test/sse",
                "https://mcp.example.test/sse?token={SECRET}",
                "{TOKEN}"
            ],
            "env": {{"DOCS_TOKEN": "{SECRET}"}}
        }}}}}}}}"#
    ));

    for args in [&["mcp", "list"][..], &["mcp", "get", "docs"][..]] {
        let answered = asked(&scratch, args, false);
        let both = said(&answered);

        assert!(answered.status.success(), "{both}");
        assert!(!both.contains(SECRET) && !both.contains(TOKEN), "{both}");
    }
    let described = asked(&scratch, &["mcp", "get", "docs"], false);
    let described = String::from_utf8_lossy(&described.stdout);
    assert!(described.contains("DOCS_TOKEN=<redacted>"), "{described}");
    assert!(
        described.contains("https://<redacted>@mcp.example.test/sse"),
        "{described}"
    );
}

/// Installs an extension under the home whose manifest says something a
/// terminal would act on, and one whose manifest does not read.
fn installing(scratch: &Scratch) {
    let at = scratch.crucible().join("extensions");
    fs::create_dir_all(at.join("reviewer")).expect("an extension directory");
    fs::write(
        at.join("reviewer").join("manifest.json"),
        r#"{
  "id": "acme.reviewer",
  "version": "1.4.0\u001b[2J\nacme.trusted 9.9.9",
  "protocol": "1.0",
  "entrypoint": "bin/reviewer",
  "minimumCrucible": "0.34.0",
  "capabilities": ["registerTools"],
  "contributions": ["tools"]
}"#,
    )
    .expect("a manifest");
    fs::create_dir_all(at.join("broken")).expect("an extension directory");
    fs::write(at.join("broken").join("manifest.json"), "{\u{1b}[31m").expect("a manifest");
}

#[test]
fn extensions_list_and_the_flag_it_replaces_print_the_same_list_and_start_nothing() {
    let scratch = Scratch::new("extensions");
    assert_bound(&scratch);
    installing(&scratch);
    let before = tree(&scratch.home());

    let listed = asked(&scratch, &["extensions", "list"], true);
    let flagged = asked(&scratch, &["--extensions"], true);

    assert!(listed.status.success(), "{}", said(&listed));
    assert!(flagged.status.success(), "{}", said(&flagged));
    assert_eq!(listed.stdout, flagged.stdout);
    assert!(listed.stderr.is_empty() && flagged.stderr.is_empty());

    let text = String::from_utf8_lossy(&listed.stdout);
    assert!(text.contains("acme.reviewer 1.4.0"), "{text}");
    assert!(!text.contains('\u{1b}'), "{text:?}");
    assert!(
        !text.lines().any(|line| line.starts_with("acme.trusted")),
        "{text}"
    );
    assert_eq!(tree(&scratch.home()), before, "a list wrote to the home");
}

/// What a recorded session was asked, which no list may say.
const PROMPT: &str = "prompt-sentinel-never-listed";

/// A session recorded in `root`, asked [`PROMPT`], with `title` saved over it
/// where there is one.
fn recorded(scratch: &Scratch, root: &Path, title: Option<&str>) -> String {
    let logs = scratch.crucible().join("sessions");
    let workspace = crucible_workspace::Workspace::open(root).expect("a workspace");
    let session =
        crucible_session::Session::start(&logs, &workspace, Some("main")).expect("a new session");
    session.append(&crucible_types::Message::said(PROMPT));
    let id = session.id().expect("a recorded session").clone();
    drop(session);
    if let Some(title) = title {
        crucible_session::retitle(&logs, &id, title).expect("a saved title");
    }
    id.as_str().to_owned()
}

/// The one document `answered` wrote, read.
fn document(answered: &Output) -> serde_json::Value {
    let written = answered
        .stdout
        .strip_suffix(b"\n")
        .unwrap_or_else(|| panic!("one line ending in a newline: {}", said(answered)));
    assert!(!written.contains(&b'\n'), "{}", said(answered));
    serde_json::from_slice(written).unwrap_or_else(|_| panic!("JSON: {}", said(answered)))
}

/// What a document holds at `pointer`, or a failure saying it holds nothing
/// there.
fn field<'a>(document: &'a serde_json::Value, pointer: &str) -> &'a serde_json::Value {
    document
        .pointer(pointer)
        .unwrap_or_else(|| panic!("nothing at {pointer} in {document}"))
}

#[test]
fn sessions_list_says_what_was_recorded_here_and_changes_nothing() {
    let scratch = Scratch::new("sessions");
    let elsewhere = scratch.0.join("elsewhere");
    fs::create_dir_all(&elsewhere).expect("another directory");
    let titled = recorded(&scratch, &scratch.work(), Some("fix the parser"));
    let plain = recorded(&scratch, &scratch.work(), None);
    let other = recorded(&scratch, &elsewhere, None);
    let before = tree(&scratch.home());

    let text = asked(&scratch, &["sessions", "list"], false);
    let json = asked(&scratch, &["sessions", "list", "--json"], false);

    assert_eq!(tree(&scratch.home()), before, "a list wrote to the home");
    for answered in [&text, &json] {
        assert!(answered.status.success(), "{}", said(answered));
        assert!(answered.stderr.is_empty(), "{}", said(answered));
        let both = said(answered);
        assert!(!both.contains(PROMPT), "{both}");
        assert!(!both.contains(&other), "{both}");
    }
    let shown = String::from_utf8_lossy(&text.stdout);
    assert!(shown.starts_with("2 sessions recorded for "), "{shown}");
    assert!(
        shown.contains(&format!(
            "  {titled}  just now  1 message  on main  fix the parser\n"
        )),
        "{shown}"
    );
    assert!(
        shown.contains(&format!(
            "  {plain}  just now  1 message  on main  untitled\n"
        )),
        "{shown}"
    );

    let document = document(&json);
    assert_eq!(field(&document, "/format_version"), 1);
    assert_eq!(field(&document, "/kind"), "sessions");
    assert_eq!(field(&document, "/status"), "complete");
    assert_eq!(field(&document, "/truncated"), false);
    assert_eq!(field(&document, "/omitted"), 0);
    let ids: Vec<&str> = field(&document, "/sessions")
        .as_array()
        .expect("a list of sessions")
        .iter()
        .map(|one| field(one, "/id").as_str().expect("an id"))
        .collect();
    assert_eq!(ids, [plain.as_str(), titled.as_str()]);
    assert_eq!(field(&document, "/sessions/1/title/text"), "fix the parser");
    assert_eq!(field(&document, "/sessions/1/branch/text"), "main");
}

#[test]
fn a_session_directory_with_no_index_is_listed_as_incomplete() {
    let scratch = Scratch::new("sessions-unindexed");
    fs::create_dir_all(scratch.crucible().join("sessions")).expect("a session directory");
    let before = tree(&scratch.home());

    let json = asked(&scratch, &["sessions", "list", "--json"], false);

    assert!(json.status.success(), "{}", said(&json));
    let document = document(&json);
    assert_eq!(field(&document, "/status"), "incomplete");
    assert_eq!(field(&document, "/unindexed"), true);
    assert_eq!(tree(&scratch.home()), before, "a list wrote an index");
}

#[test]
fn a_session_index_that_does_not_read_fails_without_quoting_it_or_naming_it_in_the_document() {
    let scratch = Scratch::new("sessions-malformed");
    let logs = scratch.crucible().join("sessions");
    fs::create_dir_all(&logs).expect("a session directory");
    fs::write(logs.join("recent.sessions"), format!("{PROMPT}\n")).expect("an index");

    let json = asked(&scratch, &["sessions", "list", "--json"], false);
    let text = asked(&scratch, &["sessions", "list"], false);

    assert_eq!(json.status.code(), Some(1), "{}", said(&json));
    let document = document(&json);
    assert_eq!(field(&document, "/format_version"), 1);
    assert_eq!(field(&document, "/kind"), "sessions");
    assert_eq!(field(&document, "/status"), "failed");
    assert_eq!(
        field(&document, "/problem/text"),
        "the session index could not be read; standard error says why"
    );
    assert!(
        !String::from_utf8_lossy(&json.stdout).contains(&*scratch.0.to_string_lossy()),
        "{}",
        said(&json)
    );

    assert_eq!(text.status.code(), Some(1), "{}", said(&text));
    assert!(text.stdout.is_empty(), "{}", said(&text));
    for answered in [&json, &text] {
        let stderr = String::from_utf8_lossy(&answered.stderr);
        assert!(
            stderr.starts_with("crucible: could not use the session index "),
            "{stderr}"
        );
        assert!(!said(answered).contains(PROMPT), "{}", said(answered));
    }
}

#[test]
fn a_sessions_list_asked_with_what_it_does_not_take_is_usage() {
    let scratch = Scratch::new("sessions-usage");

    for args in [
        &["sessions", "list", "extra"][..],
        &["sessions", "list", "--resume", "some-id"][..],
        &["--continue", "sessions", "list"][..],
        &["sessions"][..],
    ] {
        let answered = asked(&scratch, args, false);
        assert_eq!(
            answered.status.code(),
            Some(2),
            "{args:?}: {}",
            said(&answered)
        );
        assert!(answered.stdout.is_empty(), "{args:?}: {}", said(&answered));
    }
}
