//! The host's kernel memory while a formula has TeX open files for as long
//! as it is let.
//!
//! The sandbox bounds what a run may write by the bytes it writes, but a file
//! TeX opens and closes empty costs nothing against that bound and still
//! costs the host's kernel an inode and a directory entry, which live in its
//! slab caches outside every limit a run is given. A formula can loop
//! `\immediate\openout` over new names for the whole of its command time, so
//! this measures what that costs the host: `Slab` in `/proc/meminfo`, sampled
//! every 50 ms, at its peak over the run against where it stood as the run
//! began, net of the same reading over 8 s with nothing running just before.
//! Three such pairs, control then run, so a host whose slab drifts on its own
//! is read as drift and not as TeX.
//!
//! The budget holds both ends. No pair's net peak rise may pass 128 MiB, the
//! most one formula may take of a host's kernel memory, and the median of
//! the three must reach 8 MiB, so a run that never reached its loop, which
//! would hold the ceiling trivially, is not a measurement. Every run must also
//! be stopped by its command time and answered as having run out of it, within
//! the engine's own deadlines.
//!
//! It runs the real engine on this machine's own sandbox, with this machine's
//! own TeX, found as a session finds them; the sandbox's broker is looked for
//! beside this probe, where `scripts/sh/bench.sh tex-slab` builds it. Where
//! TeX or the backend is absent it fails, saying which, and never skips.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use crucible_app::math::{
    COMMAND_TIME, MARGIN, MathPictures, STAGING, TexEngine, Unavailable, Unmade, available,
};
#[cfg(target_os = "linux")]
use crucible_sandbox::SandboxService;
#[cfg(target_os = "linux")]
use crucible_sandbox_local::LocalSandbox;

/// The most one pair's net peak rise may be, in MiB.
const LIMIT: f64 = 128.0;

/// The least the median net peak rise must be, in MiB.
const FLOOR: f64 = 8.0;

/// How often `Slab` is read.
#[cfg(target_os = "linux")]
const SAMPLE: Duration = Duration::from_millis(50);

/// How long each control reads the host with nothing running.
#[cfg(target_os = "linux")]
const CONTROL: Duration = Duration::from_secs(8);

/// Control and run pairs.
#[cfg(target_os = "linux")]
const PAIRS: usize = 3;

/// A formula that opens and closes a file under a new name, over and over,
/// until its run is stopped.
#[cfg(target_os = "linux")]
const OPENING: &str = r"\count255=0 \loop\immediate\openout15=f\the\count255.tex \immediate\closeout15 \advance\count255 by 1 \ifnum\count255>0 \repeat";

/// The name of the sandbox's broker, looked for beside this probe.
#[cfg(target_os = "linux")]
const BROKER: &str = "crucible-sandbox-broker";

/// What can go wrong in the probe itself.
#[derive(Debug, thiserror::Error)]
enum ProbeError {
    /// The measurement could not be reported, or the host read.
    #[error("bench-tex-slab: {0}")]
    Io(#[from] io::Error),

    /// The sandbox this probe measures is Linux's.
    #[cfg(not(target_os = "linux"))]
    #[error("bench-tex-slab: the sandbox backend is absent: this probe measures Linux's")]
    Unsupported,

    /// The broker was not built beside the probe.
    #[cfg(target_os = "linux")]
    #[error(
        "bench-tex-slab: the sandbox backend is absent: no {BROKER} beside this probe; \
         `scripts/sh/bench.sh tex-slab` builds it"
    )]
    NoBroker,

    /// The check found no TeX, or no backend that holds a run's limits.
    #[cfg(target_os = "linux")]
    #[error("bench-tex-slab: {absent} is absent: {why}")]
    Absent {
        absent: &'static str,
        why: Unavailable,
    },

    /// `/proc/meminfo` had no `Slab` line this probe could read.
    #[cfg(target_os = "linux")]
    #[error("bench-tex-slab: /proc/meminfo has no Slab line")]
    Unread,

    /// A run was not stopped by its command time.
    #[cfg(target_os = "linux")]
    #[error(
        "bench-tex-slab: run {pair} answered {answer} after {took:?}, not run out of its time \
         by {COMMAND_TIME:?}"
    )]
    Unstopped {
        pair: usize,
        answer: String,
        took: Duration,
    },

    /// A runtime could not be built, or a run's task failed.
    #[cfg(target_os = "linux")]
    #[error("bench-tex-slab: the run's task failed: {0}")]
    Task(String),
}

/// The host's `Slab`, in KiB.
#[cfg(target_os = "linux")]
fn slab() -> Result<u64, ProbeError> {
    let info = std::fs::read_to_string("/proc/meminfo")?;
    info.lines()
        .find_map(|line| line.strip_prefix("Slab:"))
        .and_then(|rest| rest.trim().strip_suffix("kB"))
        .and_then(|kib| kib.trim().parse().ok())
        .ok_or(ProbeError::Unread)
}

/// The highest `Slab` read every [`SAMPLE`] until `over` says to stop, less
/// the reading as it began, in MiB.
#[cfg(target_os = "linux")]
fn peak_rise(mut over: impl FnMut() -> bool) -> Result<f64, ProbeError> {
    let first = slab()?;
    let mut peak = first;
    while !over() {
        std::thread::sleep(SAMPLE);
        peak = peak.max(slab()?);
    }
    peak = peak.max(slab()?);
    // KiB to MiB, and a peak never under where it began.
    #[allow(
        clippy::cast_precision_loss,
        reason = "a host's slab in KiB is far below 2^52"
    )]
    let rise = peak.saturating_sub(first) as f64 / 1024.0;
    Ok(rise)
}

/// What one pair read.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy)]
struct Pair {
    control: f64,
    run: f64,
    took: Duration,
}

#[cfg(target_os = "linux")]
impl Pair {
    fn net(self) -> f64 {
        self.run - self.control
    }
}

/// The engine, as a session's check finds it on this machine's sandbox.
#[cfg(target_os = "linux")]
fn engine(runtime: &tokio::runtime::Runtime) -> Result<Arc<TexEngine>, ProbeError> {
    let own = std::env::current_exe()?;
    if !own.with_file_name(BROKER).is_file() {
        return Err(ProbeError::NoBroker);
    }
    let service: Arc<dyn SandboxService> =
        Arc::new(LocalSandbox::new().watching_on(runtime.handle().clone()));
    runtime
        .block_on(available(service, COMMAND_TIME))
        .map(Arc::new)
        .map_err(|why| ProbeError::Absent {
            absent: match why {
                Unavailable::ProgramMissing => "TeX",
                Unavailable::NoBackend
                | Unavailable::Refused(_)
                | Unavailable::SandboxRefused
                | Unavailable::Unconfined => "the sandbox backend",
                Unavailable::TimedOut | Unavailable::NotReached => "a TeX check that finishes",
            },
            why,
        })
}

/// One control, then one run of [`OPENING`].
#[cfg(target_os = "linux")]
fn pair(
    index: usize,
    runtime: &tokio::runtime::Runtime,
    engine: &Arc<TexEngine>,
) -> Result<Pair, ProbeError> {
    let quiet = Instant::now();
    let control = peak_rise(|| quiet.elapsed() >= CONTROL)?;

    let making = Arc::clone(engine);
    let began = Instant::now();
    let task = runtime.spawn(async move { making.make(OPENING).await });
    let run = peak_rise(|| task.is_finished())?;
    let took = began.elapsed();
    let answer = runtime
        .block_on(task)
        .map_err(|failed| ProbeError::Task(failed.to_string()))?;
    let stopped = matches!(answer, Err(Unmade::Timeout))
        && took >= COMMAND_TIME
        && took < STAGING + COMMAND_TIME + MARGIN;
    if !stopped {
        return Err(ProbeError::Unstopped {
            pair: index,
            answer: match answer {
                Ok(svg) => format!("a picture of {} bytes", svg.bytes().len()),
                Err(unmade) => format!("{unmade:?}"),
            },
            took,
        });
    }
    Ok(Pair { control, run, took })
}

#[cfg(target_os = "linux")]
fn measure() -> Result<Vec<Pair>, ProbeError> {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()?;
    let engine = engine(&runtime)?;
    (1..=PAIRS)
        .map(|index| pair(index, &runtime, &engine))
        .collect()
}

#[cfg(not(target_os = "linux"))]
fn measure() -> Result<Vec<()>, ProbeError> {
    Err(ProbeError::Unsupported)
}

/// Reports the pairs, and says whether they hold the budget.
#[cfg(target_os = "linux")]
fn report(pairs: &[Pair]) -> Result<bool, ProbeError> {
    let mut nets: Vec<f64> = pairs.iter().map(|pair| pair.net()).collect();
    nets.sort_by(f64::total_cmp);
    let worst = nets.last().copied().unwrap_or(f64::INFINITY);
    let median = nets.get(nets.len() / 2).copied().unwrap_or(0.0);

    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on.
    let mut line = format!("{worst:.1} MiB {LIMIT:.0} median={median:.1} floor={FLOOR:.0}");
    for (index, pair) in pairs.iter().enumerate() {
        let _ = write!(
            line,
            " control{n}={:.1} run{n}={:.1} net{n}={:.1}",
            pair.control,
            pair.run,
            pair.net(),
            n = index + 1,
        );
    }
    line.push('\n');
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()?;

    let mut evidence = String::new();
    for (index, pair) in pairs.iter().enumerate() {
        let _ = writeln!(
            evidence,
            "         pair {}: slab rose {:.1} MiB idle and {:.1} MiB under TeX, which was \
             stopped after {:.1} s",
            index + 1,
            pair.control,
            pair.run,
            pair.took.as_secs_f64(),
        );
    }
    let holds = worst <= LIMIT && median >= FLOOR;
    if median < FLOOR {
        let _ = writeln!(
            evidence,
            "    FAIL the median net rise, {median:.1} MiB, is under {FLOOR:.0} MiB: TeX's loop \
             did not reach the slab, so its ceiling measured nothing"
        );
    }
    io::stderr().write_all(evidence.as_bytes())?;
    Ok(holds)
}

/// Says why no reading could be taken at all, on stderr, where
/// `scripts/sh/bench.sh` puts everything a human reads.
fn explain(problem: &ProbeError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL {problem}");
    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let pairs = match measure() {
        Ok(pairs) => pairs,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };
    #[cfg(target_os = "linux")]
    {
        match report(&pairs) {
            Ok(true) => ExitCode::SUCCESS,
            Ok(false) => ExitCode::FAILURE,
            Err(problem) => {
                let _ = explain(&problem);
                ExitCode::FAILURE
            }
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pairs, LIMIT, FLOOR);
        ExitCode::FAILURE
    }
}
