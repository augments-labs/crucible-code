//! Time the two argument-only exits that must bypass ordinary startup.
//!
//! `--help` and `--version` are often run by shell completion, installers and
//! probes. Neither needs a terminal, configuration, a session or a provider,
//! so each is measured through a pipe until the process exits. The reported
//! value is the slower p95; both readings are preserved as evidence.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use startup::{Readings, StartupError};

/// The shared budget, in milliseconds.
const LIMIT: f64 = 12.0;

fn report(help: f64, version: f64) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = write!(
        line,
        "{:.1} ms {LIMIT:.0} help={help:.1} version={version:.1}",
        help.max(version)
    );
    line.push('\n');
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

fn explain(problem: &StartupError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL bench-cli-exit: {problem}");
    io::stderr().write_all(line.as_bytes())
}

fn detail(label: &str, readings: &Readings) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    bench-cli-exit {label}: {}", readings.spread());
    io::stderr().write_all(line.as_bytes())
}

fn measured(
    label: &'static str,
    args: &'static [&'static str],
    needle: &'static str,
) -> Result<Readings, StartupError> {
    let budget = Duration::from_secs_f64(LIMIT / 1000.0);
    startup::best(budget, || startup::exits(label, args, needle))
}

fn main() -> ExitCode {
    let help = match measured("--help", &["--help"], "Usage: crucible") {
        Ok(readings) => readings,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };
    let version = match measured("--version", &["--version"], env!("CARGO_PKG_VERSION")) {
        Ok(readings) => readings,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    let help_p95 = match help.p95() {
        Ok(reading) => reading.as_secs_f64() * 1000.0,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };
    let version_p95 = match version.p95() {
        Ok(reading) => reading.as_secs_f64() * 1000.0,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    if report(help_p95, version_p95).is_err() {
        return ExitCode::FAILURE;
    }
    if help_p95 > LIMIT || version_p95 > LIMIT {
        let _ = detail("--help", &help);
        let _ = detail("--version", &version);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
