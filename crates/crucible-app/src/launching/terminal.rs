//! A sign-in at a real terminal: the child's own process group, the stop
//! that keeps it from reading before the handover, the handover's bound and
//! the group kill.
//!
//! Everything here is Unix: a process group, signals and a foreground group
//! are the platform's, and the route is refused before a process starts where
//! they are not there.

use std::path::Path;
use std::time::Duration;

use crucible_execution::Failure;
use crucible_external_harnesses::{Program, Request};

use super::{SignInTerminal, Unpiped};

/// How long a sign-in at a real terminal may run before its whole process
/// group is killed.
///
/// A person may be reading a link, opening a browser and typing a code, so this
/// is the longer bound the design gives that route. A test build lowers it so
/// the kill can be watched rather than waited for.
#[cfg(not(feature = "proof"))]
pub(super) const TERMINAL_SIGNING_IN: Duration = Duration::from_mins(10);
#[cfg(feature = "proof")]
pub(super) const TERMINAL_SIGNING_IN: Duration = Duration::from_millis(250);

/// How often a sign-in at a real terminal asks whether its child has ended.
#[cfg(unix)]
const TERMINAL_POLL: Duration = Duration::from_millis(25);

/// How long the child is given to stop after `SIGSTOP` before the handover is
/// refused: delivery is asynchronous, and a child still running when the
/// terminal is handed over could read it before it is the foreground group.
#[cfg(unix)]
const STOPPED_BOUND: Duration = Duration::from_secs(2);

/// How a command at a real terminal ended, from the point of view of the bound.
#[cfg(unix)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum TerminalRun {
    /// It ended on its own, and whether it said success.
    Ended(bool),
    /// It was still running when its bound passed, and its group was killed.
    Overran,
}

/// How a command at a real terminal ended, and whether its whole group is
/// known gone: one whose group could not be confirmed stopped holds its
/// instance's control for good.
#[cfg(unix)]
pub(super) struct TerminalExit {
    /// What it ended as, or why its end could not be read.
    pub(super) run: Result<TerminalRun, Unpiped>,
    /// Whether the child and its group are known gone.
    pub(super) stopped: bool,
}

/// Starts `program` in `root` with `request` and `terminal`, holds it stopped
/// until the terminal is its own, waits at most `bound` for it, and kills its
/// whole process group if the bound passes.
///
/// The command inherits this process's standard input, output and error: that
/// is the real terminal the route exists for. It is put in a process group of
/// its own and stopped before [`SignInTerminal::handed`] is told its id, so the
/// front end can make that group the terminal's foreground group, and the
/// child is resumed only once the handover has succeeded: a child that reads
/// the terminal before then cannot lose what it reads, and one the kernel
/// stopped with `SIGTTIN` before the stop was delivered is resumed too. A
/// handover that fails kills the group rather than run the child with this
/// process's group in the foreground.
///
/// # Errors
///
/// [`TerminalExit::run`] where the command could not be started, held, handed
/// over, resumed or waited for.
#[cfg(unix)]
pub(super) fn ran_at_terminal(
    program: &Program,
    root: &Path,
    request: &Request,
    bound: Duration,
    terminal: &dyn SignInTerminal,
) -> TerminalExit {
    use std::os::unix::process::CommandExt as _;

    let outside = || TerminalExit {
        run: Err(Unpiped::Outside(program.path().display().to_string())),
        stopped: true,
    };
    if program.unchanged().is_err() {
        return outside();
    }
    let mut command = std::process::Command::new(program.path());
    command
        .args(&request.arguments)
        .current_dir(root)
        .env_clear()
        .envs(
            request
                .environment
                .iter()
                .map(|(name, value)| (name.as_str(), value.as_str())),
        )
        .stdin(std::process::Stdio::inherit())
        .stdout(std::process::Stdio::inherit())
        .stderr(std::process::Stdio::inherit())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(why) => {
            return TerminalExit {
                run: Err(Unpiped::Unreadable(Failure::new(&format!(
                    "the sign-in command could not be started: {why}"
                )))),
                stopped: true,
            };
        }
    };
    let pid = child.id();
    // Stopped from here, not from inside the child before it is executed: the
    // command's own error pipe stays open until it is executed, so a child
    // stopped before then would leave `spawn` waiting for it. A child that
    // reaches the terminal in the moment before this signal arrives is stopped
    // by the terminal itself, with `SIGTTIN`, and is resumed below as well.
    if let Err(why) = signal_group(pid, rustix::process::Signal::STOP) {
        if let Ok(Some(status)) = child.try_wait() {
            return TerminalExit {
                run: Ok(TerminalRun::Ended(status.success())),
                stopped: true,
            };
        }
        let stopped = end_group(pid, &mut child);
        return TerminalExit {
            run: Err(Unpiped::Unreadable(Failure::new(&format!(
                "the sign-in command could not be held for the terminal: {why}"
            )))),
            stopped,
        };
    }
    match hold_stopped(pid) {
        Stopped::Yes => {}
        // It ended before it could be stopped; there is no terminal to hand
        // it, and its own end is what the door reads.
        Stopped::Ended(success) => {
            return TerminalExit {
                run: Ok(TerminalRun::Ended(success)),
                stopped: true,
            };
        }
        Stopped::Unknown => {
            let stopped = end_group(pid, &mut child);
            return TerminalExit {
                run: Err(Unpiped::Unreadable(Failure::new(
                    "the sign-in command did not stop for the terminal to be handed to it",
                ))),
                stopped,
            };
        }
    }
    if let Err(why) = terminal.handed(pid) {
        let stopped = end_group(pid, &mut child);
        terminal.resumed();
        return TerminalExit {
            run: Err(Unpiped::Unhanded(why)),
            stopped,
        };
    }
    if let Err(why) = signal_group(pid, rustix::process::Signal::CONT) {
        let stopped = end_group(pid, &mut child);
        terminal.resumed();
        return TerminalExit {
            run: Err(Unpiped::Unreadable(Failure::new(&format!(
                "the sign-in command could not be resumed once it had the terminal: {why}"
            )))),
            stopped,
        };
    }
    let until = std::time::Instant::now() + bound;
    let mut stopped = true;
    let ended = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status.success()),
            Ok(None) => {}
            Err(why) => {
                stopped = end_group(pid, &mut child);
                terminal.resumed();
                return TerminalExit {
                    run: Err(Unpiped::Unreadable(Failure::new(&format!(
                        "the sign-in command could not be waited for: {why}"
                    )))),
                    stopped,
                };
            }
        }
        if std::time::Instant::now() >= until {
            stopped = end_group(pid, &mut child);
            break Err(());
        }
        std::thread::sleep(TERMINAL_POLL);
    };
    terminal.resumed();
    TerminalExit {
        run: match ended {
            Ok(success) => Ok(TerminalRun::Ended(success)),
            Err(()) => Ok(TerminalRun::Overran),
        },
        stopped,
    }
}

/// Sends `signal` to the process group `pid`.
#[cfg(unix)]
fn signal_group(pid: u32, signal: rustix::process::Signal) -> std::io::Result<()> {
    let unusable = || {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a process id this process cannot name",
        )
    };
    let pid = i32::try_from(pid).map_err(|_| unusable())?;
    let pid = rustix::process::Pid::from_raw(pid).ok_or_else(unusable)?;
    rustix::process::kill_process_group(pid, signal).map_err(Into::into)
}

/// How a child asked to stop answered.
#[cfg(unix)]
enum Stopped {
    /// It is stopped, and the terminal may be handed to its group.
    Yes,
    /// It ended before it could be stopped, and whether it said success.
    Ended(bool),
    /// It is neither known stopped nor known ended within the bound.
    Unknown,
}

/// Waits, bounded, until `pid` is stopped by the `SIGSTOP` sent to its group.
///
/// `SIGSTOP` is delivered asynchronously, so the signal having been sent says
/// nothing about whether the child has stopped. A child that ended first is
/// reported as ended, since it has no terminal to be given.
#[cfg(unix)]
fn hold_stopped(pid: u32) -> Stopped {
    let Some(pid) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return Stopped::Unknown;
    };
    let until = std::time::Instant::now() + STOPPED_BOUND;
    loop {
        match rustix::process::waitpid(
            Some(pid),
            rustix::process::WaitOptions::UNTRACED | rustix::process::WaitOptions::NOHANG,
        ) {
            Ok(Some((_, status))) if status.stopped() => return Stopped::Yes,
            Ok(Some((_, status))) => {
                return Stopped::Ended(status.exit_status() == Some(0));
            }
            Ok(None) => {}
            Err(_) => return Stopped::Unknown,
        }
        if std::time::Instant::now() >= until {
            return Stopped::Unknown;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Kills the process group `pid` and waits for the child it led, so nothing it
/// started is left and no zombie is left behind, and says whether both are
/// known to have happened.
#[cfg(unix)]
fn end_group(pid: u32, child: &mut std::process::Child) -> bool {
    let killed = signal_group(pid, rustix::process::Signal::KILL).is_ok();
    killed && child.wait().is_ok()
}

#[cfg(all(test, unix, feature = "proof"))]
mod tests {
    use std::fs;
    use std::sync::Mutex;

    use crucible_external_harnesses::Discovery;

    use crate::launching::HandoverFailed;
    use crate::sample::Sample;

    use super::*;

    #[test]
    fn a_real_terminal_sign_in_is_killed_with_its_group_when_the_bound_passes() {
        use std::os::unix::fs::PermissionsExt as _;
        #[derive(Default)]
        struct Lent(Mutex<(Vec<u32>, usize)>);
        impl SignInTerminal for Lent {
            fn handed(&self, pid: u32) -> Result<(), HandoverFailed> {
                self.0.lock().unwrap().0.push(pid);
                Ok(())
            }
            fn resumed(&self) {
                self.0.lock().unwrap().1 += 1;
            }
        }

        let sample = Sample::new("terminal-sign-in-bound");
        let root = sample.root();
        let script = root.join("login");
        fs::write(
            &script,
            "#!/bin/sh\nsleep 30 &\necho $! > grandchild\nwait\n",
        )
        .unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let Discovery::Found(program) = Discovery::of(Some(&script)) else {
            panic!("the command is a program");
        };
        let lent = Lent::default();
        let ended = ran_at_terminal(
            &program,
            &root,
            &Request::showing(Vec::new(), Vec::new()),
            Duration::from_millis(100),
            &lent,
        )
        .run
        .expect("the command was started");

        assert_eq!(ended, TerminalRun::Overran);
        assert_eq!(lent.0.lock().unwrap().1, 1, "the terminal came back once");
        let grandchild: i32 = fs::read_to_string(root.join("grandchild"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        let pid = rustix::process::Pid::from_raw(grandchild).unwrap();
        let until = std::time::Instant::now() + Duration::from_secs(5);
        while rustix::process::test_kill_process_group(pid).is_ok() {
            assert!(
                std::time::Instant::now() < until,
                "the group outlived the kill"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}
