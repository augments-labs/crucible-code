//! Time from `exec` to the point where a prompt can be typed at.
//!
//! Distinct from the first frame, and a longer budget, because between the two
//! sits everything the session needs before it can accept anything: the
//! workspace opened, the session log created and its writer thread started, the
//! credential resolved, the tools built. Sixty milliseconds is the point at
//! which a keystroke stops feeling like it was waiting for the program and
//! starts feeling like the program was waiting for it.
//!
//! The clock stops when the prompt mark arrives, which is the last thing
//! written before the first read from standard input.

mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;

use startup::StartupError;

/// The budget, in milliseconds.
const LIMIT: f64 = 60.0;

/// The prompt mark. Written and flushed immediately before the first read, so
/// its arrival is the moment input is accepted.
const NEEDLE: &str = "\u{203a} ";

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
