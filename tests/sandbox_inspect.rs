//! What `crucible sandbox inspect` and `--sandbox` answer on this machine, and
//! the exit each answer ends with.
//!
//! The built binary is run in a directory and a home of the test's own, with
//! an environment cleared down to what it needs, so nothing the machine running
//! this keeps is read and nothing is written anywhere a test did not make.

#![allow(clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use crucible_client_api::inspection::{Inspection, KIND};

/// A directory under the system temporary directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crucible-sandbox-inspect-{probe}-{}",
            std::process::id()
        ));
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

/// What the built binary answers to `args`, run in `scratch`, with crucible's
/// home named outright unless `homeless`.
fn asked(scratch: &Scratch, args: &[&str], homeless: bool) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crucible"));
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .current_dir(scratch.work())
        .stdin(Stdio::null());
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

#[test]
fn the_alias_and_the_command_write_one_report_and_succeed() {
    let scratch = Scratch::new("report");
    fs::create_dir_all(scratch.work().join(".crucible")).expect("a project directory");
    fs::write(
        scratch.work().join(".crucible/config.json"),
        r#"{"sandbox":{"enabled":true}}"#,
    )
    .expect("a project file");
    let before = tree(&scratch.0);

    // Whatever this machine can confine with, and whether or not it would
    // take the policy, a report was made, and a report made is a success.
    let alias = asked(&scratch, &["--sandbox"], false);
    let command = asked(&scratch, &["sandbox", "inspect"], false);
    for answered in [&alias, &command] {
        assert_eq!(answered.status.code(), Some(0), "{answered:?}");
        assert!(answered.stderr.is_empty(), "{answered:?}");
    }
    let said = String::from_utf8_lossy(&alias.stdout);
    assert!(said.starts_with("sandbox enabled in "), "{said}");
    assert!(
        said.contains("  mode      required by project configuration"),
        "{said}"
    );
    assert!(said.ends_with('\n'), "{said}");
    // The alias is the command, word for word.
    assert_eq!(alias.stdout, command.stdout);

    let json = asked(&scratch, &["sandbox", "inspect", "--json"], false);
    assert_eq!(json.status.code(), Some(0), "{json:?}");
    assert!(json.stderr.is_empty(), "{json:?}");
    let line = json
        .stdout
        .strip_suffix(b"\n")
        .expect("one document, ending in a newline");
    assert!(!line.contains(&b'\n'), "more than one line");
    let written = String::from_utf8_lossy(line);
    assert!(written.contains(r#""format_version":1"#), "{written}");
    assert!(
        written.contains(&format!(r#""kind":"{KIND}""#)),
        "{written}"
    );
    let read = Inspection::decode(&json.stdout).expect("a document that reads back");
    assert!(
        ["ready", "refused", "unavailable"].contains(&read.status()),
        "{written}"
    );
    // No path reaches the document, not even the one asked about.
    assert!(
        !written.contains(&*scratch.work().to_string_lossy()),
        "{written}"
    );

    // Asking three times wrote nothing, here or in crucible's home.
    assert_eq!(tree(&scratch.0), before);
}

#[test]
fn an_inspection_that_could_not_be_made_fails_in_both_forms() {
    let scratch = Scratch::new("homeless");

    // With no home to read configuration from, there is no policy to report
    // on. Text goes where a failure always goes; the document still comes, so
    // a script reading standard output is never left with nothing.
    for args in [&["--sandbox"][..], &["sandbox", "inspect"]] {
        let answered = asked(&scratch, args, true);
        assert_eq!(answered.status.code(), Some(1), "{answered:?}");
        assert!(answered.stdout.is_empty(), "{answered:?}");
        assert!(answered.stderr.starts_with(b"crucible: "), "{answered:?}");
    }

    let json = asked(&scratch, &["sandbox", "inspect", "--json"], true);
    assert_eq!(json.status.code(), Some(1), "{json:?}");
    assert!(json.stderr.starts_with(b"crucible: "), "{json:?}");
    let read = Inspection::decode(&json.stdout).expect("a document that reads back");
    assert_eq!(read.status(), "failed");
}

#[test]
fn a_command_line_that_does_not_parse_is_a_usage_error() {
    let scratch = Scratch::new("usage");
    for args in [
        &["sandbox", "inspect", "--bogus"][..],
        &["sandbox", "inspect", "--json", "extra"],
        &["--sandbox", "sandbox", "inspect"],
    ] {
        let answered = asked(&scratch, args, false);
        assert_eq!(answered.status.code(), Some(2), "{args:?}: {answered:?}");
        assert!(answered.stdout.is_empty(), "{answered:?}");
    }
}
