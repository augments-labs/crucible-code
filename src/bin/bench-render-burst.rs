//! Render throughput under a token burst.
//!
//! A model streaming at full speed hands the renderer a delta every few
//! milliseconds, and every delta is a frame: wrap, rewind, erase, redraw. The
//! budget says the renderer commits at least thirty of those a second, which is
//! the rate below which streamed text stops looking like typing and starts
//! looking like stalling.
//!
//! Measured against a real file descriptor rather than an in-memory buffer, so
//! the write and the flush are the syscalls they will be in production. The
//! terminal is reported as a terminal, so the escape sequences are built and
//! written too -- benchmarking the redirected path would measure the cheap one.
//!
//! Thirty a second is a floor with a great deal of headroom, which on its own
//! would make this a benchmark that cannot fail. So the rate is measured twice:
//! once at the start of the burst and once at the end, and the two must be
//! close. That is the check that actually bites, because the way this gets slow
//! is not a constant factor -- it is a redraw that grows with the transcript,
//! and a redraw like that is fast in the first second and hopeless in the
//! hundredth. The reported number is the *sustained* rate, since a session is
//! long and the opening frames are not the ones a user is waiting on.

use std::fmt::Write as _;
use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::process::ExitCode;
use std::time::Instant;

use crucible_tui::{Renderer, Size, Terminal, TerminalError};

/// The floor, in frames per second.
const LIMIT: f64 = 30.0;

/// Frames to measure. Large enough that a scheduler hiccup does not decide the
/// answer, small enough that the probe stays under a second on a slow machine.
const FRAMES: usize = 20_000;

/// Frames in each of the two timed windows, at the start and the end.
const WINDOW: usize = FRAMES / 10;

/// How far the sustained rate may fall behind the opening rate.
///
/// A renderer whose cost is bounded holds roughly level, so anything is slack.
/// A renderer whose cost grows with the transcript has already lost an order of
/// magnitude by the end of a burst this size, and far more by the end of a real
/// session -- which is the failure this number exists to catch while the margin
/// is still recoverable.
const SUSTAINED_FRACTION: f64 = 0.5;

/// Frames to run and throw away, so the measurement is not paying for the first
/// allocation of every reused buffer.
const WARMUP: usize = 2_000;

/// A terminal-sized window, so wrapping and the bounded tail both engage.
const COLUMNS: usize = 80;
const ROWS: usize = 24;

/// What can go wrong in the probe itself.
#[derive(Debug, thiserror::Error)]
enum ProbeError {
    /// The measurement could not be reported.
    #[error("bench-render-burst: {0}")]
    Io(#[from] io::Error),

    /// The renderer failed mid-burst.
    #[error("bench-render-burst: {0}")]
    Terminal(#[from] TerminalError),
}

/// Standard output's discard file, pretending to be a terminal.
///
/// The renderer asks whether it is talking to a terminal in order to decide
/// whether to emit cursor movement at all, and the escape sequences are part of
/// what is being measured.
#[derive(Debug)]
struct Discard {
    out: File,
}

impl Discard {
    fn open() -> Result<Self, io::Error> {
        let path = if cfg!(windows) { "NUL" } else { "/dev/null" };
        Ok(Self {
            out: OpenOptions::new().write(true).open(path)?,
        })
    }
}

impl Terminal for Discard {
    fn size(&self) -> Result<Size, TerminalError> {
        Ok(Size {
            columns: COLUMNS,
            rows: ROWS,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        self.out.write_all(text.as_bytes())?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.out.flush()?;
        Ok(())
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

/// Deltas shaped like the ones a provider actually sends: a few characters at a
/// time, mostly mid-word, with a line ending every so often. Built once, so the
/// measured loop is not timing a formatter.
fn burst() -> Vec<String> {
    let words = [
        "The ",
        "runner ",
        "drives ",
        "turns ",
        "over ",
        "traits ",
        "alone, ",
        "so ",
        "a ",
        "provider ",
        "never ",
        "reaches ",
        "a ",
        "tool. ",
    ];

    let mut deltas = Vec::with_capacity(256);

    for (index, word) in words.iter().cycle().take(256).enumerate() {
        deltas.push((*word).to_owned());

        // A newline every dozen or so words, which is roughly one per wrapped
        // row at this width -- the case where the tail overflows and a row is
        // committed to scrollback.
        if index % 12 == 11 {
            deltas.push("\n".to_owned());
        }
    }

    deltas
}

/// What one burst measured.
#[derive(Debug, Clone, Copy)]
struct Burst {
    /// Frames per second over the first window, with the tail still short.
    opening: f64,
    /// Frames per second over the last window, deep into the transcript.
    sustained: f64,
}

fn measure() -> Result<Burst, ProbeError> {
    let deltas = burst();
    let mut render = Renderer::new(Discard::open()?)?;

    let stream = |render: &mut Renderer<Discard>, index: usize| -> Result<(), ProbeError> {
        let delta = deltas.get(index % deltas.len()).map_or("", String::as_str);
        render.stream(delta)?;
        Ok(())
    };

    for index in 0..WARMUP {
        stream(&mut render, index)?;
    }

    let start = Instant::now();
    for index in 0..WINDOW {
        stream(&mut render, index)?;
    }
    let opening = start.elapsed();

    for index in WINDOW..FRAMES - WINDOW {
        stream(&mut render, index)?;
    }

    let late = Instant::now();
    for index in FRAMES - WINDOW..FRAMES {
        stream(&mut render, index)?;
    }
    let closing = late.elapsed();

    render.settle()?;

    // A window this size takes milliseconds, not nanoseconds, so the precision
    // lost converting the count is far below the noise in the measurement.
    #[allow(clippy::cast_precision_loss)]
    let frames = WINDOW as f64;

    Ok(Burst {
        opening: frames / opening.as_secs_f64(),
        sustained: frames / closing.as_secs_f64(),
    })
}

fn report(burst: Burst) -> Result<(), ProbeError> {
    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on inside a probe.
    let mut line = String::new();
    let _ = write!(line, "{:.1} commits/s {LIMIT:.0}", burst.sustained);
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()?;
    Ok(())
}

/// Says why the burst failed, on stderr, where `scripts/bench.sh` puts
/// everything a human reads. The one line on stdout stays the measurement.
fn explain(burst: Burst) -> Result<(), ProbeError> {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "    FAIL rendering slows as the transcript grows: \
         {:.0}/s at the start of the burst, {:.0}/s at the end \
         ({:.0}% of it, floor {:.0}%)",
        burst.opening,
        burst.sustained,
        burst.sustained / burst.opening * 100.0,
        SUSTAINED_FRACTION * 100.0,
    );

    io::stderr().write_all(line.as_bytes())?;
    Ok(())
}

fn main() -> ExitCode {
    let Ok(burst) = measure() else {
        return ExitCode::FAILURE;
    };

    if report(burst).is_err() {
        return ExitCode::FAILURE;
    }

    if burst.sustained < LIMIT {
        return ExitCode::FAILURE;
    }

    if burst.sustained < burst.opening * SUSTAINED_FRACTION {
        let _ = explain(burst);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
