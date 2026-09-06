//! Time from asking for the session picker to seeing a session previewed.
//!
//! The picker is the one screen that reads other people's work before it can
//! draw: the log directory is scanned for what belongs to this workspace, and
//! the marked session's tail is read and drawn the way resuming it would draw
//! it. Every one of those is a cost that no test notices and that grows with
//! somebody's afternoon rather than with this program.
//!
//! So the fixture's newest session is a long one, deep enough that the preview's
//! read has to stop before the beginning of it, and the clock stops when the
//! last thing that session said arrives on screen. Twenty milliseconds is the
//! budget, and it is the first frame's number for the first frame's reason: a
//! screen asked for by a keystroke either looks like it was already there or
//! looks like it was fetched.
//!
//! That is the number that decides what a preview may do — a session drawn onto
//! a recording that repaints, a log read whole rather than from its end, a walk
//! that costs the length of the session squared. Each of those is invisible in a
//! test suite and obvious here.
//!
//! Timed from the keystroke, not from `exec` — startup has two budgets of its
//! own, and charging them here would leave this one moving whenever they did.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use startup::{Measure, StartupError};

/// The budget, in milliseconds.
const LIMIT: f64 = 20.0;

/// Written and flushed immediately before the first read.
const READY: &str = "\u{203a} ";

/// What asks for the picker, and the return that runs it.
const LINE: &str = "/resume\r";

/// The last thing the deepest planted session said, so seeing it is seeing a
/// preview drawn from the far end of a log that had to be cut. Taken from the
/// fixture that writes it, rather than spelled a second time here.
const NEEDLE: &str = startup::ENDED;

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
    let _ = writeln!(line, "    FAIL bench-resume-preview: {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says what the readings looked like, beside a reading that went over budget.
fn detail(spread: &str) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    bench-resume-preview {spread}");

    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let budget = Duration::from_secs_f64(LIMIT / 1000.0);

    let readings = match startup::best(budget, || {
        startup::readings(Measure::Typed {
            ready: READY,
            line: LINE,
            needle: NEEDLE,
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
