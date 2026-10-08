//! `crucible update`: whether a later release is out, and putting it in place,
//! from a command line.
//!
//! What is asked, installed and refused is [`crucible_app::update`]'s; this
//! file hands it what only the process knows, the environment's release
//! source, the running executable and this build's version, and writes what
//! comes back. An answer goes to standard output, a refusal to standard error,
//! written [`failing`]: a refusal's cause can hold text from an archive, so
//! what is written is the refusal's own sentence, escaped, never its cause.
//!
//! The exit status says the answer: 3 where a check found a later release, 1
//! where nothing was answered, and 0 otherwise.

use std::io::{self, Write as _};
use std::process::ExitCode;

use crucible_app::update::{self, Answer, Asked, Failed, SOURCE, SelfUpdateCommand};

use super::failure::failing;

#[cfg(test)]
mod tests;

/// Runs `crucible update` as `check` and `dry_run` ask, and stops.
pub(super) fn updated(check: bool, dry_run: bool) -> ExitCode {
    let asked = match (check, dry_run) {
        (true, _) => Asked::Check,
        (false, true) => Asked::DryRun,
        (false, false) => Asked::Apply,
    };
    let Ok(executable) = std::env::current_exe() else {
        let _ = io::stderr()
            .write_all(failing("could not find the running crucible executable").as_bytes());
        return ExitCode::FAILURE;
    };
    let source = std::env::var_os(SOURCE);
    let (said, stopped) = update::update(&SelfUpdateCommand {
        asked,
        source: source.as_deref(),
        executable: &executable,
        running: env!("CARGO_PKG_VERSION"),
    });
    let mut exit = match &said {
        Ok(answer) => {
            let _ = io::stdout().write_all(answered(answer).as_bytes());
            ExitCode::from(answer.exit())
        }
        Err(failed) => {
            let _ = io::stderr().write_all(&refusal(failed));
            ExitCode::FAILURE
        }
    };
    if let Err(unfinished) = stopped {
        let _ = io::stderr().write_all(failing(&unfinished.to_string()).as_bytes());
        exit = ExitCode::FAILURE;
    }
    exit
}

/// The line `answer` is written as.
fn answered(answer: &Answer) -> String {
    format!("{answer}\n")
}

/// The line a failure is written as: its own sentence, escaped and cut to
/// one line's ceiling, and none of the causes behind it.
fn refusal(failed: &Failed) -> Vec<u8> {
    failing(&failed.to_string()).into_bytes()
}
