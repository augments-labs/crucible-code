//! The prompt a key is typed at, on a real terminal, ended from outside.
//!
//! Hiding what is typed is a mode put on the terminal, and a mode outlives the
//! process that set it: a login ended by a `kill` or a hang-up while the prompt
//! waits must hand the terminal back as it found it, echoing and reading whole
//! lines, before it ends the way the signal ends a process.
//!
//! Linux only, for the reason the whole-screen tests give: the child is
//! started through `setsid --ctty`, from util-linux, so the pseudo terminal
//! opened here is its controlling terminal without `unsafe` in this tree.

use std::ffi::OsStr;
use std::fs::{File, OpenOptions};
use std::io::Read as _;
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use rustix::pty::{self, OpenptFlags};
use rustix::termios::{self, LocalModes};

use super::{Scratch, Sentinel, confined, tree};

/// What the prompt says once the terminal hides what is typed at it.
const HIDDEN: &str = "API key (it does not show";

/// How long one step may take before the run is called stuck.
const CEILING: Duration = Duration::from_secs(20);

/// A pseudo terminal pair, near side first, the far side in the mode a new
/// terminal opens in.
fn opened() -> (File, File) {
    let near = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .expect("a pseudo terminal");
    pty::grantpt(&near).expect("the far side is ours");
    pty::unlockpt(&near).expect("the far side is unlocked");
    let named = pty::ptsname(&near, Vec::new()).expect("the far side has a name");
    let far = OpenOptions::new()
        .read(true)
        .write(true)
        .open(OsStr::from_bytes(named.as_bytes()))
        .expect("the far side opens");
    (File::from(near), far)
}

/// Whether the terminal echoes what is typed, and whether it reads whole
/// lines: the two things a hidden prompt takes away.
fn cooked(near: &File) -> (bool, bool) {
    let mode = termios::tcgetattr(near).expect("the terminal has a mode");
    (
        mode.local_modes.contains(LocalModes::ECHO),
        mode.local_modes.contains(LocalModes::ICANON),
    )
}

#[test]
fn a_signal_at_the_hidden_key_prompt_hands_the_terminal_back_before_the_process_ends() {
    for (signal, number) in [("TERM", 15), ("HUP", 1)] {
        let scratch = Scratch::new(&format!("hidden-{signal}"));
        let sentinel = Sentinel::new();
        let before = tree(scratch.root());
        let (near, far) = opened();
        assert_eq!(
            cooked(&near),
            (true, true),
            "{signal}: the terminal began hidden"
        );

        let mut command = Command::new("setsid");
        command
            .arg("--ctty")
            .arg(env!("CARGO_BIN_EXE_crucible"))
            .args(["auth", "login", "anthropic"]);
        confined(&mut command, &scratch, &sentinel, &[]);
        let mut child = command
            .stdin(Stdio::from(
                far.try_clone().expect("a handle on the far side"),
            ))
            .stdout(Stdio::from(
                far.try_clone().expect("a handle on the far side"),
            ))
            .stderr(Stdio::from(far))
            .spawn()
            .expect("setsid --ctty starts crucible; util-linux provides it");
        // The command holds its copies of the far side until it goes, and
        // while anything here holds one the reader below never sees the end.
        drop(command);

        // Read on a thread, which ends when the last handle on the far side
        // closes: the process ending, with no clock deciding that it has.
        let (sender, bytes) = mpsc::channel();
        let mut reading = near.try_clone().expect("a second handle on the terminal");
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 4096];
            while let Ok(read) = reading.read(&mut buffer) {
                let Some(more) = buffer.get(..read).filter(|more| !more.is_empty()) else {
                    break;
                };
                if sender.send(more.to_vec()).is_err() {
                    break;
                }
            }
        });

        let deadline = Instant::now() + CEILING;
        let mut said = Vec::new();
        while !String::from_utf8_lossy(&said).contains(HIDDEN) {
            match bytes.recv_timeout(Duration::from_millis(100)) {
                Ok(more) => said.extend_from_slice(&more),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    panic!("{signal}: crucible ended before it asked for the key")
                }
            }
            assert!(
                Instant::now() < deadline,
                "{signal}: the prompt never stood"
            );
        }
        assert_eq!(
            cooked(&near),
            (false, false),
            "{signal}: the prompt stood on a terminal that still showed what was typed"
        );

        let sent = Command::new("kill")
            .args(["-s", signal, &child.id().to_string()])
            .status()
            .expect("kill is on the path");
        assert!(sent.success(), "{signal} never reached crucible");
        while let Ok(_) | Err(RecvTimeoutError::Timeout) =
            bytes.recv_timeout(Duration::from_millis(100))
        {
            assert!(
                Instant::now() < deadline,
                "{signal}: crucible outlived the signal"
            );
        }
        let ended = child.wait().expect("crucible ended");

        assert_eq!(
            cooked(&near),
            (true, true),
            "{signal}: the terminal was left hiding what is typed"
        );
        assert_eq!(
            ended.signal(),
            Some(number),
            "{signal}: crucible did not end the way the signal ends a process: {ended:?}"
        );
        assert_eq!(
            tree(scratch.root()),
            before,
            "{signal}: something was written"
        );
        assert!(!sentinel.heard(), "{signal}: something reached out");
    }
}
