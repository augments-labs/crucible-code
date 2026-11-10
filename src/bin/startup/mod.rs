//! Timing the real binary from `exec` through a real terminal.
//!
//! Shared by the probes that time a screen. Measuring in-process would leave out
//! everything that actually costs a startup — the exec, dynamic loader, first
//! page faults and binary size — so this spawns `crucible` the way a person does
//! and reads what a terminal receives.
//!
//! The child gets the far side of a pseudo terminal as its controlling terminal,
//! in raw mode and at a fixed size. The terminal path therefore includes raw
//! input, window-size discovery, escape assembly and every flush hidden by a
//! redirected-output probe. The clock starts before `spawn`, so the small cost
//! of `setsid` and the probe's own fork is charged too; that makes the reading a
//! little worse than a shell launch, which is the safe direction for a budget.
//!
//! A screen reached by typing is timed from the keystroke instead, because the
//! launch under it is already two budgets of its own.

use std::collections::HashSet;
#[cfg(target_os = "linux")]
use std::ffi::OsStr;
use std::ffi::OsString;
use std::fs;
#[cfg(target_os = "linux")]
use std::fs::{File, OpenOptions};
use std::io;
#[cfg(target_os = "linux")]
use std::io::{Read as _, Write as _};
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt as _;
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::{Child, Command, Stdio};
#[cfg(target_os = "linux")]
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
#[cfg(target_os = "linux")]
use std::thread::JoinHandle;
use std::time::Duration;
#[cfg(target_os = "linux")]
use std::time::Instant;

#[cfg(target_os = "linux")]
use rustix::pty::{self, OpenptFlags};
#[cfg(target_os = "linux")]
use rustix::termios::{self, OptionalActions, Winsize};

/// Runs in one window.
///
/// The percentile below is nearest-rank, so the size of what it is taken over
/// decides how far into the tail it stands: over twenty readings the 95th is
/// the second worst, which is one slow launch away from deciding a budget.
/// Twenty-two puts it at the second worst still, and that is the point — a
/// window is *meant* to be decidable by a stall, so that the median across
/// windows can throw the stalled ones away.
const PER_WINDOW: usize = 22;

/// Windows the runs are cut into.
///
/// Odd, so the median is a window that was measured rather than the average of
/// two that were. Nine of them means five have to be disturbed together before
/// the reading is anything but what this program does.
///
/// The same shape as the burst probes' phases, and for the same reason. A
/// percentile over one long run is decided by whatever else the machine was
/// doing while it ran, and on a shared runner that is another job's build: a
/// stall lands in the window it happened in, while a program that got slower is
/// slower in all nine. Taking the tail of each window and the middle of those
/// keeps the statistic a tail — it is still the second worst of twenty-two —
/// and stops one bad stretch of wall clock from being the answer.
const WINDOWS: usize = 9;

/// Runs per reading.
///
/// A launch costs a few milliseconds, except one whose reading is the first
/// input on a picture candidate that never answers: that input waits out the
/// picture exchange's reply wait, so a batch of those takes at least `RUNS`
/// times that wait.
const RUNS: usize = WINDOWS * PER_WINDOW;

/// A stuck launch is a failed measurement rather than a benchmark that hangs.
#[cfg(target_os = "linux")]
const CEILING: Duration = Duration::from_secs(5);

/// A normal terminal-sized window, fixed so layout work is the same everywhere.
#[cfg(target_os = "linux")]
const COLUMNS: u16 = 80;
#[cfg(target_os = "linux")]
const ROWS: u16 = 24;

/// The pixels of one cell the window reports, as a terminal that can draw
/// pictures does.
#[cfg(target_os = "linux")]
const CELL: (u16, u16) = (8, 16);

/// A placeholder, so the binary gets past the credential check and as far as
/// drawing. Nothing is ever sent: standard input is closed, so no turn starts.
#[cfg(target_os = "linux")]
const KEY: &str = "bench-not-a-key";

/// A user-level configuration file, so the reading includes parsing one.
///
/// A budget measured against an empty home would say nothing about the thing
/// this budget exists to bound — a file read and parsed before the first frame
/// — and would keep saying nothing as the document grew. Every block is
/// represented, because they are resolved by walking the document and a block
/// nobody writes down is a block nobody measures.
const CONFIG: &str = r#"{
  "updates": {"check": "never"},
  "providers": {
    "anthropic": {"model": "claude-sonnet-5", "apiKeyEnv": "ANTHROPIC_API_KEY"},
    "openai": {"model": "gpt-5.6-terra"}
  },
  "env": {"RUST_LOG": "warn", "PAGER": "cat"},
  "output": {"color": "auto", "toolDetail": "compact"}
}"#;

/// Where [`CONFIG`] opens its `output` block.
const OUTPUT: &str = r#""output": {"#;

/// [`CONFIG`], with `output.screen` set to `screen` when a probe names one.
///
/// A probe that names none writes the document byte for byte, so every budget
/// measured before a probe could name its screen still reads the same file.
fn configuration(screen: Option<&str>) -> String {
    match screen {
        None => CONFIG.to_owned(),
        Some(mode) => CONFIG.replacen(OUTPUT, &format!(r#"{OUTPUT}"screen": "{mode}", "#), 1),
    }
}

/// What the fake terminal does with a question the child asks it.
///
/// A real terminal answers some escape sequences and ignores the rest, and the
/// shipped program may wait on one before it draws. A probe that never answers
/// measures the wait; one that answers at once measures the exchange without
/// it. Those are two different readings of the same startup path, so both are
/// taken rather than one standing for the other.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Answering {
    /// Every query the child writes is answered as soon as it is whole.
    AtOnce,
    /// Nothing is ever answered.
    Never,
    /// Every answer arrives [`LATE`] after its question, long after the
    /// child decided without it, as over a slow link, and in the pieces a
    /// [`Answering::Split`] answer arrives in.
    Late,
    /// A question followed by a cursor-position query has the cursor position
    /// answered first, which no terminal that answers in order does.
    OutOfOrder,
    /// Every answer arrives in pieces, cut before and after each escape
    /// inside it, with [`SPLIT`] between them. The cut after an escape is the
    /// one `crossterm` reads as Esc and typed characters when its read falls
    /// between the two.
    Split,
}

/// How long after its question a [`Answering::Late`] answer arrives.
#[cfg(target_os = "linux")]
const LATE: Duration = Duration::from_millis(50);

/// The pause between the pieces of a [`Answering::Split`] answer.
#[cfg(target_os = "linux")]
const SPLIT: Duration = Duration::from_millis(2);

/// The writes a terminal answering this way makes of `said`, each with how
/// long after the question it is made.
#[cfg(target_os = "linux")]
fn paced(answering: Answering, said: Vec<u8>) -> Vec<(Duration, Vec<u8>)> {
    if said.is_empty() {
        return Vec::new();
    }
    match answering {
        Answering::Never => Vec::new(),
        Answering::AtOnce | Answering::OutOfOrder => vec![(Duration::ZERO, said)],
        Answering::Late => pieces(said, LATE),
        Answering::Split => pieces(said, Duration::ZERO),
    }
}

/// `said` cut before and after each escape, the first piece `from` after the
/// question and each next one [`SPLIT`] after it.
#[cfg(target_os = "linux")]
fn pieces(said: Vec<u8>, from: Duration) -> Vec<(Duration, Vec<u8>)> {
    let mut pieces: Vec<Vec<u8>> = Vec::new();
    for byte in said {
        match pieces.last_mut() {
            Some(piece) if byte != 0x1b && piece != b"\x1b" => piece.push(byte),
            _ => pieces.push(vec![byte]),
        }
    }
    pieces
        .into_iter()
        .zip(0_u32..)
        .map(|(piece, at)| (from + SPLIT * at, piece))
        .collect()
}

/// Whether the measured child is allowed to draw in colour.
///
/// A budget measured with `NO_COLOR=1` says nothing about the path a user with
/// colour takes, and the bytes and work a palette adds are what this is for.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Colour {
    /// The child reads no `NO_COLOR`, so a terminal gets colour.
    On,
    /// `NO_COLOR=1`, as every probe measured before colour had its own.
    Off,
}

/// What proves that the timed startup operation happened.
///
/// This source is compiled separately into one probe per variant, so each
/// resulting binary deliberately constructs one of them and not the others.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub(crate) enum Measure {
    /// The first visible bytes of the opening frame reached a terminal.
    Frame { needle: &'static str },
    /// Once ready, one character was sent and its application-rendered copy
    /// reached the terminal. Raw mode has disabled the kernel's own echo.
    Input {
        ready: &'static str,
        probe: &'static str,
    },
    /// Once ready, a whole line was typed and the screen it opens arrived.
    ///
    /// Timed from the moment the line was sent rather than from `exec`: what
    /// startup costs is already a budget of its own two rows above, and
    /// charging it again here would leave a screen's budget moving whenever
    /// startup did.
    Typed {
        ready: &'static str,
        line: &'static str,
        needle: &'static str,
    },
}

impl Measure {
    fn label(self) -> &'static str {
        match self {
            Self::Frame { needle } | Self::Typed { needle, .. } => needle,
            Self::Input { probe, .. } => probe,
        }
    }
}

/// Why a reading could not be taken.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StartupError {
    /// Something the probe itself needed failed.
    #[error("{0}")]
    Io(#[from] io::Error),

    /// The benchmark workspace could not be resolved.
    #[error("{0}")]
    Workspace(#[from] crucible_workspace::PathError),

    /// A production-shaped benchmark session could not be started.
    #[error("{0}")]
    Session(#[from] crucible_session::SessionError),

    /// A production-shaped benchmark session could not be finished.
    #[error("session log: {0}")]
    Record(Box<str>),

    /// The binary under test has not been built.
    #[error("no binary at {0}: run `cargo build --release --bin crucible`")]
    Unbuilt(PathBuf),

    /// The child exited without ever printing what was being waited for.
    #[cfg(target_os = "linux")]
    #[error("crucible exited without printing {0:?}")]
    Silent(&'static str),

    /// A noninteractive startup path did not exit successfully with its proof.
    #[error("crucible {label} exited with {status} and did not print {needle:?}")]
    FastExit {
        /// Which argument path was measured.
        label: &'static str,
        /// The process status as the operating system reported it.
        status: std::process::ExitStatus,
        /// What its standard output had to contain.
        needle: &'static str,
    },

    /// The deadline elapsed while the child still held the terminal.
    #[cfg(target_os = "linux")]
    #[error("crucible did not reach {0:?} within five seconds")]
    Timeout(&'static str),

    /// The probe has no safe PTY implementation on this platform.
    #[cfg(not(target_os = "linux"))]
    #[error("startup PTY measurements require Linux")]
    Unsupported,

    /// The runner a probe was told it is on is not one it knows.
    #[error(
        "CRUCIBLE_BENCH_RUNNER is {0:?}: leave it unset on this machine, or set it to `shared` on a CI runner"
    )]
    Runner(OsString),

    /// No readings at all, so there is no percentile to take.
    #[error("no readings")]
    Nothing,
}

/// Takes [`RUNS`] readings of the same thing.
pub(crate) fn readings(measure: Measure) -> Result<Readings, StartupError> {
    readings_in(measure, None)
}

/// The same, with `output.screen` set to `screen` when a probe names one.
pub(crate) fn readings_in(
    measure: Measure,
    screen: Option<&str>,
) -> Result<Readings, StartupError> {
    readings_on(measure, screen, Answering::Never, Colour::Off)
}

/// The same, with the fake terminal's answers and the child's colour chosen.
///
/// A probe that names neither gets the terminal every probe measured before:
/// silent, and told not to write colour.
pub(crate) fn readings_on(
    measure: Measure,
    screen: Option<&str>,
    answering: Answering,
    colour: Colour,
) -> Result<Readings, StartupError> {
    let binary = beside("crucible")?;
    let home = Scratch::in_screen(measure.label(), screen)?;

    let mut taken = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        home.restore_fixture()?;
        taken.push(once(&binary, home.path(), measure, answering, colour)?);
    }

    Ok(Readings::new(taken))
}

/// The first input with colour on, the fake terminal answering as `answering`.
///
/// One measurement for the probe a terminal answers and the one it never does,
/// so the two readings differ only in the answers. The prompt mark is waited
/// for alone, without the space the no-colour first input waits on: colour puts
/// a reset between the mark and what follows it, so the plain pair is no longer
/// contiguous in the terminal's bytes. The key sent is one uncommon character,
/// absent from startup output and visible in the input box.
pub(crate) fn coloured_input(answering: Answering) -> Result<Readings, StartupError> {
    readings_on(
        Measure::Input {
            ready: "\u{203a}",
            probe: "\u{00a7}",
        },
        None,
        answering,
        Colour::On,
    )
}

/// Takes [`RUNS`] readings of a noninteractive argument that exits by itself.
///
/// Unlike a screen measure, this deliberately uses pipes. `--help` and
/// `--version` are promises to scripts as well as people, and neither should
/// open a terminal, read a home, or start a session. Waiting for process exit
/// makes that fast path's whole cost the reading rather than stopping at its
/// first byte and leaving teardown unmeasured.
pub(crate) fn exits(
    label: &'static str,
    args: &'static [&'static str],
    needle: &'static str,
) -> Result<Readings, StartupError> {
    let binary = beside("crucible")?;
    let mut taken = Vec::with_capacity(RUNS);

    for _ in 0..RUNS {
        let started = std::time::Instant::now();
        let output = std::process::Command::new(&binary)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .output()?;
        let elapsed = started.elapsed();

        let printed = output
            .stdout
            .windows(needle.len())
            .any(|window| window == needle.as_bytes());
        if !output.status.success() || !printed {
            return Err(StartupError::FastExit {
                label,
                status: output.status,
                needle,
            });
        }
        taken.push(elapsed);
    }

    Ok(Readings::new(taken))
}

/// How many batches of [`RUNS`] a reading over budget is taken from.
const AT_LIBERTY: usize = 3;

/// The quickest of up to [`AT_LIBERTY`] batches, by the figure the budget is
/// against.
///
/// The windows inside a batch answer a machine that stalled for a stretch, and
/// the test below says why they are entitled to fail a run that stalled through
/// most of them: slow in most windows is slow all session. What they cannot
/// answer is a batch stalled from end to end — a runner handing this process a
/// core for a fraction of the wall clock it asked for, which no window is long
/// enough to sit outside. So a batch that went over is taken again, and the
/// quickest of them is the reading, because sharing a host can only ever make a
/// batch worse.
///
/// It is not a way to pass. Nothing is thrown away that a second batch does not
/// beat, and a startup that actually got slower is over budget in all three —
/// which is the same claim the windows make, one layer up. What makes that hold
/// is that `again` launches the binary afresh every call, from a scratch home it
/// builds itself: a batch reading state a previous one left would open already
/// slow, and the guarantee would invert.
pub(crate) fn best(
    budget: Duration,
    mut again: impl FnMut() -> Result<Readings, StartupError>,
) -> Result<Readings, StartupError> {
    let mut best = again()?;
    let mut mark = best.p95()?;

    for _ in 1..AT_LIBERTY {
        if mark <= budget {
            break;
        }

        let next = again()?;
        let then = next.p95()?;

        if then < mark {
            best = next;
            mark = then;
        }
    }

    Ok(best)
}

/// The limit a probe is held to, on the runner it was started on.
///
/// `CRUCIBLE_BENCH_RUNNER` names the runner. Unset is a quiet machine, the run
/// RELEASING.md decides a release on; `shared` is a CI runner, named by the
/// workflows that measure there. Anything else is refused rather than read as
/// either, because a misspelt runner that fell back to one of the limits would
/// be a budget nobody chose.
pub(crate) fn limit<T>(quiet: T, shared: T) -> Result<T, StartupError> {
    chosen(
        std::env::var_os("CRUCIBLE_BENCH_RUNNER").as_deref(),
        quiet,
        shared,
    )
}

/// [`limit`], given the runner's name rather than reading it.
fn chosen<T>(runner: Option<&std::ffi::OsStr>, quiet: T, shared: T) -> Result<T, StartupError> {
    match runner {
        None => Ok(quiet),
        Some(named) if named == "shared" => Ok(shared),
        Some(other) => Err(StartupError::Runner(other.to_os_string())),
    }
}

/// Every reading one probe took, in the order it took them.
pub(crate) struct Readings(Vec<Duration>);

impl Readings {
    /// Kept in the order they arrived, which is the whole of what the windows
    /// are about: a machine stalls for a stretch of wall clock, and a run
    /// sorted on the way in has thrown away which readings were next to each
    /// other. Everything below that wants an order statistic sorts a window.
    fn new(taken: Vec<Duration>) -> Self {
        Self(taken)
    }

    /// The 95th percentile of each window, and the middle of those.
    pub(crate) fn p95(&self) -> Result<Duration, StartupError> {
        let mut windows = self.windows();
        windows.sort_unstable();

        windows
            .get(windows.len() / 2)
            .copied()
            .ok_or(StartupError::Nothing)
    }

    /// What each window came to, in the order the windows were run.
    fn windows(&self) -> Vec<Duration> {
        self.0.chunks(PER_WINDOW).filter_map(percentile).collect()
    }

    /// What the readings looked like, for a probe saying why it went over.
    ///
    /// A percentile on its own cannot tell a program that got slower from a
    /// machine that stalled under one: both read as a large number. The middle
    /// sitting far below the tail says which happened, and is the first thing
    /// worth looking at, so a probe that fails prints this beside its reading
    /// rather than leaving the next reader to guess it.
    pub(crate) fn spread(&self) -> String {
        let mut sorted = self.0.clone();
        sorted.sort_unstable();

        let Some(best) = sorted.first().copied() else {
            return String::from("no runs");
        };
        // All three are present: the slice is not empty, and `p95` can only
        // fail on one that is.
        let worst = sorted.last().copied().unwrap_or(best);
        let median = sorted.get(sorted.len() / 2).copied().unwrap_or(best);
        let p95 = self.p95().unwrap_or(worst);

        // The windows in the order they ran, because that is where a stall is
        // legible: one number far above the rest is a stretch of wall clock
        // this program did not own, and nine that agree is this program.
        let windows = self
            .windows()
            .iter()
            .map(|window| ms(*window))
            .collect::<Vec<_>>()
            .join(", ");

        format!(
            "{} runs: best {}, median {}, p95 {}, worst {} — windows {windows}",
            self.0.len(),
            ms(best),
            ms(median),
            ms(p95),
            ms(worst),
        )
    }
}

/// The 95th of `taken`, by nearest rank.
///
/// `None` for nothing, which is what makes a run with no readings at all a
/// reading that could not be taken rather than a zero.
fn percentile(taken: &[Duration]) -> Option<Duration> {
    let mut sorted = taken.to_vec();
    sorted.sort_unstable();

    let rank = sorted.len().saturating_mul(95).div_ceil(100).max(1) - 1;
    sorted.get(rank).copied()
}

/// One reading, in the milliseconds every budget here is written in.
fn ms(taken: Duration) -> String {
    format!("{:.1} ms", taken.as_secs_f64() * 1000.0)
}

/// One run: start against a controlling terminal and wait for its proof.
#[cfg(target_os = "linux")]
fn once(
    binary: &Path,
    home: &Path,
    measure: Measure,
    answering: Answering,
    colour: Colour,
) -> Result<Duration, StartupError> {
    let (mut running, started) = start(binary, home, answering, colour, Seat::Terminal)?;
    running.until(measure, started)
}

/// Where the child's standard streams are, beside the terminal it controls.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Seat {
    /// Every stream is the terminal, as when a person starts it.
    Terminal,
    /// Standard output goes to [`REDIRECTED`] in the home instead.
    Redirected,
    /// Standard input has nothing behind it.
    NoInput,
}

/// The file a [`Seat::Redirected`] child writes its output to, in its home.
#[cfg(target_os = "linux")]
const REDIRECTED: &str = "redirected";

/// Starts the child with the terminal as its controlling terminal, and the
/// thread that reads and answers it, and says when the spawn began.
#[cfg(target_os = "linux")]
fn start(
    binary: &Path,
    home: &Path,
    answering: Answering,
    colour: Colour,
    seat: Seat,
) -> Result<(Running, Instant), StartupError> {
    let (terminal, inside) = pair()?;
    let reading = terminal.try_clone()?;
    let replying = terminal.try_clone()?;
    let (sender, bytes) = mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("crucible-startup-terminal".into())
        .spawn(move || read(reading, replying, answering, &sender))?;

    let second = inside.try_clone()?;
    let third = inside.try_clone()?;
    let output = if seat == Seat::Redirected {
        Stdio::from(File::create(home.join(REDIRECTED))?)
    } else {
        Stdio::from(second)
    };
    let started = Instant::now();
    let child = {
        let mut command = Command::new("setsid");
        command.arg("--ctty");
        if seat == Seat::NoInput {
            // `setsid` takes the terminal from its own standard input, so the
            // child's is taken away only after the terminal is its own.
            command
                .args(["/bin/sh", "-c", "exec \"$0\" </dev/null"])
                .arg(binary);
        } else {
            command.arg(binary);
        }
        command
            .current_dir(std::env::current_dir()?)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", home)
            .env("TERM", "xterm-256color")
            // A terminal that can draw pictures, so a run with colour asks
            // the questions that confirm it; the window reports its pixels
            // below for the same reason.
            .env("KITTY_WINDOW_ID", "1")
            .env(crucible_config::HOME, home)
            .env("ANTHROPIC_API_KEY", KEY)
            .stdin(Stdio::from(inside))
            .stdout(output)
            .stderr(Stdio::from(third));
        if matches!(colour, Colour::Off) {
            command.env("NO_COLOR", "1");
        }
        // Dropped here rather than carried past the spawn: a `Command` keeps
        // its `Stdio` handles, and this process holding the far side of the
        // terminal open would keep the reading thread waiting for an end that
        // never comes.
        command.spawn()?
    };

    Ok((
        Running {
            terminal,
            child,
            bytes,
            reader: Some(reader),
        },
        started,
    ))
}

#[cfg(not(target_os = "linux"))]
fn once(
    _binary: &Path,
    _home: &Path,
    _measure: Measure,
    _answering: Answering,
    _colour: Colour,
) -> Result<Duration, StartupError> {
    Err(StartupError::Unsupported)
}

/// The process and both ends of reading its terminal, reaped on every exit.
#[cfg(target_os = "linux")]
struct Running {
    terminal: File,
    child: Child,
    bytes: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Running {
    fn until(&mut self, measure: Measure, started: Instant) -> Result<Duration, StartupError> {
        let deadline = Instant::now() + CEILING;
        let mut seen = Vec::with_capacity(4096);
        let mut sent = false;
        // Moved to the send for a typed line, which is where that reading
        // starts; every other measure is charged from `exec`.
        let mut from = started;

        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(StartupError::Timeout(measure.label()));
            }

            match self
                .bytes
                .recv_timeout(deadline.saturating_duration_since(now))
            {
                Ok(bytes) => seen.extend_from_slice(&bytes),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(StartupError::Timeout(measure.label()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(StartupError::Silent(measure.label()));
                }
            }

            match measure {
                Measure::Frame { needle } if contains(&seen, needle) => {
                    return Ok(started.elapsed());
                }
                Measure::Input { ready, probe } if !sent && contains(&seen, ready) => {
                    // Raw mode disables terminal echo, so these bytes can only
                    // come back after crucible accepted and rendered the key.
                    seen.clear();
                    self.terminal.write_all(probe.as_bytes())?;
                    self.terminal.flush()?;
                    sent = true;
                }
                Measure::Input { probe, .. } if sent && contains(&seen, probe) => {
                    return Ok(started.elapsed());
                }
                Measure::Typed { ready, line, .. } if !sent && contains(&seen, ready) => {
                    seen.clear();
                    from = Instant::now();
                    self.terminal.write_all(line.as_bytes())?;
                    self.terminal.flush()?;
                    sent = true;
                }
                Measure::Typed { needle, .. } if sent && contains(&seen, needle) => {
                    return Ok(from.elapsed());
                }
                Measure::Frame { .. } | Measure::Input { .. } | Measure::Typed { .. } => {}
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(target_os = "linux")]
fn contains(bytes: &[u8], text: &str) -> bool {
    bytes
        .windows(text.len())
        .any(|window| window == text.as_bytes())
}

/// Opens a terminal pair, fixes its size and disables the kernel's own echo.
#[cfg(target_os = "linux")]
fn pair() -> Result<(File, File), StartupError> {
    let terminal = pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
        .map_err(io::Error::from)?;
    pty::grantpt(&terminal).map_err(io::Error::from)?;
    pty::unlockpt(&terminal).map_err(io::Error::from)?;

    let named = pty::ptsname(&terminal, Vec::new()).map_err(io::Error::from)?;
    let inside = OpenOptions::new()
        .read(true)
        .write(true)
        .open(OsStr::from_bytes(named.as_bytes()))?;

    let mut mode = termios::tcgetattr(&inside).map_err(io::Error::from)?;
    mode.make_raw();
    termios::tcsetattr(&inside, OptionalActions::Now, &mode).map_err(io::Error::from)?;
    termios::tcsetwinsize(
        &terminal,
        Winsize {
            ws_row: ROWS,
            ws_col: COLUMNS,
            ws_xpixel: COLUMNS * CELL.0,
            ws_ypixel: ROWS * CELL.1,
        },
    )
    .map_err(io::Error::from)?;

    Ok((File::from(terminal), inside))
}

/// Reads until the child closes the far side of the terminal.
///
/// The same pass also answers queries when the probe asked for a terminal that
/// does. The answer goes out on the master's write side, which is the child's
/// standard input; every byte the child wrote still goes to the waiting probe,
/// answers or none.
#[cfg(target_os = "linux")]
fn read(mut terminal: File, reply: File, answering: Answering, sender: &Sender<Vec<u8>>) {
    let mut answers = Answers::new(answering);
    let mut buffer = [0_u8; 8192];
    // A terminal that answers late or in pieces does it from a thread of its
    // own, so the child's output is still read while an answer waits.
    let (later, writer, mut direct) = if matches!(answering, Answering::Late | Answering::Split) {
        let (later, due) = mpsc::channel::<(Instant, Vec<u8>)>();
        let writer = std::thread::Builder::new()
            .name("crucible-startup-answers".into())
            .spawn(move || answer_when_due(reply, &due));
        match writer {
            Ok(writer) => (Some(later), Some(writer), None),
            Err(_) => return,
        }
    } else {
        (None, None, Some(reply))
    };

    while let Ok(read) = terminal.read(&mut buffer) {
        if read == 0 {
            break;
        }

        let asked = Instant::now();
        let bytes = buffer.get(..read).unwrap_or_default();
        let said = answers.feed(bytes);
        let mut failed = false;
        for (after, piece) in paced(answering, said) {
            failed = match (&later, direct.as_mut()) {
                (Some(later), _) => later.send((asked + after, piece)).is_err(),
                (None, Some(reply)) => reply
                    .write_all(&piece)
                    .and_then(|()| reply.flush())
                    .is_err(),
                (None, None) => true,
            };
            if failed {
                break;
            }
        }
        if failed || sender.send(bytes.to_vec()).is_err() {
            break;
        }
    }

    drop(later);
    if let Some(writer) = writer {
        let _ = writer.join();
    }
}

/// Writes each answer once its time has come, in the order they were given.
#[cfg(target_os = "linux")]
fn answer_when_due(mut reply: File, due: &Receiver<(Instant, Vec<u8>)>) {
    for (at, piece) in due {
        if let Some(wait) = at.checked_duration_since(Instant::now()) {
            std::thread::sleep(wait);
        }
        if reply
            .write_all(&piece)
            .and_then(|()| reply.flush())
            .is_err()
        {
            break;
        }
    }
}

/// The longest query a question can start with.
///
/// Only a tail this long is kept between reads, because a query is one escape
/// sequence and the child may write it across two of them.
#[cfg(target_os = "linux")]
const QUERY_START: usize = 24;

/// The most the fake terminal will hold while a query has no end.
///
/// A child that writes a query it never terminates is a probe that must keep
/// reading, not a probe that grows with what the child wrote. Past this the
/// unterminated bytes are dropped and the session carries on unanswered, which
/// is what a real terminal does with a sequence it cannot parse.
#[cfg(target_os = "linux")]
const UNSENT: usize = 4096;

/// What a terminal saw in the child's output.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Asked {
    /// Where the query starts.
    start: usize,
    /// One past its last byte, or `None` while more must arrive.
    end: Option<usize>,
    /// What a terminal says back once the query is whole.
    reply: Vec<u8>,
}

/// The fake terminal's half of the conversation.
///
/// It is a scanner, not a parser: it looks for the four escapes a terminal can
/// answer and says what one would say. Every byte that is not one of those is
/// ignored, and only an unterminated query's start is held back.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Answers {
    answering: Answering,
    pending: Vec<u8>,
    /// What an [`Answering::OutOfOrder`] terminal has not said yet, held until
    /// a cursor position is asked and answered ahead of it.
    held: Vec<u8>,
}

#[cfg(target_os = "linux")]
impl Answers {
    fn new(answering: Answering) -> Self {
        Self {
            answering,
            pending: Vec::new(),
            held: Vec::new(),
        }
    }

    /// Says `reply`, or for a terminal answering out of order, holds every
    /// answer but a cursor position's and says what it held after one.
    fn say(&mut self, said: &mut Vec<u8>, asked: &Asked) {
        if !matches!(self.answering, Answering::OutOfOrder) {
            said.extend_from_slice(&asked.reply);
        } else if asked.reply == CURSOR_REPLY {
            said.extend_from_slice(&asked.reply);
            said.append(&mut self.held);
        } else if self.held.len() + asked.reply.len() <= UNSENT {
            self.held.extend_from_slice(&asked.reply);
        }
    }

    /// Adds what the child just wrote and returns what a terminal answers.
    fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if matches!(self.answering, Answering::Never) {
            return Vec::new();
        }

        self.pending.extend_from_slice(bytes);
        let mut said = Vec::new();
        let mut from = 0;

        while let Some(asked) = asked(&self.pending, from) {
            let Some(end) = asked.end else {
                // The query's start is here but its end is not. Hold it until
                // the next read, or drop it if it will never be answerable.
                self.pending.drain(..asked.start);
                if self.pending.len() > UNSENT {
                    self.pending.clear();
                }
                return said;
            };

            self.say(&mut said, &asked);
            from = end;
        }

        let keep = QUERY_START.saturating_sub(1).min(self.pending.len());
        let drop = self.pending.len().saturating_sub(keep).max(from);
        self.pending.drain(..drop);
        said
    }
}

/// Finds the first answerable escape in `bytes` at or after `from`.
#[cfg(target_os = "linux")]
fn asked(bytes: &[u8], from: usize) -> Option<Asked> {
    let cursor = find(bytes, b"\x1b[6n", from).map(|start| Asked {
        start,
        end: Some(start + b"\x1b[6n".len()),
        reply: CURSOR_REPLY.to_vec(),
    });
    let attributes = find(bytes, b"\x1b[c", from)
        .map(|start| Asked {
            start,
            end: Some(start + b"\x1b[c".len()),
            reply: attributes_reply(),
        })
        .into_iter()
        .chain(find(bytes, b"\x1b[0c", from).map(|start| Asked {
            start,
            end: Some(start + b"\x1b[0c".len()),
            reply: attributes_reply(),
        }));
    let graphics = find(bytes, b"\x1b_G", from).map(|start| kitty(bytes, start));
    let cell = find(bytes, b"\x1b]1337;ReportCellSize", from).map(|start| cell_size(bytes, start));

    cursor
        .into_iter()
        .chain(attributes)
        .chain(graphics)
        .chain(cell)
        .min_by_key(|asked| asked.start)
}

/// The terminal's answer to a cursor-position query: the top left corner.
#[cfg(target_os = "linux")]
const CURSOR_REPLY: &[u8] = b"\x1b[1;1R";

/// The terminal's answer to a device-attributes query.
///
/// Parameter `4` is Sixel graphics, which is the one attribute a protocol
/// choice reads; the rest describe a VT220-class terminal answering the way
/// xterm does.
#[cfg(target_os = "linux")]
fn attributes_reply() -> Vec<u8> {
    b"\x1b[?62;4;6;9;15;22c".to_vec()
}

/// A Kitty graphics query, which ends with ST and only asks when `a=q`.
#[cfg(target_os = "linux")]
fn kitty(bytes: &[u8], start: usize) -> Asked {
    let body = start + b"\x1b_G".len();
    let Some(over) = find(bytes, b"\x1b\\", body) else {
        return Asked {
            start,
            end: None,
            reply: Vec::new(),
        };
    };

    let parameters = bytes.get(body..over).unwrap_or_default();
    let reply = if parameters
        .windows(b"a=q".len())
        .any(|window| window == b"a=q")
    {
        format!("\x1b_Gi={};OK\x1b\\", image_id(parameters)).into_bytes()
    } else {
        Vec::new()
    };

    Asked {
        start,
        end: Some(over + b"\x1b\\".len()),
        reply,
    }
}

/// The `i` parameter of a Kitty query, or zero when it has none.
#[cfg(target_os = "linux")]
fn image_id(parameters: &[u8]) -> u64 {
    let text = String::from_utf8_lossy(parameters);
    let before_reply = text.split(';').next().unwrap_or_default();

    before_reply
        .split(',')
        .find_map(|pair| pair.strip_prefix("i=")?.parse().ok())
        .unwrap_or(0)
}

/// iTerm2's cell-size query, which ends with BEL or ST.
#[cfg(target_os = "linux")]
fn cell_size(bytes: &[u8], start: usize) -> Asked {
    let body = start + b"\x1b]1337;ReportCellSize".len();
    let bel = find(bytes, b"\x07", body).map(|at| (at, 1));
    let st = find(bytes, b"\x1b\\", body).map(|at| (at, 2));
    let end = match (bel, st) {
        (Some(bel), Some(st)) => Some(if bel.0 <= st.0 { bel } else { st }),
        (Some(bel), None) => Some(bel),
        (None, Some(st)) => Some(st),
        (None, None) => None,
    };

    match end {
        Some((end, width)) => Asked {
            start,
            end: Some(end + width),
            // The shipped program reads the ioctl rather than this reply when
            // it wants a cell's pixels; the answer is here so a query has one.
            reply: b"\x1b]1337;ReportCellSize=16;8\x07".to_vec(),
        },
        None => Asked {
            start,
            end: None,
            reply: Vec::new(),
        },
    }
}

/// Where `needle` first sits in `bytes` at or after `from`.
#[cfg(target_os = "linux")]
fn find(bytes: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    let rest = bytes.get(from..)?;
    rest.windows(needle.len())
        .position(|window| window == needle)
        .map(|at| from + at)
}

/// How many session logs the bench home is given.
///
/// A directory somebody has been working in for a while. An empty one would say
/// nothing about the read that happens before the first frame, and would keep
/// saying nothing as it filled up — the same reason the configuration file
/// above is written rather than left out.
const LOGS: usize = 100;

/// Where among them the ones this run could use sit, counting from the oldest.
///
/// Deep enough that everything above is opened and refused before anything is
/// found: what the reading then covers is the bound the scan is written to,
/// rather than the four newest files in a directory that happens to be tidy.
const USABLE: std::ops::Range<usize> = 44..48;

/// What every usable planted session was first asked.
const TITLE: &str = "what the last person to sit here asked";

/// The one planted session somebody worked in for an afternoon.
///
/// The newest usable log, so it is the entry a picker opens marked and the
/// session its preview is drawn from. A directory of first questions would say
/// nothing about what previewing costs, and would keep saying nothing as real
/// sessions grew.
const DEEPEST: usize = USABLE.end - 1;

/// Turns written into that one.
///
/// Enough to carry the log past the tail a preview reads, which [`worked_in`]
/// checks through the production glimpse rather than trusting: the reading is
/// meant to cover a read that had to stop early, not a file that happened to
/// fit.
const WORKED: usize = 240;

/// What each of those turns asked, answered, and read back.
const ASKED: &str = "where does this one get decided, and what reads it after";
const ANSWERED: &str = "In the module below. I will read it and say what it holds.";
const RETURNED: &str = "the file, as far as the tool was willing to read it, which is \
enough lines of it to stand for what a tool puts back into a session and not so many \
that one turn is the whole of the log";

/// The last thing the deepest session said, and so the last row of its preview.
///
/// A probe waiting on it reads it from here: a fixture's last word and the word
/// a measurement waits for are one fact, and two spellings of it would be a
/// probe that hangs for five seconds to say so.
pub(crate) const ENDED: &str = "the deepest session ends here";

/// The session logs, planted.
///
/// Each is a header and one message, which is what a session that was asked one
/// thing and then closed leaves behind — except [`DEEPEST`], which is a session
/// that was worked in. The ones outside [`USABLE`] name a directory the child is
/// not started in, so they are read and put down again; the ones inside it name
/// the directory it is, so their titles are read too.
///
/// Written through the production session API, so a file-format change cannot
/// quietly turn the title path into a scan of foreign logs. Fixture creation is
/// outside the timed region.
fn worked_in(sessions: &Path) -> Result<HashSet<OsString>, StartupError> {
    let elsewhere = sessions
        .parent()
        .ok_or_else(|| io::Error::other("the session fixture has no parent"))?
        .join("elsewhere");
    fs::create_dir_all(&elsewhere)?;

    let here = crucible_workspace::Workspace::open(std::env::current_dir()?)?;
    let away = crucible_workspace::Workspace::open(elsewhere)?;

    let mut planted = HashSet::with_capacity(LOGS);

    // A log's name is its start time with a random tiebreak inside one
    // millisecond, so the pause keeps the loop's order and the directory's
    // order the same: the four matching sessions then sit after the fifty-two
    // newest candidates from the other workspace, inside the production scan's
    // sixty-four-log bound. The pause is outside the timed region.
    for nth in 0..LOGS {
        let workspace = if USABLE.contains(&nth) { &here } else { &away };
        let session = crucible_session::Session::start(sessions, workspace, None)?;
        let name = session.id().cloned();
        if let Some(id) = name.as_ref() {
            planted.insert(OsString::from(format!("{}.jsonl", id.as_str())));
        }
        session.append(&crucible_types::Message::said(TITLE));
        if nth == DEEPEST {
            worked(&session);
        }
        if let Some(trouble) = session.finish() {
            return Err(StartupError::Record(trouble));
        }
        if nth == DEEPEST
            && let Some(id) = name.as_ref()
        {
            deep_enough(sessions, &here, id)?;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    Ok(planted)
}

/// Writes [`WORKED`] turns of real work into one open session.
///
/// Prompts, answers, calls and results, because a preview draws all four and a
/// log of prose alone would measure the cheapest of them. The last word is
/// [`ENDED`], which is what a probe waits to see.
fn worked(session: &crucible_session::Session) {
    use crucible_types::{
        Message, RecordedToolOutput, StopReason, ToolArgs, ToolCall, ToolId, ToolResult,
    };

    for turn in 0..WORKED {
        let call = ToolId::new(format!("call-{turn}"));

        session.append(&Message::said(format!("{ASKED} — turn {turn}?")));
        session.append(&Message::Agent {
            continuation: None,
            text: ANSWERED.into(),
            calls: vec![ToolCall {
                id: call.clone(),
                name: "read".into(),
                args: ToolArgs::new(format!(r#"{{"path":"src/module-{turn}.rs"}}"#)),
            }],
            stop: Some(StopReason::WantsTools),
        });
        session.append(&Message::ToolResults(vec![ToolResult {
            id: call,
            output: RecordedToolOutput::ok(RETURNED),
        }]));
    }

    session.append(&Message::Agent {
        continuation: None,
        text: ENDED.into(),
        calls: Vec::new(),
        stop: Some(StopReason::Yielded),
    });
}

/// Refuses a fixture the preview would not have to cut.
///
/// Asked through the production glimpse rather than of the file's size, because
/// the bound belongs to that read: a log big enough today is a log that quietly
/// stops standing for one the day the bound moves.
fn deep_enough(
    sessions: &Path,
    workspace: &crucible_workspace::Workspace,
    id: &crucible_types::SessionId,
) -> Result<(), StartupError> {
    if crucible_session::glimpse(sessions, workspace, id)?.cut() {
        return Ok(());
    }

    Err(StartupError::Io(io::Error::other(
        "the deepest planted session fits inside the tail a preview reads",
    )))
}

/// The probe's sibling in `target/release/`.
///
/// Found next to this executable rather than at a path relative to the working
/// directory, so the probe measures the binary it was built with.
fn beside(name: &str) -> Result<PathBuf, StartupError> {
    let binary = std::env::current_exe()?
        .parent()
        .ok_or_else(|| io::Error::other("the probe has no directory"))?
        .join(name);

    if binary.is_file() {
        Ok(binary)
    } else {
        Err(StartupError::Unbuilt(binary))
    }
}

/// A home of its own, holding a configuration file and taking the session logs,
/// so the runs neither read nor write anything belonging to whoever is
/// benchmarking.
#[derive(Debug)]
pub(crate) struct Scratch {
    path: PathBuf,
    /// The fixture's log names. Every measured child records a session of its
    /// own into the same directory, and without this set the next run would
    /// scan one log more than the last.
    fixture: HashSet<OsString>,
}

impl Scratch {
    pub(crate) fn new(name: &str) -> Result<Self, StartupError> {
        Self::in_screen(name, None)
    }

    /// The same, with `output.screen` set to `screen` when a probe names one.
    pub(crate) fn in_screen(name: &str, screen: Option<&str>) -> Result<Self, StartupError> {
        Self::holding(&name.trim().len().to_string(), &configuration(screen))
    }

    /// A home named `name` among this process's, configured by `document`.
    fn holding(name: &str, document: &str) -> Result<Self, StartupError> {
        let base =
            std::env::temp_dir().join(format!("crucible-bench-{name}-{}", std::process::id()));
        fs::create_dir_all(&base)?;
        fs::write(base.join("config.json"), document)?;
        let fixture = worked_in(&base.join("sessions"))?;

        Ok(Self {
            path: base,
            fixture,
        })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Removes what a measured child added, so every run scans the same
    /// hundred planted logs.
    fn restore_fixture(&self) -> Result<(), io::Error> {
        for entry in fs::read_dir(self.path.join("sessions"))? {
            let entry = entry?;
            if !self.fixture.contains(&entry.file_name()) {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::ffi::OsStr;
    use std::time::Duration;

    use super::chosen;

    use super::{
        AT_LIBERTY, CONFIG, PER_WINDOW, RUNS, Readings, Scratch, TITLE, USABLE, WINDOWS, best,
        configuration,
    };

    /// A full set of readings, stalled from end to end of the first `windows`
    /// windows and clean through the rest.
    fn stalled_in(windows: usize) -> Readings {
        let mut all = Vec::with_capacity(RUNS);

        for window in 0..WINDOWS {
            let taken = if window < windows { 100 } else { 1 };
            all.extend(std::iter::repeat_n(
                Duration::from_millis(taken),
                PER_WINDOW,
            ));
        }

        Readings::new(all)
    }

    #[test]
    fn a_stalled_stretch_does_not_decide_the_percentile() {
        // What the windows are for, and the whole of why the reading changed
        // shape: a fifth of this run was spent on a machine with other work on
        // it, and the answer is still what this program does. Over one long
        // run, forty-four slow launches out of a hundred and ninety-eight were
        // past the 95th and the budget failed.
        assert_eq!(
            stalled_in(2).p95().expect("a reading"),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn enough_of_them_still_do() {
        // The other half of it: this is a percentile, not a best-of. Slow in
        // most of the windows is slow all session, and the budget is entitled
        // to notice.
        assert_eq!(
            stalled_in(5).p95().expect("a reading"),
            Duration::from_millis(100)
        );
    }

    #[test]
    fn one_slow_launch_in_every_window_is_still_not_the_reading() {
        // The statistic stays a tail inside the window. A single launch that
        // stalled cannot be a window's answer, so a run peppered with them
        // from end to end is not slow either.
        let mut all = Vec::with_capacity(RUNS);

        for _ in 0..WINDOWS {
            for run in 0..PER_WINDOW {
                all.push(Duration::from_millis(if run == 0 { 100 } else { 1 }));
            }
        }

        assert_eq!(
            Readings::new(all).p95().expect("a reading"),
            Duration::from_millis(1)
        );
    }

    #[test]
    fn the_spread_shows_a_middle_sitting_far_below_the_tail() {
        // What tells a program that got slower from a machine that stalled:
        // the windows are printed in the order they ran, and the slow one is
        // on its own at the front.
        let said = stalled_in(1).spread();

        assert!(said.contains(&format!("{RUNS} runs")), "{said}");
        assert!(said.contains("median 1.0 ms"), "{said}");
        assert!(said.contains("p95 1.0 ms"), "{said}");
        assert!(said.contains("worst 100.0 ms"), "{said}");
        assert!(said.contains("windows 100.0 ms, 1.0 ms, 1.0 ms"), "{said}");
        assert_eq!(said.matches("100.0 ms").count(), 2, "{said}");
    }

    #[test]
    fn a_spread_of_nothing_says_so_rather_than_naming_a_reading() {
        assert_eq!(Readings::new(Vec::new()).spread(), "no runs");
    }

    /// A full set of readings, every launch of it costing the same.
    fn level(ms: u64) -> Readings {
        Readings::new(vec![Duration::from_millis(ms); RUNS])
    }

    /// The reading [`best`] settles on over `batches`, and how many it took.
    fn best_of(batches: &[u64], budget: Duration) -> (Duration, usize) {
        let at = Cell::new(0);

        let reading = best(budget, || {
            let now = at.get();
            at.set(now + 1);

            Ok(level(batches.get(now).copied().unwrap_or_default()))
        })
        .expect("a reading");

        (reading.p95().expect("a reading"), at.get())
    }

    #[test]
    fn a_batch_the_host_stalled_through_is_taken_again() {
        // What the windows cannot answer: a runner that handed this process a
        // core for a fraction of the wall clock it asked for stalls every
        // window at once, and no order statistic inside one batch sits outside
        // that. A second batch does.
        let (reading, batches) = best_of(&[90, 10, 10], Duration::from_millis(50));

        assert_eq!(batches, 2);
        assert_eq!(reading, Duration::from_millis(10));
    }

    #[test]
    fn a_startup_that_got_slower_is_over_budget_in_every_batch() {
        // The other half of it, and the reason this is not a way to pass: a
        // program that got slower is slow however many times it is launched,
        // so the quickest of three is still over.
        let budget = Duration::from_millis(50);
        let (reading, batches) = best_of(&[90, 70, 80], budget);

        assert_eq!(batches, AT_LIBERTY);
        assert_eq!(reading, Duration::from_millis(70));
        assert!(reading > budget);
    }

    #[test]
    fn a_batch_inside_its_budget_is_the_only_one_taken() {
        // A batch costs a couple of hundred launches, so nothing pays for one
        // that had nothing to answer.
        let (reading, batches) = best_of(&[10, 90, 90], Duration::from_millis(50));

        assert_eq!(batches, 1);
        assert_eq!(reading, Duration::from_millis(10));
    }

    #[test]
    fn the_fixture_exercises_four_current_format_titles() {
        let home = Scratch::new("current-format").expect("a benchmark home");
        let workspace = crucible_workspace::Workspace::open(
            std::env::current_dir().expect("the current directory"),
        )
        .expect("a workspace");

        let recent = crucible_session::recent(
            &home.path().join("sessions"),
            crucible_session::Roots::These(&[workspace.root()]),
            crucible_session::Reach::FirstFrame,
            USABLE.len(),
        );

        assert_eq!(recent.len(), USABLE.len());
        assert!(recent.iter().all(|session| session.asked() == TITLE));
    }

    #[test]
    fn an_unnamed_runner_is_held_to_the_quiet_machine_budget() {
        // This machine, and the run RELEASING.md decides a tag on: nothing
        // names a runner, so the budget is the one the probe states.
        assert_eq!(chosen(None, 20, 150).expect("a limit"), 20);
    }

    #[test]
    fn a_shared_runner_is_held_to_its_own_limit() {
        // A CI runner, named by the workflow that runs the probe there.
        assert_eq!(
            chosen(Some(OsStr::new("shared")), 20, 150).expect("a limit"),
            150
        );
    }

    #[test]
    fn a_runner_nobody_named_is_refused_rather_than_guessed_at() {
        // A misspelt runner that quietly fell back to one of the limits would
        // be a budget nobody chose.
        let refused = chosen(Some(OsStr::new("sharde")), 20, 150).expect_err("a misspelt runner");
        assert!(
            refused.to_string().contains("CRUCIBLE_BENCH_RUNNER"),
            "{refused}"
        );
    }

    #[test]
    fn a_probe_that_names_no_screen_writes_the_configuration_it_always_has() {
        assert_eq!(configuration(None).as_bytes(), CONFIG.as_bytes());
    }

    #[test]
    fn a_probe_that_names_a_screen_writes_it_and_nothing_else() {
        let named: serde_json::Value =
            serde_json::from_str(&configuration(Some("native"))).expect("a document");
        let mut plain: serde_json::Value = serde_json::from_str(CONFIG).expect("a document");

        assert_eq!(
            named.pointer("/output/screen"),
            Some(&serde_json::Value::from("native"))
        );
        plain
            .pointer_mut("/output")
            .and_then(serde_json::Value::as_object_mut)
            .expect("an output block")
            .insert("screen".to_owned(), "native".into());
        assert_eq!(named, plain);
    }

    /// The fake terminal exists only where the probes can spawn a pseudo
    /// terminal, so its tests are built only there.
    #[cfg(target_os = "linux")]
    mod fake_terminal {
        use std::time::Duration;

        use super::super::{Answering, Answers, LATE, SPLIT, UNSENT, attributes_reply, paced};

        #[test]
        fn a_late_terminal_answers_in_pieces_long_after_the_child_decided_without_it() {
            // The child gives a terminal its patience for the cursor position
            // and the background question alike; an answer past both is late.
            assert!(LATE > crucible_tui::graphics::Deadline::STARTUP.patience * 2);
            let mut answers = Answers::new(Answering::Late);

            let said = answers.feed(b"\x1b[6n");
            assert_eq!(said, b"\x1b[1;1R".to_vec());
            assert_eq!(
                paced(Answering::Late, said),
                vec![(LATE, b"\x1b".to_vec()), (LATE + SPLIT, b"[1;1R".to_vec())]
            );
        }

        #[test]
        fn an_out_of_order_terminal_answers_the_cursor_position_ahead_of_the_question_before_it() {
            let mut answers = Answers::new(Answering::OutOfOrder);
            let mut asked = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\".to_vec();
            asked.extend_from_slice(b"\x1b[6n");

            let said = answers.feed(&asked);
            assert_eq!(said, b"\x1b[1;1R\x1b_Gi=31;OK\x1b\\".to_vec());
            assert_eq!(
                paced(Answering::OutOfOrder, said.clone()),
                vec![(Duration::ZERO, said)]
            );
        }

        #[test]
        fn an_out_of_order_terminal_holds_an_answer_until_a_cursor_position_is_asked() {
            let mut answers = Answers::new(Answering::OutOfOrder);

            assert!(answers.feed(b"\x1b[c").is_empty());
            let mut said = b"\x1b[1;1R".to_vec();
            said.extend_from_slice(&attributes_reply());
            assert_eq!(answers.feed(b"\x1b[6n"), said);
        }

        #[test]
        fn a_split_terminal_cuts_each_answer_before_and_after_every_escape_inside_it() {
            let mut answers = Answers::new(Answering::Split);

            let said = answers.feed(b"\x1b[6n\x1b_Gi=31,a=q;AAAA\x1b\\");
            assert_eq!(
                paced(Answering::Split, said),
                vec![
                    (Duration::ZERO, b"\x1b".to_vec()),
                    (SPLIT, b"[1;1R".to_vec()),
                    (SPLIT * 2, b"\x1b".to_vec()),
                    (SPLIT * 3, b"_Gi=31;OK".to_vec()),
                    (SPLIT * 4, b"\x1b".to_vec()),
                    (SPLIT * 5, b"\\".to_vec()),
                ]
            );
        }

        #[test]
        fn a_terminal_that_answers_at_once_or_never_makes_one_write_or_none() {
            assert_eq!(
                paced(Answering::AtOnce, b"\x1b[1;1R".to_vec()),
                vec![(Duration::ZERO, b"\x1b[1;1R".to_vec())]
            );
            assert!(paced(Answering::Never, b"\x1b[1;1R".to_vec()).is_empty());
            assert!(paced(Answering::Split, Vec::new()).is_empty());
        }

        #[test]
        fn a_cursor_position_query_split_across_two_reads_is_answered_once() {
            // What a real terminal receives is one write, but the master's read
            // side is free to hand it over one byte at a time. A response lost to
            // that split would make the probe stall for a query the terminal
            // answered, which is a startup budget measuring the probe instead.
            let mut answers = Answers::new(Answering::AtOnce);

            assert!(answers.feed(b"\x1b").is_empty());
            assert_eq!(answers.feed(b"[6n"), b"\x1b[1;1R".to_vec());
            assert!(answers.feed(b"more text").is_empty());
        }

        #[test]
        fn device_attributes_are_answered_with_sixel_among_the_parameters() {
            let mut answers = Answers::new(Answering::AtOnce);

            let said = answers.feed(b"\x1b[c");
            assert_eq!(said, attributes_reply());
            assert!(said.windows(3).any(|window| window == b";4;"), "{said:?}");
        }

        #[test]
        fn a_kitty_graphic_query_is_answered_with_its_own_image_id() {
            let mut answers = Answers::new(Answering::AtOnce);
            let query = b"\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\";

            assert_eq!(
                answers.feed(query),
                b"\x1b_Gi=31;OK\x1b\\".to_vec(),
                "a query is answered with the id it asked about"
            );
        }

        #[test]
        fn a_kitty_transmission_is_not_a_question_and_is_not_answered() {
            // Only `a=q` asks. Answering a picture being sent would put bytes into
            // the child's input that it never asked for, in the middle of a draw.
            let mut answers = Answers::new(Answering::AtOnce);

            assert!(
                answers
                    .feed(b"\x1b_Ga=t,f=24,s=1,v=1;AAAA\x1b\\")
                    .is_empty()
            );
        }

        #[test]
        fn iterm2_cell_size_is_answered_when_the_osc_ends_with_bel_or_st() {
            for terminator in [&b"\x07"[..], &b"\x1b\\"[..]] {
                let mut answers = Answers::new(Answering::AtOnce);
                let mut query = b"\x1b]1337;ReportCellSize".to_vec();
                query.extend_from_slice(terminator);

                assert_eq!(
                    answers.feed(&query),
                    b"\x1b]1337;ReportCellSize=16;8\x07".to_vec()
                );
            }
        }

        #[test]
        fn a_terminal_that_never_answers_says_nothing_to_any_query() {
            let mut answers = Answers::new(Answering::Never);

            assert!(answers.feed(b"\x1b[6n\x1b[c").is_empty());
            assert!(answers.feed(b"\x1b_Gi=31,a=q;AAAA\x1b\\").is_empty());
        }

        #[test]
        fn an_unterminated_query_is_bounded_rather_than_collected() {
            // A child that never closes a query is a probe that must keep reading.
            // The held bytes are a tail, not a copy of everything the child wrote.
            let mut answers = Answers::new(Answering::AtOnce);
            let flood = vec![b'x'; UNSENT * 4];

            assert!(answers.feed(b"\x1b_G").is_empty());
            assert!(answers.feed(&flood).is_empty());
            assert!(answers.pending.len() <= UNSENT);
        }

        #[test]
        fn two_queries_in_one_read_are_both_answered_in_order() {
            let mut answers = Answers::new(Answering::AtOnce);

            assert_eq!(
                answers.feed(b"\x1b[6n\x1b[6n"),
                b"\x1b[1;1R\x1b[1;1R".to_vec()
            );
        }
    }

    /// The real binary, started under the fake terminal: what it writes to
    /// the terminal it controls, wherever its own streams go.
    #[cfg(target_os = "linux")]
    mod heard {
        use std::fs;
        use std::io::Write as _;
        use std::path::PathBuf;
        use std::sync::mpsc::RecvTimeoutError;
        use std::time::{Duration, Instant};

        use super::super::{
            Answering, CEILING, CONFIG, Colour, REDIRECTED, Scratch, Seat, StartupError, contains,
            start,
        };

        /// The binary this test was built beside, one directory up from the
        /// test itself.
        fn built() -> Result<PathBuf, StartupError> {
            let binary = std::env::current_exe()?
                .parent()
                .and_then(std::path::Path::parent)
                .map(|directory| directory.join("crucible"))
                .ok_or_else(|| std::io::Error::other("the test has no directory"))?;
            if binary.is_file() {
                Ok(binary)
            } else {
                Err(StartupError::Unbuilt(binary))
            }
        }

        /// The program's name, which a run writes before anything else
        /// wherever its output goes: in its window's title on a terminal, on
        /// its opening line when its output is redirected.
        const NAMED: &str = "crucible";

        /// The prompt mark, written once the session reads keys, which is
        /// after every question startup may ask.
        const READY: &str = "\u{203a}";

        /// A key nothing at startup writes, typed once the prompt is ready.
        const TYPED: &str = "\u{00a7}";

        /// How long the terminal is listened to after what was waited for.
        const SETTLE: Duration = Duration::from_millis(200);

        /// The questions the exchange writes: the cursor position and each
        /// protocol's own.
        const QUESTIONS: [&str; 4] = ["\x1b[6n", "\x1b_G", "\x1b]1337;ReportCellSize", "\x1b[c"];

        /// Every answer the fake terminal gives, as it would read typed into
        /// the box.
        const ANSWERS: [&str; 4] = ["1;1R", "i=31;OK", "?62;4;6;9;15;22c", "ReportCellSize=16;8"];

        /// One run of the real binary under the fake terminal.
        struct Run<'a> {
            /// Names its scratch home.
            name: &'a str,
            seat: Seat,
            answering: Answering,
            colour: Colour,
            /// Its configuration file.
            document: &'a str,
            /// Whether [`TYPED`] is typed at its prompt.
            typing: bool,
        }

        /// Everything the terminal received from `run`: on a terminal seat
        /// until its prompt is drawn (and, when typing, until the key typed
        /// there came back), elsewhere until it wrote [`NAMED`], or until it
        /// ended; and for [`SETTLE`] after.
        fn heard(run: &Run<'_>) -> Result<Vec<u8>, StartupError> {
            let binary = built()?;
            let home = Scratch::holding(run.name, run.document)?;
            let (mut running, _) =
                start(&binary, home.path(), run.answering, run.colour, run.seat)?;
            let waited = if run.seat == Seat::Terminal {
                READY
            } else {
                NAMED
            };
            let deadline = Instant::now() + CEILING;
            let mut seen = Vec::new();
            let mut settled: Option<Instant> = None;
            let mut typed = false;
            loop {
                let now = Instant::now();
                if settled.is_some_and(|at| now >= at) {
                    return Ok(seen);
                }
                if now >= deadline {
                    return Err(StartupError::Timeout(waited));
                }
                match running.bytes.recv_timeout(Duration::from_millis(10)) {
                    Ok(bytes) => seen.extend_from_slice(&bytes),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return Ok(seen),
                }
                if settled.is_some() {
                    continue;
                }
                let shown = if run.seat == Seat::Redirected {
                    fs::read(home.path().join(REDIRECTED)).unwrap_or_default()
                } else {
                    seen.clone()
                };
                if !contains(&shown, waited) {
                    continue;
                }
                if !run.typing {
                    settled = Some(Instant::now() + SETTLE);
                } else if !typed {
                    running.terminal.write_all(TYPED.as_bytes())?;
                    running.terminal.flush()?;
                    typed = true;
                } else if contains(&shown, TYPED) {
                    settled = Some(Instant::now() + SETTLE);
                }
            }
        }

        fn has(heard: &[u8], text: &str) -> bool {
            contains(heard, text)
        }

        /// Where `text` first sits in `heard`.
        fn at(heard: &[u8], text: &str) -> Option<usize> {
            heard
                .windows(text.len())
                .position(|window| window == text.as_bytes())
        }

        #[test]
        fn a_named_terminal_is_asked_after_the_background_question_and_before_the_prompt() {
            let heard = heard(&Run {
                name: "asked",
                seat: Seat::Terminal,
                answering: Answering::AtOnce,
                colour: Colour::On,
                document: CONFIG,
                typing: false,
            })
            .expect("a run");
            let said = String::from_utf8_lossy(&heard);

            let background = at(&heard, "\x1b]11;?").expect(&said);
            let cursor = at(&heard, "\x1b[6n").expect(&said);
            let picture = at(&heard, "\x1b_Gi=31,").expect(&said);
            let prompt = at(&heard, READY).expect(&said);
            assert!(background < cursor, "{said:?}");
            assert!(cursor < picture, "{said:?}");
            assert!(picture < prompt, "{said:?}");
        }

        #[test]
        fn where_nothing_may_be_asked_no_question_reaches_the_terminal() {
            let never = CONFIG.replacen(r#""color": "auto""#, r#""color": "never""#, 1);
            assert_ne!(never, CONFIG);
            let cases = [
                ("quiet-redirected", Seat::Redirected, Colour::On, CONFIG),
                ("quiet-no-input", Seat::NoInput, Colour::On, CONFIG),
                ("quiet-no-color", Seat::Terminal, Colour::Off, CONFIG),
                (
                    "quiet-color-never",
                    Seat::Terminal,
                    Colour::On,
                    never.as_str(),
                ),
            ];
            for (name, seat, colour, document) in cases {
                let heard = heard(&Run {
                    name,
                    seat,
                    answering: Answering::AtOnce,
                    colour,
                    document,
                    typing: false,
                })
                .unwrap_or_else(|error| panic!("{name}: {error}"));
                let said = String::from_utf8_lossy(&heard);
                // The run got as far as it goes before any question is
                // judged absent.
                if seat == Seat::Terminal {
                    assert!(has(&heard, READY), "{name} drew no prompt: {said:?}");
                }
                // The background question as well as the exchange's.
                for question in QUESTIONS.into_iter().chain(["\x1b]11;?"]) {
                    assert!(
                        !has(&heard, question),
                        "{name} asked {question:?}: {said:?}"
                    );
                }
            }
        }

        #[test]
        fn no_answer_given_at_once_late_out_of_order_or_split_becomes_a_key() {
            // Each way the fake answers, with whether the picture's question
            // is asked at all: only after a cursor position answered within
            // the patience. Split and late answers both arrive cut right
            // after each escape, which `crossterm` reads as Esc and typed
            // characters when its read falls between the pieces: split ones
            // within the patience, late ones at the fake's fifty
            // milliseconds, past every patience, so the first cursor
            // position and the device attributes ahead of it are late and
            // drained by the exchange. What the run drew is what its key
            // reader made of whatever was left, in the reads the fake made.
            // The fake never answers the background question, and a
            // picture's answer after every wait is not among these.
            let cases = [
                ("keys-at-once", Answering::AtOnce, true),
                ("keys-never", Answering::Never, false),
                ("keys-late", Answering::Late, false),
                ("keys-out-of-order", Answering::OutOfOrder, true),
                ("keys-split", Answering::Split, true),
            ];
            for (name, answering, pictured) in cases {
                let heard = heard(&Run {
                    name,
                    seat: Seat::Terminal,
                    answering,
                    colour: Colour::On,
                    document: CONFIG,
                    typing: true,
                })
                .unwrap_or_else(|error| panic!("{name}: {error}"));
                let said = String::from_utf8_lossy(&heard);
                assert!(has(&heard, "\x1b[6n"), "{name}: {said:?}");
                assert_eq!(has(&heard, "\x1b_Gi=31,"), pictured, "{name}: {said:?}");
                // The key the test typed is the one that reached the box.
                assert!(has(&heard, TYPED), "{name} lost the typed key: {said:?}");
                for answer in ANSWERS {
                    assert!(!has(&heard, answer), "{name} typed {answer:?}: {said:?}");
                }
            }
        }
    }
}
