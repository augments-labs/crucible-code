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
//!
//! The renderer is handed the drawer a session's is, so the diff fences in the
//! burst are read and drawn as the changes a session draws them as, and not
//! shown as the text they were written as, which costs less than a user pays.

// A binary may not reach into another's tree, so the driver the two burst probes
// share is a module beside them. Where the bounded pipe below is unavailable this
// probe reports itself unsupported before it draws a frame, and the driver it
// would have run has no caller — it is still compiled, and still tested.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod burst;

use burst::{Burst, SUSTAINED_FRACTION};
use crucible_tui::TerminalError;
#[cfg(target_os = "linux")]
use crucible_tui::drawing::Drawers;
#[cfg(target_os = "linux")]
use crucible_tui::{Palette, Renderer, Size, Terminal, Theme};
#[cfg(target_os = "linux")]
use crucible_tui_diff::Changes;
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
use std::sync::Arc;
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
    // A fenced block, opened once a cycle and closed only at its end, so that
    // most of what this streams is code being read rather than prose being
    // scanned. Reading is the more expensive of the two by a wide margin — a
    // parser and a theme against a marker scan — and without this the probe
    // would go on measuring the cheaper path and report the budget held.
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

    let answers = burst::answers();
    let after: usize = answers.iter().map(String::len).sum();
    let mut deltas = Vec::with_capacity(258 + (HELD.len() + after).div_ceil(5) + answers.len());
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

    // Then the fence closes and the blocks a reader holds follow it, streamed
    // in the same few characters at a time. Each is gathered whole before it
    // is drawn, with one row saying so in the meantime, and then kept with its
    // source so a resize can lay it out again, and a burst that never reached
    // one would leave that work unmeasured.
    //
    // Last come the answers both burst probes hold, two changes more: one to
    // two files and one long enough to fold, so the burst draws every kind of
    // row a change has.
    for text in std::iter::once(HELD).chain(answers.iter().map(String::as_str)) {
        let held: Vec<char> = text.chars().collect();
        deltas.extend(held.chunks(5).map(|chunk| chunk.iter().collect::<String>()));
    }

    deltas
}

/// What follows the code in every cycle of the burst: a table, a formula, a
/// diagram and a diff, each closed before the next opens.
///
/// The diff is one a drawer reads, with the file it is made to and a hunk that
/// counts its lines: one that is not is shown as written, and would be timed as
/// code.
#[cfg(target_os = "linux")]
const HELD: &str = "\n```\n\n\
    | probe | budget | measured | verdict |\n\
    | --- | ---: | ---: | :---: |\n\
    | render burst | 30/s | 412/s | held |\n\
    | live burst | 30/s | 388/s | held |\n\
    | session rss | 35 MB | 21 MB | held |\n\
    | first frame | 60 ms | 18 ms | held |\n\
    | read back | 120 ms | 44 ms | held |\n\
    \n\
    $$\n\
    \\sum_{i=1}^{n} \\frac{x_i}{2} = \\sqrt{y}\n\
    $$\n\
    \n\
    ```mermaid\n\
    graph TD\n\
    \x20 reader --> record\n\
    \x20 record --> window\n\
    ```\n\
    \n\
    ```diff\n\
    --- a/src/record.rs\n\
    +++ b/src/record.rs\n\
    @@ -7,3 +7,3 @@\n\
    \x20let block = held(text);\n\
    -let rows = lay(text);\n\
    +let rows = lay(&block, columns);\n\
    \x20keep(block, rows);\n\
    ```\n\
    \n";

/// The drawers the renderer is handed, as a session's is: the one that draws a
/// change, for the diff fences of the burst.
#[cfg(target_os = "linux")]
fn drawers() -> Drawers {
    Drawers::default().with(Arc::new(Changes))
}

/// A palette that writes every hue it has.
///
/// Without one the renderer takes the early path in `stream` and never reads
/// the markdown at all — so the markers, the fences, the highlighter and the
/// drawers behind them would all go unmeasured, and this probe would report a
/// budget held for a path nobody ran. A real terminal has colour; so does this.
#[cfg(target_os = "linux")]
fn palette() -> Palette {
    Palette::resolve(true, Theme::Dark, Some((13, 13, 16)), &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

#[cfg(target_os = "linux")]
fn measure() -> Result<Burst, ProbeError> {
    let deltas = streamed();
    let (sink, drain) = PipeSink::open()?;
    let mut render = Renderer::new(sink);
    render.wears(palette());
    render.drawers(drawers());

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
    io::stdout().flush()?;
    Ok(())
}

/// Says why no reading could be taken at all, on stderr, where
/// `scripts/sh/bench.sh` puts everything a human reads.
///
/// Without it an unopenable discard file, or a renderer that failed mid-burst,
/// reaches the operator as an empty line on stdout — reported as malformed
/// output, which is the one thing it is not.
fn explain(problem: &ProbeError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL {problem}");

    io::stderr().write_all(line.as_bytes())
}

/// Says why the burst failed, on stderr, where `scripts/sh/bench.sh` puts
/// everything a human reads. The one line on stdout stays the measurement.
fn slowing(burst: Burst) -> Result<(), ProbeError> {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "    FAIL rendering slows as the transcript grows: against a fixed \
         yardstick a frame at the end of the burst kept {:.0}% of the pace one \
         at the start had (floor {:.0}%); {:.0}/s at the start, {:.0}/s at the \
         end",
        burst.ratio() * 100.0,
        SUSTAINED_FRACTION * 100.0,
        burst.opening.rate,
        burst.sustained.rate,
    );

    io::stderr().write_all(line.as_bytes())?;
    Ok(())
}

/// Human-readable evidence for every run, including a passing one.
fn evidence(burst: Burst) -> Result<(), ProbeError> {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "         render opening {:.0}/s, sustained {:.0}/s, pace kept {:.1}%",
        burst.opening.rate,
        burst.sustained.rate,
        burst.ratio() * 100.0,
    );
    io::stderr().write_all(line.as_bytes())?;
    Ok(())
}

fn main() -> ExitCode {
    let measured = match burst::best(measure) {
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

    if measured.sustained.rate < LIMIT {
        return ExitCode::FAILURE;
    }

    if measured.ratio() < SUSTAINED_FRACTION {
        let _ = slowing(measured);
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use crucible_tui::Recording;

    use super::*;

    /// One cycle of the burst as a window tall enough for all of it shows it,
    /// drawn by a renderer handed `drawers`.
    fn cycle(drawers: Drawers) -> Vec<String> {
        let mut render = Renderer::new(Recording::new(COLUMNS, 400));
        render.wears(palette());
        render.drawers(drawers);
        for delta in streamed() {
            render.stream(&delta).expect("a delta drawn");
        }
        render.settle().expect("the answer settled");
        render
            .terminal()
            .picture()
            .rows()
            .iter()
            .map(|row| row.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn every_diff_fence_of_the_burst_is_drawn_as_the_change_a_session_draws() {
        let rows = cycle(drawers());
        for opening in [
            "╭─ src/record.rs · Added 1 line, removed 1 line",
            "╭─ src/reader.rs · Added 2 lines, removed 1 line",
            "├─ src/window.rs · Added 1 line, removed 1 line",
            "╭─ budgets.txt · Added 40 lines",
        ] {
            assert!(
                rows.iter().any(|row| row.trim_start() == opening),
                "no row {opening:?} in {rows:#?}"
            );
        }
        assert!(
            rows.iter()
                .any(|row| row.contains("lines folded · ctrl+o to expand")),
            "the long change did not fold: {rows:#?}"
        );
        for written in ["┌─ diff", "┌─ patch", "@@ "] {
            assert!(
                !rows.iter().any(|row| row.contains(written)),
                "a diff fence was shown as written, with {written:?}: {rows:#?}"
            );
        }

        // What the assertions above tell apart: handed no drawer, the same
        // burst shows each fence as the text it was written as.
        let undrawn = cycle(Drawers::default());
        assert!(
            undrawn.iter().any(|row| row.contains("┌─ diff")),
            "{undrawn:#?}"
        );
        assert!(
            undrawn.iter().any(|row| row.contains("┌─ patch")),
            "{undrawn:#?}"
        );
        assert!(
            !undrawn.iter().any(|row| row.contains("╭─ ")),
            "{undrawn:#?}"
        );
    }
}
