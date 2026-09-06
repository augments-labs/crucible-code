//! Time from `exec` to the point where a prompt can be typed at.
//!
//! Distinct from the first frame, and a longer budget, because between the two
//! sits everything the session needs before it can accept anything: the
//! workspace opened, the session log created and its writer thread started, the
//! credential resolved, the tools built. Sixty milliseconds is the point at
//! which a keystroke stops feeling like it was waiting for the program and
//! starts feeling like the program was waiting for it.
//!
//! The prompt mark is readiness, not the reading. Once it arrives the probe
//! sends one section-sign key through the terminal and stops only when that
//! character comes back in a frame. The terminal is in raw mode with echo off,
//! so seeing it proves crucible accepted and rendered the key.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use startup::{Measure, StartupError};

/// The budget, in milliseconds.
const LIMIT: f64 = 60.0;

/// Written and flushed immediately before the first read.
const READY: &str = "\u{203a} ";

/// One uncommon key, absent from startup output and visible in the input box.
const PROBE: &str = "\u{00a7}";

fn report(elapsed: f64) -> Result<(), io::Error> {
    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on inside a probe.
    let mut line = String::new();
    let _ = write!(line, "{elapsed:.1} ms {LIMIT:.0}");
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

/// Says why no reading could be taken, on stderr, where `scripts/bench.sh`
/// puts everything a human reads.
fn explain(problem: &StartupError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL bench-first-input: {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says what the readings looked like, beside a reading that went over budget.
fn detail(spread: &str) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    bench-first-input {spread}");

    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let budget = Duration::from_secs_f64(LIMIT / 1000.0);

    let readings = match startup::best(budget, || {
        startup::readings(Measure::Input {
            ready: READY,
            probe: PROBE,
        })
    }) {
        Ok(readings) => readings,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    let elapsed = match readings.p95() {
        Ok(p95) => p95.as_secs_f64() * 1000.0,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    if report(elapsed).is_err() {
        return ExitCode::FAILURE;
    }

    if elapsed > LIMIT {
        let _ = detail(&readings.spread());
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
