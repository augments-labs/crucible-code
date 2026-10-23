//! A scripted stand-in for another vendor's agent, for proofs and nothing
//! that ships.
//!
//! The peer is an executable of its own, `crucible-fake-peer`, so what it
//! writes to standard output is the protocol a test scripted and nothing else.
//! [`install`] puts it in a test's directory under the name a dialect looks
//! for, with its [`Script`] in a file named `script` beside it, and the path it
//! returns is the executable the test configures. The peer reads that file
//! through its own path and ignores its environment, and acts on no argument,
//! so the arguments a dialect builds and the empty environment a launch starts
//! from reach it unchanged.
//!
//! A script is a sequence of [`Reply`] values, one for each start: a probe, an
//! account step and a turn each consume the next. The peer keeps its place in a
//! file named `played` beside the script, written before the reply is played,
//! so a start that crashes is never replayed and one install serves a whole
//! session of launches. A start past the last reply exits [`EXHAUSTED`], and
//! one whose place cannot be kept exits [`UNWRITABLE`], each after saying so on
//! standard error, which is what a launch failure carries and a test asserts
//! on.
//!
//! What it hears is kept, bounded, in a file named `heard`, and the arguments
//! of each start in one named `argued`, so a test can say what reached it; a
//! reply can also start a grandchild that beats into a file named `lingering`
//! until it is stopped, so a test can see whether stopping the peer reached
//! the process it started, which [`ceased`] judges for every test alike.
//!
//! A peer built from other sources than the ones calling it would play a
//! script by rules the caller does not know. Both carry a digest of those
//! sources, the peer answers `--stamp` with its own, and [`install`] refuses a
//! peer whose answer differs, naming the command that rebuilds it.

use std::fmt;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::{Map, Value, json};

mod plain;
pub mod scripts;

pub use plain::Plain;

/// The status of a start past the script's last reply.
pub const EXHAUSTED: u8 = 86;

/// The status of a start whose place in the script could not be kept.
pub const UNWRITABLE: u8 = 87;

/// The status of a start whose script could not be read.
pub const UNREADABLE: u8 = 88;

/// The most bytes of what the peer heard that are kept.
pub const HEARD_BYTES: u64 = 64 * 1024;

/// The command that builds the peer, which every refusal to install names.
pub const BUILD: &str =
    "cargo build -p crucible-external-harnesses --features proof --bin crucible-fake-peer";

/// The executable's name, as Cargo builds it.
const PEER: &str = "crucible-fake-peer";

/// The file a script is read from, beside the executable.
const SCRIPT: &str = "script";

/// The file the number of replies played is kept in.
const PLAYED: &str = "played";

/// The file what the peer heard is kept in.
const HEARD: &str = "heard";

/// The file the arguments of each start are kept in.
const ARGUED: &str = "argued";

/// The file a lingering grandchild beats into.
const LINGERING: &str = "lingering";

/// The longest a grandchild lingers, so one nothing stopped still ends.
const LINGER: Duration = Duration::from_mins(1);

/// How often a lingering grandchild beats.
const BEAT: Duration = Duration::from_millis(50);

/// How long the beat of a grandchild that should have ended is watched.
const WATCHED: Duration = Duration::from_millis(400);

/// How long the beat of a grandchild that should have ended must hold still
/// before it is watched: three of the peer's [`BEAT`]s, longer than one still
/// beating goes without a beat.
const HELD: Duration = Duration::from_millis(150);

/// The longest the beat of a grandchild that should have ended is waited on to
/// hold still: a last beat under way is one write and a rename, which land
/// within a slice of a loaded machine's scheduler, while one still beating has
/// beaten twenty times by then.
const SETTLE: Duration = Duration::from_secs(1);

/// The script format this peer reads.
const FORMAT: u64 = 1;

/// Every file under `fake_peer/scripts/`, and their bytes, from one list.
macro_rules! scripts {
    ($($file:literal),+ $(,)?) => {
        /// The script sets' files, by name.
        #[cfg(test)]
        pub(crate) const SCRIPT_FILES: &[&str] = &[$($file),+];

        /// Every source the peer's behaviour is read from.
        const SOURCES: &[&[u8]] = &[
            include_bytes!("fake_peer.rs"),
            include_bytes!("bin/fake-peer.rs"),
            $(include_bytes!(concat!("fake_peer/scripts/", $file))),+
        ];
    };
}

scripts!("mod.rs", "acp.rs", "claude.rs");

/// The digest of the sources the peer is built from: 64-bit FNV-1a, over
/// each file in turn.
pub const STAMP: u64 = digest(SOURCES);

/// FNV-1a over every byte of `sources`.
const fn digest(mut sources: &[&[u8]]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    while let [source, rest @ ..] = sources {
        let mut bytes: &[u8] = source;
        while let [byte, more @ ..] = bytes {
            hash ^= *byte as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            bytes = more;
        }
        sources = rest;
    }
    hash
}

/// The stamp as the peer prints it.
fn stamp() -> String {
    format!("{STAMP:016x}")
}

/// Which of its streams the peer writes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Standard output, which is the protocol.
    Out,
    /// Standard error, which is beside it.
    Err,
}

/// One thing a reply does.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// Writes this line to standard output.
    Say(String),
    /// Reads this many lines, or every line until its input closes.
    Hear(Option<u32>),
    /// Writes this line to standard error.
    Mutter(String),
    /// Writes this many bytes, none of them a newline.
    Flood(Stream, u64),
    /// Waits this long.
    Pause(Duration),
    /// Starts a grandchild that beats until it is stopped.
    Linger,
    /// Exits with this status.
    Exit(u8),
}

/// What the peer does for one start.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reply {
    /// What it does, in order; once done it exits successfully.
    steps: Vec<Step>,
}

impl Reply {
    /// A reply that does nothing and exits successfully.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A reply that writes each of `lines` and exits, as an account step
    /// does: a turn's reply hears its opening first, since a program that has
    /// exited cannot be spoken to.
    #[must_use]
    pub fn lines<L: Into<String>>(lines: impl IntoIterator<Item = L>) -> Self {
        lines.into_iter().fold(Self::new(), Self::say)
    }

    /// Then writes `line` to standard output.
    #[must_use]
    pub fn say(mut self, line: impl Into<String>) -> Self {
        self.steps.push(Step::Say(line.into()));
        self
    }

    /// Then reads `count` lines; input that closes first ends the reply.
    #[must_use]
    pub fn hear(mut self, count: u32) -> Self {
        self.steps.push(Step::Hear(Some(count)));
        self
    }

    /// Then reads until its input closes.
    #[must_use]
    pub fn hear_out(mut self) -> Self {
        self.steps.push(Step::Hear(None));
        self
    }

    /// Then writes `line` to standard error.
    #[must_use]
    pub fn mutter(mut self, line: impl Into<String>) -> Self {
        self.steps.push(Step::Mutter(line.into()));
        self
    }

    /// Then writes `bytes` bytes to `stream`, none of them a newline.
    #[must_use]
    pub fn flood(mut self, stream: Stream, bytes: u64) -> Self {
        self.steps.push(Step::Flood(stream, bytes));
        self
    }

    /// Then waits `pause`.
    #[must_use]
    pub fn pause(mut self, pause: Duration) -> Self {
        self.steps.push(Step::Pause(pause));
        self
    }

    /// Then starts a grandchild that beats into the install directory until
    /// it is stopped, and goes on once it has begun to.
    #[must_use]
    pub fn linger(mut self) -> Self {
        self.steps.push(Step::Linger);
        self
    }

    /// Then exits with `status`.
    #[must_use]
    pub fn exit(mut self, status: u8) -> Self {
        self.steps.push(Step::Exit(status));
        self
    }

    /// This reply as the script file writes it.
    fn encoded(&self) -> Value {
        Value::Array(
            self.steps
                .iter()
                .map(|step| match step {
                    Step::Say(line) => json!({ "say": line }),
                    Step::Hear(count) => json!({ "hear": count }),
                    Step::Mutter(line) => json!({ "mutter": line }),
                    Step::Flood(stream, bytes) => json!({
                        "flood": {
                            "to": match stream {
                                Stream::Out => "stdout",
                                Stream::Err => "stderr",
                            },
                            "bytes": bytes,
                        }
                    }),
                    Step::Pause(pause) => {
                        json!({ "pause": u64::try_from(pause.as_millis()).unwrap_or(u64::MAX) })
                    }
                    Step::Linger => json!({ "linger": null }),
                    Step::Exit(status) => json!({ "exit": status }),
                })
                .collect(),
        )
    }

    /// The reply `value` encodes, or nothing where it encodes none.
    fn decoded(value: &Value) -> Option<Self> {
        let steps = value
            .as_array()?
            .iter()
            .map(|step| {
                let step: &Map<String, Value> = step.as_object()?;
                let (name, value) = step.iter().next().filter(|_| step.len() == 1)?;
                Some(match name.as_str() {
                    "say" => Step::Say(value.as_str()?.to_owned()),
                    "hear" if value.is_null() => Step::Hear(None),
                    "hear" => Step::Hear(Some(u32::try_from(value.as_u64()?).ok()?)),
                    "mutter" => Step::Mutter(value.as_str()?.to_owned()),
                    "flood" => {
                        let stream = match value.get("to")?.as_str()? {
                            "stdout" => Stream::Out,
                            "stderr" => Stream::Err,
                            _ => return None,
                        };
                        Step::Flood(stream, value.get("bytes")?.as_u64()?)
                    }
                    "pause" => Step::Pause(Duration::from_millis(value.as_u64()?)),
                    "linger" => Step::Linger,
                    "exit" => Step::Exit(u8::try_from(value.as_u64()?).ok()?),
                    _ => return None,
                })
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { steps })
    }
}

/// Every reply the peer gives, one per start, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Script {
    /// The replies, in the order the starts consume them.
    replies: Vec<Reply>,
}

impl Script {
    /// The script that gives `replies` in order.
    #[must_use]
    pub fn of(replies: impl IntoIterator<Item = Reply>) -> Self {
        Self {
            replies: replies.into_iter().collect(),
        }
    }

    /// This script as its file holds it.
    pub(crate) fn encoded(&self) -> Vec<u8> {
        json!({
            "format": FORMAT,
            "replies": self.replies.iter().map(Reply::encoded).collect::<Vec<_>>(),
        })
        .to_string()
        .into_bytes()
    }

    /// The script `bytes` hold, or nothing where they hold none.
    pub(crate) fn decoded(bytes: &[u8]) -> Option<Self> {
        let value: Value = serde_json::from_slice(bytes).ok()?;
        if value.get("format")?.as_u64()? != FORMAT {
            return None;
        }
        let replies = value
            .get("replies")?
            .as_array()?
            .iter()
            .map(Reply::decoded)
            .collect::<Option<Vec<_>>>()?;
        Some(Self { replies })
    }
}

/// Why a peer was not installed.
#[derive(Debug, thiserror::Error)]
pub enum Unbuilt {
    /// No peer is where the build puts it.
    #[error("no fake peer at {}; build it with `{}`", .0.display(), BUILD)]
    Absent(PathBuf),
    /// The peer there was built from other sources than this caller.
    #[error(
        "the fake peer at {} was built from other sources than this test; rebuild it with `{}`",
        .0.display(),
        BUILD
    )]
    Stale(PathBuf),
    /// It could not be put in place.
    #[error("the fake peer could not be installed: {0}")]
    Io(#[from] io::Error),
}

/// Puts the peer in `directory` as `program`, the name a dialect looks for,
/// with the platform's executable suffix, playing `script` from its first
/// reply; and returns the path to configure as the agent's executable.
///
/// The peer is a copy of the one Cargo built beside this test's executable,
/// never a link to it, so another test's install or cleanup cannot move the
/// change time a launch identifies it by.
///
/// # Errors
///
/// [`Unbuilt`] where there is none, where it was built from other sources, or
/// where it could not be put in place.
pub fn install(directory: &Path, program: &str, script: &Script) -> Result<PathBuf, Unbuilt> {
    installed(&built()?, directory, program, script)
}

/// Where Cargo puts a workspace binary for the test running now: beside the
/// `deps` directory the test executable is in.
fn built() -> io::Result<PathBuf> {
    let test = std::env::current_exe()?;
    let deps = test.parent().unwrap_or(&test);
    let profile = deps.parent().unwrap_or(deps);
    Ok(profile.join(format!("{PEER}{}", std::env::consts::EXE_SUFFIX)))
}

/// Installs the peer at `peer`, as [`install`] does.
pub(crate) fn installed(
    peer: &Path,
    directory: &Path,
    program: &str,
    script: &Script,
) -> Result<PathBuf, Unbuilt> {
    if !peer.is_file() {
        return Err(Unbuilt::Absent(peer.to_path_buf()));
    }
    let answer = Command::new(peer)
        .arg("--stamp")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()?;
    if !answer.status.success() || String::from_utf8_lossy(&answer.stdout).trim() != stamp() {
        return Err(Unbuilt::Stale(peer.to_path_buf()));
    }
    fs::create_dir_all(directory)?;
    let target = directory.join(format!("{program}{}", std::env::consts::EXE_SUFFIX));
    for leftover in [
        target.clone(),
        directory.join(PLAYED),
        directory.join(HEARD),
        directory.join(ARGUED),
    ] {
        match fs::remove_file(&leftover) {
            Err(problem) if problem.kind() != io::ErrorKind::NotFound => return Err(problem.into()),
            Ok(()) | Err(_) => {}
        }
    }
    // A copy, never a link: a link shares the built peer's one inode, and
    // every other test's link and unlink moves that inode's change time,
    // which a launch compares with the time it found. The copy is written and
    // closed here before anything runs it, and std opens it close-on-exec; the
    // exposure left is a process forked by another test thread while it is
    // open for writing, which holds that descriptor until it executes and can
    // make running this one fail with "text file busy" for that moment.
    fs::copy(peer, &target)?;
    fs::write(directory.join(SCRIPT), script.encoded())?;
    Ok(target)
}

/// How many starts the peer installed in `directory` has played, which is
/// none where it was never started.
#[must_use]
pub fn played(directory: &Path) -> usize {
    counter(directory).unwrap_or(0)
}

/// What the peer installed in `directory` heard, line by line, as far as it
/// was kept.
#[must_use]
pub fn heard(directory: &Path) -> Vec<String> {
    fs::read_to_string(directory.join(HEARD))
        .map(|heard| heard.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The arguments each start of the peer installed in `directory` was given,
/// in order, as far as they were kept.
#[must_use]
pub fn argued(directory: &Path) -> Vec<Vec<String>> {
    fs::read_to_string(directory.join(ARGUED))
        .map(|argued| {
            argued
                .lines()
                .map(|start| {
                    start
                        .split('\t')
                        .filter(|argument| !argument.is_empty())
                        .map(str::to_owned)
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The last beat of the grandchild a reply in `directory` started, where it
/// has beaten.
#[must_use]
pub fn lingering(directory: &Path) -> Option<u64> {
    fs::read_to_string(directory.join(LINGERING))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The beats a grandchild that should have ended was seen at: the one it had
/// settled at, and the one it had beaten to after [`WATCHED`] more. Where it
/// never settled, the first beat seen and the last.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Beating {
    /// The beat first seen to hold still, or first seen.
    pub first: Option<u64>,
    /// The beat last seen.
    pub last: Option<u64>,
}

impl fmt::Display for Beating {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(out, "{:?} then {:?}", self.first, self.last)
    }
}

/// Whether the grandchild a reply in `directory` started has ended, after its
/// owner was stopped: its beat is waited on to hold still for [`HELD`], and
/// then watched for [`WATCHED`], and a grandchild that beat in between is
/// [`Beating`].
///
/// A stop can return once the kill is sent and before the grandchild is gone,
/// and a grandchild the kill reached inside its write or rename finishes that
/// one call, so exactly one beat can land after the stop. A beat taken
/// straight after the stop and compared with a later one reads that beat as a
/// process still running, so every test that judges a stop by the beat does
/// it here.
///
/// # Errors
///
/// [`Beating`] where the beat did not hold still within [`SETTLE`], or beat
/// again while it was watched.
pub async fn ceased(directory: &Path) -> Result<(), Beating> {
    let first = settled(directory).await?;
    tokio::time::sleep(WATCHED).await;
    match lingering(directory) {
        last if last == first => Ok(()),
        last => Err(Beating { first, last }),
    }
}

/// The beat of the grandchild a reply in `directory` started, once it has held
/// still for [`HELD`], or the first and last beats seen where it has not
/// within [`SETTLE`].
async fn settled(directory: &Path) -> Result<Option<u64>, Beating> {
    let began = tokio::time::Instant::now();
    let first = lingering(directory);
    let mut was = first;
    loop {
        tokio::time::sleep(HELD).await;
        // Beats only ever count up, so two looks that agree saw none between.
        let now = lingering(directory);
        if now == was {
            return Ok(now);
        }
        if began.elapsed() >= SETTLE {
            return Err(Beating { first, last: now });
        }
        was = now;
    }
}

/// The number kept in `directory`'s counter: none where there is no counter.
fn counter(directory: &Path) -> io::Result<usize> {
    match fs::read_to_string(directory.join(PLAYED)) {
        Ok(text) => text
            .trim()
            .parse()
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "the counter is not a number")),
        Err(problem) if problem.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(problem) => Err(problem),
    }
}

/// Keeps `played` as `directory`'s counter, whole or not at all.
fn keep(directory: &Path, played: usize) -> io::Result<()> {
    let writing = directory.join(format!("{PLAYED}.writing"));
    fs::write(&writing, played.to_string())?;
    fs::rename(&writing, directory.join(PLAYED))
}

/// The peer's `main`.
#[must_use]
pub fn entry() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    match arguments.next() {
        Some(first) if first == "--stamp" => {
            let mut out = io::stdout().lock();
            match writeln!(out, "{}", stamp()).and_then(|()| out.flush()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            }
        }
        Some(first) if first == "--linger" => match arguments.next() {
            Some(directory) => linger(Path::new(&directory)),
            None => ExitCode::FAILURE,
        },
        _ => play(),
    }
}

/// Says `line` on standard error, and exits with `status`.
fn refuse(line: &str, status: u8) -> ExitCode {
    let mut err = io::stderr().lock();
    // There is nobody to tell that standard error is gone.
    let _ = writeln!(err, "{line}").and_then(|()| err.flush());
    ExitCode::from(status)
}

/// Plays the next reply of the script beside this executable.
fn play() -> ExitCode {
    let directory = match std::env::current_exe() {
        Ok(itself) => match itself.parent() {
            Some(directory) => directory.to_path_buf(),
            None => return refuse("fake peer: cannot find its directory", UNREADABLE),
        },
        Err(_) => return refuse("fake peer: cannot find itself", UNREADABLE),
    };
    let Some(script) = fs::read(directory.join(SCRIPT))
        .ok()
        .and_then(|bytes| Script::decoded(&bytes))
    else {
        return refuse("fake peer: script unreadable", UNREADABLE);
    };
    let Ok(played) = counter(&directory) else {
        return refuse("fake peer: counter unwritable", UNWRITABLE);
    };
    let Some(reply) = script.replies.get(played) else {
        return refuse("fake peer: script exhausted", EXHAUSTED);
    };
    // Kept before the reply is played, so a start that crashes is not played
    // again by the next one.
    if keep(&directory, played.saturating_add(1)).is_err() {
        return refuse("fake peer: counter unwritable", UNWRITABLE);
    }
    let arguments: Vec<String> = std::env::args_os()
        .skip(1)
        .map(|argument| argument.to_string_lossy().replace(['\t', '\n'], " "))
        .collect();
    remember(
        &directory,
        ARGUED,
        format!("{}\n", arguments.join("\t")).as_bytes(),
    );
    match perform(&directory, reply) {
        Ok(status) => status,
        // Its host stopped listening; there is nobody left to play to.
        Err(_) => ExitCode::FAILURE,
    }
}

/// Does what `reply` says, in `directory`.
fn perform(directory: &Path, reply: &Reply) -> io::Result<ExitCode> {
    let mut input = io::stdin().lock();
    for step in &reply.steps {
        match step {
            Step::Say(line) => {
                let mut out = io::stdout().lock();
                writeln!(out, "{line}")?;
                out.flush()?;
            }
            Step::Hear(count) => {
                let mut left = *count;
                while left != Some(0) {
                    let mut line = Vec::new();
                    let read = (&mut input)
                        .take(crucible_transport::FRAME_BYTES as u64 + 1)
                        .read_until(b'\n', &mut line)?;
                    if read == 0 {
                        return Ok(ExitCode::SUCCESS);
                    }
                    remember(directory, HEARD, &line);
                    left = left.map(|left| left.saturating_sub(1));
                }
            }
            Step::Mutter(line) => {
                let mut err = io::stderr().lock();
                writeln!(err, "{line}")?;
                err.flush()?;
            }
            Step::Flood(stream, bytes) => {
                let chunk = [b'x'; 8 * 1024];
                let mut left = *bytes;
                while left > 0 {
                    let now = usize::try_from(left.min(chunk.len() as u64)).unwrap_or(chunk.len());
                    let piece = chunk.get(..now).unwrap_or(&chunk);
                    match stream {
                        Stream::Out => io::stdout().lock().write_all(piece)?,
                        Stream::Err => io::stderr().lock().write_all(piece)?,
                    }
                    left = left.saturating_sub(now as u64);
                }
                match stream {
                    Stream::Out => io::stdout().lock().flush()?,
                    Stream::Err => io::stderr().lock().flush()?,
                }
            }
            Step::Pause(pause) => std::thread::sleep(*pause),
            Step::Linger => {
                Command::new(std::env::current_exe()?)
                    .arg("--linger")
                    .arg(directory)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?;
                let mut waited = Duration::ZERO;
                while lingering(directory).is_none() && waited < Duration::from_secs(5) {
                    std::thread::sleep(BEAT);
                    waited = waited.saturating_add(BEAT);
                }
            }
            Step::Exit(status) => return Ok(ExitCode::from(*status)),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Keeps `line` in `directory`'s file `file`, while what it holds is under
/// [`HEARD_BYTES`].
fn remember(directory: &Path, file: &str, line: &[u8]) {
    let path = directory.join(file);
    let kept = fs::metadata(&path).map_or(0, |kept| kept.len());
    if kept.saturating_add(line.len() as u64) > HEARD_BYTES {
        return;
    }
    // What was heard is evidence for the test, not part of the reply.
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line));
}

/// Beats into `directory` until stopped, or until [`LINGER`] has passed. Each
/// beat is written beside the file and renamed over it, so a reader sees the
/// last beat whole and never a file that is being truncated or half written.
fn linger(directory: &Path) -> ExitCode {
    let path = directory.join(LINGERING);
    let writing = directory.join(format!("{LINGERING}.writing"));
    let beats = LINGER.as_millis() / BEAT.as_millis();
    for beat in 0..beats {
        if fs::write(&writing, beat.to_string())
            .and_then(|()| fs::rename(&writing, &path))
            .is_err()
        {
            return ExitCode::FAILURE;
        }
        std::thread::sleep(BEAT);
    }
    ExitCode::SUCCESS
}
