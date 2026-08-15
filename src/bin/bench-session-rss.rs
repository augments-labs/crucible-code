//! Peak resident memory of the shipped process after a twenty-turn session.
//!
//! The measured process is `target/release/crucible`, not a runner assembled
//! inside this probe. It starts behind a controlling pseudo terminal, reads its
//! normal configuration, draws through its normal terminal, writes normal
//! session logs and sends HTTP through its normal provider adapter. The server
//! is a deterministic loopback fixture in this parent process; no request can
//! leave the machine and no production-only seam is involved.
//!
//! Each turn asks the real read tool to read a planted file, then receives a
//! streamed answer. RSS and proportional set size come from `smaps_rollup` at
//! startup and after turns one, five and twenty; `VmHWM` supplies the child's
//! peak, including spikes between samples. Thread, descriptor and process-tree
//! counts are emitted beside the budget so a flat total cannot hide a growing
//! resource of another kind.

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::process::ExitCode;

#[cfg(target_os = "linux")]
use std::ffi::OsStr;
#[cfg(target_os = "linux")]
use std::fs::{self, File, OpenOptions};
#[cfg(target_os = "linux")]
use std::io::{BufRead as _, BufReader, Read as _};
#[cfg(target_os = "linux")]
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
#[cfg(target_os = "linux")]
use std::os::unix::ffi::OsStrExt as _;
#[cfg(target_os = "linux")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "linux")]
use std::process::{Child, Command, Stdio};
#[cfg(target_os = "linux")]
use std::sync::Arc;
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(target_os = "linux")]
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
#[cfg(target_os = "linux")]
use std::thread::JoinHandle;
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use rustix::pty::{self, OpenptFlags};
#[cfg(target_os = "linux")]
use rustix::termios::{self, OptionalActions, Winsize};

/// The budget, in megabytes.
const LIMIT: f64 = 35.0;

/// Turns in the shipped session being measured.
#[cfg(target_os = "linux")]
const TURNS: usize = 20;

/// Lines the real read tool sees in every turn.
#[cfg(target_os = "linux")]
const PLANTED: usize = 2_000;

/// Text deltas in every completed answer.
#[cfg(target_os = "linux")]
const DELTAS: usize = 400;

#[cfg(target_os = "linux")]
const COLUMNS: u16 = 80;
#[cfg(target_os = "linux")]
const ROWS: u16 = 24;

/// A launch or turn that stops producing bytes is a failed measurement.
#[cfg(target_os = "linux")]
const CEILING: Duration = Duration::from_secs(10);

/// After the proof arrives, this much silence lets the turn-finished event and
/// session writer catch up before the process is sampled.
#[cfg(target_os = "linux")]
const QUIET: Duration = Duration::from_millis(20);

/// Bounds the process-tree walk if the fixture ever starts spawning children.
#[cfg(target_os = "linux")]
const PROCESSES: usize = 64;

/// What can go wrong in the probe itself.
#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("bench-session-rss: {0}")]
    Io(#[from] io::Error),

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss: crucible did not reach {0:?} within ten seconds")]
    Timeout(Box<str>),

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss: crucible exited before reaching {0:?}")]
    Exited(Box<str>),

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss: no {field} reading for process {pid}")]
    Unmeasurable { pid: u32, field: &'static str },

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss: process tree exceeded {PROCESSES} processes")]
    ProcessTree,

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss: local provider thread panicked")]
    ProviderPanicked,

    #[cfg(not(target_os = "linux"))]
    #[error("bench-session-rss: shipped-process RSS measurement requires Linux")]
    Unsupported,
}

/// The evidence kept under the existing RSS budget value.
#[derive(Debug, Clone, Copy)]
struct Measurement {
    peak: f64,
    start: f64,
    one: f64,
    five: f64,
    twenty: f64,
    pss: f64,
    slope: f64,
    threads: usize,
    fds: usize,
    processes: usize,
}

#[cfg(target_os = "linux")]
fn measure() -> Result<Measurement, ProbeError> {
    let mut vendor = Vendor::start()?;
    let scratch = Scratch::new(vendor.address())?;
    let binary = beside("crucible")?;
    let mut running = Running::start(&binary, &scratch)?;

    running.until("ask mode on")?;
    let start = Resource::read(running.id())?;
    let mut one = start;
    let mut five = start;
    let mut twenty = start;

    for turn in 1..=TURNS {
        running.send(&format!("turn {turn}: read big.rs and describe it\r"))?;
        running.until(&marker(turn))?;

        match turn {
            1 => one = Resource::read(running.id())?,
            5 => five = Resource::read(running.id())?,
            TURNS => twenty = Resource::read(running.id())?,
            _ => {}
        }
    }

    drop(running);
    vendor.finish()?;

    let turns = f64::from(u16::try_from(TURNS - 5).unwrap_or(u16::MAX));
    Ok(Measurement {
        peak: kibibytes(twenty.peak),
        start: kibibytes(start.rss),
        one: kibibytes(one.rss),
        five: kibibytes(five.rss),
        twenty: kibibytes(twenty.rss),
        pss: kibibytes(twenty.pss),
        slope: kibibytes(twenty.rss.saturating_sub(five.rss)) / turns,
        threads: twenty.threads,
        fds: twenty.fds,
        processes: twenty.processes,
    })
}

#[cfg(not(target_os = "linux"))]
fn measure() -> Result<Measurement, ProbeError> {
    Err(ProbeError::Unsupported)
}

#[cfg(target_os = "linux")]
fn kibibytes(value: u64) -> f64 {
    // Linux reports these values in KiB and realistic process measurements
    // remain far below the first integer a binary64 cannot represent exactly.
    #[allow(clippy::cast_precision_loss)]
    let value = value as f64;
    value / 1024.0
}

/// A private home, workspace and planted source file for the child.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Scratch {
    base: PathBuf,
    home: PathBuf,
    workspace: PathBuf,
}

#[cfg(target_os = "linux")]
impl Scratch {
    fn new(endpoint: &str) -> Result<Self, io::Error> {
        let base = std::env::temp_dir().join(format!("crucible-bench-rss-{}", std::process::id()));
        let home = base.join("home");
        let workspace = base.join("work");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&home)?;
        fs::create_dir_all(&workspace)?;

        let config = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
             \"provider\": \"anthropic\",\n  \"providers\": {{\n    \
             \"anthropic\": {{\"model\": \"bench\", \"baseUrl\": \"{endpoint}\"}}\n  \
             }},\n  \"output\": {{\"color\": \"never\", \"mouse\": \"off\"}}\n}}\n"
        );
        fs::write(home.join("config.json"), config)?;

        let mut planted = String::with_capacity(PLANTED * 48);
        for line in 0..PLANTED {
            let _ = writeln!(planted, "    let field_{line} = compute(&state, {line})?;");
        }
        fs::write(workspace.join("big.rs"), planted)?;

        Ok(Self {
            base,
            home,
            workspace,
        })
    }
}

#[cfg(target_os = "linux")]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// A deterministic Anthropic-compatible provider on the loopback interface.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Vendor {
    address: String,
    wake: SocketAddr,
    stopping: Arc<AtomicBool>,
    serving: Option<JoinHandle<Result<(), io::Error>>>,
}

#[cfg(target_os = "linux")]
impl Vendor {
    fn start() -> Result<Self, io::Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let wake = listener.local_addr()?;
        listener.set_nonblocking(true)?;
        let stopping = Arc::new(AtomicBool::new(false));
        let told = stopping.clone();
        let serving = std::thread::Builder::new()
            .name("crucible-rss-provider".into())
            .spawn(move || serve(&listener, &told))?;

        Ok(Self {
            address: format!("http://{wake}/v1/messages"),
            wake,
            stopping,
            serving: Some(serving),
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn finish(&mut self) -> Result<(), ProbeError> {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.wake);
        if let Some(serving) = self.serving.take() {
            serving.join().map_err(|_| ProbeError::ProviderPanicked)??;
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
impl Drop for Vendor {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

#[cfg(target_os = "linux")]
fn serve(listener: &TcpListener, stopping: &AtomicBool) -> Result<(), io::Error> {
    let mut requests = 0;
    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut connection, _)) => {
                connection.set_nonblocking(false)?;
                connection.set_read_timeout(Some(CEILING))?;
                request(&mut connection)?;
                requests += 1;
                let body = if requests % 2 == 1 {
                    tool().to_owned()
                } else {
                    answer(requests / 2)
                };
                respond(&mut connection, &body)?;
            }
            Err(problem) if problem.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(problem) => return Err(problem),
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn request(connection: &mut TcpStream) -> Result<(), io::Error> {
    let mut read = BufReader::new(connection);
    let mut line = String::new();
    let mut bytes = 0;
    let mut body = 0_u64;
    loop {
        line.clear();
        let got = read.read_line(&mut line)?;
        if got == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        bytes += got;
        if bytes > 64 * 1024 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "request headers exceeded 64 KiB",
            ));
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            body = value.trim().parse().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid content-length")
            })?;
        }
    }

    if body > 32 * 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "request body exceeded 32 MiB",
        ));
    }
    let copied = io::copy(&mut read.take(body), &mut io::sink())?;
    if copied != body {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "request body ended before content-length",
        ));
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn respond(connection: &mut TcpStream, body: &str) -> Result<(), io::Error> {
    write!(
        connection,
        "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len(),
    )?;
    connection.write_all(body.as_bytes())?;
    connection.flush()
}

#[cfg(target_os = "linux")]
fn tool() -> &'static str {
    concat!(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_tool\"}}\n\n",
        "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}\n\n",
        "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"big.rs\\\"}\"}}\n\n",
        "event: content_block_stop\ndata: {\"index\":0}\n\n",
        "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    )
}

#[cfg(target_os = "linux")]
fn answer(turn: usize) -> String {
    let mut body = String::with_capacity(DELTAS * 112);
    body.push_str(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_answer\"}}\n\n\
         event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    );
    for word in 0..DELTAS {
        let _ = write!(
            body,
            "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"field {word} \"}}}}\n\n"
        );
    }
    let _ = write!(
        body,
        "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{}\"}}}}\n\n",
        marker(turn),
    );
    body.push_str(
        "event: content_block_stop\ndata: {\"index\":0}\n\n\
         event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n\
         event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    body
}

#[cfg(target_os = "linux")]
fn marker(turn: usize) -> String {
    format!("turn-{turn}-complete")
}

/// The shipped child and its side of a real terminal.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Running {
    terminal: Option<File>,
    child: Child,
    bytes: Receiver<Vec<u8>>,
    reader: Option<JoinHandle<()>>,
}

#[cfg(target_os = "linux")]
impl Running {
    fn start(binary: &Path, scratch: &Scratch) -> Result<Self, io::Error> {
        let (terminal, inside) = pair()?;
        let reading = terminal.try_clone()?;
        let (sender, bytes) = mpsc::sync_channel(16);
        let reader = std::thread::Builder::new()
            .name("crucible-rss-terminal".into())
            .spawn(move || read(reading, &sender))?;
        let second = inside.try_clone()?;
        let third = inside.try_clone()?;

        let child = Command::new("setsid")
            .arg("--ctty")
            .arg(binary)
            .current_dir(&scratch.workspace)
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &scratch.home)
            .env("TERM", "xterm-256color")
            .env("NO_COLOR", "1")
            .env(crucible_config::HOME, &scratch.home)
            .env("ANTHROPIC_API_KEY", "bench-not-a-key")
            .stdin(Stdio::from(inside))
            .stdout(Stdio::from(second))
            .stderr(Stdio::from(third))
            .spawn()?;

        Ok(Self {
            terminal: Some(terminal),
            child,
            bytes,
            reader: Some(reader),
        })
    }

    fn id(&self) -> u32 {
        self.child.id()
    }

    fn send(&mut self, text: &str) -> Result<(), io::Error> {
        let terminal = self
            .terminal
            .as_mut()
            .ok_or_else(|| io::Error::new(io::ErrorKind::BrokenPipe, "terminal is closed"))?;
        terminal.write_all(text.as_bytes())?;
        terminal.flush()
    }

    fn until(&mut self, wanted: &str) -> Result<(), ProbeError> {
        let deadline = Instant::now() + CEILING;
        let mut seen = Vec::with_capacity(16 * 1024);
        let mut found = false;

        loop {
            let now = Instant::now();
            if now >= deadline {
                return Err(ProbeError::Timeout(wanted.into()));
            }
            let wait = if found {
                QUIET
            } else {
                deadline.saturating_duration_since(now)
            };

            match self.bytes.recv_timeout(wait) {
                Ok(bytes) => {
                    seen.extend_from_slice(&bytes);
                    found |= contains(&seen, wanted);
                    if seen.len() > 64 * 1024 {
                        let keep = wanted.len().saturating_sub(1).max(1024);
                        let from = seen.len().saturating_sub(keep);
                        seen.drain(..from);
                    }
                }
                Err(RecvTimeoutError::Timeout) if found => return Ok(()),
                Err(RecvTimeoutError::Timeout) => {
                    return Err(ProbeError::Timeout(wanted.into()));
                }
                Err(RecvTimeoutError::Disconnected) => {
                    return Err(ProbeError::Exited(wanted.into()));
                }
            }
        }
    }
}

#[cfg(target_os = "linux")]
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        drop(self.terminal.take());
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(target_os = "linux")]
fn pair() -> Result<(File, File), io::Error> {
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

#[cfg(target_os = "linux")]
fn read(mut terminal: File, sender: &SyncSender<Vec<u8>>) {
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

#[cfg(target_os = "linux")]
fn contains(bytes: &[u8], text: &str) -> bool {
    bytes
        .windows(text.len())
        .any(|window| window == text.as_bytes())
}

#[cfg(target_os = "linux")]
fn beside(name: &str) -> Result<PathBuf, io::Error> {
    let own = std::env::current_exe()?;
    let directory = own
        .parent()
        .ok_or_else(|| io::Error::other("benchmark executable has no parent directory"))?;
    let candidate = directory.join(name);
    candidate
        .is_file()
        .then_some(candidate.clone())
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, candidate.display().to_string()))
}

/// One simultaneous process-tree sample, plus every root high-water mark.
#[cfg(target_os = "linux")]
#[derive(Debug, Default, Clone, Copy)]
struct Resource {
    rss: u64,
    pss: u64,
    peak: u64,
    threads: usize,
    fds: usize,
    processes: usize,
}

#[cfg(target_os = "linux")]
impl Resource {
    fn read(root: u32) -> Result<Self, ProbeError> {
        let mut total = Self::default();
        let mut pending = vec![root];
        let mut at = 0;

        while let Some(pid) = pending.get(at).copied() {
            at += 1;
            if pending.len() > PROCESSES {
                return Err(ProbeError::ProcessTree);
            }

            let rollup = fs::read_to_string(format!("/proc/{pid}/smaps_rollup"))?;
            let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
            total.rss +=
                field(&rollup, "Rss:").ok_or(ProbeError::Unmeasurable { pid, field: "Rss" })?;
            total.pss +=
                field(&rollup, "Pss:").ok_or(ProbeError::Unmeasurable { pid, field: "Pss" })?;
            total.peak += field(&status, "VmHWM:").ok_or(ProbeError::Unmeasurable {
                pid,
                field: "VmHWM",
            })?;
            total.threads += field(&status, "Threads:")
                .and_then(|value| usize::try_from(value).ok())
                .ok_or(ProbeError::Unmeasurable {
                    pid,
                    field: "Threads",
                })?;
            total.fds += fs::read_dir(format!("/proc/{pid}/fd"))?.count();
            total.processes += 1;

            let children = fs::read_to_string(format!("/proc/{pid}/task/{pid}/children"))?;
            pending.extend(
                children
                    .split_whitespace()
                    .filter_map(|child| child.parse::<u32>().ok()),
            );
        }

        Ok(total)
    }
}

#[cfg(target_os = "linux")]
fn field(text: &str, named: &str) -> Option<u64> {
    text.lines().find_map(|line| {
        line.strip_prefix(named)?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn report(measured: Measurement) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = write!(
        line,
        "{:.1} MB {LIMIT:.0} start={:.1} turn1={:.1} turn5={:.1} turn20={:.1} \
         pss20={:.1} slope={:.3} threads={} fds={} processes={}",
        measured.peak,
        measured.start,
        measured.one,
        measured.five,
        measured.twenty,
        measured.pss,
        measured.slope,
        measured.threads,
        measured.fds,
        measured.processes,
    );
    line.push('\n');
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

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

    if report(measured).is_err() || measured.peak > LIMIT {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
