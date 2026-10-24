//! `crucible agents install`: putting the release of an installed coding agent
//! the agent catalog offers where crucible runs it from, from a command line.
//!
//! What is downloaded, checked and kept is [`crucible_app::installing`]'s; this
//! file finds crucible's home and writes what comes back. How far the download
//! has come goes to standard error, on one line rewritten in place where
//! standard error is a terminal and as one line where it is not; the answer
//! goes to standard output and a refusal to standard error, written
//! [`failing`].
//!
//! `crucible agents unpack-check`, which the help leaves out, is the agent
//! catalog's intake asking how a download unpacks by the rules an install
//! holds it to; the file list it records goes to standard output as
//! [`crucible_app::installing::unpack_check`] gives it, and a refusal to
//! standard error, written [`failing`].
//!
//! `crucible agents probe`, which the help leaves out as well, is the same
//! intake starting the agent once in a sandbox with no network and asking it to
//! initialize; what [`crucible_app::probing::probe`] records goes to standard
//! output, and a refusal to standard error, written [`failing`].

use std::io::{self, IsTerminal as _, Write as _};
use std::path::Path;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use crucible_app::backends::REGISTRY;
use crucible_app::catalog_fetch::trust_in;
use crucible_app::installing::{self, Installed};
use crucible_app::probing::{self, Mode};
use crucible_config::{Driver, Home};
use crucible_sandbox::SandboxService;
use tokio::runtime::Handle;

use super::failure::failing;

#[cfg(test)]
mod tests;

/// The built-in driver `written` names, for the command line's parser.
///
/// A catalog agent is no driver's release: which agent it is, is the key it
/// is added under, so `catalog` is refused here with the command that adds
/// one.
pub(super) fn driver(written: &str) -> Result<Driver, String> {
    let known: Vec<&str> = Driver::ALL
        .into_iter()
        .filter(|driver| built_in(*driver))
        .map(Driver::name)
        .collect();
    match Driver::ALL
        .into_iter()
        .find(|driver| driver.name() == written)
    {
        Some(driver) if built_in(driver) => Ok(driver),
        Some(_) => Err(format!(
            "a catalog agent is added by its key with `crucible agents add`; install expects one \
             of {}",
            known.join(", ")
        )),
        None => Err(format!("expected one of {}", known.join(", "))),
    }
}

/// Whether `driver` is one agent of its own, whose release the catalog offers.
const fn built_in(driver: Driver) -> bool {
    match driver {
        Driver::Claude | Driver::Antigravity => true,
        Driver::Catalog => false,
    }
}

/// Runs `crucible agents install` for `driver`, and stops.
pub(super) fn install(driver: Driver) -> ExitCode {
    let Some(row) = REGISTRY.built(driver) else {
        let refused = format!("this build of crucible runs no {}", driver.name());
        let _ = io::stderr().write_all(failing(&refused).as_bytes());
        return ExitCode::FAILURE;
    };
    let home = match Home::find(&|name| std::env::var_os(name)) {
        Ok(home) => home,
        Err(homeless) => {
            let _ = io::stderr().write_all(failing(&homeless.to_string()).as_bytes());
            return ExitCode::FAILURE;
        }
    };
    let mut shown = Progress::new(driver, io::stderr().is_terminal());
    // The releases on offer as the catalog's trust state says now.
    let trust = trust_in(home.path());
    let (said, stopped) = installing::install(row, home.path(), &trust, &mut |held, of| {
        let _ = io::stderr().write_all(shown.heard(held, of).as_bytes());
    });
    let _ = io::stderr().write_all(shown.ended().as_bytes());
    let mut exit = match &said {
        Ok(installed) => {
            let _ = io::stdout().write_all(answered(driver, installed).as_bytes());
            ExitCode::SUCCESS
        }
        Err(uninstalled) => {
            let _ = io::stderr().write_all(failing(&uninstalled.to_string()).as_bytes());
            ExitCode::FAILURE
        }
    };
    if let Err(unfinished) = stopped {
        let _ = io::stderr().write_all(failing(&unfinished.to_string()).as_bytes());
        exit = ExitCode::FAILURE;
    }
    exit
}

/// Runs `crucible agents unpack-check` over the artifact the catalog spells
/// in `entry` and its download at `artifact`, unpacking into `out`, and stops.
pub(super) fn unpack_check(entry: &Path, artifact: &Path, out: &Path) -> ExitCode {
    written(checked(entry, artifact, out))
}

/// Runs `crucible agents probe` over the release unpacked into `root` and the
/// entry at `entry`, and stops.
pub(super) fn probe(root: &Path, entry: &Path, unconfined: bool) -> ExitCode {
    written(
        moded(unconfined, &|name| std::env::var_os(name))
            .and_then(|mode| probed(root, entry, mode, &probing::local, probing::BOUND)),
    )
}

/// The mode `--unconfined` asks for as `lookup` reads the environment, or the
/// refusal for standard error.
fn moded(
    unconfined: bool,
    lookup: &dyn Fn(&str) -> Option<std::ffi::OsString>,
) -> Result<Mode, String> {
    Mode::of(unconfined, lookup).map_err(|refused| failing(&caused(&refused)))
}

/// Writes `answer` to standard output where it is an answer and to standard
/// error where it is a refusal.
fn written(answer: Result<String, String>) -> ExitCode {
    match answer {
        Ok(text) => {
            let _ = io::stdout().write_all(text.as_bytes());
            ExitCode::SUCCESS
        }
        Err(refused) => {
            let _ = io::stderr().write_all(refused.as_bytes());
            ExitCode::FAILURE
        }
    }
}

/// What `crucible agents probe` writes: the JSON for standard output, or the
/// refusal for standard error, saying what refused it and why as each cause
/// beneath it gives it. The agent is given `within` to answer, and the
/// sandbox is the one `sandbox` makes on the application's runtime.
fn probed(
    root: &Path,
    entry: &Path,
    mode: Mode,
    sandbox: &dyn Fn(Handle) -> Arc<dyn SandboxService>,
    within: Duration,
) -> Result<String, String> {
    probing::probe(root, entry, mode, sandbox, within).map_err(|refused| failing(&caused(&refused)))
}

/// What `crucible agents unpack-check` writes: the file list for standard
/// output, or the refusal for standard error, saying what refused it and why
/// as each cause beneath it gives it.
fn checked(entry: &Path, artifact: &Path, out: &Path) -> Result<String, String> {
    installing::unpack_check(entry, artifact, out).map_err(|refused| failing(&caused(&refused)))
}

/// The most causes beneath a refusal [`caused`] says.
const CAUSES: usize = 8;

/// `refused` and the causes beneath it, each after a colon; a cause whose
/// words are already said is not said again.
fn caused(refused: &dyn std::error::Error) -> String {
    let mut said = refused.to_string();
    let mut cause = refused.source();
    for _ in 0..CAUSES {
        let Some(next) = cause else {
            break;
        };
        let words = next.to_string();
        if !said.contains(&words) {
            said.push_str(": ");
            said.push_str(&words);
        }
        cause = next.source();
    }
    said
}

/// The line `installed` is written as.
fn answered(driver: Driver, installed: &Installed) -> String {
    let agent = driver.name();
    let version = &installed.version;
    let at = installed.executable.display();
    if installed.kept {
        format!("{agent} {version} is already installed at {at}.\n")
    } else {
        format!(
            "Installed {agent} {version} at {at}. /login offers it under \
             Use an installed coding agent.\n"
        )
    }
}

/// What has been said about a download's progress.
struct Progress {
    driver: Driver,
    terminal: bool,
    /// The last whole percent said, once anything has been.
    said: Option<u64>,
}

impl Progress {
    fn new(driver: Driver, terminal: bool) -> Self {
        Self {
            driver,
            terminal,
            said: None,
        }
    }

    /// What to write now that `held` of `of` bytes have arrived: on a
    /// terminal the percent come, each time it changes, and elsewhere one line
    /// opening the download.
    fn heard(&mut self, held: u64, of: u64) -> String {
        let percent = held
            .saturating_mul(100)
            .checked_div(of)
            .unwrap_or(100)
            .min(100);
        let opening = format!("Downloading {} ({} MB)", self.driver.name(), of / 1_000_000);
        let text = if self.terminal && self.said != Some(percent) {
            format!("\r{opening}: {percent:>3}%")
        } else if !self.terminal && self.said.is_none() {
            format!("{opening}…\n")
        } else {
            String::new()
        };
        self.said = Some(percent);
        text
    }

    /// What ends what was said: the line a terminal was rewriting, where one
    /// was.
    fn ended(&self) -> &'static str {
        if self.terminal && self.said.is_some() {
            "\n"
        } else {
            ""
        }
    }
}
