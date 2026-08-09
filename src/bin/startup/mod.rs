//! Timing the real binary from `exec` to something on screen.
//!
//! Shared by the two startup probes, which differ only in what they wait for
//! and what they will tolerate. Measuring in-process would leave out everything
//! that actually costs a startup — the exec, the dynamic loader, the first page
//! faults, the size of the binary itself — so this spawns `crucible` the way a
//! shell does and reads its output.
//!
//! Two things the reading does not include, both stated rather than hidden:
//! the child's output is a pipe, not a terminal, so the renderer takes its
//! plain path and the escape sequences are not assembled; and the shell's own
//! `fork` is not counted, only the `exec`. Both are small against a budget that
//! is dominated by process start.

use std::fs;
use std::io::{self, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Runs per reading. Enough that the 95th percentile means something, few
/// enough that the probe stays well under a second.
const RUNS: usize = 20;

/// A placeholder, so the binary gets past the credential check and as far as
/// drawing. Nothing is ever sent: standard input is closed, so no turn starts.
const KEY: &str = "bench-not-a-key";

/// A user-level configuration file, so the reading includes parsing one.
///
/// A budget measured against an empty home would say nothing about the thing
/// this budget exists to bound — a file read and parsed before the first frame
/// — and would keep saying nothing as the document grew. Every block is
/// represented, because they are resolved by walking the document and a block
/// nobody writes down is a block nobody measures.
const CONFIG: &str = r#"{
  "providers": {
    "anthropic": {"model": "claude-sonnet-5", "apiKeyEnv": "ANTHROPIC_API_KEY"},
    "openai": {"model": "gpt-5.2"}
  },
  "env": {"RUST_LOG": "warn", "PAGER": "cat"},
  "output": {"color": "auto", "toolDetail": "compact"}
}"#;

/// Why a reading could not be taken.
#[derive(Debug, thiserror::Error)]
pub(crate) enum StartupError {
    /// Something the probe itself needed failed.
    #[error("{0}")]
    Io(#[from] io::Error),

    /// The binary under test has not been built.
    #[error("no binary at {0}: run `cargo build --release --bin crucible`")]
    Unbuilt(PathBuf),

    /// The child exited without ever printing what was being waited for.
    #[error("crucible exited without printing {0:?}")]
    Silent(&'static str),

    /// No readings at all, so there is no percentile to take.
    #[error("no readings")]
    Nothing,
}

/// Takes [`RUNS`] readings and returns the 95th percentile.
///
/// `needle` is the first output that proves the thing being timed has happened.
/// Nearest-rank percentile: with twenty readings that is the second worst, so
/// one scheduler hiccup does not decide the answer and two do.
pub(crate) fn percentile(needle: &'static str) -> Result<Duration, StartupError> {
    let binary = beside("crucible")?;
    let home = Scratch::new(needle)?;

    let mut readings = Vec::with_capacity(RUNS);
    for _ in 0..RUNS {
        readings.push(once(&binary, home.path(), needle)?);
    }
    readings.sort_unstable();

    let rank = readings.len().saturating_mul(95).div_ceil(100).max(1) - 1;
    readings.get(rank).copied().ok_or(StartupError::Nothing)
}

/// One run: spawn, and stop the clock when `needle` arrives.
fn once(binary: &Path, home: &Path, needle: &'static str) -> Result<Duration, StartupError> {
    let started = Instant::now();

    let mut child = Command::new(binary)
        // Closed rather than a pipe left open: reaching the end of input is
        // what makes the child leave on its own once it has drawn.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        // Crucible's own home, so twenty runs write their sessions somewhere
        // this probe deletes rather than into the home directory of whoever is
        // benchmarking.
        .env(crucible_config::HOME, home)
        .env("ANTHROPIC_API_KEY", KEY)
        .spawn()?;

    let mut out = child
        .stdout
        .take()
        .ok_or_else(|| io::Error::other("no pipe on the child's stdout"))?;

    let mut seen: Vec<u8> = Vec::with_capacity(1024);
    let mut buffer = [0_u8; 4096];

    let reading = loop {
        let read = out.read(&mut buffer)?;
        if read == 0 {
            break None;
        }
        seen.extend_from_slice(buffer.get(..read).unwrap_or_default());

        // Bytes rather than text: a read can land mid-character, and a lossy
        // conversion would replace the very bytes being looked for.
        if seen
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
        {
            break Some(started.elapsed());
        }
    };

    // Drained and reaped either way, so a slow machine does not leave twenty
    // children behind holding a pipe open.
    let _ = out.read_to_end(&mut Vec::new());
    let _ = child.wait();

    reading.ok_or(StartupError::Silent(needle))
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
/// so twenty runs neither read nor write anything belonging to whoever is
/// benchmarking.
#[derive(Debug)]
pub(crate) struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Result<Self, StartupError> {
        let base = std::env::temp_dir().join(format!(
            "crucible-bench-{}-{}",
            name.trim().len(),
            std::process::id()
        ));
        fs::create_dir_all(&base)?;
        fs::write(base.join("config.json"), CONFIG)?;
        Ok(Self(base))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
