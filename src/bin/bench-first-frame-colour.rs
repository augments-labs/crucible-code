//! Time from `exec` to the first thing on screen, with colour on.
//!
//! The same measurement as `bench-first-frame`, by the same method and over the
//! same number of runs, with the one difference a user can choose: no
//! `NO_COLOR`, so a terminal is written in colour. A palette is bytes, layout
//! and work the no-colour probe never does, and a startup budget that left it
//! out would bound a path nobody sees.
//!
//! A terminal can also answer a question, or stay silent, and a program that
//! asks one may wait before it draws. Those are two different waits down the
//! same path, so both are measured and the slower is the reading.
//!
//! The first thing drawn is the opening banner, so the clock stops when its
//! first word arrives.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use startup::{Answering, Colour, Measure, Readings, StartupError};

/// The budget, in milliseconds.
const LIMIT: f64 = 20.0;

/// The limit on a shared CI runner, in milliseconds.
///
/// The same as the no-colour first frame's, because it is the same startup and
/// the same stalls: 150 ms clears the worst launch measured across 98 CI runs
/// by a third and still stops a startup many times slower.
const SHARED_RUNNER_LIMIT: f64 = 150.0;

/// The first output of a run that got as far as drawing.
///
/// The word alone, without the space the no-colour probe waits on: colour puts
/// a reset between the name and what follows it, so the plain string is no
/// longer contiguous in the terminal's bytes.
const NEEDLE: &str = "crucible";

/// Something that has an answer and something that has none.
const BOTH: [Answering; 2] = [Answering::AtOnce, Answering::Never];

/// The p95 of one answering behaviour, and what the windows looked like.
fn reading(answering: Answering, budget: Duration) -> Result<(Readings, f64), StartupError> {
    let readings = startup::best(budget, || {
        startup::readings_on(
            Measure::Frame { needle: NEEDLE },
            None,
            answering,
            Colour::On,
        )
    })?;
    let elapsed = readings.p95()?.as_secs_f64() * 1000.0;

    Ok((readings, elapsed))
}

fn report(answered: f64, silent: f64, limit: f64) -> Result<(), io::Error> {
    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on inside a probe.
    let mut line = String::new();
    let _ = write!(
        line,
        "{:.1} ms {limit:.0} answered={answered:.1} silent={silent:.1}",
        answered.max(silent),
    );
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

/// Says why no reading could be taken, on stderr, where `scripts/sh/bench.sh`
/// puts everything a human reads.
fn explain(problem: &StartupError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL bench-first-frame-colour: {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says what the readings looked like, beside a reading that went over budget.
fn detail(said: &str) -> Result<(), io::Error> {
    io::stderr().write_all(said.as_bytes())
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

    let mut answered = 0.0;
    let mut silent = 0.0;
    let mut said = String::new();

    for answering in BOTH {
        let (readings, elapsed) = match reading(answering, budget) {
            Ok(reading) => reading,
            Err(problem) => {
                let _ = explain(&problem);
                return ExitCode::FAILURE;
            }
        };

        let _ = writeln!(
            said,
            "    bench-first-frame-colour {answering:?} {}",
            readings.spread()
        );
        match answering {
            Answering::AtOnce => answered = elapsed,
            Answering::Never => silent = elapsed,
            // Ways of answering that the fake terminal's own tests use;
            // `BOTH` names neither of them.
            Answering::Late | Answering::OutOfOrder | Answering::Split => {}
        }
    }

    if report(answered, silent, limit).is_err() {
        return ExitCode::FAILURE;
    }

    let slowest = answered.max(silent);
    if slowest > limit {
        let _ = detail(&said);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
