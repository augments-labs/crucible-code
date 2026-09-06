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
/// A launch costs a couple of milliseconds, so the whole probe still finishes
/// well inside a second.
const RUNS: usize = WINDOWS * PER_WINDOW;

/// A stuck launch is a failed measurement rather than a benchmark that hangs.
#[cfg(target_os = "linux")]
const CEILING: Duration = Duration::from_secs(5);

/// A normal terminal-sized window, fixed so layout work is the same everywhere.
#[cfg(target_os = "linux")]
const COLUMNS: u16 = 80;
#[cfg(target_os = "linux")]
const ROWS: u16 = 24;

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
    Workspace(#[from] crucible_core::PathError),

    /// A production-shaped benchmark session could not be started.
    #[error("{0}")]
    Session(#[from] crucible_runner::SessionError),

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

    /// No readings at all, so there is no percentile to take.
    #[error("no readings")]
    Nothing,
}

/// Takes [`RUNS`] readings of the same thing.
pub(crate) fn readings(measure: Measure) -> Result<Readings, StartupError> {
    let binary = beside("crucible")?;
    let home = Scratch::new(measure.label())?;

    let mut taken = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        home.restore_fixture()?;
        taken.push(once(&binary, home.path(), measure)?);
    }

    Ok(Readings::new(taken))
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
fn once(binary: &Path, home: &Path, measure: Measure) -> Result<Duration, StartupError> {
    let (terminal, inside) = pair()?;
    let reading = terminal.try_clone()?;
    let (sender, bytes) = mpsc::channel();
    let reader = std::thread::Builder::new()
        .name("crucible-startup-terminal".into())
        .spawn(move || read(reading, &sender))?;

    let second = inside.try_clone()?;
    let third = inside.try_clone()?;
    let started = Instant::now();
    let child = Command::new("setsid")
        .arg("--ctty")
        .arg(binary)
        .current_dir(std::env::current_dir()?)
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", home)
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .env(crucible_config::HOME, home)
        .env("ANTHROPIC_API_KEY", KEY)
        .stdin(Stdio::from(inside))
        .stdout(Stdio::from(second))
        .stderr(Stdio::from(third))
        .spawn()?;

    let mut running = Running {
        terminal,
        child,
        bytes,
        reader: Some(reader),
    };
    running.until(measure, started)
}

#[cfg(not(target_os = "linux"))]
fn once(_binary: &Path, _home: &Path, _measure: Measure) -> Result<Duration, StartupError> {
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
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .map_err(io::Error::from)?;

    Ok((File::from(terminal), inside))
}

/// Reads until the child closes the far side of the terminal.
#[cfg(target_os = "linux")]
fn read(mut terminal: File, sender: &Sender<Vec<u8>>) {
    let mut buffer = [0_u8; 8192];
    while let Ok(read) = terminal.read(&mut buffer) {
        if read == 0 {
            break;
        }
        if sender
            .send(buffer.get(..read).unwrap_or_default().to_vec())
            .is_err()
        {
            break;
        }
    }
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

    let here = crucible_core::Workspace::open(std::env::current_dir()?)?;
    let away = crucible_core::Workspace::open(elsewhere)?;

    let mut planted = HashSet::with_capacity(LOGS);

    // A log's name is its start time with a random tiebreak inside one
    // millisecond, so the pause keeps the loop's order and the directory's
    // order the same: the four matching sessions then sit after the fifty-two
    // newest candidates from the other workspace, inside the production scan's
    // sixty-four-log bound. The pause is outside the timed region.
    for nth in 0..LOGS {
        let workspace = if USABLE.contains(&nth) { &here } else { &away };
        let session = crucible_runner::Session::start(sessions, workspace, None)?;
        let name = session.id().cloned();
        if let Some(id) = name.as_ref() {
            planted.insert(OsString::from(format!("{}.jsonl", id.as_str())));
        }
        session.append(&crucible_core::Message::said(TITLE));
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
fn worked(session: &crucible_runner::Session) {
    use crucible_core::{Message, StopReason, ToolArgs, ToolCall, ToolId, ToolOutput, ToolResult};

    for turn in 0..WORKED {
        let call = ToolId::new(format!("call-{turn}"));

        session.append(&Message::said(format!("{ASKED} — turn {turn}?")));
        session.append(&Message::Agent {
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
            output: ToolOutput::ok(RETURNED),
        }]));
    }

    session.append(&Message::Agent {
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
    workspace: &crucible_core::Workspace,
    id: &crucible_core::SessionId,
) -> Result<(), StartupError> {
    if crucible_runner::glimpse(sessions, workspace, id)?.cut() {
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
    fn new(name: &str) -> Result<Self, StartupError> {
        let base = std::env::temp_dir().join(format!(
            "crucible-bench-{}-{}",
            name.trim().len(),
            std::process::id()
        ));
        fs::create_dir_all(&base)?;
        fs::write(base.join("config.json"), CONFIG)?;
        let fixture = worked_in(&base.join("sessions"))?;

        Ok(Self {
            path: base,
            fixture,
        })
    }

    fn path(&self) -> &Path {
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
    use std::time::Duration;

    use super::{AT_LIBERTY, PER_WINDOW, RUNS, Readings, Scratch, TITLE, USABLE, WINDOWS, best};

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
        let workspace =
            crucible_core::Workspace::open(std::env::current_dir().expect("the current directory"))
                .expect("a workspace");

        let recent =
            crucible_runner::recent(&home.path().join("sessions"), &workspace, USABLE.len());

        assert_eq!(recent.len(), USABLE.len());
        assert!(recent.iter().all(|session| session.asked() == TITLE));
    }
}
