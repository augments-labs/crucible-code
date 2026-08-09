//! Peak resident memory after a twenty-turn session.
//!
//! Thirty-five megabytes is the budget. What it really guards is shape rather
//! than size: a harness that keeps the whole transcript on screen, or holds
//! every tool result to re-render it, grows with the session rather than with
//! the turn. The transcript itself is held whole and copied into each request,
//! and that copy is what this number is set to cover; what must not appear
//! beside it is a second thing that grows the same way. Twenty turns is enough
//! for that difference to show and short enough to run in a gate.
//!
//! Everything here is the real thing except the socket. The provider is the
//! real Anthropic adapter parsing real server-sent events; the tool is the real
//! `read` against a real file; the session log is written to a real disk by its
//! real thread; the renderer draws real frames. Only the bytes come from memory
//! instead of a network, because a benchmark must not need one.
//!
//! Peak, not current: `VmHWM` is the high-water mark, so a spike that is freed
//! before the probe looks still counts. Reading it needs `/proc`, so this is a
//! Linux measurement and says so rather than guessing elsewhere.

use std::cell::RefCell;
use std::fmt::Write as _;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Cursor, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use crucible_core::{
    ApiKey, Ask, Cancel, Event, Header, HeaderKey, Post, Remember, Sensitivity, ToolCall, Verdict,
    Workspace,
};
use crucible_provider::{Anthropic, Response, Transport, TransportError};
use crucible_runner::{Model, Runner, Session, Tools};
use crucible_tools::Read;
use crucible_tui::{Renderer, Size, Terminal, TerminalError};

/// The budget, in megabytes.
const LIMIT: f64 = 35.0;

/// Turns in the session being measured.
const TURNS: usize = 20;

/// Lines in the file the model reads every turn. Large enough that the tool
/// answer is a real one and not a rounding error.
const PLANTED: usize = 2_000;

/// Text deltas in each answer, which is roughly what a few hundred words of
/// streamed prose arrives as.
const DELTAS: usize = 400;

/// A terminal-sized window, so wrapping and the bounded tail both engage.
const COLUMNS: usize = 80;
const ROWS: usize = 24;

/// What can go wrong in the probe itself.
#[derive(Debug, thiserror::Error)]
enum ProbeError {
    #[error("bench-session-rss: {0}")]
    Io(#[from] io::Error),

    #[error("bench-session-rss: {0}")]
    Session(#[from] crucible_runner::SessionError),

    #[error("bench-session-rss: {0}")]
    Workspace(#[from] crucible_core::PathError),

    #[error("bench-session-rss: {0}")]
    Terminal(#[from] TerminalError),

    /// No high-water mark to read.
    #[error("bench-session-rss: no VmHWM in /proc/self/status: peak RSS needs Linux")]
    Unmeasurable,
}

/// A scratch workspace and somewhere to put its sessions.
#[derive(Debug)]
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self, ProbeError> {
        let base = std::env::temp_dir().join(format!("crucible-bench-rss-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("work"))?;
        fs::create_dir_all(base.join("logs"))?;

        // Plausible source, not filler: the tool numbers every line and cuts
        // long ones, and both cost what they cost on real text.
        let mut planted = String::with_capacity(PLANTED * 48);
        for line in 0..PLANTED {
            let _ = writeln!(planted, "    let field_{line} = compute(&state, {line})?;");
        }
        fs::write(base.join("work").join("big.rs"), planted)?;

        Ok(Self(base))
    }

    fn workspace(&self) -> Result<Workspace, ProbeError> {
        Ok(Workspace::open(self.0.join("work"))?)
    }

    fn logs(&self) -> PathBuf {
        self.0.join("logs")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The recorded halves of one turn: the model asks for the tool, then answers.
#[derive(Debug)]
struct Canned {
    calling: String,
    saying: String,
    round: std::sync::atomic::AtomicUsize,
}

impl Canned {
    fn new() -> Self {
        let calling = concat!(
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"toolu_1\",\"name\":\"read\",\"input\":{}}}\n\n",
            "event: content_block_delta\ndata: {\"index\":0,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"path\\\":\\\"big.rs\\\"}\"}}\n\n",
            "event: content_block_stop\ndata: {\"index\":0}\n\n",
            "event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"tool_use\"}}\n\n",
        )
        .to_owned();

        let mut saying = String::with_capacity(DELTAS * 96);
        saying.push_str(
            "event: content_block_start\ndata: {\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
        );
        for word in 0..DELTAS {
            let _ = write!(
                saying,
                "event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"field {word} \"}}}}\n\n"
            );
        }
        saying
            .push_str("event: message_delta\ndata: {\"delta\":{\"stop_reason\":\"end_turn\"}}\n\n");

        Self {
            calling,
            saying,
            round: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

impl Transport for Canned {
    fn post(
        &self,
        _url: &str,
        _headers: &[(Box<str>, Box<str>)],
        _body: &str,
    ) -> Result<Response, TransportError> {
        // Alternating, because that is the shape of a turn: the model asks for
        // a tool, sees the result, and then answers.
        let round = self
            .round
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let body = if round.is_multiple_of(2) {
            &self.calling
        } else {
            &self.saying
        };

        Ok(Response {
            status: 200,
            body: Box::new(Cursor::new(body.clone().into_bytes())),
        })
    }
}

/// Standard output's discard file, pretending to be a terminal.
///
/// The renderer asks whether it is talking to one in order to decide whether to
/// emit cursor movement at all, and the escape sequences are part of what the
/// live tail costs.
#[derive(Debug)]
struct Discard(File);

impl Discard {
    fn open() -> Result<Self, io::Error> {
        let path = if cfg!(windows) { "NUL" } else { "/dev/null" };
        Ok(Self(OpenOptions::new().write(true).open(path)?))
    }
}

impl Terminal for Discard {
    fn size(&self) -> Result<Size, TerminalError> {
        Ok(Size {
            columns: COLUMNS,
            rows: ROWS,
        })
    }

    fn write(&mut self, text: &str) -> Result<(), TerminalError> {
        self.0.write_all(text.as_bytes())?;
        Ok(())
    }

    fn flush(&mut self) -> Result<(), TerminalError> {
        self.0.flush()?;
        Ok(())
    }

    fn is_terminal(&self) -> bool {
        true
    }
}

/// Events, drawn as they arrive.
///
/// Single-threaded here, so a cell is enough where the binary needs a channel.
/// What matters for the measurement is that every event reaches a real frame.
#[derive(Debug)]
struct Drawing(RefCell<Renderer<Discard>>);

impl Post for Drawing {
    fn post(&self, event: Event) {
        let Ok(mut renderer) = self.0.try_borrow_mut() else {
            return;
        };

        let _ = match event {
            Event::Delta { text } => renderer.stream(&text),
            Event::ToolRequested { call } => renderer.commit(&call.name),
            Event::ToolFinished { .. } | Event::TurnFinished { .. } => renderer.settle(),
            Event::TurnStarted { .. } | Event::Failed { .. } => Ok(()),
        };
    }
}

/// Allows everything, so the measured session is a whole one.
///
/// Never reached in practice: `read` is read-only and the permission engine
/// does not ask about those. Denying here would silently shorten the session
/// if that ever changed, and a benchmark that quietly measures less is worse
/// than one that fails.
struct Allowing;

impl Ask for Allowing {
    fn ask(&mut self, _call: &ToolCall, _sensitivity: &Sensitivity) -> (Verdict, Remember) {
        (Verdict::Allow, Remember::Session)
    }
}

/// Runs the session and returns the peak resident set, in megabytes.
fn measure() -> Result<f64, ProbeError> {
    let scratch = Scratch::new()?;
    let workspace = scratch.workspace()?;
    let cancel = Cancel::new();

    let mut tools = Tools::new();
    tools.add(Box::new(Read::new(workspace.clone())));

    let mut runner = Runner::new(
        Box::new(Anthropic::new(
            Box::new(HeaderKey::new(
                ApiKey::new("bench-not-a-key"),
                Header::bare("x-api-key"),
            )),
            Box::new(Canned::new()),
        )),
        tools,
        Model {
            name: "bench".into(),
            max_tokens: 8192,
            system: Some("Measured, not asked.".into()),
        },
        Session::start(&scratch.logs(), &workspace)?,
    );

    let events = Drawing(RefCell::new(Renderer::new(Discard::open()?)));
    let mut ask = Allowing;

    for turn in 0..TURNS {
        let prompt = format!("turn {turn}: read big.rs and describe it");
        // A turn that failed measured less than a turn that ran, so the reading
        // would be an understatement rather than a failure. Better to stop.
        runner
            .turn(&prompt, &mut ask, &events, &cancel)
            .map_err(|problem| io::Error::other(problem.to_string()))?;
    }

    peak()
}

/// The high-water mark of this process's resident set, in megabytes.
fn peak() -> Result<f64, ProbeError> {
    let status = fs::read_to_string(Path::new("/proc/self/status"))?;

    let kilobytes: f64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .ok_or(ProbeError::Unmeasurable)?;

    Ok(kilobytes / 1024.0)
}

fn report(megabytes: f64) -> Result<(), io::Error> {
    // `println!` is denied workspace-wide, so the reading goes out through a
    // write whose failure is handled rather than panicked on inside a probe.
    let mut line = String::new();
    let _ = write!(line, "{megabytes:.1} MB {LIMIT:.0}");
    line.push('\n');

    io::stdout().write_all(line.as_bytes())?;
    io::stdout().flush()
}

/// Says why no reading could be taken, on stderr, where `scripts/bench.sh`
/// puts everything a human reads.
fn explain(problem: &ProbeError) -> Result<(), io::Error> {
    let mut line = String::new();
    let _ = writeln!(line, "    FAIL {problem}");

    io::stderr().write_all(line.as_bytes())
}

fn main() -> ExitCode {
    let megabytes = match measure() {
        Ok(megabytes) => megabytes,
        Err(problem) => {
            let _ = explain(&problem);
            return ExitCode::FAILURE;
        }
    };

    if report(megabytes).is_err() {
        return ExitCode::FAILURE;
    }

    if megabytes > LIMIT {
        return ExitCode::FAILURE;
    }

    ExitCode::SUCCESS
}
