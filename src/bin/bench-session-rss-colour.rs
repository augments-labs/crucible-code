//! Peak resident memory of the shipped process after a twenty-turn session,
//! with colour on.
//!
//! The same measurement as `bench-session-rss`, by the same session and the
//! same three phases, with the one difference a user can choose: no
//! `NO_COLOR`, so a terminal is written in colour. A palette is bytes and work
//! the no-colour probe never does, and a memory budget that left it out would
//! bound a session nobody sees.
//!
//! The measured process is `target/release/crucible`, not a runner assembled
//! inside this probe. It starts behind a controlling pseudo terminal, reads its
//! normal configuration, draws through its normal terminal, writes normal
//! session logs and sends HTTP through its normal provider adapter. The server
//! is a deterministic loopback fixture in this parent process; no request can
//! leave the machine and no production-only seam is involved.
//!
//! Each turn asks the real read tool to read a planted file and the real edit
//! tool to rewrite the twenty lines of a second one, all but its first and its
//! last, then receives a streamed answer that ends in a table, the two lines
//! that stayed in a fence of Rust, and the diff of that rewrite in a fence. A
//! table is kept as the source it was written as beside the rows it was laid
//! out as, and so is a diff, which is drawn as a change; the edit's own change
//! is drawn too, and being folded is kept written out for the key that shows it
//! whole.
//! So a session of them holds more than a session of prose the same length, and
//! the fill below rolls every one of them out past the ceiling with its source.
//! The measurement is refused if the second file does not end as the last edit
//! leaves it, since a session whose edits were turned away drew no change of a
//! tool's.
//!
//! What reading code costs is in this reading. A line of a change that did not
//! move is drawn in the colours of its file's language, and so is a fence that
//! names one, and the first such line of a session loads the syntax
//! definitions, which are megabytes kept for the rest of it. Every edit here
//! leaves two lines where they were, the diff in the answer says the same, and
//! the fence beside it names Rust, so the first turn loads them and every
//! figure after it holds them.
//!
//! RSS and proportional set size come from `smaps_rollup` at startup and after
//! turns one, five and twenty; `VmHWM` supplies the child's peak, including
//! spikes between samples. Thread, descriptor and process-tree counts are
//! emitted beside the budget so a flat total cannot hide a growing resource of
//! another kind.
//!
//! Twenty turns is the budget's session and the reading it reports. It is not
//! the longest session anybody has, and this process now keeps the transcript
//! itself — so a second phase follows the sampled twenty: turns whose every
//! delta ends a line, enough of them to push the record past the ceiling it
//! keeps and out the other side. That ceiling is what the phase measures, and a
//! record that grew with the session would show here and nowhere else, because
//! twenty turns of prose never reach it. Its figures are reported beside the
//! budget's and held to the same limit.
//!
//! A third session carries pictures. A transcript keeps a reference to every
//! file a prompt named and the bytes live for exactly one request, so what a
//! picture costs is a spike inside a request rather than a session that grows
//! -- and a spike is the one thing a sampled reading can miss and a high-water
//! mark cannot. Every prompt of that session names more megabytes than one
//! request may carry, so every request in it is at the ceiling. Its own figure
//! is reported beside the other two and held to the same limit, and the fixture
//! refuses the measurement if no request ever arrived carrying the bytes.

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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
///
/// Seven above the no-colour probe's, for what reading code in colour costs.
/// Each language a session reads compiles its patterns and keeps them, about
/// 5 MB for the one this session reads, and no way of loading the definitions
/// removes that: a set read whole and a set read a language at a time come to
/// the same patterns compiled.
const LIMIT: f64 = 42.0;

/// How much resident memory may grow across one more full lifecycle after the
/// retained record has already rolled past its ceiling.
const RETAINED_LIMIT: f64 = 4.0;

/// Turns in the shipped session being measured.
#[cfg(target_os = "linux")]
const TURNS: usize = 20;

/// Lines the real read tool sees in every turn.
#[cfg(target_os = "linux")]
const PLANTED: usize = 2_000;

/// Text deltas in every completed answer of a measured turn.
#[cfg(target_os = "linux")]
const DELTAS: usize = 400;

/// Rows of the table that ends every measured answer, a row a delta.
#[cfg(target_os = "linux")]
const TABLE_ROWS: usize = 24;

/// Lines of the file every measured turn's edit rewrites, and its answer then
/// writes the diff of: with as many taken out as put in, more rows than the box
/// of a change shows before it folds.
#[cfg(target_os = "linux")]
const EDITED: usize = 20;

/// Requests in a measured turn: the read, the edit, and the answer.
#[cfg(target_os = "linux")]
const REQUESTS: usize = 3;

/// Turns after the sampled twenty, each of them all line endings.
#[cfg(target_os = "linux")]
const FILL: usize = 10;

/// Lines in each of those turns. Ten times this is more than the record holds,
/// so the last fill turn is drawn over lines the first one has already had
/// dropped, which is the state the budget has never otherwise been read in.
#[cfg(target_os = "linux")]
const LINES: usize = 2_500;

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

/// The model the sampled session asks for.
///
/// Deliberately a name no table entry answers. The fixture explicitly gives it
/// a large operational window below so provider defaults cannot turn the stable
/// twenty-turn memory workload into a compaction workload.
#[cfg(target_os = "linux")]
const SAMPLED: &str = "bench";

/// The model the picture session asks for.
///
/// A name this build does have an entry for, because what may be attached is
/// what the model accepts times what the provider can spell, and a name the
/// table has never heard of attaches nothing at all. The largest window any
/// entry carries, so twenty turns of tool results stay far short of it and
/// compaction -- which would replace the transcript the pictures hang off --
/// cannot fire inside a measurement.
#[cfg(target_os = "linux")]
const LOOKING: &str = "claude-opus-5";

/// Pictures every prompt of that session names.
///
/// More megabytes than one request may carry, so the oldest of them are not:
/// every request in the session is *at* the ceiling rather than somewhere below
/// it, and the reading is of the ceiling rather than of however many pictures
/// happened to be lying around. Stated as a count of megabyte files rather than
/// against the ceiling's own figure, so lowering that figure leaves this true.
#[cfg(target_os = "linux")]
const PICTURES: usize = 8;

/// How large each of them is.
#[cfg(target_os = "linux")]
const PICTURE: usize = 1024 * 1024;

/// What a PNG starts with.
///
/// The shipped process confirms the extension against the bytes before it
/// attaches anything, and this is the whole of what that check reads. Nothing
/// downstream decodes the rest: no vendor is on the other side of this wire.
#[cfg(target_os = "linux")]
const SIGNATURE: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// What can go wrong in the probe itself.
#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("bench-session-rss-colour: {0}")]
    Io(#[from] io::Error),

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss-colour: crucible did not reach {0:?} within ten seconds")]
    Timeout(Box<str>),

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss-colour: crucible exited before reaching {0:?}")]
    Exited(Box<str>),

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss-colour: no {field} reading for process {pid}")]
    Unmeasurable { pid: u32, field: &'static str },

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss-colour: process tree exceeded {PROCESSES} processes")]
    ProcessTree,

    #[cfg(target_os = "linux")]
    #[error("bench-session-rss-colour: local provider thread panicked")]
    ProviderPanicked,

    #[cfg(target_os = "linux")]
    #[error(
        "bench-session-rss-colour: the largest request carried {largest} bytes, which is under the \
         attachment ceiling -- the pictures were not attached and the reading measures nothing"
    )]
    Unattached { largest: u64 },

    #[cfg(target_os = "linux")]
    #[error(
        "bench-session-rss-colour: the file the session's edits rewrite did not end as the \
         last of them leaves it -- the changes were not made and the reading measures a \
         session that drew none"
    )]
    Unedited,

    #[cfg(not(target_os = "linux"))]
    #[error("bench-session-rss-colour: shipped-process RSS measurement requires Linux")]
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
    rolled: f64,
    filled: f64,
    crest: f64,
    pictures: f64,
    pss: f64,
    slope: f64,
    threads: usize,
    fds: usize,
    processes: usize,
}

#[cfg(target_os = "linux")]
fn measure() -> Result<Measurement, ProbeError> {
    let mut vendor = Vendor::start()?;
    let scratch = Scratch::new(vendor.address(), SAMPLED)?;
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

    let mut rolled = Resource::read(running.id())?;
    for turn in 1..=FILL {
        running.send(&format!("fill {turn}: say it again at length\r"))?;
        running.until(&marker(TURNS + turn))?;
        if turn == FILL - 1 {
            rolled = Resource::read(running.id())?;
        }
    }
    let filled = Resource::read(running.id())?;

    drop(running);
    vendor.finish()?;

    // Asked of the file rather than of the screen: an edit the session turned
    // away leaves a refusal where the change would be drawn, and a session of
    // refusals would be read as one that holds twenty changes for nothing.
    let left = fs::read_to_string(scratch.workspace.join(REWRITTEN))?;
    if left.strip_suffix('\n') != Some(left_by(TURNS).as_str()) {
        return Err(ProbeError::Unedited);
    }

    let pictures = carrying()?;

    let turns = f64::from(u16::try_from(TURNS - 5).unwrap_or(u16::MAX));
    Ok(Measurement {
        peak: kibibytes(twenty.peak),
        start: kibibytes(start.rss),
        one: kibibytes(one.rss),
        five: kibibytes(five.rss),
        twenty: kibibytes(twenty.rss),
        rolled: kibibytes(rolled.rss),
        filled: kibibytes(filled.rss),
        crest: kibibytes(filled.peak),
        pictures,
        pss: kibibytes(twenty.pss),
        slope: kibibytes(twenty.rss.saturating_sub(five.rss)) / turns,
        threads: twenty.threads,
        fds: twenty.fds,
        processes: twenty.processes,
    })
}

/// The high-water mark of a session whose every request carried the ceiling.
///
/// Its own child, home and fixture rather than more turns of the one above: a
/// prompt attaches nothing until the model is one this build knows the limits
/// of, and the sampled session is run against a name on purpose unknown to the
/// table. Twenty turns of the same shape as those, with the pictures named in
/// every prompt alongside the file the read tool is pointed at, so this is a
/// working session that also carries pictures rather than a session that only
/// does.
///
/// What comes back is the peak, because that is where the bytes are. They are
/// read, encoded, sent and dropped inside one request, and a reading taken
/// between turns would find a transcript holding references and report that the
/// pictures were free.
#[cfg(target_os = "linux")]
fn carrying() -> Result<f64, ProbeError> {
    let mut vendor = Vendor::start()?;
    let scratch = Scratch::new(vendor.address(), LOOKING)?;
    let named = scratch.pictures()?;
    let binary = beside("crucible")?;
    let mut running = Running::start(&binary, &scratch)?;

    running.until("ask mode on")?;
    for turn in 1..=TURNS {
        running.send(&format!("turn {turn}: read big.rs and describe {named}\r"))?;
        running.until(&marker(turn))?;
    }
    let carried = Resource::read(running.id())?;

    drop(running);
    let largest = vendor.largest();
    vendor.finish()?;

    // Asked of the fixture rather than of the screen, because the bytes are
    // what is being measured and the wire is where they can be counted. A
    // refusal at the prompt draws a sentence and sends a request the ordinary
    // size, which would read as a session that cost nothing to carry pictures
    // -- the one wrong answer this probe could give without anybody noticing.
    if largest < crucible_attachments::CEILING as u64 {
        return Err(ProbeError::Unattached { largest });
    }

    Ok(kibibytes(carried.peak))
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
    /// A rule lets the session's edits through, as a user who had been asked
    /// once would have it: a question would stop the turn at a prompt nobody
    /// is there to answer.
    fn new(endpoint: &str, model: &str) -> Result<Self, io::Error> {
        let base = std::env::temp_dir().join(format!("crucible-bench-rss-{}", std::process::id()));
        let home = base.join("home");
        let workspace = base.join("work");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&home)?;
        fs::create_dir_all(&workspace)?;

        let config = format!(
            "{{\n  \"updates\": {{\"check\": \"never\"}},\n  \
             \"provider\": \"anthropic\",\n  \"providers\": {{\n    \
             \"anthropic\": {{\"model\": \"{model}\", \"baseUrl\": \"{endpoint}\", \
             \"defaultContextWindow\": 1000000}}\n  \
             }},\n  \"output\": {{\"color\": \"auto\"}},\n  \
             \"permissions\": {{\"allow\": [\"edit(*)\"]}}\n}}\n"
        );
        fs::write(home.join("config.json"), config)?;

        fs::write(workspace.join("big.rs"), planted())?;
        fs::write(workspace.join(REWRITTEN), format!("{}\n", left_by(0)))?;

        Ok(Self {
            base,
            home,
            workspace,
        })
    }

    /// Plants the pictures, and returns the words a prompt names them by.
    ///
    /// Real files, because the only door an attachment comes through is a path
    /// somebody typed: the shipped process resolves the word against the
    /// workspace, confirms the signature against the extension, reads the bytes
    /// and hashes them. A buffer synthesized in this probe would reach none of
    /// that, and none of that is free.
    fn pictures(&self) -> Result<String, io::Error> {
        let mut bytes = vec![0_u8; PICTURE];
        bytes
            .get_mut(..SIGNATURE.len())
            .ok_or_else(|| io::Error::other("a picture smaller than a PNG signature"))?
            .copy_from_slice(SIGNATURE);

        let mut named = String::with_capacity(PICTURES.saturating_mul(16));
        for picture in 1..=PICTURES {
            let name = format!("shot-{picture}.png");
            fs::write(self.workspace.join(&name), &bytes)?;
            if !named.is_empty() {
                named.push(' ');
            }
            named.push_str(&name);
        }

        Ok(named)
    }
}

#[cfg(target_os = "linux")]
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.base);
    }
}

/// Line `at` of a planted file, calling `call`.
#[cfg(target_os = "linux")]
fn row(at: usize, call: &str) -> String {
    format!("    let field_{at} = {call}(&state, {at})?;")
}

/// The planted file as a session finds it.
#[cfg(target_os = "linux")]
fn planted() -> String {
    let mut planted = String::with_capacity(PLANTED * 48);
    for line in 0..PLANTED {
        planted.push_str(&row(line, "compute"));
        planted.push('\n');
    }
    planted
}

/// A deterministic Anthropic-compatible provider on the loopback interface.
#[cfg(target_os = "linux")]
#[derive(Debug)]
struct Vendor {
    address: String,
    wake: SocketAddr,
    stopping: Arc<AtomicBool>,
    /// The largest request body this fixture was sent, in bytes.
    largest: Arc<AtomicU64>,
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
        let largest = Arc::new(AtomicU64::new(0));
        let counted = largest.clone();
        let serving = std::thread::Builder::new()
            .name("crucible-rss-provider".into())
            .spawn(move || serve(&listener, &told, &counted))?;

        Ok(Self {
            address: format!("http://{wake}/v1/messages"),
            wake,
            stopping,
            largest,
            serving: Some(serving),
        })
    }

    fn address(&self) -> &str {
        &self.address
    }

    /// The largest request body this fixture was sent, in bytes.
    fn largest(&self) -> u64 {
        self.largest.load(Ordering::Acquire)
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
fn serve(
    listener: &TcpListener,
    stopping: &AtomicBool,
    largest: &AtomicU64,
) -> Result<(), io::Error> {
    let mut requests = 0;
    while !stopping.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((mut connection, _)) => {
                connection.set_nonblocking(false)?;
                connection.set_read_timeout(Some(CEILING))?;
                largest.fetch_max(request(&mut connection)?, Ordering::AcqRel);
                requests += 1;
                respond(&mut connection, &replied(requests))?;
            }
            Err(problem) if problem.kind() == io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(1));
            }
            Err(problem) => return Err(problem),
        }
    }
    Ok(())
}

/// What the fixture answers the `request`th request of a session with.
///
/// A measured turn is three requests -- the call that reads the file, the
/// call that changes it, then the answer that follows their results. A fill
/// turn is one, because the planted file has been read and changed twenty
/// times already and doing so ten more would measure the tools rather than
/// the record.
#[cfg(target_os = "linux")]
fn replied(request: usize) -> String {
    let measured = TURNS * REQUESTS;
    if request > measured {
        return answer(TURNS + request - measured, LINES, true);
    }
    let turn = request.div_ceil(REQUESTS);
    match request % REQUESTS {
        1 => tool().to_owned(),
        2 => edit(turn),
        _ => answer(turn, DELTAS, false),
    }
}

#[cfg(target_os = "linux")]
fn request(connection: &mut TcpStream) -> Result<u64, io::Error> {
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
    Ok(body)
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

/// The file every measured turn's edit rewrites.
#[cfg(target_os = "linux")]
const REWRITTEN: &str = "rewritten.rs";

/// Whether line `at` of that file is one no edit moves: its first and its
/// last, which every change of it keeps to read the others against.
#[cfg(target_os = "linux")]
const fn stays(at: usize) -> bool {
    at == 0 || at + 1 == EDITED
}

/// The file an edit rewrites as `turn` leaves it, its last line not ended:
/// as it is planted, for turn zero. Every line but the two that stay names the
/// turn, so none of those is the same after another turn.
#[cfg(target_os = "linux")]
fn left_by(turn: usize) -> String {
    (0..EDITED)
        .map(|at| {
            let turn = if stays(at) { 0 } else { turn };
            row(at, &format!("settled_{turn}"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `text` as the inside of a JSON string.
#[cfg(target_os = "linux")]
fn quoted(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

/// The call that rewrites [`REWRITTEN`] from what the turn before left to
/// what `turn` leaves.
///
/// The edit tool is given the text to find and what to put there, so the call
/// quotes every line of the file whole, the two that stay among them.
#[cfg(target_os = "linux")]
fn edit(turn: usize) -> String {
    let input = format!(
        "{{\"path\":\"{REWRITTEN}\",\"find\":\"{}\",\"replace\":\"{}\"}}",
        quoted(&left_by(turn.saturating_sub(1))),
        quoted(&left_by(turn)),
    );
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_edit\"}}}}\n\n\
         event: content_block_start\ndata: {{\"index\":0,\"content_block\":{{\"type\":\"tool_use\",\"id\":\"toolu_2\",\"name\":\"edit\",\"input\":{{}}}}}}\n\n\
         event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}}}\n\n\
         event: content_block_stop\ndata: {{\"index\":0}}\n\n\
         event: message_delta\ndata: {{\"delta\":{{\"stop_reason\":\"tool_use\"}}}}\n\n\
         event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
        quoted(&input),
    )
}

/// One completed answer: `deltas` pieces of text and the marker that ends them.
///
/// `lines` is what separates the two phases. A measured turn streams prose that
/// wraps, and prose that wraps is one line of the record however many rows it
/// takes; a fill turn ends every delta, so every delta is a line. Lines are what
/// the record bounds, so filling it is a question of how many endings arrive
/// rather than of how much text does. A measured turn's prose is followed by a
/// table, streamed a row at a time, which the blank line after it closes, then
/// by the lines the turn's edit left where they were in a fence that names
/// Rust, and then by the change that edit made, written as the unified diff of
/// it in a fence with those lines as the rows it keeps. Both fences are
/// streamed a line at a time and closed by their last line.
#[cfg(target_os = "linux")]
fn answer(turn: usize, deltas: usize, lines: bool) -> String {
    // Two bytes into the JSON, where they are the escape a line ending is
    // written as -- not a line ending in this file, which would end the data
    // line and truncate the event.
    let ending = if lines { "\\n" } else { "" };
    let mut body = String::with_capacity(
        deltas
            .saturating_add(TABLE_ROWS + 2 + EDITED * 2 + 4)
            .saturating_mul(112)
            .saturating_add(512),
    );
    body.push_str(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_answer\"}}\n\n\
         event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
    );
    for word in 0..deltas {
        let _ = write!(
            body,
            "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"field {word} {ending}\"}}}}\n\n"
        );
    }
    if !lines {
        let _ = write!(
            body,
            "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"\\n\\n| turn | field | read |\\n| --- | --- | ---: |\\n\"}}}}\n\n"
        );
        for row in 0..TABLE_ROWS {
            let _ = write!(
                body,
                "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"| {turn} | field {row} | {row} |\\n\"}}}}\n\n"
            );
        }
        body.push_str(
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"\\n\"}}\n\n",
        );

        let (was, now) = (left_by(turn.saturating_sub(1)), left_by(turn));
        let moved = |at: &usize| !stays(*at);
        let kept = |at: usize| now.lines().nth(at).map(|line| format!("{line}\n"));

        let mut fences = vec!["```rust\n".to_owned()];
        fences.extend((0..EDITED).filter(|at| stays(*at)).filter_map(kept));
        fences.push("```\n\n".to_owned());

        fences.push(format!(
            "```diff\n--- a/{REWRITTEN}\n+++ b/{REWRITTEN}\n@@ -1,{EDITED} +1,{EDITED} @@\n"
        ));
        fences.extend(kept(0).map(|line| format!(" {line}")));
        let taken = was.lines().enumerate().filter(|(at, _)| moved(at));
        fences.extend(taken.map(|(_, line)| format!("-{line}\n")));
        let put = now.lines().enumerate().filter(|(at, _)| moved(at));
        fences.extend(put.map(|(_, line)| format!("+{line}\n")));
        fences.extend(kept(EDITED - 1).map(|line| format!(" {line}")));
        fences.push("```\n\n".to_owned());
        for line in fences {
            let _ = write!(
                body,
                "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"{}\"}}}}\n\n",
                quoted(&line),
            );
        }
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
         rolled={:.1} filled={:.1} crest={:.1} pictures={:.1} pss20={:.1} slope={:.3} \
         retained={:.1} threads={} fds={} processes={}",
        measured.peak,
        measured.start,
        measured.one,
        measured.five,
        measured.twenty,
        measured.rolled,
        measured.filled,
        measured.crest,
        measured.pictures,
        measured.pss,
        measured.slope,
        (measured.filled - measured.rolled).max(0.0),
        measured.threads,
        measured.fds,
        measured.processes,
    );
    line.push('\n');
    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

/// Says which of the three readings went over, on stderr, where
/// `scripts/sh/bench.sh` puts everything a human reads.
///
/// The one line on stdout carries the twenty-turn figure whichever reading
/// failed, because that is the budget as it is written and the script reads it
/// positionally. Without this line a full record that blew the limit would
/// reach the operator as twenty ordinary turns having done it.
fn over(measured: Measurement) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(
        line,
        "    FAIL peak {:.1} MB over twenty turns, {:.1} MB once the record was \
         full, {:.1} MB carrying pictures at the ceiling (limit {LIMIT:.0} MB); \
         one more full lifecycle retained {:.1} MB after the record was already rolling \
         (limit {RETAINED_LIMIT:.0} MB)",
        measured.peak,
        measured.crest,
        measured.pictures,
        (measured.filled - measured.rolled).max(0.0),
    );
    io::stderr().write_all(line.as_bytes())
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

    // Three absolute readings share one limit. The first is the budget as it
    // is written; the second says the record ceiling holds it after rollover;
    // the third says the attachment ceiling holds the bytes. The within-process
    // delta adds a different claim: once rollover is already happening, one
    // more lifecycle does not retain another transcript-sized allocation.
    if report(measured).is_err() {
        return ExitCode::FAILURE;
    }
    let retained = (measured.filled - measured.rolled).max(0.0);
    if measured.peak > LIMIT
        || measured.crest > LIMIT
        || measured.pictures > LIMIT
        || retained > RETAINED_LIMIT
    {
        let _ = over(measured);
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
#[cfg(target_os = "linux")]
mod tests {
    use super::*;

    /// What a response the fixture wrote says: the tool it calls, the input it
    /// calls it with, and the text its deltas add up to.
    struct Said {
        tool: Option<String>,
        input: String,
        text: String,
    }

    /// The text at `pointer` in `value`.
    fn at<'a>(value: &'a serde_json::Value, pointer: &str) -> Option<&'a str> {
        value.pointer(pointer).and_then(serde_json::Value::as_str)
    }

    fn said(body: &str) -> Said {
        let mut said = Said {
            tool: None,
            input: String::new(),
            text: String::new(),
        };
        for data in body.lines().filter_map(|line| line.strip_prefix("data: ")) {
            let event: serde_json::Value = serde_json::from_str(data).expect("an event in JSON");
            if let Some(name) = at(&event, "/content_block/name") {
                said.tool = Some(name.to_owned());
            }
            if let Some(part) = at(&event, "/delta/partial_json") {
                said.input.push_str(part);
            }
            if let Some(text) = at(&event, "/delta/text") {
                said.text.push_str(text);
            }
        }
        said
    }

    #[test]
    fn a_measured_turn_is_a_read_an_edit_and_an_answer_and_a_fill_turn_an_answer() {
        for turn in 1..=TURNS {
            let first = (turn - 1) * REQUESTS + 1;
            assert_eq!(
                said(&replied(first)).tool.as_deref(),
                Some("read"),
                "turn {turn}"
            );
            assert_eq!(
                said(&replied(first + 1)).tool.as_deref(),
                Some("edit"),
                "turn {turn}"
            );
            let answer = said(&replied(first + 2));
            assert_eq!(answer.tool, None, "turn {turn}");
            assert!(answer.text.ends_with(&marker(turn)), "turn {turn}");
        }

        let measured = TURNS * REQUESTS;
        for fill in 1..=FILL {
            let answer = said(&replied(measured + fill));
            assert_eq!(answer.tool, None, "fill {fill}");
            assert!(answer.text.ends_with(&marker(TURNS + fill)), "fill {fill}");
        }
    }

    #[test]
    fn every_edit_rewrites_the_whole_of_its_file_and_leaves_its_first_and_last_line_as_they_were() {
        let mut file = format!("{}\n", left_by(0));
        for turn in 1..=TURNS {
            let call = said(&edit(turn));
            assert_eq!(call.tool.as_deref(), Some("edit"), "turn {turn}");
            let input: serde_json::Value =
                serde_json::from_str(&call.input).expect("the input in JSON");
            assert_eq!(at(&input, "/path"), Some(REWRITTEN), "turn {turn}");
            let find = at(&input, "/find").expect("the text to find");
            let replace = at(&input, "/replace").expect("the text to put");

            // The tool refuses text it finds twice or not at all.
            assert_eq!(file.matches(find).count(), 1, "turn {turn}");
            // Every line of the file is quoted, and all but the first and the
            // last come back otherwise: those two are kept in the change to
            // read the others against, and are read for their language's
            // colours.
            assert_eq!(format!("{find}\n"), file, "turn {turn}");
            assert_eq!(find.lines().count(), EDITED, "turn {turn}");
            assert_eq!(replace.lines().count(), EDITED, "turn {turn}");
            let stayed: Vec<usize> = find
                .lines()
                .zip(replace.lines())
                .enumerate()
                .filter_map(|(at, (was, now))| (was == now).then_some(at))
                .collect();
            assert_eq!(stayed, [0, EDITED - 1], "turn {turn}");
            file = file.replacen(find, replace, 1);
        }

        assert_eq!(file, format!("{}\n", left_by(TURNS)));
        // The file the read tool reads is another, and no edit touches it.
        assert_ne!(REWRITTEN, "big.rs");
    }

    #[test]
    fn every_measured_turn_keeps_a_line_of_its_file_and_answers_with_it_kept_and_with_rust() {
        // What makes a session read code for a language's colours, asked of
        // every measured turn as the fixture serves it. Each one missing is
        // named rather than the first, so a fixture that loses one says which.
        let mut file = format!("{}\n", left_by(0));
        let mut missing = Vec::new();
        for turn in 1..=TURNS {
            let first = (turn - 1) * REQUESTS + 1;

            let call = said(&replied(first + 1));
            let input: serde_json::Value =
                serde_json::from_str(&call.input).expect("the input in JSON");
            let find = at(&input, "/find").expect("the text to find");
            let replace = at(&input, "/replace").expect("the text to put");
            let after = file.replacen(find, replace, 1);
            assert_ne!(after, file, "turn {turn}: the edit changes nothing");

            // A line the edit leaves where and as it was is a line its change
            // keeps, and a kept line is drawn in the colours of its language.
            let stayed: Vec<&str> = file
                .lines()
                .zip(after.lines())
                .filter_map(|(was, now)| (was == now).then_some(was))
                .collect();
            if stayed.is_empty() {
                missing.push(format!(
                    "turn {turn}: the edit leaves no line of {REWRITTEN} as it was"
                ));
            }

            // The diff the answer writes says the same: a row neither taken
            // out nor put in, which is one of the lines that stayed.
            let text = said(&replied(first + 2)).text;
            let kept = text
                .split_once("\n```diff\n")
                .and_then(|(_, fence)| fence.split_once("```\n"))
                .is_some_and(|(diff, _)| {
                    diff.lines()
                        .filter_map(|row| row.strip_prefix(' '))
                        .any(|row| stayed.contains(&row))
                });
            if !kept {
                missing.push(format!(
                    "turn {turn}: the answer's diff fence holds no row of a line that stayed"
                ));
            }

            // And code the answer shows for itself, in a fence that names the
            // language by the word a model writes.
            let code = text
                .split_once("\n```rust\n")
                .and_then(|(_, fence)| fence.split_once("```\n"))
                .is_some_and(|(code, _)| !code.trim().is_empty());
            if !code {
                missing.push(format!(
                    "turn {turn}: the answer holds no fence that names rust"
                ));
            }

            file = after;
        }

        assert!(missing.is_empty(), "{missing:#?}");
    }

    #[test]
    fn a_measured_answer_ends_in_the_diff_of_its_turn_and_a_fill_answer_holds_none() {
        let text = said(&answer(3, DELTAS, false)).text;
        let (_, fence) = text.split_once("\n```diff\n").expect("a diff fence");
        let (diff, after) = fence.split_once("```\n").expect("the fence closed");
        assert_eq!(after, format!("\n{}", marker(3)));

        // A unified diff of what the third turn's edit does: the file it is
        // made to, and a hunk whose counts are the lines under it, the first
        // line and the last kept around every line the second turn left taken
        // out and every line the third leaves put.
        let mut lines = diff.lines();
        assert_eq!(lines.next(), Some("--- a/rewritten.rs"));
        assert_eq!(lines.next(), Some("+++ b/rewritten.rs"));
        assert_eq!(lines.next(), Some("@@ -1,20 +1,20 @@"));
        let lines: Vec<&str> = lines.collect();
        let [first, moved @ .., last] = lines.as_slice() else {
            panic!("a hunk of two rows or more: {lines:?}");
        };
        assert_eq!(*first, "     let field_0 = settled_0(&state, 0)?;");
        assert_eq!(*last, "     let field_19 = settled_0(&state, 19)?;");
        let (out, put) = moved.split_at(EDITED - 2);
        assert_eq!(
            out.first().copied(),
            Some("-    let field_1 = settled_2(&state, 1)?;")
        );
        assert_eq!(
            out.last().copied(),
            Some("-    let field_18 = settled_2(&state, 18)?;")
        );
        assert!(out.iter().all(|line| line.starts_with('-')), "{out:?}");
        assert_eq!(put.len(), EDITED - 2);
        assert_eq!(
            put.first().copied(),
            Some("+    let field_1 = settled_3(&state, 1)?;")
        );
        assert_eq!(
            put.last().copied(),
            Some("+    let field_18 = settled_3(&state, 18)?;")
        );
        assert!(put.iter().all(|line| line.starts_with('+')), "{put:?}");

        // The table is still there, before the two lines that stayed, in a
        // fence of their own that names their language.
        assert!(
            text.contains(concat!(
                "| 3 | field 23 | 23 |\n\n",
                "```rust\n",
                "    let field_0 = settled_0(&state, 0)?;\n",
                "    let field_19 = settled_0(&state, 19)?;\n",
                "```\n\n",
                "```diff\n",
            )),
            "{text}"
        );

        let fill = said(&answer(TURNS + 1, 8, true)).text;
        assert!(!fill.contains("```"), "{fill}");
    }
}
