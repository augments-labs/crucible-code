//! Time from `exec` to the point where a prompt can be typed at, with colour
//! on, on a terminal that is a picture candidate and never answers.
//!
//! The same reading as `bench-first-input-colour`, by the same method, prompt
//! mark, key and colour, from a terminal that stays silent. Such a terminal is
//! asked where its cursor is before a key is read, and is read on for the
//! picture exchange's reply wait in case the answer is only late, so that no
//! answer is left to be typed. The start waits that out, so this budget is that
//! wait added to the answered start's own, and the test below fails when the
//! wait changes until somebody chooses the budget that goes with it.
//!
//! The prompt mark is readiness, not the reading. Once it arrives the probe
//! sends one section-sign key through the terminal and stops only when that
//! character comes back in a frame.
//!
//! Every launch sleeps through the wait, so the windows are taken at once with
//! their starts spread across it, and a batch takes one window's wall clock
//! rather than nine's.

#[allow(dead_code)]
mod startup;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
use std::time::Duration;

use startup::{Answering, StartupError};

/// The budget, in milliseconds.
///
/// 2000 ms, the reply wait a start reads a silent candidate for
/// (`crucible_tui::graphics::Deadline::STARTUP.reply`), plus the 60 ms the
/// answered first input with colour is held to.
const LIMIT: f64 = 2060.0;

/// The limit on a shared CI runner, in milliseconds.
///
/// 2000 ms, the same reply wait, plus the 250 ms the answered first input with
/// colour is held to on a shared runner, whose stalls land on the startup
/// around the wait as they do on the answered one.
const SHARED_RUNNER_LIMIT: f64 = 2250.0;

/// The reply wait every launch here sits through, which the windows are
/// spread across so that each start runs alone.
///
/// `crucible_tui::graphics::Deadline::STARTUP.reply`, written out because the
/// probe measures the built binary and names its crates in the test alone,
/// which fails when the two part.
const REPLY_WAIT: Duration = Duration::from_secs(2);

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
    let _ = writeln!(line, "    FAIL bench-first-input-colour-silent: {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says what the readings looked like, beside a reading that went over budget.
fn detail(spread: &str) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    bench-first-input-colour-silent {spread}");

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
        startup::coloured_input_at_once(Answering::Never, REPLY_WAIT)
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

#[cfg(test)]
mod tests {
    use super::{LIMIT, REPLY_WAIT, SHARED_RUNNER_LIMIT};

    #[test]
    fn the_windows_are_spread_across_the_startup_reply_wait() {
        let reply = crucible_tui::graphics::Deadline::STARTUP.reply;

        assert_eq!(
            REPLY_WAIT, reply,
            "the probe spreads its windows across {REPLY_WAIT:?}, but a start waits \
             {reply:?} for a silent terminal, so the launches of one window land on \
             another's"
        );
    }

    /// The answered probe's limit `name`, as its source declares it.
    fn answered(name: &str) -> f64 {
        let declared = format!("const {name}: f64 = ");
        include_str!("bench-first-input-colour.rs")
            .lines()
            .find_map(|line| line.strip_prefix(declared.as_str()))
            .and_then(|value| value.strip_suffix(';'))
            .and_then(|value| value.parse().ok())
            .unwrap_or_else(|| panic!("bench-first-input-colour declares no {name}"))
    }

    /// `limit` as a whole number of milliseconds, refused rather than rounded
    /// when it is not one: a whole `f64` displays with no point, so only then
    /// does its text parse as an integer.
    fn whole(limit: f64, what: &str) -> u128 {
        limit.to_string().parse().unwrap_or_else(|_| {
            panic!("{what} is {limit} ms, which is not a whole number of milliseconds")
        })
    }

    #[test]
    fn each_limit_is_the_startup_reply_wait_plus_the_answered_limit() {
        // Whole milliseconds, because a wait in seconds as a float times a
        // thousand is not always the whole number it was written as.
        let reply = crucible_tui::graphics::Deadline::STARTUP.reply.as_millis();

        for (silent, name) in [
            (LIMIT, "LIMIT"),
            (SHARED_RUNNER_LIMIT, "SHARED_RUNNER_LIMIT"),
        ] {
            let silent = whole(silent, name);
            let answered = whole(answered(name), &format!("the answered probe's {name}"));
            assert_eq!(
                silent,
                reply + answered,
                "{name} is {silent} ms, not the {reply} ms reply wait plus the answered \
                 probe's {answered} ms; a changed wait is a budget to choose, not one to follow"
            );
        }
    }
}
