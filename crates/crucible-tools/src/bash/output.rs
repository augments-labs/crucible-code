//! Waiting for a command, and collecting what it produced.
//!
//! Separate from the call that started it because the hard part here is not the
//! shell: it is that a command can outlive its own exit. A killed process leaves
//! grandchildren holding its pipes, a long one fills a pipe buffer and blocks
//! until somebody reads it, and either one turns a naive wait into a hang. So
//! the pipes are drained on threads from the moment the command starts, and
//! every wait in this module is bounded.

use std::collections::VecDeque;
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
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
        out: joined(&out, &err),
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

/// The two ends of one stream, and how much of the middle went.
///
/// The bound has to be here rather than on the way out, because the way out
/// runs once and the reader runs for as long as the command does: `yes` or
/// `cat /dev/urandom` fills memory with bytes that were always going to be
/// thrown away, and the 30 KB the model finally sees says nothing about the
/// gigabyte it took to choose them. Each end holds `OUTPUT`, which is twice
/// what the join can take from either end of one stream — the slack is what
/// makes the two streams composable without either one having to know how long
/// the other is.
#[derive(Default)]
struct Kept {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: usize,
}

impl Kept {
    /// Takes what it can of `arrived` and counts the rest.
    fn push(&mut self, arrived: &[u8]) {
        // The head fills once and then never moves again, which is what makes
        // it the *first* bytes rather than some later window of them.
        let room = OUTPUT.saturating_sub(self.head.len()).min(arrived.len());
        let (first, rest) = arrived.split_at(room);
        self.head.extend_from_slice(first);
        if rest.is_empty() {
            return;
        }

        // Only the last `OUTPUT` bytes of a batch can outlive it, so a batch
        // bigger than the ring is cut down to its own tail in one step instead
        // of one byte at a time — a command emitting megabytes a second is
        // exactly the one that must not pay per byte here.
        let stale = rest.len().saturating_sub(OUTPUT);
        let (gone, keep) = rest.split_at(stale);
        self.dropped += gone.len();

        // `keep` is at most `OUTPUT`, so the spill is never more than the ring
        // is currently holding.
        let spill = (self.tail.len() + keep.len()).saturating_sub(OUTPUT);
        self.tail.drain(..spill);
        self.dropped += spill;
        self.tail.extend(keep);
    }

    /// The two ends, in order, with the gap between them unmarked — [`cut`] is
    /// where it gets said, because that is where the two streams have been put
    /// together and there is one gap to describe.
    fn bytes(&self) -> Vec<u8> {
        // Bounded by `2 * OUTPUT` however long the command ran, which is what
        // this type exists to guarantee.
        let mut all = self.head.clone();
        all.extend(&self.tail);
        all
    }
}

/// One of a command's output pipes, being read on a thread of its own.
struct Pipe {
    /// Shared rather than returned, so what has arrived can be taken without
    /// waiting for the end that may never come.
    kept: Arc<Mutex<Kept>>,
    /// Told to the reader when this end of it goes away. A grandchild can hold
    /// a pipe open long after the turn it belonged to is over, and a reader
    /// nobody is waiting for should not still be collecting for it.
    stop: Arc<AtomicBool>,
    reader: thread::JoinHandle<()>,
}

impl Pipe {
    /// Starts reading `pipe` on a thread.
    fn drain<R: std::io::Read + Send + 'static>(pipe: Option<R>) -> Self {
        let kept = Arc::new(Mutex::new(Kept::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (into, until) = (Arc::clone(&kept), Arc::clone(&stop));

        let reader = thread::spawn(move || {
            let Some(mut pipe) = pipe else { return };
            let mut buffer = [0_u8; 8192];

            loop {
                let read = match pipe.read(&mut buffer) {
                    // The end of the pipe, and the only way out of here that
                    // [`ended`] is entitled to read as one.
                    Ok(0) => return,
                    Ok(read) => read,
                    // What `read` documents as non-fatal and asks callers to
                    // retry. Producing one takes a signal handler that returns,
                    // and this process installs none — catching a signal needs
                    // `unsafe`, which the workspace denies — so it cannot
                    // happen today. It is retried rather than reasoned about
                    // because the fact protecting it lives in another crate:
                    // the day anything here catches a resize, dropping this
                    // would end a reader mid-command and send the cut output
                    // back looking complete.
                    Err(problem) if problem.kind() == std::io::ErrorKind::Interrupted => continue,
                    // A pipe we own, on a descriptor nothing else closes. What
                    // is left is the far end being gone, which is the end of it
                    // under another name.
                    Err(_) => return,
                };

                if until.load(Ordering::Relaxed) {
                    return;
                }
                // Neither of these can be the arm that runs. `read` never
                // reports more than the buffer it was handed, and the lock is
                // poisoned only by a panic inside `push` or `bytes` — neither
                // of which indexes, unwraps or does arithmetic that is not
                // saturating, in a crate where all three are denied anyway.
                let (Some(arrived), Ok(mut kept)) = (buffer.get(..read), into.lock()) else {
                    return;
                };
                kept.push(arrived);
            }
        });

        Self { kept, stop, reader }
    }

    /// Whether the reader has reached the end of the pipe.
    ///
    /// Answered by the thread having stopped, which is the same question only
    /// because every other way out of that loop is one that cannot be taken.
    /// Anything that makes one of them possible owes an answer here as well: a
    /// reader that gave up early reports as a command that finished, and what
    /// it collected goes back to the model looking like all of it.
    fn ended(&self) -> bool {
        self.reader.is_finished()
    }

    /// What has arrived so far, and how many bytes were let go to keep it
    /// bounded.
    ///
    /// This never joins the reader. A killed command can leave a grandchild
    /// holding the pipe open — a background server is the usual one — and
    /// waiting for the end would then wait for a process nothing will stop.
    fn take(&self) -> (Vec<u8>, usize) {
        self.kept
            .lock()
            .map(|kept| (kept.bytes(), kept.dropped))
            .unwrap_or_default()
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // The thread is not joined here either, for the reason `take` gives.
        // What this does is stop it collecting: it returns the next time the
        // pipe hands it anything, and lets go of its share of the buffer then.
        self.stop.store(true, Ordering::Relaxed);
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

/// Both streams in one piece of text, cut to size.
///
/// They are concatenated rather than labelled because a command's diagnostics
/// belong next to the output they explain, and a model reading `cargo test`
/// needs the failure and the summary in one piece of text. Concatenated, not
/// interleaved: the whole of `stdout` and then the whole of `stderr`. Two pipes
/// read on two threads carry no shared order and none is recorded, so the
/// result is not the sequence a terminal would have shown — a progress line on
/// `stderr` says nothing here about which `stdout` line it came between.
fn joined(out: &Pipe, err: &Pipe) -> String {
    let (mut both, from_out) = out.take();
    let (rest, from_err) = err.take();
    both.extend(rest);

    cut(&String::from_utf8_lossy(&both), from_out + from_err)
}

/// The head and the tail, when there is more than anything can use.
///
/// The middle goes because the two ends carry the meaning: what the command
/// started doing, and how it ended. `already` is what the readers let go before
/// this ever saw it, and it belongs in the same count — one number for the gap,
/// because there is one gap.
fn cut(text: &str, already: usize) -> String {
    if text.len() <= OUTPUT {
        // Nothing to elide. A stream that let anything go kept a full head and
        // a full tail, which is `2 * OUTPUT` on its own, so `already` is zero
        // whenever this branch is the one taken.
        return text.trim_end().to_owned();
    }

    let half = OUTPUT / 2;
    let head = text.get(..boundary(text, half)).unwrap_or_default();
    let tail = text
        .get(boundary(text, text.len() - half)..)
        .unwrap_or_default();
    let dropped = already + text.len() - head.len() - tail.len();

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

#[cfg(test)]
mod tests;
