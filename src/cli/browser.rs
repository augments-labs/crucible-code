//! Best-effort handoff from an account login to the system browser.
//!
//! The URI is one argument to a fixed platform launcher, never shell text. The
//! launcher is reaped on a named worker and killed after two seconds if it did
//! not detach; a browser must not leave a zombie or an unbounded helper behind
//! merely because authorization can also be completed by copying the page from
//! the terminal.
//!
//! The launcher keeps crucible's environment, because a browser needs the
//! display, the session bus and its own settings to open at all, but not the
//! variables a provider key is read from. A browser it starts lives on after
//! crucible and has no use for one.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REAP_LIFETIME: Duration = Duration::from_secs(2);
const REAP_POLL: Duration = Duration::from_millis(20);

/// Opens `uri` with the operating system's browser association, with none of
/// the `withheld` variables in the launcher's environment.
///
/// # Errors
///
/// [`BrowserError`] when the platform has no launcher, the launcher could not
/// start, or its bounded reaper thread could not be created.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
pub(super) fn open<'a>(
    uri: &str,
    withheld: impl IntoIterator<Item = &'a str>,
) -> Result<(), BrowserError> {
    spawn(without(command(uri), withheld))
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
pub(super) fn open<'a>(
    _uri: &str,
    _withheld: impl IntoIterator<Item = &'a str>,
) -> Result<(), BrowserError> {
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

/// `command`, kept from inheriting any of `withheld`.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn without<'a>(mut command: Command, withheld: impl IntoIterator<Item = &'a str>) -> Command {
    for name in withheld {
        command.env_remove(name);
    }
    command
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

#[cfg(all(test, any(target_os = "linux", target_os = "macos", windows)))]
mod tests {
    use std::ffi::OsStr;

    use super::{command, without};

    #[test]
    fn the_launcher_inherits_no_variable_a_key_is_read_from() {
        // Set on nothing here: the process environment is not written in a
        // test. A name removed from a `Command` is one the child does not
        // inherit whether or not the parent holds it.
        let launcher = without(
            command("https://example.invalid/authorize"),
            ["ANTHROPIC_API_KEY", "WORK_ANTHROPIC_KEY"],
        );

        let removed: Vec<&OsStr> = launcher
            .get_envs()
            .filter_map(|(name, value)| value.is_none().then_some(name))
            .collect();

        assert_eq!(
            removed,
            [
                OsStr::new("ANTHROPIC_API_KEY"),
                OsStr::new("WORK_ANTHROPIC_KEY")
            ]
        );
    }
}
