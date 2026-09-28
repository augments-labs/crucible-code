//! The paths that answer and stop start no thread, so none of them builds the
//! application's runtime.
//!
//! `--help` and `--version` are answered while the arguments are parsed, and
//! `--extensions` and `config check` before anything a run is made of is
//! opened. Shell completion, installers and somebody checking their files reach
//! for them, and `bench-cli-exit` holds the first two to a startup budget. The
//! runtime is built the first time a run asks for it, and building it starts
//! its threads, so a path that built it on the way would start threads.
//!
//! That is what is watched: each path is run with no thread to spare. Linux
//! counts every thread a user has against that user's process limit, and a
//! limit of none refuses every thread and child the process asks for, so a
//! path that built the runtime would fail to start it rather than answer. Only
//! Linux counts threads there. A user the limit does not bind, such as root,
//! fails the check each test makes first instead of passing every path
//! unobserved.

#![cfg(target_os = "linux")]
// Test-only helpers fail the owning case when its controlled fixture is invalid.
#![allow(clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// A directory of this test's own, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(case: &str) -> Self {
        let path =
            std::env::temp_dir().join(format!("crucible-fast-paths-{case}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("a temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// Runs `program` with `args` under a process limit of none.
///
/// The shell lowers the limit and then replaces itself with the program, so
/// the limit is in force before the program's first instruction and nothing
/// between the two asks for a thread. The environment is built from nothing,
/// with a home of the test's own, so no person's configuration is read.
fn threadless(home: &Path, program: &Path, args: &[&str]) -> Output {
    Command::new("bash")
        .arg("-c")
        .arg(r#"ulimit -u 0 && exec "$0" "$@""#)
        .arg(program)
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("CRUCIBLE_CODE_HOME", home.join(".crucible"))
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .current_dir(home)
        .stdin(Stdio::null())
        .output()
        .expect("bash runs")
}

/// Fails the calling test unless the limit binds whoever runs it: a program
/// that asks for one child starts it with no limit, and is refused under it.
///
/// `timeout` starts its command as a child and gives up at once when it
/// cannot, where a shell may retry a refused child for seconds first. Run
/// without the limit first, because a `timeout` that is missing or broken
/// fails under the limit too, and would pass for a refusal.
fn assert_bound(home: &Path) {
    let free = Command::new("timeout")
        .args(["5", "true"])
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .current_dir(home)
        .stdin(Stdio::null())
        .status();
    assert!(
        free.is_ok_and(|status| status.success()),
        "`timeout 5 true` failed with no process limit, so a refusal under one would prove \
         nothing here"
    );

    let forked = threadless(home, Path::new("timeout"), &["5", "true"]);

    assert!(
        !forked.status.success(),
        "a child started under a process limit of none, so this user is not bound by it and \
         the fast paths cannot be watched here"
    );
}

#[test]
fn a_process_limit_of_none_refuses_a_child_here() {
    let scratch = Scratch::new("control");

    assert_bound(scratch.path());
}

#[test]
fn every_fast_path_answers_without_starting_a_thread() {
    let scratch = Scratch::new("paths");
    let crucible = Path::new(env!("CARGO_BIN_EXE_crucible"));
    assert_bound(scratch.path());

    for (args, answer) in [
        (&["--help"][..], "Usage: crucible"),
        (&["--version"][..], env!("CARGO_PKG_VERSION")),
        (&["--extensions"][..], "no extensions in"),
        (&["config", "check"][..], "configuration valid"),
    ] {
        let answered = threadless(scratch.path(), crucible, args);
        let said = String::from_utf8_lossy(&answered.stdout);

        assert!(
            answered.status.success() && said.contains(answer),
            "`crucible {}` needed a thread to answer: {}\n--- stdout ---\n{said}\n--- stderr ---\n{}",
            args.join(" "),
            answered.status,
            String::from_utf8_lossy(&answered.stderr),
        );
    }
}
