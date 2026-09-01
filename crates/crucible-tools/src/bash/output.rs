//! Waiting for a command, and collecting what it produced.
//!
//! Separate from the call that started it because the hard part here is not the
//! shell: it is that a command can outlive its own exit. A killed process leaves
//! grandchildren holding its pipes, a long one fills a pipe buffer and blocks
//! until somebody reads it, and either one turns a naive wait into a hang. So
//! the pipes are drained on threads from the moment the command starts, and
//! every wait in this module is bounded.
//!
//! A command can also outlive the *call* — that is [`super::background`], and it
//! is the one path out of here that does not end what it was waiting on. What
//! makes that safe is not this module: it is that the registry taking the command
//! owns ending it, and that the two never both think they do. [`Waited`] is where
//! the handover is, and it stops guarding the moment it hands over.

use std::collections::VecDeque;
use std::io;
use std::process::ExitStatus;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crucible_core::{
    Cancel, SandboxOutput, SandboxProcess, SandboxRead, SandboxViolation, ToolError, ToolOutput,
    Watch, Wrote,
};

use super::background::{Background, Taking};

use super::{NAME, TICK, io as tool_io};
use crate::bound::OUTPUT;

/// One stream's fixed prefix and rolling suffix budgets.
const CAPTURE_HEAD: usize = OUTPUT / 2;
const CAPTURE_TAIL: usize = OUTPUT - CAPTURE_HEAD;

/// Raw output left beside a capture-elision note. The invocation pipeline
/// later applies the authoritative encoded bound.
pub(super) const CAPTURE_TEXT: usize = OUTPUT - 256;

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
    process: Box<dyn SandboxProcess>,
    waiting: &Waiting<'_>,
) -> Result<Left, ToolError> {
    let Waiting {
        allowed,
        cancel,
        watch,
        leaving,
    } = waiting;
    // Own the child before the first fallible pipe operation. Any `?` below
    // therefore stops the process scope and performs only a bounded reap.
    let started = Instant::now();
    let mut running = Waited::new(process);
    let mut out = Pipe::drain(running.taking()?.take_stdout(), "stdout")?;
    let mut err = Pipe::drain(running.taking()?.take_stderr(), "stderr")?;

    let deadline = started + *allowed;
    let mut expired = false;

    // A child is not one of this program's threads: nothing in it will notice
    // the flag, so the only way to stop it is to kill it.
    let status = loop {
        match running.taking()?.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(source) => return Err(tool_io("could not wait for the command", source)),
        }

        // Asked before the deadline and before the cancel, because it is the one
        // of the three that keeps the command: a press and a timeout landing in
        // the same tick should leave the command running rather than kill it.
        if let Some(why) = leaving.as_ref().and_then(|leaving| leaving.now(started))
            && let Some(process) = running.given()
        {
            return Ok(Left::Running(Taking {
                process,
                out,
                err,
                since: started,
                why,
            }));
        }

        if cancel.requested() {
            let _ = running.stop()?;
            return Err(ToolError::Cancelled(NAME.into()));
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

    let violation = running.taking()?.violation();
    expired |= violation == Some(SandboxViolation::CommandTime);

    // A shell can exit successfully while a descendant of it continues, and the
    // process group or job survives its leader. Ended here on every path that
    // leaves through this function — which is every path but one: a command let
    // go of above is owned by the registry that took it, and that is what ends it
    // instead. Nothing reaches the end of a command's life without somebody
    // holding it.
    if !expired {
        running.finish_after_exit()?;
    }

    let ended = settle(&out, &err);
    out.close()?;
    err.close()?;

    let captured = joined(&out, &err);
    Ok(Left::Answered(
        Finished {
            code: status.and_then(|status| status.code()),
            out: captured.text,
            original: captured.original,
            omitted: captured.omitted,
            arriving: !ended,
            expired,
            output_limited: violation == Some(SandboxViolation::Output),
        }
        .report(),
    ))
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
///
/// It owns the scope rather than borrowing it, because one exit path from
/// [`collect`] hands both on to whatever will end them later instead.
struct Waited {
    process: Option<Box<dyn SandboxProcess>>,
    released: bool,
}

impl Waited {
    fn new(process: Box<dyn SandboxProcess>) -> Self {
        Self {
            process: Some(process),
            released: false,
        }
    }

    /// The child, for the wait to look at.
    ///
    /// An error rather than a panic where it has been handed on: every path that
    /// asks has already returned by then, so this is the arm nothing takes, and a
    /// session is worth more than a proof about a branch.
    fn taking(&mut self) -> Result<&mut (dyn SandboxProcess + 'static), ToolError> {
        self.process.as_deref_mut().ok_or_else(|| {
            tool_io(
                "the command was handed over while it was still being waited for",
                io::Error::other("no child left to wait for"),
            )
        })
    }

    /// Hands the command and its containment to a caller that will end them.
    ///
    /// After this the guard ends nothing: what it was guarding against is a
    /// command nobody owns, and somebody does.
    fn given(&mut self) -> Option<Box<dyn SandboxProcess>> {
        let taken = self.process.take()?;
        self.released = true;

        Some(taken)
    }

    /// Stops the scope and gives the shell a bounded interval to become reapable.
    fn stop(&mut self) -> Result<Option<ExitStatus>, ToolError> {
        let Some(process) = self.process.as_deref_mut() else {
            return Ok(None);
        };

        end(process).map_err(|source| tool_io("could not stop the command", source))?;
        let status = reap(self.taking()?, SETTLE)
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
        let Some(process) = self.process.as_deref_mut() else {
            return Ok(());
        };

        end(process).map_err(|source| tool_io("could not stop command descendants", source))?;
        self.released = true;
        Ok(())
    }
}

impl Drop for Waited {
    fn drop(&mut self) {
        if self.released {
            return;
        }

        // Destructors cannot report a second failure over the error already on
        // its way out. They can still guarantee that cleanup itself is bounded.
        let Some(process) = self.process.as_deref_mut() else {
            return;
        };

        let _ = end(process);
        let _ = reap(process, SETTLE);
    }
}

/// Waits only until `allowed`; a failed termination can never become a hang.
fn reap(
    process: &mut (dyn SandboxProcess + 'static),
    allowed: Duration,
) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + allowed;
    loop {
        if let Some(status) = process.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(TICK);
    }
}

/// Ends a command's whole process group, whatever the platform calls one.
///
/// Named here rather than in two places because two modules end a command now:
/// the wait that owns one, and the registry that took one over.
pub(super) fn end(process: &mut (dyn SandboxProcess + 'static)) -> io::Result<()> {
    process.stop()
}

/// Everything the wait needs besides the command itself.
///
/// Gathered rather than passed one by one, for the reason the runner gathers a
/// pass's worth: a call with six arguments beside it is a call nobody can read,
/// and these four are one thing — the terms the command is being waited under.
pub(super) struct Waiting<'a> {
    /// How long it may take before it is stopped.
    pub(super) allowed: Duration,
    /// Whether the user has asked everything to stop.
    pub(super) cancel: &'a Cancel,
    /// Where what it prints goes while it runs.
    pub(super) watch: &'a dyn Watch,
    /// Whether it may be let go of, and when.
    pub(super) leaving: Option<Leaving<'a>>,
}

/// Which of the two ways in let go of the command.
///
/// The command cannot tell them apart, and nothing about how it runs depends on
/// which — but the model reading the result can act on it, and the two are not
/// the same thing to act on. One is its own call coming back; the other is the
/// developer stepping in mid-command and saying carry on without it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Why {
    /// The call asked for it, and the moment it was watched for has passed.
    Asked,
    /// Somebody pressed the key while it ran.
    Pressed,
}

/// When a command is to be let go of rather than waited for.
///
/// Two ways in, kept apart on the way out: see [`Why`].
pub(super) struct Leaving<'a> {
    /// Where it goes once it is let go of.
    pub(super) left: &'a Background,
    /// Let go of it once this much has passed, where the call asked to. `None`
    /// where only the key can ask.
    pub(super) after: Option<Duration>,
}

impl Leaving<'_> {
    /// Whether the command should be let go of now, and on whose account.
    ///
    /// The key is asked about second and its answer is *spent* — read and
    /// cleared — so one press cannot let go of two commands. Asking the clock
    /// first is what keeps a call that asked to be left running from also
    /// swallowing a press meant for the next command, and it is why a call that
    /// asked is never reported as a press.
    fn now(&self, started: Instant) -> Option<Why> {
        if self.after.is_some_and(|after| started.elapsed() >= after) {
            return Some(Why::Asked);
        }

        self.left.wanted().then_some(Why::Pressed)
    }
}

/// What one call came back with: an answer, or a command still running.
pub(super) enum Left {
    /// The command is over, and this is what it said.
    Answered(ToolOutput),
    /// It is still going, and whoever asked for that now owns it.
    Running(Taking),
}

/// What a command left behind.
struct Finished {
    /// `None` when a signal ended it, which is what a kill looks like.
    code: Option<i32>,
    out: String,
    /// Raw bytes both process pipes produced before capture elision.
    original: usize,
    /// Raw bytes removed from their middle during capture.
    omitted: usize,
    /// Whether the readers were still short of the end of a pipe when the wait
    /// for them ran out, which makes `out` a prefix of the output.
    arriving: bool,
    expired: bool,
    output_limited: bool,
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
            ))
            .with_capture_elision(self.original, self.omitted);
        }

        if self.output_limited {
            let held = if self.arriving {
                ", and something it left running still holds the output open"
            } else {
                ""
            };

            return ToolOutput::failed(format!(
                "{body}\n\n[stopped: the command exceeded its captured-output ceiling{held}]"
            ))
            .with_capture_elision(self.original, self.omitted);
        }

        // Said whatever the exit status was: the command can succeed and still
        // have more to print. Unsaid, the model reads a prefix as the whole and
        // concludes the command printed nothing else.
        if self.arriving {
            body.push_str(
                "\n\n[output was still arriving: something the command left running holds it open]",
            );
        }

        let output = match self.code {
            Some(0) => ToolOutput::ok(body),
            Some(code) => ToolOutput::failed(format!("{body}\n\n[exit status {code}]")),
            None => ToolOutput::failed(format!("{body}\n\n[the command was killed]")),
        };
        output.with_capture_elision(self.original, self.omitted)
    }
}

/// The two ends of one stream, and how much of the middle went.
///
/// The bound has to be here rather than on the way out, because the way out
/// runs once and the reader runs for as long as the command does: `yes` or
/// `cat /dev/urandom` fills memory with bytes that were always going to be
/// thrown away, and the 30 KB the model finally sees says nothing about the
/// gigabyte it took to choose them. The fixed head and rolling tail together
/// retain at most `OUTPUT` per stream; joining the two streams performs one
/// second bounded selection before the invocation pipeline applies the exact
/// encoded-result ceiling.
#[derive(Default)]
struct Kept {
    head: Vec<u8>,
    tail: VecDeque<u8>,
    dropped: usize,
    /// What has arrived since it was last handed over, for the reader watching
    /// the command run. Bounded by [`FRESH`] and emptied by
    /// [`Kept::hand_over`]; nothing accumulates here across a command.
    fresh: VecDeque<u8>,
    /// How many lines have arrived, the ones since dropped included.
    ///
    /// Counted as they come rather than read off what is kept, because what is
    /// kept has a hole in the middle of it: a count taken from the two ends would
    /// miss every line that fell between them. It is what a command nobody is
    /// waiting on is counted by, since there is no result to read a figure off.
    lines: usize,
}

impl Kept {
    /// Takes what it can of `arrived` and counts the rest.
    fn push(&mut self, arrived: &[u8]) {
        // The lint here asks for a crate whose whole subject is counting bytes
        // quickly. This counts newlines in one pipe read — eight kilobytes at
        // most — once per read, on a thread whose other job is waiting. A
        // dependency is not what that is worth, and the ladder this project adds
        // one by says so.
        #[allow(clippy::naive_bytecount)]
        let arrived_lines = arrived.iter().filter(|byte| **byte == b'\n').count();

        self.lines = self.lines.saturating_add(arrived_lines);

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
        let room = CAPTURE_HEAD
            .saturating_sub(self.head.len())
            .min(arrived.len());
        let (first, rest) = arrived.split_at(room);
        self.head.extend_from_slice(first);
        if rest.is_empty() {
            return;
        }

        // Only the rolling-tail budget of a batch can outlive it, so a batch
        // bigger than the ring is cut down to its own tail in one step instead
        // of one byte at a time — a command emitting megabytes a second is
        // exactly the one that must not pay per byte here.
        let stale = rest.len().saturating_sub(CAPTURE_TAIL);
        let (gone, keep) = rest.split_at(stale);
        self.dropped = self.dropped.saturating_add(gone.len());

        // `keep` is at most the tail budget, so the spill is never more than
        // the ring is currently holding.
        let spill = self
            .tail
            .len()
            .saturating_add(keep.len())
            .saturating_sub(CAPTURE_TAIL);
        self.tail.drain(..spill);
        self.dropped = self.dropped.saturating_add(spill);
        self.tail.extend(keep);
    }

    /// Counts bytes a lower hard ceiling consumed without retaining.
    fn discard(&mut self, bytes: usize) {
        self.dropped = self.dropped.saturating_add(bytes);
    }

    /// How many bytes have arrived, the ones since dropped included.
    fn taken(&self) -> usize {
        self.head
            .len()
            .saturating_add(self.tail.len())
            .saturating_add(self.dropped)
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
        // Bounded by `OUTPUT` however long the command ran, which is what this
        // type exists to guarantee.
        let mut all = self.head.clone();
        all.extend(&self.tail);
        all
    }
}

/// One of a command's output pipes, being read on a thread of its own.
pub(super) struct Pipe {
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
    fn drain(
        pipe: Option<Box<dyn SandboxOutput>>,
        stream: &'static str,
    ) -> Result<Self, ToolError> {
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
        let reader = thread::Builder::new()
            .name(format!("crucible-bash-{stream}"))
            .spawn(move || {
                let mut buffer = [0_u8; 8192];

                loop {
                    if until.load(Ordering::Relaxed) {
                        return Ok(());
                    }

                    let (read, discarded) = match pipe.read_ready(&mut buffer) {
                        // The end of the pipe, and the only way out of here that
                        // [`ended`] is entitled to read as one.
                        Ok(SandboxRead::End) => return Ok(()),
                        Ok(SandboxRead::Bytes(read)) => (read, 0),
                        Ok(SandboxRead::Limited {
                            retained,
                            discarded,
                        }) => (retained, discarded),
                        Ok(SandboxRead::Pending) => {
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
                    kept.discard(discarded);
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

    /// How many lines and bytes have arrived on this pipe.
    ///
    /// For a command nobody is waiting on: the row under the box counts what it
    /// has printed, and there is no result to read the figure off yet.
    pub(super) fn counted(&self) -> (usize, usize) {
        self.kept
            .lock()
            .map(|kept| (kept.lines, kept.taken()))
            .unwrap_or_default()
    }

    /// The end of what has arrived, for the view that stands one whole.
    pub(super) fn text(&self) -> String {
        self.kept
            .lock()
            .map(|kept| String::from_utf8_lossy(&kept.bytes()).into_owned())
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
fn joined(out: &Pipe, err: &Pipe) -> Captured {
    let (mut both, from_out) = out.take();
    let (rest, from_err) = err.take();
    both.extend(rest);

    captured(&both, from_out.saturating_add(from_err))
}

struct Captured {
    text: String,
    original: usize,
    omitted: usize,
}

/// The head and the tail, when there is more than anything can use.
///
/// The middle goes because the two ends carry the meaning: what the command
/// started doing, and how it ended. `already` is what the readers let go before
/// this ever saw it, and it belongs in the same count — one number for the gap,
/// because there is one gap.
#[cfg(test)]
fn cut(text: &str, already: usize) -> String {
    captured(text.as_bytes(), already).text
}

fn captured(bytes: &[u8], already: usize) -> Captured {
    let original = bytes.len().saturating_add(already);
    if already == 0 && bytes.len() <= CAPTURE_TEXT {
        return Captured {
            text: String::from_utf8_lossy(bytes).trim_end().to_owned(),
            original,
            omitted: 0,
        };
    }

    let source = CAPTURE_TEXT.min(bytes.len());
    let head_budget = source / 2;
    let tail_budget = source.saturating_sub(head_budget);
    let (head_end, tail_start) = match std::str::from_utf8(bytes) {
        Ok(text) => (
            boundary_before(text, head_budget),
            boundary(text, text.len().saturating_sub(tail_budget)),
        ),
        Err(_) => (head_budget, bytes.len().saturating_sub(tail_budget)),
    };
    let head = String::from_utf8_lossy(bytes.get(..head_end).unwrap_or_default());
    let tail = String::from_utf8_lossy(bytes.get(tail_start..).unwrap_or_default());
    let kept = head_end.saturating_add(bytes.len().saturating_sub(tail_start));
    let omitted = already.saturating_add(bytes.len().saturating_sub(kept));
    let text = format!(
        "{head}\n\n[process output was {original} bytes; {omitted} bytes omitted from the middle during capture]\n\n{tail}"
    )
    .trim_end()
    .to_owned();

    Captured {
        text,
        original,
        omitted,
    }
}

/// The nearest character boundary at or after `at`, so a cut never lands
/// inside a character.
fn boundary(text: &str, at: usize) -> usize {
    (at..=text.len())
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or(text.len())
}

/// The nearest character boundary at or before `at`.
fn boundary_before(text: &str, at: usize) -> usize {
    (0..=at.min(text.len()))
        .rev()
        .find(|index| text.is_char_boundary(*index))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;
