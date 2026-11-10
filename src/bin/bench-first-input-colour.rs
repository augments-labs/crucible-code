//! Time from `exec` to the point where a prompt can be typed at, with colour
//! on, on a terminal that answers.
//!
//! The same measurement as `bench-first-input`, by the same method and over the
//! same number of runs, with the one difference a user can choose: no
//! `NO_COLOR`, so a terminal is written in colour. Between `exec` and a prompt
//! sits everything the session needs before it can accept anything, and a
//! palette is part of what the startup path spends there.
//!
//! A terminal can also answer a question, or stay silent, and a program that
//! asks one may wait before it draws. Those are two different waits down the
//! same path, so each has a probe and a budget of its own: this one is answered
//! at once, and `bench-first-input-colour-silent` takes the same reading from a
//! terminal that never answers.
//!
//! The prompt mark is readiness, not the reading. Once it arrives the probe
//! sends one section-sign key through the terminal and stops only when that
//! character comes back in a frame.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use startup::{Answering, StartupError};

/// The budget, in milliseconds.
const LIMIT: f64 = 60.0;

/// The limit on a shared CI runner, in milliseconds.
///
/// The same as the no-colour first input's: the same startup and the same
/// stalls over a longer path, where 250 ms clears the worst measured launch by
/// about a third.
const SHARED_RUNNER_LIMIT: f64 = 250.0;

fn report(elapsed: f64, limit: f64) -> Result<(), io::Error> {
    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on inside a probe.
    let mut line = String::new();
    let _ = write!(line, "{elapsed:.1} ms {limit:.0}");
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

/// Says why no reading could be taken, on stderr, where `scripts/sh/bench.sh`
/// puts everything a human reads.
fn explain(problem: &StartupError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL bench-first-input-colour: {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says what the readings looked like, beside a reading that went over budget.
fn detail(spread: &str) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    bench-first-input-colour {spread}");

    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let limit = match startup::limit(LIMIT, SHARED_RUNNER_LIMIT) {
        Ok(limit) => limit,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };
    let budget = Duration::from_secs_f64(limit / 1000.0);

    let readings = match startup::best(budget, || startup::coloured_input(Answering::AtOnce)) {
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

    if report(elapsed, limit).is_err() {
        return ExitCode::FAILURE;
    }

    if elapsed > limit {
        let _ = detail(&readings.spread());
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
