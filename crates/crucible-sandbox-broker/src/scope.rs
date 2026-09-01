//! PID-namespace scope death proof owned by namespace PID 1.

use std::fs;
use std::io;
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
