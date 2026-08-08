//! Time from `exec` to the first thing on screen.
//!
//! Twenty milliseconds is the budget, and it is not arbitrary: below about that
//! a program feels like it was already running, and above it there is a visible
//! gap between pressing return and seeing anything. It is also the number that
//! decides what may go on the startup path — a config file parsed eagerly, a
//! directory walked before the prompt, a provider that checks its key over the
//! network. Each of those is invisible in a test suite and obvious here.
//!
//! The first thing drawn is the opening banner, so the clock stops when its
//! first word arrives.

mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;

use startup::StartupError;

/// The budget, in milliseconds.
const LIMIT: f64 = 20.0;

/// The first output of a run that got as far as drawing.
const NEEDLE: &str = "crucible ";

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
    let _ = writeln!(line, "    FAIL bench-first-frame: {problem}");

    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let elapsed = match startup::percentile(NEEDLE) {
        Ok(elapsed) => elapsed.as_secs_f64() * 1000.0,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    if report(elapsed).is_err() {
        return ExitCode::FAILURE;
    }

    if elapsed > LIMIT {
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
