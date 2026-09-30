//! Time to read one tool result back from where it stands in a session log.
//!
//! What opening a row costs once the result behind it is no longer held. The
//! terminal keeps what results said up to a ceiling and lets the oldest go, so
//! a long session is one where most rows that offer to expand are answered from
//! the log rather than from memory, one result at a time, when somebody opens
//! them.
//!
//! So the log read is the fixture's deepest session — the one the resume picker
//! previews, worked in for an afternoon — and the result is its first turn's,
//! the one furthest from where the session ended. Twenty milliseconds is the
//! budget, for the reason the preview's is: a view asked for by a key either
//! looks like it was already there or looks like it was fetched.
//!
//! Timed in-process around the read alone. Planting the fixture and finding
//! the place are outside the clock; what is inside is everything opening the
//! row does to the log.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::{Duration, Instant};

use crucible_session::{DisplayItem, Place, Session};
use crucible_types::Message;
use crucible_workspace::Workspace;

/// The budget, in milliseconds.
const LIMIT: f64 = 20.0;

/// Reads timed, the slowest twentieth of which the budget does not have to
/// cover.
const RUNS: usize = 200;

fn report(elapsed: f64) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = write!(line, "{elapsed:.1} ms {LIMIT:.0}");
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

/// Says why no reading could be taken, on stderr, where `scripts/sh/bench.sh`
/// puts everything a human reads.
fn explain(problem: &str) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL bench-read-back: {problem}");

    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let elapsed = match measured() {
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
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

/// The 95th percentile of [`RUNS`] reads of the deepest session's first result.
fn measured() -> Result<Duration, String> {
    let home = startup::Scratch::new("read-back").map_err(|problem| problem.to_string())?;
    let here = std::env::current_dir()
        .map_err(|problem| problem.to_string())
        .and_then(|at| Workspace::open(at).map_err(|problem| problem.to_string()))?;

    // The newest log recorded for this directory, which the fixture makes the
    // deepest one.
    let (session, _) = Session::resume(&home.path().join("sessions"), &here)
        .map_err(|problem| problem.to_string())?;
    let place = first_place(&session)?;

    let mut taken = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        let started = Instant::now();
        let output = session
            .read_back(&place)
            .map_err(|problem| problem.to_string())?;
        taken.push(started.elapsed());
        std::hint::black_box(output);
    }

    taken.sort_unstable();
    taken
        .get(RUNS * 95 / 100)
        .copied()
        .ok_or_else(|| "no reading was taken".to_owned())
}

/// Where the first result of `session` stands, as the replay a reader is shown
/// reports it.
fn first_place(session: &Session) -> Result<Place, String> {
    let mut history = session
        .display_history()
        .map_err(|problem| problem.to_string())?
        .ok_or_else(|| "the deepest session has no log".to_owned())?;

    while let Some(item) = history.next() {
        let item = item.map_err(|problem| problem.to_string())?;
        if let DisplayItem::Message {
            message: Message::ToolResults(results),
            ..
        } = item
            && let (Some(result), Some(position)) = (results.first(), history.placed())
        {
            return Ok(Place::new(result.id.clone(), position));
        }
    }

    Err("the deepest session holds no result".to_owned())
}
