//! Render throughput under a token burst.
//!
//! A model streaming at full speed hands the renderer a delta every few
//! milliseconds, and every delta is a frame: the text is folded into the record,
//! the lines the transcript band covers are laid out, and the rows whose picture
//! is not already on screen are written. The budget says at least thirty of
//! those a second, which is the rate below which streamed text stops looking
//! like typing and starts looking like stalling.
//!
//! Frames is what this counts and frames is what it reports. A frame is one
//! `stream` call; the renderer's other ways in put a finished line or a
//! component's rows into the same record, and no part of this burst performs
//! either.
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
//! close. What each of those two readings is made of is `burst`'s, beside this
//! file, and is shared with the probe that measures the turn band. That is the
//! check that actually bites, and on this renderer it guards the property the
//! whole design rests on: a frame folds only the lines the band covers, so its
//! cost is the window's and not the session's. The record grows for the whole of
//! this burst underneath it. A frame that reached past the band -- folding
//! everything it holds, or searching from the top for where the foot is -- is
//! fast in the first second and hopeless in the hundredth, and the ratio is
//! where that shows. The reported number is the *sustained* rate, since a
//! session is long and the opening frames are not the ones a user is waiting on.

// A binary may not reach into another's tree, so the driver the two burst probes
// share is a module beside them. Where the bounded pipe below is unavailable this
// probe reports itself unsupported before it draws a frame, and the driver it
// would have run has no caller — it is still compiled, and still tested.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod burst;

use burst::{Burst, SUSTAINED_FRACTION};
use crucible_tui::TerminalError;
#[cfg(target_os = "linux")]
use crucible_tui::{Renderer, Size, Terminal};
#[cfg(target_os = "linux")]
use rustix::pipe::{PipeFlags, pipe_with};
use std::fmt::Write as _;
#[cfg(target_os = "linux")]
use std::fs::File;
#[cfg(target_os = "linux")]
use std::io::Read as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::thread::JoinHandle;

/// The floor, in frames per second.
const LIMIT: f64 = 30.0;

/// A terminal-sized window, so wrapping and a band that scrolls both engage.
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
fn streamed() -> Vec<String> {
    // A fenced block, opened once and never closed, so that most of what this
    // streams is code being read rather than prose being scanned. Reading is
    // the more expensive of the two by a wide margin — a parser and a theme
    // against a marker scan — and it is the half that arrived most recently, so
    // it is the half a burst has to cover. Without this the probe would go on
    // measuring the cheaper path and report the budget held.
    let opening = "```rust\n".to_owned();

    let words = [
        "let ",
        "runner ",
        "= ",
        "Runner::new(1); ",
        "// ",
        "over ",
        "traits ",
        "alone ",
        "fn ",
        "provider() ",
        "-> ",
        "String ",
        "{ ",
        "\"tool\" } ",
    ];

    let mut deltas = Vec::with_capacity(258);
    deltas.push(opening);

    for (index, word) in words.iter().cycle().take(256).enumerate() {
        deltas.push((*word).to_owned());

        // A newline every dozen or so words, which is roughly one per wrapped
        // row at this width -- the frame where the band scrolls. Every row of it
        // moves up one, so every row differs from what the last frame left and
        // the diff saves nothing. It is the dearest frame there is, and a burst
        // that never reached one would report the cheap frame as the rate.
        if index % 12 == 11 {
            deltas.push("\n".to_owned());
        }
    }

    deltas
}

#[cfg(target_os = "linux")]
fn measure() -> Result<Burst, ProbeError> {
    let deltas = streamed();
    let (sink, drain) = PipeSink::open()?;
    let mut render = Renderer::new(sink);

    // A palette that writes every hue it has. Without one the renderer takes
    // the early path in `stream` and never reads the markdown at all — so the
    // markers, the fence and the highlighter behind it would all go unmeasured,
    // and this probe would report a budget held for a path nobody ran. A real
    // terminal has colour; so does this.
    render.wears(crucible_tui::Palette::resolve(
        true,
        crucible_tui::Theme::Dark,
        Some((13, 13, 16)),
        &|name| (name == "COLORTERM").then(|| "truecolor".to_owned()),
    ));

    let measured = burst::measure(|index| -> Result<(), ProbeError> {
        let delta = deltas.get(index % deltas.len()).map_or("", String::as_str);
        render.stream(delta)?;
        Ok(())
    })?;

    render.settle()?;
    // Closing the writer is what tells the fixed-size consumer it has seen the
    // complete burst. Join it so a read error cannot be mistaken for a rate.
    drop(render);
    drain.finish()?;

    Ok(measured)
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
    let measured = match measure() {
        Ok(measured) => measured,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    if report(measured).is_err() {
        return ExitCode::FAILURE;
    }
    if evidence(measured).is_err() {
        return ExitCode::FAILURE;
    }

    if measured.sustained < LIMIT {
        return ExitCode::FAILURE;
    }

    if measured.sustained < measured.opening * SUSTAINED_FRACTION {
        let _ = slowing(measured);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
