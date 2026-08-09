//! Waiting for a command, and collecting what it produced.
//!
//! Separate from the call that started it because the hard part here is not the
//! shell: it is that a command can outlive its own exit. A killed process leaves
//! grandchildren holding its pipes, a long one fills a pipe buffer and blocks
//! until somebody reads it, and either one turns a naive wait into a hang. So
//! the pipes are drained on threads from the moment the command starts, and
//! every wait in this module is bounded.

use std::process::Child;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{Cancel, ToolError, ToolOutput};

use super::{NAME, TICK, io};

/// How much output comes back. Past this the interesting part is the end —
/// the error, the summary line — so the middle is what goes.
pub(super) const OUTPUT: usize = 30_000;

/// How long the readers get to reach the end of their pipes once the command
/// itself is over. Reading what is already buffered takes no time at all, so
/// this is only ever spent when something else is still holding a pipe open.
const SETTLE: Duration = Duration::from_millis(200);

/// Waits for `child`, killing it if the deadline passes or the user stops the
/// turn, and reports what it produced.
///
/// The pipes are drained on their own threads. Waiting first and reading
/// afterwards would deadlock the moment a command produced more output than a
/// pipe buffer holds, which is most commands worth running.
pub(super) fn collect(
    mut child: Child,
    allowed: Duration,
    cancel: &Cancel,
) -> Result<ToolOutput, ToolError> {
    let out = Pipe::drain(child.stdout.take());
    let err = Pipe::drain(child.stderr.take());

    let deadline = Instant::now() + allowed;
    let mut expired = false;

    // A child is not one of this program's threads: nothing in it will notice
    // the flag, so the only way to stop it is to kill it.
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(source) => return Err(io("could not wait for the command", source)),
        }

        if cancel.requested() {
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
    }
    .report())
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

        // One marker, whatever happened. A command killed for running too long
        // has usually left something holding the pipe as well, and saying both
        // in two notes reads as two problems — the second of which names a
        // cause that is not why this stopped. So the timeout takes the marker
        // and carries the other fact inside it, because a prefix still has to
        // say that it is one.
        if self.expired {
            let held = if self.arriving {
                ", and something it left running still holds the output open"
            } else {
                ""
            };

            return ToolOutput::failed(format!(
                "{body}\n\n[stopped: the command ran too long{held}]"
            ));
        }

        // Said whatever the exit status was: the command can succeed and still
        // have more to print. Unsaid, the model reads a prefix as the whole and
        // concludes the command printed nothing else.
        if self.arriving {
            body.push_str(
                "\n\n[output was still arriving: something the command left running holds it open]",
            );
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
pub(super) fn cut(text: &str) -> String {
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
