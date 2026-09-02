//! PID-namespace scope death proof owned by namespace PID 1.

use std::fs;
use std::io;
use std::os::unix::process::ExitStatusExt as _;
use std::process::ExitStatus;
use std::thread;
use std::time::{Duration, Instant};

use rustix::io::Errno;
use rustix::process::{Pid, Signal, WaitOptions};

const SCOPE_DEATH_DEADLINE: Duration = Duration::from_secs(2);
const REAP_INTERVAL: Duration = Duration::from_millis(5);

pub(crate) fn empty() -> io::Result<()> {
    let deadline = Instant::now() + SCOPE_DEATH_DEADLINE;
    loop {
        match rustix::process::kill_process_group(Pid::INIT, Signal::KILL) {
            Ok(()) | Err(Errno::SRCH) => {}
            Err(problem) => return Err(problem.into()),
        }
        reap_available()?;
        if namespace_processes()?.is_empty() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "sandbox workload scope did not become empty",
            ));
        }
        thread::sleep(REAP_INTERVAL);
    }
}

fn reap_available() -> io::Result<()> {
    loop {
        match rustix::process::wait(WaitOptions::NOHANG) {
            Ok(Some(_)) => {}
            Ok(None) | Err(Errno::CHILD) => return Ok(()),
            Err(problem) => return Err(problem.into()),
        }
    }
}

/// Reaps every exited descendant reparented to PID 1, returning the workload's
/// own status if it was among them.
///
/// A workload that keeps double-forking short-lived children would otherwise
/// leave each of them a zombie until it exits itself, and a namespace's zombies
/// hold process identifiers in every ancestor namespace, the host's included.
pub(crate) fn reap_until_workload(workload: u32) -> io::Result<Option<ExitStatus>> {
    let mut status = None;
    loop {
        match rustix::process::wait(WaitOptions::NOHANG) {
            Ok(Some((pid, waited))) => {
                if pid.as_raw_nonzero().get().unsigned_abs() == workload {
                    status = Some(ExitStatus::from_raw(waited.as_raw()));
                }
            }
            Ok(None) | Err(Errno::CHILD) => return Ok(status),
            Err(problem) => return Err(problem.into()),
        }
    }
}

fn namespace_processes() -> io::Result<Vec<u32>> {
    let own = std::process::id();
    let mut processes = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let Ok(process) = name.parse::<u32>() else {
            continue;
        };
        if process != own {
            processes.push(process);
        }
    }
    Ok(processes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    #[test]
    #[expect(
        clippy::zombie_processes,
        reason = "both children are reaped through the function under test, not through `Child`"
    )]
    fn reaping_returns_the_workload_status_and_discards_other_zombies() {
        let mut bystander = Command::new("true")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("bystander child");
        let workload = Command::new("sh")
            .args(["-c", "exit 7"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("workload child");

        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = reap_until_workload(workload.id()).expect("reap") {
                break status;
            }
            assert!(Instant::now() < deadline, "the workload was never reaped");
            thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(status.code(), Some(7));
        assert!(
            bystander.try_wait().is_err(),
            "the bystander zombie was left for the workload's exit"
        );
    }
}
