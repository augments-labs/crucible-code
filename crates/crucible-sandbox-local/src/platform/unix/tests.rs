//! A command's process group stopped while one of its members forks.
//!
//! Each command forks without pause, so the group kill is sent with a fork
//! under way most of the time, and what that fork leaves is a `sleep` far
//! longer than any killed process takes to go. The command's output pipe tells
//! them apart: it closes once nothing in the group is left running to hold it.

use std::process::Stdio;
use std::thread;
use std::time::{Duration, Instant};

use super::*;

/// Trials per test, so that a kill racing no fork is not what passes one.
const TRIALS: usize = 25;

/// How long a killed group has to let go of the output pipe: far longer than
/// a killed process takes, far shorter than the sleep an escaped fork runs.
const GONE: Duration = Duration::from_secs(1);

/// How long a leader that exits at once may take to be seen to.
const WAIT: Duration = Duration::from_secs(10);

/// A shell at the head of its own group, running `script` with its output on
/// a pipe.
fn spawned(script: &str) -> (Scope, Child, Terminator) {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let scope = Scope::new(&mut command);
    let child = command.spawn().expect("the platform's shell");
    let group = scope.terminator(&child).expect("the command's group");
    (scope, child, group)
}

/// Whether the command's output closed within [`GONE`] of its leader being
/// reaped. A process still holding it is killed, which is safe because a
/// group with a living member keeps its number.
fn released(child: &mut Child, group: Terminator) -> bool {
    let mut output = child.stdout.take().expect("a piped output");
    prepare(&output).expect("a non-blocking output");
    let deadline = Instant::now() + GONE;
    let mut buffer = [0; 64];
    loop {
        if matches!(read(&mut output, &mut buffer), Ok(ReadState::End)) {
            return true;
        }
        if Instant::now() >= deadline {
            let _ = group.stop();
            return false;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_fork_under_way_when_the_leader_exits_is_stopped_before_the_leader_is_reaped() {
    for trial in 0..TRIALS {
        let (scope, mut child, group) = spawned("(while :; do sleep 5 & done) & echo done");
        let deadline = Instant::now() + WAIT;
        while scope.try_wait(&mut child, group).expect("a look").is_none() {
            assert!(Instant::now() < deadline, "trial {trial}: never reaped");
            thread::sleep(Duration::from_millis(5));
        }

        assert!(
            released(&mut child, group),
            "trial {trial}: a process forked as the group was killed outlived it"
        );
    }
}

#[test]
fn a_fork_under_way_when_the_command_is_stopped_is_stopped_before_the_leader_is_reaped() {
    for trial in 0..TRIALS {
        let (_scope, mut child, group) = spawned("while :; do sleep 5 & done");
        thread::sleep(Duration::from_millis(20));
        Scope::stop(&mut child).expect("the stop");
        child.wait().expect("the killed leader");

        assert!(
            released(&mut child, group),
            "trial {trial}: a process forked as the group was killed outlived it"
        );
    }
}
