//! What `crucible completion SHELL` writes, and what it leaves alone doing so.
//!
//! The command is run from a shell's start-up file, so it is watched for the
//! two things a start-up file cannot afford: touching anything of the user's,
//! and failing noisily when the far end of the pipe goes away. The built binary
//! is run in a home whose configuration would be refused by any command that
//! read it, with an environment cleared down to what it needs and no terminal.
//! That the scripts agree with the command tree is held beside the tree, in
//! the binary's own tests.

#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::panic)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

const SHELLS: [&str; 5] = ["bash", "zsh", "fish", "powershell", "elvish"];

/// A work directory and a home under the system temporary directory, removed
/// when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(probe: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "crucible-completion-{probe}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("work")).expect("a temporary directory");
        fs::create_dir_all(path.join("home").join(".crucible")).expect("a temporary directory");
        Self(path)
    }

    fn crucible(&self) -> PathBuf {
        self.0.join("home").join(".crucible")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The built binary, ready to run `args` in `scratch` with nothing but a path.
fn crucible(scratch: &Scratch, args: &[&str]) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_crucible"));
    command
        .args(args)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", scratch.0.join("home"))
        .env("CRUCIBLE_CODE_HOME", scratch.crucible())
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .current_dir(scratch.0.join("work"))
        .stdin(Stdio::null());
    command
}

fn asked(scratch: &Scratch, shell: &str) -> Output {
    crucible(scratch, &["completion", shell])
        .output()
        .expect("the built binary runs")
}

/// Every entry under `root` with its bytes where it is a file.
fn tree(root: &Path) -> Vec<(PathBuf, Option<Vec<u8>>)> {
    let mut seen = Vec::new();
    let mut left = vec![root.to_path_buf()];
    while let Some(directory) = left.pop() {
        for entry in fs::read_dir(&directory).expect("a readable directory") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                left.push(path.clone());
                seen.push((path, None));
            } else {
                let bytes = fs::read(&path).expect("a readable file");
                seen.push((path, Some(bytes)));
            }
        }
    }
    seen.sort();
    seen
}

#[test]
fn completion_writes_a_script_for_each_shell_and_nothing_else() {
    let scratch = Scratch::new("each");
    for shell in SHELLS {
        let said = asked(&scratch, shell);

        assert!(said.status.success(), "{shell}: {:?}", said.status);
        assert!(said.stderr.is_empty(), "{shell} wrote to standard error");
        let script = String::from_utf8(said.stdout).expect("a script is text");
        assert!(script.contains("crucible"), "{shell} names no command");
        assert!(
            script.contains("inspect"),
            "{shell} never reaches sandbox inspect"
        );
    }
}

#[test]
fn completion_is_answered_before_the_home_is_read() {
    // A configuration no command that read it would accept, and a session
    // directory that does not exist: any command that opened either would say
    // so, and one that wrote would leave a trace in the compared tree.
    let scratch = Scratch::new("home");
    fs::write(scratch.crucible().join("config.json"), "{ not json").expect("a home config");
    let before = tree(&scratch.0);

    let said = asked(&scratch, "bash");

    assert!(said.status.success(), "{:?}", said.status);
    assert!(said.stderr.is_empty());
    assert!(!said.stdout.is_empty());
    assert_eq!(before, tree(&scratch.0), "the command wrote under its home");
}

#[test]
fn completion_is_the_same_script_every_time_it_is_asked() {
    let scratch = Scratch::new("same");
    for shell in SHELLS {
        let first = asked(&scratch, shell).stdout;

        assert!(!first.is_empty(), "{shell}: no script was written");
        assert_eq!(
            first,
            asked(&scratch, shell).stdout,
            "{shell}: a script that differs between runs would be rewritten by every new terminal"
        );
    }
}

#[test]
fn completion_refuses_a_shell_it_has_no_script_for() {
    let scratch = Scratch::new("refused");

    let said = asked(&scratch, "tcsh");

    assert_eq!(said.status.code(), Some(2), "{:?}", said.status);
    assert!(said.stdout.is_empty());
    let refusal = String::from_utf8_lossy(&said.stderr);
    assert!(
        refusal.contains("invalid value 'tcsh'") && refusal.contains("[possible values: bash,"),
        "the refusal does not list the shells there are scripts for: {refusal}"
    );
}

#[test]
fn completion_reaches_a_closed_pipe_without_panicking() {
    // `head -n1` on the far side is the ordinary way this happens. Closing the
    // read end before the child writes is the case the generator would panic
    // on, and the child is slow enough to start that the close comes first;
    // where it does not, the write succeeds and the answer is the same.
    let scratch = Scratch::new("pipe");
    for shell in SHELLS {
        let mut child = crucible(&scratch, &["completion", shell])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the built binary runs");
        drop(child.stdout.take());

        let said = child.wait_with_output().expect("the child ends");

        assert!(said.status.success(), "{shell}: {:?}", said.status);
        assert!(
            said.stderr.is_empty(),
            "{shell}: {}",
            String::from_utf8_lossy(&said.stderr)
        );
    }
}

#[test]
fn completion_reaches_a_full_disk_without_panicking() {
    let scratch = Scratch::new("full");
    let full = fs::OpenOptions::new()
        .write(true)
        .open("/dev/full")
        .expect("/dev/full");

    let said = crucible(&scratch, &["completion", "fish"])
        .stdout(Stdio::from(full))
        .stderr(Stdio::piped())
        .output()
        .expect("the built binary runs");

    assert!(said.status.success(), "{:?}", said.status);
    assert!(said.stderr.is_empty());
}
