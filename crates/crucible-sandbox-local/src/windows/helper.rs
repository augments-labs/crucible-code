//! How the native-Windows broker is started for one confined command.
//!
//! The broker is started through a cleared process environment, like every
//! other child crucible confines, so a provider key or any other variable of
//! crucible's own never reaches it. The confined command's map travels inside
//! the launch frame on the broker's standard input, and the account-side
//! broker is started by the host broker with its private desktop name alone.
//!
//! `SystemRoot` is the one variable the host broker keeps. Nothing in the
//! broker reads a variable of its own, but it calls DPAPI, the account and
//! Filtering Platform services and the secondary logon service, and Windows
//! loads the components behind them from paths written as `%SystemRoot%`.
//! `TEMP`, `TMP` and `PATH` are left out: the broker makes no temporary file,
//! resolves no program by name, and finds itself through its own image path.

use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// The host variables the broker is started with.
const KEPT: [&str; 1] = ["SystemRoot"];

/// Starts `broker` in `working_directory` with only the [`KEPT`] variables
/// `host` holds a value for.
pub(crate) fn command(
    broker: &Path,
    working_directory: &Path,
    host: impl Fn(&str) -> Option<OsString>,
) -> Command {
    let mut process = Command::new(broker);
    process.current_dir(working_directory).env_clear();
    for name in KEPT {
        if let Some(value) = host(name) {
            process.env(name, value);
        }
    }
    process
}

#[cfg(all(test, unix))]
mod tests {
    use std::ffi::OsString;
    use std::path::Path;

    /// What `env` printed for the started process, as `(name, value)` pairs.
    fn started(host: impl Fn(&str) -> Option<OsString>) -> std::io::Result<Vec<(String, String)>> {
        let output = super::command(Path::new("/usr/bin/env"), Path::new("/"), host).output()?;
        if !output.status.success() {
            return Err(std::io::Error::other("env failed"));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|line| {
                let (name, value) = line.split_once('=').unwrap_or((line, ""));
                (name.to_owned(), value.to_owned())
            })
            .collect())
    }

    /// Names only, so a failure never prints a value the host holds.
    fn names(environment: &[(String, String)]) -> Vec<&str> {
        environment.iter().map(|(name, _)| name.as_str()).collect()
    }

    #[test]
    fn the_broker_is_started_with_a_cleared_environment_and_only_system_root() {
        // The test process always has variables of its own, such as PATH,
        // that an inherited environment would carry into the helper.
        assert!(std::env::vars_os().next().is_some());
        let host = |name: &str| match name {
            "SystemRoot" => Some(OsString::from(r"C:\Windows")),
            "ANTHROPIC_API_KEY" => Some(OsString::from("host-secret")),
            _ => None,
        };
        let environment = started(host).expect("env runs");
        assert_eq!(names(&environment), ["SystemRoot"]);
        assert_eq!(
            environment,
            [("SystemRoot".to_owned(), r"C:\Windows".to_owned())]
        );
    }

    #[test]
    fn a_host_without_system_root_gives_the_broker_nothing() {
        assert_eq!(
            names(&started(|_| None).expect("env runs")),
            Vec::<&str>::new()
        );
    }
}
