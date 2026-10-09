//! Time from `exec` to the first thing on screen, with `output.screen: native`.
//!
//! The same measurement as `bench-first-frame`, by the same method and over the
//! same number of runs, with the one difference a user can choose: native mode
//! writes into the terminal's own scrollback instead of taking the alternate
//! screen. That is a separate path to the first frame, so it has a budget of
//! its own rather than borrowing the fullscreen one.
//!
//! The first thing drawn is the opening banner, so the clock stops when its
//! first word arrives.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use startup::{Measure, StartupError};

/// The budget, in milliseconds.
///
/// The same as the fullscreen first frame's, and for the same reason: the gap
/// a person notices does not depend on which screen the frame lands in. Across
/// twenty runs on a quiet machine the two read 2.2 to 2.3 ms each.
const LIMIT: f64 = 20.0;

/// The limit on a shared CI runner, in milliseconds.
///
/// The same as the fullscreen first frame's. Across twenty CI runs the two
/// probes stalled together, in the same runs and by about the same amount: a
/// typical reading near 5 ms for each, and 28.6 against 29.1 ms in the worst
/// run. A tighter limit here would fail on a runner's stall that the fullscreen
/// probe is allowed.
const SHARED_RUNNER_LIMIT: f64 = 150.0;

/// The first output of a run that got as far as drawing.
const NEEDLE: &str = "crucible ";

/// The screen mode this probe measures.
const SCREEN: &str = "native";

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
    let _ = writeln!(line, "    FAIL bench-native-frame: {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says what the readings looked like, beside a reading that went over budget.
fn detail(spread: &str) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    bench-native-frame {spread}");

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

    let readings = match startup::best(budget, || {
        startup::readings_in(Measure::Frame { needle: NEEDLE }, Some(SCREEN))
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

    if report(elapsed, limit).is_err() {
        return ExitCode::FAILURE;
    }

    if elapsed > limit {
        let _ = detail(&readings.spread());
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
