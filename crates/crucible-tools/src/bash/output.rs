//! Waiting for a command, and collecting what it produced.
//!
//! Separate from the call that started it because the hard part here is not the
//! shell: it is that a command can outlive its own exit. A killed process leaves
//! grandchildren holding its pipes, a long one fills a pipe buffer and blocks
//! until somebody reads it, and either one turns a naive wait into a hang. So
//! the pipes are drained on threads from the moment the command starts, and
//! every wait in this module is bounded.

use std::collections::VecDeque;
use std::io;
use std::process::{Child, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{Cancel, ToolError, ToolOutput, Watch, Wrote};

use super::platform::{Output, ReadState, Scope};
use super::{NAME, TICK, io as tool_io};
use crate::bound::OUTPUT;

/// The most one handover carries, in bytes.
///
/// A window on a live command rather than a record of it: the reader is being
/// shown the last few rows of what a build is doing, and a command emitting
/// megabytes a second would fill any figure chosen here between one tick and the
/// next. So what does not fit is dropped **for the reader only** — the result the
/// model is sent keeps its head, its tail and the count of what fell out of the
/// middle, exactly as it did before any of this existed. The two losses are
/// counted separately for that reason: `Kept::dropped` is about the answer, and
/// nothing counts this one, because a window has never claimed to be complete.
///
/// One pipe read, so a command printing at a readable rate never loses a byte
/// of what is shown.
const FRESH: usize = 8 * 1024;

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
    child: Child,
    scope: &Scope,
    allowed: Duration,
    cancel: &Cancel,
    watch: &dyn Watch,
) -> Result<ToolOutput, ToolError> {
    // Own the child before the first fallible pipe operation. Any `?` below
    // therefore stops the process scope and performs only a bounded reap.
    let mut running = Running::new(child, scope);
    let mut out = Pipe::drain(running.child.stdout.take(), "stdout")?;
    let mut err = Pipe::drain(running.child.stderr.take(), "stderr")?;

    let deadline = Instant::now() + allowed;
    let mut expired = false;

    // A child is not one of this program's threads: nothing in it will notice
    // the flag, so the only way to stop it is to kill it.
    let status = loop {
        match running.child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(source) => return Err(tool_io("could not wait for the command", source)),
        }

        if cancel.requested() {
            let _ = running.stop()?;
            return Err(ToolError::Cancelled(NAME));
        }

        if Instant::now() >= deadline {
            let status = running.stop()?;
            expired = true;
            break status;
        }

        // Handed over here rather than from the reader threads, and that is the
        // load-bearing choice in this loop. A reader that blocked on a full
        // channel would stop draining its pipe, the pipe would fill, and the
        // command would stall behind the terminal — which is the deadlock the
        // whole module is arranged to prevent. This thread is already the one
        // doing nothing but waiting, so it is the one that can afford to wait
        // again.
        //
        // Both pipes, in one order, every tick. A command writing to each of
        // them can have its lines interleaved differently from how it wrote
        // them; that is true of the answer as well, where they are joined the
        // same way, and no reading of two pipes can be better than this.
        for said in [out.hand_over(), err.hand_over()] {
            if !said.is_empty() {
                watch.wrote(Wrote::new(said));
            }
        }

        thread::sleep(TICK);
    };

    // A shell can exit successfully while a background descendant continues.
    // This tool has no handle it could return for such a process, so keeping it
    // would make both its lifetime and its resources unbounded. The process
    // group or job survives its leader and is ended here on every exit path.
    if !expired {
        running.finish_after_exit()?;
    }

    let ended = settle(&out, &err);
    out.close()?;
    err.close()?;

    Ok(Finished {
        code: status.and_then(|status| status.code()),
        out: joined(&out, &err),
        arriving: !ended,
        expired,
    }
    .report())
}

/// Stops a child that could not be handed to [`collect`].
///
/// Used after Windows job attachment fails. The shell is still suspended, but
/// waiting for it without first ending it would be just as unbounded as any
/// other child wait.
#[cfg(windows)]
pub(super) fn discard(child: Child, scope: &Scope) {
    drop(Running::new(child, scope));
}

/// A child whose scope is stopped and reaped on every return path.
///
/// `Child::kill` signals the shell alone, and a shell is rarely the only
/// process a line makes: every other member of a pipeline is a child of it,
/// so they are reparented and keep running once it is gone. `yes > /dev/null |
/// cat` then burns a core for the rest of the session, after the tool has
/// returned and with nothing left holding a handle to it.
///
/// So the signal goes to the process group instead, which [`super`] puts the
/// shell at the head of when it spawns one. Group membership is inherited, and
/// a non-interactive shell does no job control of its own, so ordinary
/// descendants remain in it — including the ones the shell had already stopped
/// waiting for. A Unix program that deliberately creates a new session can
/// escape a process group; the `bash` module therefore does not claim that
/// commands themselves are confined.
///
/// Windows uses a kill-on-close job and Unix a process group; both live in the
/// platform boundary so the wait never has a branch that stops only the shell.
struct Running<'a> {
    child: Child,
    scope: &'a Scope,
    released: bool,
}

impl<'a> Running<'a> {
    fn new(child: Child, scope: &'a Scope) -> Self {
        Self {
            child,
            scope,
            released: false,
        }
    }

    /// Stops the scope and gives the shell a bounded interval to become reapable.
    fn stop(&mut self) -> Result<Option<ExitStatus>, ToolError> {
        stop_scope(self.scope, &mut self.child)
            .map_err(|source| tool_io("could not stop the command", source))?;
        let status = reap(&mut self.child, SETTLE)
            .map_err(|source| tool_io("could not inspect the stopped command", source))?;
        let Some(status) = status else {
            return Err(tool_io(
                "could not reap the stopped command",
                io::Error::new(
                    io::ErrorKind::TimedOut,
                    "the command did not exit after it was stopped",
                ),
            ));
        };
        self.released = true;
        Ok(Some(status))
    }

    /// Stops descendants after `try_wait` has already reaped the shell.
    fn finish_after_exit(&mut self) -> Result<(), ToolError> {
        stop_scope(self.scope, &mut self.child)
            .map_err(|source| tool_io("could not stop command descendants", source))?;
        self.released = true;
        Ok(())
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        // Destructors cannot report a second failure over the error already on
        // its way out. They can still guarantee that cleanup itself is bounded.
        let _ = stop_scope(self.scope, &mut self.child);
        let _ = reap(&mut self.child, SETTLE);
    }
}

/// Waits only until `allowed`; a failed termination can never become a hang.
fn reap(child: &mut Child, allowed: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + allowed;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(TICK);
    }
}

#[cfg(unix)]
fn stop_scope(_scope: &Scope, child: &mut Child) -> io::Result<()> {
    Scope::stop(child)
}

#[cfg(windows)]
fn stop_scope(scope: &Scope, child: &mut Child) -> io::Result<()> {
    scope.stop(child)
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
    /// What has arrived since it was last handed over, for the reader watching
    /// the command run. Bounded by [`FRESH`] and emptied by
    /// [`Kept::hand_over`]; nothing accumulates here across a command.
    fresh: VecDeque<u8>,
}

impl Kept {
    /// Takes what it can of `arrived` and counts the rest.
    fn push(&mut self, arrived: &[u8]) {
        // The reader's window first, and bounded on its own: it is emptied every
        // tick, so what it holds is one tick's worth rather than a command's.
        let recent = arrived
            .get(arrived.len().saturating_sub(FRESH)..)
            .unwrap_or(arrived);
        let spare = self.fresh.len().saturating_add(recent.len());
        let over = spare.saturating_sub(FRESH).min(self.fresh.len());
        self.fresh.drain(..over);
        self.fresh.extend(recent);

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
        self.dropped = self.dropped.saturating_add(gone.len());

        // `keep` is at most `OUTPUT`, so the spill is never more than the ring
        // is currently holding.
        let spill = self
            .tail
            .len()
            .saturating_add(keep.len())
            .saturating_sub(OUTPUT);
        self.tail.drain(..spill);
        self.dropped = self.dropped.saturating_add(spill);
        self.tail.extend(keep);
    }

    /// What has arrived since this was last called, as far as the last whole
    /// character in it.
    ///
    /// A pipe is read in fixed blocks, so a block boundary can fall inside a
    /// multi-byte character. Handing that over would put a replacement mark on
    /// screen and then another one next tick, for a character that was never
    /// damaged — so an incomplete sequence at the end is held back and joins the
    /// next handover. A sequence that is genuinely not UTF-8 is handed over with
    /// its bad bytes in it, because holding *that* back would stall the window
    /// for as long as the command ran.
    fn hand_over(&mut self) -> String {
        if self.fresh.is_empty() {
            return String::new();
        }

        // Bounded by `FRESH`, which is what makes collecting it whole safe.
        let arrived: Vec<u8> = self.fresh.iter().copied().collect();

        // Everything except a sequence still arriving at the end. Walked rather
        // than answered in one step because a bad byte is not the end of the
        // buffer: stopping at the first one would hand over a single mark per
        // tick and leave the rest waiting, so a command printing anything
        // binary would crawl instead of scrolling.
        let mut whole = 0;
        loop {
            let rest = arrived.get(whole..).unwrap_or_default();
            match std::str::from_utf8(rest) {
                Ok(_) => {
                    whole = arrived.len();
                    break;
                }
                Err(problem) => match problem.error_len() {
                    // Incomplete at the end, and it may be the front of a
                    // character the next read completes.
                    None => {
                        whole = whole.saturating_add(problem.valid_up_to());
                        break;
                    }
                    // Not UTF-8 anywhere. Step over it and keep looking; the
                    // lossy conversion below is what says so on screen.
                    Some(bad) => {
                        whole = whole
                            .saturating_add(problem.valid_up_to())
                            .saturating_add(bad);
                    }
                },
            }
        }

        self.fresh.drain(..whole);
        String::from_utf8_lossy(arrived.get(..whole).unwrap_or_default()).into_owned()
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
    reader: Option<thread::JoinHandle<io::Result<()>>>,
}

impl Pipe {
    /// Starts reading `pipe` on a thread.
    fn drain<R: Output>(pipe: Option<R>, stream: &'static str) -> Result<Self, ToolError> {
        let kept = Arc::new(Mutex::new(Kept::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (into, until) = (Arc::clone(&kept), Arc::clone(&stop));

        let Some(mut pipe) = pipe else {
            return Ok(Self {
                kept,
                stop,
                reader: None,
            });
        };
        pipe.prepare()
            .map_err(|source| tool_io("could not prepare a command output pipe", source))?;

        let reader = thread::Builder::new()
            .name(format!("crucible-bash-{stream}"))
            .spawn(move || {
                let mut buffer = [0_u8; 8192];

                loop {
                    if until.load(Ordering::Relaxed) {
                        return Ok(());
                    }

                    let read = match pipe.read_ready(&mut buffer) {
                        // The end of the pipe, and the only way out of here that
                        // [`ended`] is entitled to read as one.
                        Ok(ReadState::End) => return Ok(()),
                        Ok(ReadState::Bytes(read)) => read,
                        Ok(ReadState::Pending) => {
                            thread::sleep(TICK);
                            continue;
                        }
                        // What `read` documents as non-fatal and asks callers to
                        // retry. Producing one takes a signal handler that returns,
                        // and this process installs none — catching a signal needs
                        // `unsafe`, which the workspace denies — so it cannot
                        // happen today. It is retried rather than reasoned about
                        // because the fact protecting it lives in another crate:
                        // the day anything here catches a resize, dropping this
                        // would end a reader mid-command and send the cut output
                        // back looking complete.
                        Err(problem) if problem.kind() == io::ErrorKind::Interrupted => continue,
                        Err(problem) => return Err(problem),
                    };

                    if until.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    // Neither of these can be the arm that runs. `read` never
                    // reports more than the buffer it was handed, and the lock is
                    // poisoned only by a panic inside `push` or `bytes` — neither
                    // of which indexes, unwraps or does arithmetic that is not
                    // saturating, in a crate where all three are denied anyway.
                    let (Some(arrived), Ok(mut kept)) = (buffer.get(..read), into.lock()) else {
                        return Ok(());
                    };
                    kept.push(arrived);
                }
            })
            .map_err(|source| tool_io("could not start a command output reader", source))?;

        Ok(Self {
            kept,
            stop,
            reader: Some(reader),
        })
    }

    /// Whether the reader has reached the end of the pipe.
    ///
    /// Answered by the thread having stopped. An I/O failure can also stop the
    /// thread, but [`Self::close`] joins it and reports that failure before a
    /// `ToolOutput` can be returned; `ended` only bounds how long collection
    /// waits before that definitive result.
    fn ended(&self) -> bool {
        self.reader
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
    }

    /// What has arrived so far, and how many bytes were let go to keep it
    /// bounded.
    ///
    /// This never joins the reader. It takes a bounded snapshot while the
    /// collection path remains responsible for stopping and joining the
    /// pollable reader afterwards.
    fn take(&self) -> (Vec<u8>, usize) {
        self.kept
            .lock()
            .map(|kept| (kept.bytes(), kept.dropped))
            .unwrap_or_default()
    }

    /// What has arrived on this pipe since the last time it was asked.
    ///
    /// Empty where nothing has, which is the ordinary answer for a command
    /// between two lines of output.
    fn hand_over(&self) -> String {
        self.kept
            .lock()
            .map(|mut kept| kept.hand_over())
            .unwrap_or_default()
    }

    /// Stops and joins the reader, reporting spawn-side failures as tool errors.
    fn close(&mut self) -> Result<(), ToolError> {
        self.stop.store(true, Ordering::Relaxed);
        let Some(reader) = self.reader.take() else {
            return Ok(());
        };
        match reader.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(source)) => Err(tool_io("could not read command output", source)),
            Err(_) => Err(tool_io(
                "a command output reader stopped unexpectedly",
                io::Error::other("the output reader thread panicked"),
            )),
        }
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // Pollable reads make this join bounded even when a descendant still
        // owns the writer. Error paths therefore release their reader too.
        let _ = self.close();
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

    cut(
        &String::from_utf8_lossy(&both),
        from_out.saturating_add(from_err),
    )
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
    let dropped = already.saturating_add(
        text.len()
            .saturating_sub(head.len())
            .saturating_sub(tail.len()),
    );

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
