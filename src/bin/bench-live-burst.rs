//! Turn-band throughput while a command is printing.
//!
//! A running command hands over what it has printed every twenty milliseconds
//! and the footing over the box is laid out and drawn again for it: the sample
//! rows change, the count row changes, and the band they stand in is written
//! again. That is a different operation from the one `bench-render-burst`
//! measures — that one counts `stream` calls, which fold arriving text into the
//! record the transcript band is drawn from; this one counts `under` calls,
//! which stand a component's rows in a band of their own and leave the record
//! alone.
//!
//! Both are held to the same floor, because the reason for the floor is the same:
//! below thirty a second a picture that is meant to be moving reads as stalled.
//!
//! What is measured is a frame's whole cost and not the renderer's share of it.
//! The rows are laid out inside the timed loop, because a real frame lays them
//! out too — a probe that built them once would report the cost of writing bytes
//! and miss the cost of deciding which bytes.
//!
//! Measured against a bounded kernel pipe rather than a buffer, so writes and
//! flushes are real syscalls and a producer that outruns its consumer meets
//! kernel backpressure instead of growing a heap buffer. The sink reports itself
//! as a terminal because `under` does nothing at all where output is redirected
//! — a band is a thing only a screen has — so against anything else there would
//! be no frame to time.
//!
//! Twice, and the two must be close. How each of those two rates is arrived at
//! is `burst`'s, beside this file, and is shared with the probe that streams.
//! What the ratio catches here is not what it catches there, because nothing is
//! streaming and the record stays empty for the whole burst: it is a frame that
//! keeps something. These rows stand over the transcript and never join it, so a
//! burst of them has to leave the renderer holding what one frame left it
//! holding. A frame that accumulated — rows appended where they should have
//! replaced, anything growing with the count of frames rather than with the
//! window — is fast in the first second and hopeless in the hundredth, and the
//! ratio is the only place it shows.

// A binary may not reach into another's tree, so the driver the two burst probes
// share is a module beside them. Where the bounded pipe below is unavailable this
// probe reports itself unsupported before it draws a frame, and the driver it
// would have run has no caller — it is still compiled, and still tested.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod burst;

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
use std::time::Duration;

use burst::{Burst, SUSTAINED_FRACTION};
use crucible_tui::TerminalError;
#[cfg(target_os = "linux")]
use crucible_tui::{Glyphs, Palette, Renderer, Row, Size, Slot, Terminal, Theme, Working};
#[cfg(target_os = "linux")]
use rustix::pipe::{PipeFlags, pipe_with};

/// The floor, in frames per second.
const LIMIT: f64 = 30.0;

/// A terminal-sized window, so the turn band is as tall as a real one leaves it.
#[cfg(target_os = "linux")]
const COLUMNS: usize = 80;
#[cfg(target_os = "linux")]
const ROWS: usize = 24;

/// How many rows of a command's output the footing shows, which is the figure the
/// footing itself is built to.
#[cfg(target_os = "linux")]
const SAMPLE: usize = 5;

/// What can go wrong in the probe itself.
#[derive(Debug, thiserror::Error)]
enum ProbeError {
    /// The measurement could not be reported.
    #[error("bench-live-burst: {0}")]
    Io(#[from] io::Error),

    /// The renderer failed mid-burst.
    #[error("bench-live-burst: {0}")]
    Terminal(#[from] TerminalError),

    /// This probe's bounded descriptor implementation is platform-specific.
    #[cfg(not(target_os = "linux"))]
    #[error("bench-live-burst: bounded pipe measurements require Linux")]
    Unsupported,

    /// The fixed-size drain could not finish after the renderer closed.
    #[cfg(target_os = "linux")]
    #[error("bench-live-burst: pipe drain panicked")]
    DrainPanicked,
}

/// The write end of a bounded kernel pipe, pretending to be a terminal.
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
            .name("crucible-live-drain".into())
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

/// Lines shaped like a build's, which is the command this surface was built for.
///
/// Built once, so the timed loop is not measuring a formatter — the layout it
/// *is* measuring happens inside it.
#[cfg(target_os = "linux")]
fn printed() -> Vec<String> {
    (0..256)
        .map(|line| format!("   Compiling crucible-part-{line} v0.5.0 (crates/part-{line})"))
        .collect()
}

/// The footing as it stands over the box while a command is printing.
///
/// The shape rather than the caller: the rows a running call puts there are a
/// call line, the sample, the row counting it, and the row saying the turn is
/// running. Built here because a probe may not reach into the binary's own tree.
#[cfg(target_os = "linux")]
fn footing(lines: &[String], from: usize, running: Duration) -> Vec<Row> {
    let glyphs = Glyphs::Unicode;
    let mut rows = vec![
        Row::new(),
        Row::new()
            .then(Slot::Accent, glyphs.called())
            .then(Slot::Strong, " Bash")
            .then(Slot::Quiet, "(cargo build --release)"),
    ];

    for offset in 0..SAMPLE {
        let at = from.saturating_add(offset) % lines.len();
        let line = lines.get(at).map_or("", String::as_str);

        rows.push(Row::new().then(
            Slot::Quiet,
            format!(
                "    {}",
                crucible_tui::clip(line, COLUMNS.saturating_sub(4))
            ),
        ));
    }

    // The count moves every frame, as it does on screen: it is what keeps the
    // sample from reading as the whole of what the command has said.
    rows.push(Row::new().then(
        Slot::Quiet,
        format!("    {} lines · {}.{} kB", from, from / 20, from % 10),
    ));

    rows.push(Row::new());
    rows.push(
        Working {
            doing: "running",
            running,
            spent: Some(1_200),
            stops: Some("esc to interrupt"),
        }
        .row(COLUMNS, glyphs),
    );
    rows.push(Row::new());

    rows
}

#[cfg(target_os = "linux")]
fn measure() -> Result<Burst, ProbeError> {
    let lines = printed();
    let (sink, drain) = PipeSink::open()?;
    let mut render = Renderer::new(sink);
    let palette = Palette::resolve(true, Theme::Dark, None, &|_| None);

    let measured = burst::measure(|index| -> Result<(), ProbeError> {
        // Laid out inside the frame, because a real one is. The clock moves with
        // the index so the row saying a turn is running changes as it does on
        // screen.
        let rows = footing(&lines, index, Duration::from_millis(index as u64 * 16));
        render.under(&rows, None, palette)?;
        Ok(())
    })?;

    render.settle()?;
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
        "{:.1} frames/s {LIMIT:.0} opening={:.1} sustained={:.1} \
         opening_pace={:.1} sustained_pace={:.1} ratio={:.3}",
        burst.sustained.rate,
        burst.opening.rate,
        burst.sustained.rate,
        burst.opening.pace,
        burst.sustained.pace,
        burst.ratio(),
    );
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    Ok(())
}

fn main() -> ExitCode {
    let measured = match measure() {
        Ok(measured) => measured,
        Err(problem) => {
            let mut said = problem.to_string();
            said.push('\n');
            let _ = io::stderr().write_all(said.as_bytes());

            return ExitCode::FAILURE;
        }
    };

    if report(measured).is_err() {
        return ExitCode::FAILURE;
    }

    if measured.sustained.rate < LIMIT || measured.ratio() < SUSTAINED_FRACTION {
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
