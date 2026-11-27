//! The drawing thread's longest wait while a formula's picture is pending.
//!
//! A display formula is shown as its text when its block closes and is handed
//! to whatever makes its picture, which is TeX in the sandbox, seconds away at
//! worst. The drawing thread is the one that answers a key, so it must never
//! wait for that: it tells a formula and goes on, and looks for an answer each
//! time an input wait rests. The budget says no call the drawing thread makes
//! into the renderer while a picture is pending holds it longer than 20 ms,
//! the first frame's budget, so a key pressed while TeX works is answered as
//! promptly as the first frame is drawn.
//!
//! The renderer is the one a session builds, handed what a session on a
//! terminal that confirmed Kitty is handed: math drawn as pictures, the
//! drawers, and a picture maker that draws a picture's SVG to pixels and
//! encodes it, so an answered formula is placed in the same call that finds it.
//! What makes the pictures is one of two stand-ins for the session's engine,
//! each answering on a thread of its own as the session's task does: one that
//! takes 0.6 s over each formula and answers with the picture `dvisvgm` makes
//! of the largest display formula that draws within the raster's caps, and
//! one that never answers. The answer streams in deltas of a few characters,
//! and between deltas and after it the drawing thread rests as a session's
//! does, with nothing typed, taking each rest's work through
//! [`Renderer::waiting`](crucible_tui::Renderer::waiting) with no patience and
//! then sleeping the renderer's own look interval. Every `stream`, `settle` and
//! rest is timed while any formula told has no answer, and the longest is the
//! reading.
//!
//! Its rests read the probe's standard input as a session reads its terminal,
//! so standard input is made a pseudo-terminal of the probe's own with
//! nothing typed into it, whatever ran the probe. The measurement is refused
//! unless every formula was told, the slow engine answered every one and its
//! pictures went to the terminal, and the one that never answers was left
//! pending throughout.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex, PoisonError};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use crucible_raster::{Refused, png, raster_width, rasterise};
use crucible_tui::TerminalError;
#[cfg(target_os = "linux")]
use crucible_tui::chrome::Undrawn;
#[cfg(target_os = "linux")]
use crucible_tui::drawing::{Drawers, Mode, Svg};
#[cfg(target_os = "linux")]
use crucible_tui::graphics::{
    Capability, Cell, Form, Formula, Formulas, Payload, PictureKey, Pictures, Protocol,
};
#[cfg(target_os = "linux")]
use crucible_tui::{KeptPicture, Palette, Renderer, Size, Terminal, Theme};
#[cfg(target_os = "linux")]
use crucible_tui_math::Formulas as Maths;
#[cfg(target_os = "linux")]
use rustix::pty::{self, OpenptFlags};

/// The ceiling, in milliseconds, on any one call while a picture is pending.
#[cfg(target_os = "linux")]
const LIMIT: f64 = 20.0;

/// A window tall enough to show every formula of the answer at once, so each
/// picture made is sent to the terminal as it is placed.
#[cfg(target_os = "linux")]
const COLUMNS: usize = 80;
#[cfg(target_os = "linux")]
const ROWS: usize = 96;

/// The pixels of each of the window's cells, across and down.
#[cfg(target_os = "linux")]
const CELL: Cell = Cell {
    width: 8,
    height: 16,
};

/// How long the slow engine takes over each formula.
#[cfg(target_os = "linux")]
const MAKING: Duration = Duration::from_millis(600);

/// How long the drawing thread sleeps between rests while a formula is
/// pending: the renderer's own look interval.
#[cfg(target_os = "linux")]
const LOOK: Duration = Duration::from_millis(50);

/// How long the drawing thread goes on resting once the answer has streamed
/// with the engine that never answers.
#[cfg(target_os = "linux")]
const UNANSWERED: Duration = Duration::from_secs(2);

/// The longest either run may take before it is refused.
#[cfg(target_os = "linux")]
const CEILING: Duration = Duration::from_secs(20);

/// The longest a frame waits for the decode slot before it draws a picture as
/// text, as long as a session's waits.
#[cfg(target_os = "linux")]
const WAIT: Duration = Duration::from_millis(40);

/// The display formulas of the answer.
#[cfg(target_os = "linux")]
const FORMULAS: usize = 6;

/// The picture the slow engine answers every formula with: what `dvisvgm`
/// makes of `display-cases` through the session engine's own route, the one
/// the colour-on memory probe's build keeps.
#[cfg(target_os = "linux")]
const PICTURE: &str = include_str!("../cli/math_engine/display-cases.svg");

/// What can go wrong in the probe itself.
#[derive(Debug, thiserror::Error)]
enum ProbeError {
    /// The measurement could not be reported, or standard input not made a
    /// terminal.
    #[error("bench-picture-wait: {0}")]
    Io(#[from] io::Error),

    /// The renderer failed.
    #[error("bench-picture-wait: {0}")]
    Terminal(#[from] TerminalError),

    /// This probe's pseudo-terminal is platform-specific.
    #[cfg(not(target_os = "linux"))]
    #[error("bench-picture-wait: a pseudo-terminal for the rests requires Linux")]
    Unsupported,

    /// The stand-in picture was refused before any formula was made.
    #[cfg(target_os = "linux")]
    #[error("bench-picture-wait: the stand-in picture was refused: {0:?}")]
    Unpictured(Undrawn),

    /// The renderer told fewer formulas than the answer has, so the wait was
    /// measured over less than a session's.
    #[cfg(target_os = "linux")]
    #[error("bench-picture-wait: the {engine} engine was told {told} of {FORMULAS} formulas")]
    Untold { engine: &'static str, told: usize },

    /// The slow engine's pictures did not all come back and go out.
    #[cfg(target_os = "linux")]
    #[error(
        "bench-picture-wait: the slow engine answered {answered} of {FORMULAS} formulas and \
         {sent} pictures went to the terminal, within {CEILING:?}; the picture maker drew \
         {refused} as text{why}"
    )]
    Unanswered {
        answered: usize,
        sent: usize,
        refused: usize,
        why: String,
    },

    /// The engine that never answers was handed an answer anyway, so nothing
    /// was pending.
    #[cfg(target_os = "linux")]
    #[error("bench-picture-wait: the engine that never answers stopped pending")]
    Unpending,
}

/// What a stand-in has been told and has answered, shared between the
/// drawing thread's side and the thread that makes the pictures.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct Made {
    /// Every formula told, oldest first.
    told: usize,
    /// Answers made and not yet handed to the renderer.
    ready: Vec<(Formula, Option<Svg>)>,
    /// Answers handed to the renderer.
    answered: usize,
}

/// One stand-in for the session's engine, as the renderer holds it.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone)]
struct Engine {
    made: Arc<Mutex<Made>>,
    /// Where a formula told goes to be made; `None` for the engine that never
    /// answers.
    making: Option<std::sync::mpsc::Sender<Formula>>,
}

#[cfg(target_os = "linux")]
impl Engine {
    /// The engine that takes [`MAKING`] over each formula, on a thread of its
    /// own, and answers with `picture`.
    fn slow(picture: Svg) -> Result<Self, io::Error> {
        let made = Arc::new(Mutex::new(Made::default()));
        let (sender, formulas) = std::sync::mpsc::channel::<Formula>();
        let answering = Arc::clone(&made);
        std::thread::Builder::new()
            .name("crucible-picture-wait-engine".into())
            .spawn(move || {
                for formula in formulas {
                    std::thread::sleep(MAKING);
                    answering
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .ready
                        .push((formula, Some(picture.clone())));
                }
            })?;
        Ok(Self {
            made,
            making: Some(sender),
        })
    }

    /// The engine that is told every formula and never answers one.
    fn never() -> Self {
        Self {
            made: Arc::new(Mutex::new(Made::default())),
            making: None,
        }
    }

    fn held(&self) -> std::sync::MutexGuard<'_, Made> {
        self.made.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether a formula told has no answer the renderer has taken.
    fn waits(&self) -> bool {
        let made = self.held();
        made.answered < made.told
    }
}

#[cfg(target_os = "linux")]
impl Formulas for Engine {
    fn told(&mut self, formula: Formula) {
        self.held().told += 1;
        if let Some(making) = &self.making {
            let _ = making.send(formula);
        }
    }

    fn turns(&mut self) {}

    fn answered(&mut self) -> Option<(Formula, Option<Svg>)> {
        let mut made = self.held();
        if made.ready.is_empty() {
            return None;
        }
        let answer = made.ready.remove(0);
        made.answered += 1;
        Some(answer)
    }

    fn pending(&self) -> bool {
        self.waits()
    }
}

/// A terminal that keeps nothing it is written and counts the pictures sent
/// to it.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct Counted {
    /// Pictures transmitted.
    sent: usize,
}

#[cfg(target_os = "linux")]
impl Terminal for Counted {
    fn size(&self) -> Result<Size, TerminalError> {
        Ok(Size {
            columns: COLUMNS,
            rows: ROWS,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        self.sent += text.matches("\x1b_Ga=t,").count();
        std::hint::black_box(text);
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        Ok(())
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

/// A picture drawn to pixels and encoded for Kitty as a session's is: at the
/// narrowest of its own width and its room's, after a wait for the decode slot
/// no longer than a session's. It counts what it drew as text, and keeps the
/// first reason, so a run whose pictures never went out says why.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Default)]
struct Raster {
    refused: Arc<Mutex<(usize, Option<Undrawn>)>>,
}

#[cfg(target_os = "linux")]
impl Raster {
    fn drawn(&self, made: Result<Payload, Undrawn>) -> Result<Payload, Undrawn> {
        if let Err(why) = &made {
            let mut refused = self.refused.lock().unwrap_or_else(PoisonError::into_inner);
            refused.0 += 1;
            refused.1.get_or_insert(*why);
        }
        made
    }

    fn refusals(&self) -> (usize, Option<Undrawn>) {
        *self.refused.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(target_os = "linux")]
impl Pictures for Raster {
    fn cached(&mut self, _: &str, _: &PictureKey) -> Option<(Form, Payload)> {
        None
    }

    fn make(&mut self, _: &str, key: &PictureKey, form: &Form) -> Result<Payload, Undrawn> {
        self.drawn(encoded(form, key))
    }

    fn reencode(&mut self, kept: &KeptPicture, key: &PictureKey) -> Result<Payload, Undrawn> {
        self.drawn(encoded(&kept.form, key))
    }
}

/// `form` drawn in `key`'s palette at the width its room allows and encoded
/// for Kitty; or why it is drawn as text.
#[cfg(target_os = "linux")]
fn encoded(form: &Form, key: &PictureKey) -> Result<Payload, Undrawn> {
    let Form::Svg(svg) = form else {
        return Err(Undrawn::NotHere);
    };
    match key.protocol {
        Protocol::Kitty => {}
        Protocol::Iterm2 | Protocol::Sixel => return Err(Undrawn::NotHere),
    }
    let svg = svg.as_str().as_bytes();
    let room = u32::try_from(key.bucket.columns())
        .unwrap_or(u32::MAX)
        .saturating_mul(u32::from(key.cell.width));
    // Taken before the size is read, as a session's maker takes it, since
    // reading the size parses the whole picture.
    let _slot = crucible_raster::Slot::take(WAIT).map_err(refusal)?;
    let width = raster_width(svg).map_err(refusal)?.min(room);
    let pixels = rasterise(svg, width, &key.palette.inks()).map_err(refusal)?;
    let bytes = png(&pixels).map_err(refusal)?;
    Ok(Payload::new(
        Protocol::Kitty,
        bytes,
        (pixels.width(), pixels.height()),
        key.cell,
    ))
}

/// Why a picture the raster refused is drawn as text, as a session says it.
#[cfg(target_os = "linux")]
const fn refusal(refused: Refused) -> Undrawn {
    match refused {
        Refused::TooLarge => Undrawn::PastCap,
        Refused::NotAnImage | Refused::OverTime => Undrawn::NotHere,
    }
}

/// A palette that writes every hue it has, as a real terminal's does: without
/// one the renderer never reads the markdown, and no formula is told.
#[cfg(target_os = "linux")]
fn palette() -> Palette {
    Palette::resolve(true, Theme::Dark, Some((13, 13, 16)), &|name| {
        (name == "COLORTERM").then(|| "truecolor".to_owned())
    })
}

/// The answer, in deltas of a few characters: prose, then each display
/// formula with a line of prose after it.
#[cfg(target_os = "linux")]
fn streamed() -> Vec<String> {
    let mut answer = String::from("The rates the run measured, one formula for each:\n\n");
    for index in 1..=FORMULAS {
        let _ = write!(
            answer,
            "$$\nr_{index} = \\frac{{1}}{{n}} \\sum_{{i=1}}^{{n}} \\frac{{f_i}}{{t_i}}\n$$\n\n\
             The {index} rate is read off the formula above.\n\n"
        );
    }
    let characters: Vec<char> = answer.chars().collect();
    characters
        .chunks(5)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

/// Makes standard input a pseudo-terminal nothing is typed into, and keeps
/// its other end open for as long as the returned handle lives.
#[cfg(target_os = "linux")]
fn quiet_input() -> Result<std::fs::File, io::Error> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let terminal = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .map_err(io::Error::from)?;
    pty::grantpt(&terminal).map_err(io::Error::from)?;
    pty::unlockpt(&terminal).map_err(io::Error::from)?;
    let named = pty::ptsname(&terminal, Vec::new()).map_err(io::Error::from)?;
    let inside = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(OsStr::from_bytes(named.as_bytes()))?;
    rustix::stdio::dup2_stdin(&inside).map_err(io::Error::from)?;
    Ok(std::fs::File::from(terminal))
}

/// What one engine's run measured.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct Run {
    /// The longest call while a formula was pending, in milliseconds.
    longest: f64,
    /// The calls timed while a formula was pending.
    calls: usize,
    /// Formulas the engine was told.
    told: usize,
    /// Answers the renderer took.
    answered: usize,
    /// Pictures sent to the terminal.
    sent: usize,
    /// Pictures the maker drew as text, and the first reason.
    refused: (usize, Option<Undrawn>),
}

/// The longest of the calls timed so far, and how many there were.
#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct Timed {
    longest: Duration,
    calls: usize,
}

#[cfg(target_os = "linux")]
impl Timed {
    /// Runs `call`, counting its length when a formula was pending as it began
    /// or as it ended.
    fn call<R>(
        &mut self,
        engine: &Engine,
        call: impl FnOnce() -> Result<R, TerminalError>,
    ) -> Result<R, TerminalError> {
        let before = engine.waits();
        let began = Instant::now();
        let result = call();
        let took = began.elapsed();
        if before || engine.waits() {
            self.longest = self.longest.max(took);
            self.calls += 1;
        }
        result
    }
}

/// The answer streamed and rested over with `engine` making its pictures,
/// resting on after it until `done` says the run is over.
#[cfg(target_os = "linux")]
fn run(engine: &Engine, done: impl Fn(&Engine, Duration) -> bool) -> Result<Run, ProbeError> {
    let mut render = Renderer::new(Counted::default());
    render.wears(palette());
    render.drawers(Drawers::default().with(Arc::new(Maths)));
    render.math(Mode::Image);
    render.images(Capability::Graphics {
        protocol: Protocol::Kitty,
        cell: CELL,
    });
    let raster = Raster::default();
    render.pictures(Box::new(raster.clone()));
    render.makes_formulas(Box::new(engine.clone()));
    render.starts_turn();

    let mut timed = Timed::default();
    let began = Instant::now();
    for delta in streamed() {
        timed.call(engine, || render.stream(&delta))?;
        timed.call(engine, || render.waiting(Duration::ZERO))?;
    }
    timed.call(engine, || render.settle())?;
    let settled = Instant::now();
    while !done(engine, settled.elapsed()) && began.elapsed() < CEILING {
        timed.call(engine, || render.waiting(Duration::ZERO))?;
        std::thread::sleep(LOOK);
    }

    let made = engine.held();
    Ok(Run {
        longest: timed.longest.as_secs_f64() * 1000.0,
        calls: timed.calls,
        told: made.told,
        answered: made.answered,
        sent: render.terminal().sent,
        refused: raster.refusals(),
    })
}

#[cfg(target_os = "linux")]
fn measure() -> Result<(Run, Run), ProbeError> {
    let _input = quiet_input()?;
    let picture = Svg::new(PICTURE.to_owned()).map_err(ProbeError::Unpictured)?;

    let slow = Engine::slow(picture)?;
    let made = run(&slow, |engine, _| {
        let made = engine.held();
        made.told >= FORMULAS && made.answered >= made.told
    })?;
    if made.told != FORMULAS {
        return Err(ProbeError::Untold {
            engine: "slow",
            told: made.told,
        });
    }
    if made.answered != FORMULAS || made.sent < FORMULAS {
        return Err(ProbeError::Unanswered {
            answered: made.answered,
            sent: made.sent,
            refused: made.refused.0,
            why: made
                .refused
                .1
                .map(|why| format!(", first as {why:?}"))
                .unwrap_or_default(),
        });
    }

    let never = Engine::never();
    let unmade = run(&never, |_, rested| rested >= UNANSWERED)?;
    if unmade.told != FORMULAS {
        return Err(ProbeError::Untold {
            engine: "never-answering",
            told: unmade.told,
        });
    }
    if unmade.answered != 0 || !never.waits() {
        return Err(ProbeError::Unpending);
    }
    Ok((made, unmade))
}

#[cfg(not(target_os = "linux"))]
fn measure() -> Result<((), ()), ProbeError> {
    Err(ProbeError::Unsupported)
}

#[cfg(target_os = "linux")]
fn report(slow: Run, never: Run) -> Result<f64, ProbeError> {
    let longest = slow.longest.max(never.longest);
    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on.
    let mut line = String::new();
    let _ = writeln!(
        line,
        "{longest:.2} ms {LIMIT:.0} slow={:.2} never={:.2} slow_calls={} never_calls={} \
         told={} placed={}",
        slow.longest, never.longest, slow.calls, never.calls, slow.told, slow.sent,
    );
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()?;

    let mut evidence = String::new();
    let _ = writeln!(
        evidence,
        "         longest call while a picture was pending: {:.2} ms with an engine taking \
         {MAKING:?} a formula ({} calls, {} pictures placed), {:.2} ms with one that never \
         answers ({} calls)",
        slow.longest, slow.calls, slow.sent, never.longest, never.calls,
    );
    io::stderr().write_all(evidence.as_bytes())?;
    Ok(longest)
}

/// Says why no reading could be taken at all, on stderr, where
/// `scripts/sh/bench.sh` puts everything a human reads.
fn explain(problem: &ProbeError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL {problem}");
    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let measured = match measure() {
        Ok(measured) => measured,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };
    #[cfg(target_os = "linux")]
    {
        let (slow, never) = measured;
        match report(slow, never) {
            Ok(longest) if longest <= LIMIT => ExitCode::SUCCESS,
            Ok(_) => ExitCode::FAILURE,
            Err(problem) => {
                let _ = explain(&problem);
                ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let ((), ()) = measured;
        ExitCode::FAILURE
    }
}
