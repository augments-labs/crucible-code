//! Best-effort handoff from an account login to the system browser.
//!
//! The URI is one argument to a fixed platform launcher, never shell text. The
//! launcher is reaped on a named worker and killed after two seconds if it did
//! not detach; a browser must not leave a zombie or an unbounded helper behind
//! merely because authorization can also be completed by copying the page from
//! the terminal.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REAP_LIFETIME: Duration = Duration::from_secs(2);
const REAP_POLL: Duration = Duration::from_millis(20);

/// Opens `uri` with the operating system's browser association.
///
/// # Errors
///
/// [`BrowserError`] when the platform has no launcher, the launcher could not
/// start, or its bounded reaper thread could not be created.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(super) fn open(uri: &str) -> Result<(), BrowserError> {
    spawn(command(uri))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn open(_uri: &str) -> Result<(), BrowserError> {
    Err(BrowserError::Unsupported)
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn spawn(mut command: Command) -> Result<(), BrowserError> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let (send, receive) = std::sync::mpsc::sync_channel::<std::process::Child>(1);
    std::thread::Builder::new()
        .name("crucible-browser-reaper".to_owned())
        .spawn(move || {
            let Ok(mut child) = receive.recv() else {
                return;
            };
            let started = Instant::now();
            while started.elapsed() < REAP_LIFETIME {
                match child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) => std::thread::sleep(REAP_POLL),
                    Err(_) => break,
                }
            }
            let _ = child.kill();
            let _ = child.wait();
        })
        .map_err(BrowserError::Worker)?;
    let child = command.spawn().map_err(BrowserError::Launch)?;
    if let Err(problem) = send.send(child) {
        let mut child = problem.0;
        let _ = child.kill();
        let _ = child.wait();
        return Err(BrowserError::ReaperStopped);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn command(uri: &str) -> Command {
    let mut command = Command::new("xdg-open");
    command.arg(uri);
    command
}

#[cfg(target_os = "macos")]
fn command(uri: &str) -> Command {
    let mut command = Command::new("open");
    command.arg(uri);
    command
}

#[cfg(windows)]
fn command(uri: &str) -> Command {
    let mut command = Command::new("explorer.exe");
    command.arg(uri);
    command
}

/// Why the best-effort browser handoff did not start.
#[derive(Debug, thiserror::Error)]
pub(super) enum BrowserError {
    /// This target has no launcher known to this build.
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    #[error("this platform has no browser launcher")]
    Unsupported,
    /// The fixed platform launcher could not be spawned.
    #[error("the browser launcher could not start: {0}")]
    Launch(std::io::Error),
    /// The worker responsible for reaping the launcher could not start.
    #[error("the browser launcher could not be reaped: {0}")]
    Worker(std::io::Error),
    /// The reaper stopped before it received the launcher.
    #[error("the browser launcher reaper stopped unexpectedly")]
    ReaperStopped,
}
