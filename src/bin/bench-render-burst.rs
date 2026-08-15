//! Render throughput under a token burst.
//!
//! A model streaming at full speed hands the renderer a delta every few
//! milliseconds, and every delta is a frame: wrap, rewind, erase, redraw. The
//! budget says at least thirty of those a second, which is the rate below which
//! streamed text stops looking like typing and starts looking like stalling.
//!
//! Frames is what this counts and frames is what it reports. A frame is one
//! `stream` call, and the renderer's other job -- writing a finished line to
//! scrollback -- is a different operation that no part of this burst performs.
//!
//! Measured against a bounded kernel pipe rather than an in-memory buffer or
//! `/dev/null`. A drain thread consumes the pipe in fixed-size reads, so writes
//! and flushes are real syscalls and a producer that outruns its consumer meets
//! kernel backpressure instead of growing a heap buffer. The sink reports itself
//! as a terminal, so escape assembly is measured too.
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
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::Read as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::thread::JoinHandle;
#[cfg(target_os = "linux")]
use std::time::Instant;

use crucible_tui::TerminalError;
#[cfg(target_os = "linux")]
use crucible_tui::{Renderer, Size, Terminal};
#[cfg(target_os = "linux")]
use rustix::pipe::{PipeFlags, pipe_with};

/// The floor, in frames per second.
const LIMIT: f64 = 30.0;

/// Frames to measure. Large enough that a scheduler hiccup does not decide the
/// answer, small enough that the probe stays under a second on a slow machine.
#[cfg(target_os = "linux")]
const FRAMES: usize = 20_000;

/// Frames in each of the two timed windows, at the start and the end.
#[cfg(target_os = "linux")]
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
#[cfg(target_os = "linux")]
const WARMUP: usize = 2_000;

/// A terminal-sized window, so wrapping and the bounded tail both engage.
#[cfg(target_os = "linux")]
const COLUMNS: usize = 80;
#[cfg(target_os = "linux")]
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

    /// This probe's bounded descriptor implementation is platform-specific.
    #[cfg(not(target_os = "linux"))]
    #[error("bench-render-burst: bounded pipe measurements require Linux")]
    Unsupported,

    /// The fixed-size drain could not finish after the renderer closed.
    #[cfg(target_os = "linux")]
    #[error("bench-render-burst: pipe drain panicked")]
    DrainPanicked,
}

/// The write end of a bounded kernel pipe, pretending to be a terminal.
///
/// The renderer asks whether it is talking to a terminal in order to decide
/// whether to emit cursor movement at all, and the escape sequences are part of
/// what is being measured.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct PipeSink {
    out: File,
}

/// The fixed-memory consumer for [`PipeSink`].
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Drain {
    thread: JoinHandle<Result<(), io::Error>>,
}

#[cfg(target_os = "linux")]
impl PipeSink {
    fn open() -> Result<(Self, Drain), io::Error> {
        let (read, write) = pipe_with(PipeFlags::CLOEXEC).map_err(io::Error::from)?;
        let thread = std::thread::Builder::new()
            .name("crucible-render-drain".into())
            .spawn(move || drain(File::from(read)))?;
        Ok((
            Self {
                out: File::from(write),
            },
            Drain { thread },
        ))
    }
}

#[cfg(target_os = "linux")]
impl Drain {
    fn finish(self) -> Result<(), ProbeError> {
        self.thread
            .join()
            .map_err(|_| ProbeError::DrainPanicked)??;
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn drain(mut input: File) -> Result<(), io::Error> {
    let mut buffer = [0_u8; 4096];
    loop {
        let read = input.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        std::hint::black_box(buffer.get(..read));
    }
}

#[cfg(target_os = "linux")]
impl Terminal for PipeSink {
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
#[cfg(target_os = "linux")]
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

impl Burst {
    fn ratio(self) -> f64 {
        self.sustained / self.opening
    }
}

#[cfg(target_os = "linux")]
fn measure() -> Result<Burst, ProbeError> {
    let deltas = burst();
    let (sink, drain) = PipeSink::open()?;
    let mut render = Renderer::new(sink);

    let stream = |render: &mut Renderer<PipeSink>, index: usize| -> Result<(), ProbeError> {
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
    // Closing the writer is what tells the fixed-size consumer it has seen the
    // complete burst. Join it so a read error cannot be mistaken for a rate.
    drop(render);
    drain.finish()?;

    // A window this size takes milliseconds, not nanoseconds, so the precision
    // lost converting the count is far below the noise in the measurement.
    #[allow(clippy::cast_precision_loss)]
    let frames = WINDOW as f64;

    Ok(Burst {
        opening: frames / opening.as_secs_f64(),
        sustained: frames / closing.as_secs_f64(),
    })
}

#[cfg(not(target_os = "linux"))]
fn measure() -> Result<Burst, ProbeError> {
    Err(ProbeError::Unsupported)
}

fn report(burst: Burst) -> Result<(), ProbeError> {
    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on inside a probe.
    let mut line = String::new();
    let _ = write!(
        line,
        "{:.1} frames/s {LIMIT:.0} opening={:.1} sustained={:.1} ratio={:.3}",
        burst.sustained,
        burst.opening,
        burst.sustained,
        burst.ratio(),
    );
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()?;
    Ok(())
}

/// Says why no reading could be taken at all, on stderr, where
/// `scripts/bench.sh` puts everything a human reads.
///
/// Without it an unopenable discard file, or a renderer that failed mid-burst,
/// reaches the operator as an empty line on stdout — reported as malformed
/// output, which is the one thing it is not.
fn explain(problem: &ProbeError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says why the burst failed, on stderr, where `scripts/bench.sh` puts
/// everything a human reads. The one line on stdout stays the measurement.
fn slowing(burst: Burst) -> Result<(), ProbeError> {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "    FAIL rendering slows as the transcript grows: \
         {:.0}/s at the start of the burst, {:.0}/s at the end \
         ({:.0}% of it, floor {:.0}%)",
        burst.opening,
        burst.sustained,
        burst.ratio() * 100.0,
        SUSTAINED_FRACTION * 100.0,
    );

    io::stderr().write_all(line.as_bytes())?;
    Ok(())
}

/// Human-readable evidence for every run, including a passing one.
fn evidence(burst: Burst) -> Result<(), ProbeError> {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "         render opening {:.0}/s, sustained {:.0}/s, ratio {:.1}%",
        burst.opening,
        burst.sustained,
        burst.ratio() * 100.0,
    );
    io::stderr().write_all(line.as_bytes())?;
    Ok(())
}

fn main() -> ExitCode {
    let burst = match measure() {
        Ok(burst) => burst,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    if report(burst).is_err() {
        return ExitCode::FAILURE;
    }
    if evidence(burst).is_err() {
        return ExitCode::FAILURE;
    }

    if burst.sustained < LIMIT {
        return ExitCode::FAILURE;
    }

    if burst.sustained < burst.opening * SUSTAINED_FRACTION {
        let _ = slowing(burst);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
