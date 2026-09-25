//! Commands that go on running after their call has answered.
//!
//! The module the rest of `bash` was written against the absence of. A command
//! used to be ended on every exit path, and [`super::output`] said why: a handle
//! kept would make both the process's lifetime and its resources unbounded. This
//! is what makes keeping one bounded instead — a cap on how many, each one's
//! output held to the same figure a foreground command's is, and every process
//! group asked to end when this is let go of. Failed cleanup keeps its entry
//! while the registry lives, so the panel can retry without losing ownership;
//! when the registry itself is let go, its backstop gives every cell to an
//! owned release task rather than treating a dropped handle as a stop.
//!
//! **Bound to the run rather than to the session.** `/clear` starts a new session
//! and this is untouched by it, because a running dev server is a fact about the
//! machine rather than about the context — and unlike a forgotten transcript, a
//! killed server cannot be resumed. It is made in the binary, cloned into the
//! tool, and held by the binary for as long as the process lives; the last clone
//! going is what requests cleanup for every group.
//!
//! **Owned on the runtime, never on the thread that draws.** Each command kept
//! here is owned by a task of its own on the runtime [`Background::watching_on`]
//! names. Three things ask the command's process anything once it is kept: that
//! task, which looks at its status on every tick, ends what the command left
//! running once it has exited, stops it when a key asks, and ends it when the
//! registry is let go of; an acceptance, which borrows the process for the one
//! call that binds its receipt; and the release task, which asks the one stop
//! that nothing else is left to ask. The complete owner step runs on one
//! blocking thread of the runtime's pool, so a status, publication wait, stop,
//! reap, or reader join never occupies a runtime worker. A stop is asked there
//! as [`crucible_runtime::Bridge::CommandStop`] — one poll, which is the whole
//! bound, because the process contract keeps its bounded stop work inside the
//! contract; a stop that would have had to wait is refused rather than held.
//! Asking again is then the caller's own: a descendant stop, the end a
//! registry being let go asks for, and the release task are asked again from
//! their next attempt — the release task's own interval growing each time —
//! while a stop asked for by a key or by an abandoned result waits for that ask
//! to be made again, its entry standing refused in between. A process whose
//! status look or whose stop stalls therefore holds one blocking thread, never
//! the thread that draws or the runtime's timer driver. The thread that draws
//! only reads what the tasks found, and asks for a stop without waiting for it:
//! the stop's outcome is read on a later frame, the row gone or marked as
//! refused. The registry's lock is never held across a call into a process. A
//! registry that has been named no runtime takes no command.
//!
//! **Nothing here consults the cancel.** <kbd>Esc</kbd> stops the turn, and a
//! command somebody deliberately let go of is not part of the turn that started
//! it. The only things that end one are being asked to, and the process leaving.
//!
//! What it cannot promise by itself: a signal that kills crucible outright runs
//! no destructor. The enforcing Linux backend binds its broker and PID namespace
//! to the host process so that loss still ends the workload; compatibility mode
//! has no equivalent kernel boundary, and the shipped documentation says so
//! rather than implying otherwise. A destructor reached where no runtime is
//! running is the same case in miniature: a stop is a future, so the
//! reservation is given back and the handle is left to the operating system
//! rather than counted as a stop that happened.

use std::future::Future;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LockResult, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use super::output::Pipe;
use crucible_runtime::{BoxFuture, Bridge};
use crucible_sandbox::{SandboxError, SandboxProcess};
use crucible_tools::{CallResultAcceptance, CallResultReceipt};
use tokio::runtime::Handle;
use tokio::task::JoinHandle;

/// How many commands may be left running at once.
///
/// A server, a watcher and a tunnel with one spare. It is a budget rather than a
/// number picked to be generous: each one holds two reader tasks and its own
/// bounded output for as long as it runs, and at most one of the runtime's
/// blocking threads while its owner asks its process something, and a fifth
/// call is answered with a refusal naming the four in the way — which the model
/// can act on.
pub const MOST: usize = 4;

/// How long a command that has ended is given, on the way out, to finish
/// publishing what it wrote.
///
/// Shorter than the wait a call makes, because somebody is waiting for this
/// process to be gone, and what the wait is for may be held by another crucible
/// of this user.
#[cfg(not(test))]
const PUBLICATION: Duration = Duration::from_secs(5);
#[cfg(test)]
const PUBLICATION: Duration = Duration::from_millis(1500);

/// How long a stop is given on the way out, once any publication has had its
/// patience: several times what the in-tree stops take when their bounded
/// steps run to their bounds.
pub(super) const STOPPING: Duration = Duration::from_secs(2);

/// How long one lifecycle handoff may keep a process borrowed before the
/// caller refuses it and hands the process back to its owner. It is a bound on
/// the builtins' wait, not a promise about a backend's future: a future that
/// has already begun is still allowed to finish on its own cleanup task.
#[cfg(not(test))]
pub(super) const ACCEPTANCE: Duration = STOPPING;
#[cfg(test)]
pub(super) const ACCEPTANCE: Duration = Duration::from_millis(500);

/// How long letting the registry go waits for its commands' owners to end
/// them: the patience a command that has ended gets to publish, and then a
/// stop's. An owner not done by then is inside a call into its process that
/// has not come back, or was never given a thread to run on; the registry
/// hands every remaining cell to a bounded release task, and a call that has
/// not come back is left to the runtime's own bounded shutdown.
const LEAVING: Duration = PUBLICATION.saturating_add(STOPPING);

/// How much of what one ended command printed travels in the note about it.
///
/// [`MOST`] commands can end into a single note, so the share rather than the
/// whole: a note carrying every one of them at a result's full ceiling would be
/// four results in one message. Divided this way the note is bounded by the same
/// figure one tool result is, however many ended at once.
const SHARE: usize = crate::bound::OUTPUT / MOST;

/// One command's process, shared by its entry and its owner.
///
/// `None` while one of the three has taken it out to work on it: the owner on
/// every look, an acceptance while it binds its receipt, or a cleanup task
/// while it asks the process to stop. Whoever takes it calls into it with no
/// lock held and puts it back, so neither the registry's lock nor this one is
/// ever held across a call into a process.
type Cell = Arc<Mutex<Option<Box<dyn SandboxProcess>>>>;

/// Takes the process out of `cell`, where nobody else has it.
fn taken(cell: &Cell) -> Option<Box<dyn SandboxProcess>> {
    cell.lock().unwrap_or_else(PoisonError::into_inner).take()
}

/// Puts the process back into `cell`.
///
/// A poisoned cell is still the only owner of this process, so its lock is
/// recovered rather than treating the process as unowned. An occupied cell is
/// not replaced: a second process would be a bug, and dropping that second
/// handle is still safer than silently losing the one the entry already owns.
fn returned(cell: &Cell, process: Box<dyn SandboxProcess>) {
    let mut held = cell.lock().unwrap_or_else(PoisonError::into_inner);
    if held.is_none() {
        *held = Some(process);
    } else {
        drop(held);
        drop(process);
    }
}

/// A process borrowed from a [`Cell`] until its owner has a result for it.
///
/// The guard is the cancellation boundary for every operation made through a
/// loan: a normal return, a failed future, a panic, or a dropped task all put
/// the process back in the cell, and only [`Loan::confirm`] gives up that
/// ownership after a successful stop. The one call on a command's process made
/// without one is the call that begins an acceptance, which runs on the process
/// [`Taking`] still holds on the way in: that is a second boundary, and its
/// `Drop` hands the process to the same release task a cancelled loan leaves it
/// to.
struct Loan {
    cell: Cell,
    process: Option<Box<dyn SandboxProcess>>,
}

impl Loan {
    fn take(cell: &Cell) -> Option<Self> {
        taken(cell).map(|process| Self {
            cell: Arc::clone(cell),
            process: Some(process),
        })
    }

    fn as_mut(&mut self) -> Option<&mut (dyn SandboxProcess + 'static)> {
        self.process.as_deref_mut()
    }

    fn confirm(mut self) {
        drop(self.process.take());
    }
}

impl Drop for Loan {
    fn drop(&mut self) {
        if let Some(process) = self.process.take() {
            returned(&self.cell, process);
        }
    }
}

/// One bounded process operation owned by a blocking task.
///
/// The loan makes the operation's result separable from process ownership: a
/// successful caller receives the process back, while a failed join, panic, or
/// dropped task leaves it in the cell for the cleanup owner to retry.
pub(super) struct ProcessTask<T> {
    cell: Cell,
    join: Option<JoinHandle<io::Result<T>>>,
    completed: bool,
}

impl<T> ProcessTask<T> {
    pub(super) fn start<F>(process: Box<dyn SandboxProcess>, operation: F) -> Self
    where
        F: FnOnce(&mut (dyn SandboxProcess + 'static)) -> io::Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let cell: Cell = Arc::new(Mutex::new(Some(process)));
        let task_cell = Arc::clone(&cell);
        let join = tokio::task::spawn_blocking(move || {
            let Some(mut loan) = Loan::take(&task_cell) else {
                return Err(io::Error::other("the process task found no process"));
            };
            let result = match loan.as_mut() {
                Some(process) => operation(process),
                None => Err(io::Error::other("the process task lost its process")),
            };
            drop(loan);
            result
        });
        Self {
            cell,
            join: Some(join),
            completed: false,
        }
    }

    pub(super) async fn wait(mut self) -> (Option<Box<dyn SandboxProcess>>, io::Result<T>) {
        let Some(join) = self.join.take() else {
            return (None, Err(io::Error::other("the process task had no join")));
        };
        let result = match join.await {
            Ok(result) => result,
            Err(_) => Err(io::Error::other("the process task came apart")),
        };
        self.completed = true;
        (taken(&self.cell), result)
    }
}

impl<T> Drop for ProcessTask<T> {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let cell = Arc::clone(&self.cell);
        schedule_cell_release(Handle::try_current().ok(), cell, None);
    }
}

/// What a command's owner has been asked, by the key that stops one and by an
/// acceptance that was abandoned.
#[derive(Default)]
struct Asks {
    /// A stop has been asked for and not yet taken up.
    stop: AtomicBool,
    /// Its result was abandoned before it could be accepted, and it is to be
    /// ended whether or not it has ended on its own: nothing will report it.
    abandoned: AtomicBool,
}

/// One command left running, and everything that ends it.
struct Left {
    /// What the panel calls it, in the words the call sent.
    called: Box<str>,
    /// The one line the call gave about what it is for, empty where it gave
    /// none. Kept beside the command because the row that reports the ending
    /// is drawn long after the call is gone, and a reader who allowed "watch
    /// the release run" is owed that back rather than the shell it expanded to.
    said: Box<str>,
    /// The number the result gave the model, and the panel shows.
    number: usize,
    /// Its process, which only its owner and an acceptance call into.
    process: Cell,
    /// What its owner has been asked.
    asks: Arc<Asks>,
    /// The task on the runtime that owns it. Taken by the registry's end,
    /// which waits for it.
    owner: Option<JoinHandle<()>>,
    /// Still draining, so what it prints goes on being kept and bounded.
    out: Pipe,
    err: Pipe,
    since: Instant,
    /// Runner finalization has not yet bound the durable result receipt.
    accepting: bool,
    /// The last stop asked for it was refused. Cleared when another is asked.
    refused: bool,
    /// What its owner found once it ended, for [`Background::reap`] to report.
    /// `None` until it has.
    done: Option<Ended>,
}

/// One row of what is running, for whatever is drawing it.
///
/// The tool's name travels beside the command for the reason a [`Summary`] travels
/// beside a [`ToolCall`]: how a row spells a tool is the row's decision, and a
/// name already capitalised here would be this crate deciding it.
///
/// [`Summary`]: crucible_tools::Summary
/// [`ToolCall`]: crucible_types::ToolCall
///
/// A copy of the facts rather than a borrow of the command: the thread that draws
/// asks between frames and must not hold a lock across one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// The name of the tool that started it, as that tool knows it.
    pub tool: &'static str,
    /// The number a call was answered with.
    pub number: usize,
    /// The command, as the call sent it.
    pub called: Box<str>,
    /// How long it has been running.
    pub running: Duration,
    /// How many lines it has printed.
    pub lines: usize,
    /// How many bytes it has printed.
    pub bytes: usize,
    /// The last stop asked for it was refused, and it is still running: its
    /// cleanup failed, and asking again retries it.
    pub refused: bool,
}

/// One command that ended while nobody was waiting for it.
///
/// Taken once and gone: the reader is told in a line and the model in a note,
/// and a fact reported twice would be two servers falling over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    /// The name of the tool that started it, as that tool knows it.
    pub tool: &'static str,
    /// The number it was running as.
    pub number: usize,
    /// The command, as the call sent it.
    pub called: Box<str>,
    /// What the call said it was for, empty where it said nothing.
    pub said: Box<str>,
    /// What it exited with, or `None` where a signal ended it or its ending
    /// went wrong.
    pub code: Option<i32>,
    /// How many lines it printed in total.
    pub lines: usize,
    /// What it printed, bounded and cut the way an answer is, and ending in a
    /// note that it is incomplete where a read of it failed before the end, or
    /// where the ending was reported before every reader had reached the end.
    ///
    /// The whole reason the model is told any of this. A note that a command
    /// ended and never what it said leaves the question the command was
    /// answering still open, and running something else to ask it again is the
    /// only move left — which is the polling the note exists to make
    /// unnecessary.
    pub printed: Box<str>,
    /// Why no successful publication was confirmed, where its ending could not
    /// be completed: most often a root it wrote into changed while it ran.
    pub unpublished: Option<Box<str>>,
}

/// Everything left running, behind the one lock that owns it. Shared with
/// every command's owner, which reads its own entry here and files what it
/// found.
#[derive(Default)]
struct Held {
    left: Vec<Left>,
    ended: Vec<Ended>,
    counted: usize,
    reserved: usize,
    /// Where each command's owner is started.
    runtime: Option<Handle>,
    /// The registry has been let go of, and every owner is to end its command.
    leaving: bool,
}

impl Held {
    /// An abandoned result cannot be accepted again, so its command is asked
    /// to stop. A stop that fails leaves the entry with the registry like any
    /// other background command, so the panel can still retry it.
    fn abandon(&mut self, number: usize) {
        let Some(left) = self
            .left
            .iter_mut()
            .find(|left| left.number == number && left.accepting)
        else {
            return;
        };
        left.accepting = false;
        left.asks.abandoned.store(true, Ordering::Release);
    }
}

/// The registry itself, and what ends every command left running in it once
/// the last holder lets it go.
///
/// Held by the registry's clones and by what a call keeps of it — its lease,
/// its entry and its acceptance — and never by a command's owner, which holds
/// only what is behind it. So the last holder going is never an owner, and an
/// owner is never the one waiting for owners to end.
#[derive(Default)]
struct Registry {
    held: Arc<Mutex<Held>>,
}

impl Registry {
    fn lock(&self) -> LockResult<MutexGuard<'_, Held>> {
        self.held.lock()
    }
}

impl Drop for Registry {
    fn drop(&mut self) {
        // Every owner is told, and ends its command at its next step: one that
        // has ended is let finish publishing what it wrote first, within its
        // patience, because ending it would discard it. The process owner
        // retains any backend quarantine required when cleanup cannot be
        // confirmed. Unwind reaches this; an abort would skip it.
        //
        // Waited for here rather than left to happen, so the stops land on a
        // runtime that is still running: the application shuts its runtime
        // down only after this is let go of. Waited for only so long. An owner
        // not done by then is aborted, which takes it where it waits between
        // steps; a step inside a call into a process that never comes back
        // cannot be stopped by anyone, and the runtime's shutdown reports the
        // thread still inside it.
        let owners: Vec<JoinHandle<()>> = {
            let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            held.leaving = true;
            held.left
                .iter_mut()
                .filter_map(|left| left.owner.take())
                .collect()
        };
        let deadline = Instant::now() + LEAVING;
        while owners.iter().any(|owner| !owner.is_finished()) {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            thread::sleep(super::TICK.min(left));
        }
        for owner in &owners {
            owner.abort();
        }

        // And what no owner reached is given an owned release task here: a
        // command whose owner was never given a thread to run on, or was
        // aborted between its steps, is still in its cell. The task retries a
        // refused stop and keeps the process owned; the dropping thread never
        // waits for a backend's destructor.
        let (runtime, cells) = {
            let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            let cells: Vec<Cell> = held
                .left
                .iter()
                .filter(|left| left.done.is_none())
                .map(|left| Arc::clone(&left.process))
                .collect();
            (held.runtime.clone(), cells)
        };
        for cell in cells {
            schedule_cell_release(runtime.clone(), cell, None);
        }
    }
}

/// Every command left running, shared by the tool that starts them and the
/// binary that draws and ends them.
#[derive(Clone, Default)]
pub struct Background {
    standing: Arc<Registry>,
    /// Set by the thread that reads keys and read by the one waiting on a
    /// command. A flag rather than a channel for the reason
    /// [`crucible_runtime::Cancel`] is one: it is asked about between two
    /// twenty-millisecond ticks, and nothing needs to be delivered.
    asked: Arc<AtomicBool>,
}

/// Registry metadata that travels with one owned process insertion.
#[derive(Clone, Copy)]
pub(super) struct Keep<'a> {
    pub(super) called: &'a str,
    pub(super) said: &'a str,
    pub(super) accepting: bool,
}

// The lint asking a hand-written `Debug` for every field is asking for the one
// thing this may not print: what is behind the lock is a list of command lines,
// and a command line is where a token gets typed by accident. How many are
// running is the whole of what a reader of a `{:?}` needs.
#[allow(clippy::missing_fields_in_debug)]
impl std::fmt::Debug for Background {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Background")
            .field("running", &self.running().len())
            .field("asked", &self.asked.load(Ordering::Relaxed))
            .finish()
    }
}

impl Background {
    /// Nothing running yet, and no runtime to own a command on.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Owns every command kept from here on with a task of its own on
    /// `runtime`, which needs its timer, as the application's has.
    ///
    /// Named after the registry is made rather than when, because the
    /// registry outlives the conversation that first needs a runtime; every
    /// clone shares what is named here. Until a runtime is named, a command
    /// handed over is ended rather than kept.
    pub fn watching_on(&self, runtime: Handle) {
        if let Ok(mut standing) = self.standing.lock() {
            standing.runtime = Some(runtime);
        }
    }

    /// Asks the command being waited on to be left running.
    ///
    /// Answered on the wait loop's next look, which is the same latency a
    /// cancellation has. Nothing happens where no command is running: the flag is
    /// spent by the next call that reads it, and a call that starts a moment later
    /// is a command nobody asked to let go of.
    pub fn ask(&self) {
        self.asked.store(true, Ordering::Relaxed);
    }

    /// Whether letting go has been asked for, spending the request.
    ///
    /// Read and cleared together so two commands cannot both take one press.
    pub fn wanted(&self) -> bool {
        self.asked.swap(false, Ordering::Relaxed)
    }

    /// Reserves application cleanup ownership before a background command's
    /// one-shot release boundary is crossed.
    pub(super) fn reserve(&self) -> Option<Lease> {
        let mut standing = self.standing.lock().ok()?;
        if standing.left.len().saturating_add(standing.reserved) >= MOST {
            return None;
        }
        standing.counted = standing.counted.saturating_add(1);
        let number = standing.counted;
        standing.reserved = standing.reserved.saturating_add(1);
        Some(Lease {
            standing: Arc::clone(&self.standing),
            held: true,
            number,
        })
    }

    /// Whether a command could reserve ownership before its release boundary.
    pub(super) fn available(&self) -> bool {
        self.standing
            .lock()
            .is_ok_and(|standing| standing.left.len().saturating_add(standing.reserved) < MOST)
    }

    /// Takes a running command, answering with the number it is now known by,
    /// and starts the task that owns it from here on.
    ///
    /// `None` where the lease given for it belongs to another registry or has
    /// already been spent, where the cap is already met, where no runtime has
    /// been named to own it on, or where registry ownership is unavailable.
    /// `taking` came in by value, so the caller has already let go of the
    /// child in every one of those, and `keep` is the last code that could act
    /// on it. A refused handover schedules the same bounded release task as a
    /// dropped destructor; it does not turn a failed release into a dropped
    /// handle.
    pub(super) fn keep(&self, mut taking: Taking, plan: Keep<'_>) -> Option<Kept> {
        let Keep {
            called,
            said,
            accepting,
        } = plan;
        let mut lease = taking.lease.take();
        let admitted = match self.standing.lock() {
            Ok(mut standing) => {
                let admission = match (standing.runtime.clone(), lease.as_mut()) {
                    (None, _) => None,
                    (Some(runtime), Some(lease)) => (Arc::ptr_eq(&self.standing, &lease.standing)
                        && lease.consume_locked(&mut standing))
                    .then_some((runtime, lease.number)),
                    (Some(_), None)
                        if standing.left.len().saturating_add(standing.reserved) >= MOST =>
                    {
                        None
                    }
                    (Some(runtime), None) => {
                        standing.counted = standing.counted.saturating_add(1);
                        Some((runtime, standing.counted))
                    }
                };
                match admission {
                    Some((runtime, number)) => {
                        let Some(out) = taking.out.take() else {
                            taking.lease = lease;
                            return None;
                        };
                        let Some(err) = taking.err.take() else {
                            taking.lease = lease;
                            return None;
                        };
                        let Some(process) = taking.process.take() else {
                            taking.lease = lease;
                            return None;
                        };
                        let process: Cell = Arc::new(Mutex::new(Some(process)));
                        let asks = Arc::new(Asks::default());
                        let owner = runtime.spawn(
                            Owner {
                                number,
                                held: Arc::clone(&self.standing.held),
                                process: Arc::clone(&process),
                                asks: Arc::clone(&asks),
                                reaping: Reaping::default(),
                                leaving: None,
                                released: false,
                            }
                            .run(),
                        );
                        standing.left.push(Left {
                            called: called.into(),
                            said: said.into(),
                            number,
                            process,
                            asks,
                            owner: Some(owner),
                            out,
                            err,
                            since: taking.since,
                            accepting,
                            refused: false,
                            done: None,
                        });
                        Ok(Kept {
                            standing: Arc::clone(&self.standing),
                            number,
                            accepting,
                        })
                    }
                    None => Err(taking),
                }
            }
            Err(_) => Err(taking),
        };
        match admitted {
            Ok(kept) => Some(kept),
            Err(mut taking) => {
                taking.lease = lease;
                drop(taking);
                None
            }
        }
    }

    /// How many commands are still running.
    ///
    /// Apart from [`Background::running`] because the row under the box asks this
    /// on every frame and wants only the number: the other one copies a command
    /// line per entry, and copying four of those sixty times a second to draw one
    /// digit is work nobody asked for.
    #[must_use]
    pub fn count(&self) -> usize {
        self.standing
            .lock()
            .map_or(0, |standing| standing.left.len())
    }

    /// What is running, for the row under the box and the panel behind it.
    #[must_use]
    pub fn running(&self) -> Vec<Standing> {
        let Ok(standing) = self.standing.lock() else {
            return Vec::new();
        };

        standing
            .left
            .iter()
            .map(|left| {
                let (lines, bytes) = left.counted();

                Standing {
                    tool: super::NAME,
                    number: left.number,
                    called: left.called.clone(),
                    running: left.since.elapsed(),
                    lines,
                    bytes,
                    refused: left.refused,
                }
            })
            .collect()
    }

    /// The whole of what a command has printed, for the panel that stands one
    /// running: every kept byte, uncut and untrimmed, stream by stream; where
    /// a stream dropped bytes past its own ceiling, the same marker an answer
    /// would carry, at that stream's own hole, naming what it printed and
    /// every byte its reader let go of.
    ///
    /// Built through `output::stood` rather than `output::gathered`,
    /// whose cut is an answer's own ceiling and not this panel's: what is kept
    /// is bounded per stream already, so the panel adds its markers to that
    /// rather than cutting what an answer would have to.
    #[must_use]
    pub fn wrote(&self, number: usize) -> Option<String> {
        let standing = self.standing.lock().ok()?;

        standing
            .left
            .iter()
            .find(|left| left.number == number)
            .map(|left| super::output::stood(&left.out, &left.err))
    }

    /// What a command running as `number` has printed, as `bash_output`'s own
    /// answer: cut to an answer's own ceiling rather than the panel's wider
    /// one, and carrying the counts [`super::output::gathered`] built so the
    /// caller can pass them on to
    /// [`crucible_tools::ToolOutput::with_capture_elision`] — the same way a
    /// finished command's own answer does, so a later limiter pass that must
    /// cut through this call's own marker still has the true count to repeat.
    #[must_use]
    pub(super) fn printed(&self, number: usize) -> Option<super::output::Captured> {
        let standing = self.standing.lock().ok()?;

        standing
            .left
            .iter()
            .find(|left| left.number == number)
            .map(|left| super::output::gathered(&left.out, &left.err, super::output::CAPTURE_TEXT))
    }

    /// How many bytes the command running as `number` holds on to, across
    /// both its pipes.
    #[cfg(test)]
    pub(super) fn retained(&self, number: usize) -> Option<usize> {
        let standing = self.standing.lock().ok()?;
        standing
            .left
            .iter()
            .find(|left| left.number == number)
            .map(|left| left.out.retained().saturating_add(left.err.retained()))
    }

    /// Asks the owner of the command running as `number` to end it, and
    /// returns without waiting for the stop.
    ///
    /// The key that asks is read on the thread that draws, and a stop can take
    /// as long as its backend does. So its outcome is read on a later frame:
    /// a command that was stopped is gone from [`Self::running`], and one whose
    /// stop failed stands there marked [`Standing::refused`], keeping its
    /// entry and capacity for another attempt. Asking again clears that mark
    /// at once and queues one more attempt, which runs after any stop already
    /// under way: the mark comes back only if that attempt fails too.
    ///
    /// Silent about a number nothing answers to: the panel is drawn from a list
    /// that may be a frame old, and a key pressed against a command that has just
    /// exited has got what it asked for. So has one pressed against a command that
    /// has ended and waits to be reported, or waits its turn to publish:
    /// [`Self::reap`] reports it.
    ///
    /// # Errors
    ///
    /// Registry ownership is unavailable.
    pub fn stop(&self, number: usize) -> io::Result<()> {
        let mut standing = self
            .standing
            .lock()
            .map_err(|_| io::Error::other("background registry ownership is unavailable"))?;
        if let Some(left) = standing
            .left
            .iter_mut()
            .find(|left| left.number == number && left.done.is_none())
        {
            left.refused = false;
            left.asks.stop.store(true, Ordering::Release);
        }
        Ok(())
    }

    /// Every command that has ended and not yet been reported to the model.
    ///
    /// Drained, because a note is written fresh from what this hands over and a
    /// fact carried into two of them would be one server falling over twice. The
    /// reader was told when it happened; this is the other audience, and where it
    /// is told depends only on who asks first — a turn in flight reads this
    /// between its passes, and a turn being started reads it under its
    /// instructions. The drain is what makes it exactly one of the two.
    #[must_use]
    pub fn reported(&self) -> Vec<Ended> {
        self.standing
            .lock()
            .map(|mut standing| standing.ended.drain(..).collect())
            .unwrap_or_default()
    }

    /// Takes whatever has ended on its own, and says which.
    ///
    /// Called on the beat the row above the box already redraws on. Every
    /// ending here was found by the command's owner on the runtime: what this
    /// does is take the entries whose owner is done with them, which asks no
    /// process anything, joins no reader and waits for nothing.
    ///
    /// Each one is reported exactly once. What is returned is owed to two
    /// audiences — a line for the reader now, and a note for the model — so it is
    /// taken by the caller that has both.
    pub fn reap(&self) -> Vec<Ended> {
        let (ended, gone) = {
            let Ok(mut standing) = self.standing.lock() else {
                return Vec::new();
            };
            let (mut gone, still): (Vec<Left>, Vec<Left>) = standing
                .left
                .drain(..)
                .partition(|left| left.done.is_some());
            standing.left = still;
            let ended: Vec<Ended> = gone
                .iter_mut()
                .filter_map(|left| left.done.take())
                .collect();
            standing.ended.extend(ended.iter().cloned());
            (ended, gone)
        };
        // Outside the lock, though there is nothing left in them to wait for:
        // their owners have already dropped their processes and joined their
        // readers.
        drop(gone);
        ended
    }
}

/// Where a command's owner stands after one look at it.
enum Step {
    /// Nothing more to do until the next tick.
    Again,
    /// A stop asked for succeeded, and nothing is left running.
    Stopped,
    /// It ended on its own, with this status and this reason nothing it
    /// wrote was published, and what it left running has been ended.
    Ended(Option<i32>, Option<String>),
    /// The registry has been let go of.
    Leaving,
    /// Its entry is gone.
    Gone,
}

/// What the registry says of one command now.
enum Seen {
    /// Its entry, whether its result is still being accepted, and whether
    /// both its readers have reached the end.
    Here { accepting: bool, drained: bool },
    /// The registry has been let go of.
    Leaving,
    /// It has no entry.
    Gone,
}

/// What a command's owner keeps between its looks at it.
#[derive(Default)]
struct Reaping {
    /// When the process was first seen to have gone, for the grace its readers
    /// get to reach the end of its pipes. `None` until it has.
    exited: Option<Instant>,
    /// When what it left running was stopped. `None` until it has; not asked
    /// again once it is `Some`. Read against [`super::output::DRAIN`] again
    /// from here, so a pipe the stop itself lets go of is given the same grace
    /// to show its end that the first wait gave it.
    stopped: Option<Instant>,
    /// The `(code, unpublished)` decided on the look that made `stopped`
    /// `Some`, kept rather than asked of `try_wait` again: stopping a
    /// command's descendants can itself resolve a status that `try_wait` had
    /// not yet reported, and a look that asked again after the stop could read
    /// that new status in place of the one already decided. `None` until
    /// `stopped` is.
    settled: Option<(Option<i32>, Option<String>)>,
    /// When it was first seen to have ended with its publication unfinished.
    /// `None` until it has, and the ceiling its wait gets is counted from it.
    publishing: Option<Instant>,
}

/// Whether an owner has more work after one blocking step.
enum Next {
    /// Another step after the next tick.
    Tick,
    /// The command is over, or no longer this owner's.
    Done,
}

/// The task that owns one command from the moment it is kept.
///
/// The only code that asks the command's process anything from then on,
/// besides an acceptance binding its receipt. Each complete owner step runs on
/// one bounded blocking task, so a status, publication, stop, or reap never
/// occupies a runtime worker. The process loan is returned to the cell unless
/// the step has a confirmed stop.
struct Owner {
    number: usize,
    /// What is behind the registry, and never the registry itself: see
    /// [`Registry`].
    held: Arc<Mutex<Held>>,
    process: Cell,
    asks: Arc<Asks>,
    reaping: Reaping,
    /// When it began ending the command because the registry was let go of.
    leaving: Option<Instant>,
    /// A confirmed stop has already given up the process handle.
    released: bool,
}

impl Owner {
    /// Takes one step per tick until the command is over, or until the
    /// registry is let go of and it has been ended.
    async fn run(mut self) {
        loop {
            let stepped = tokio::task::spawn_blocking(move || {
                let next = self.step();
                (self, next)
            })
            .await;
            let Ok((owner, next)) = stepped else {
                // The owner is dropped inside the failed task. Its drop guard
                // retains the cell and marks the entry for a cleanup retry.
                return;
            };
            self = owner;
            match next {
                Next::Tick => tokio::time::sleep(super::TICK).await,
                Next::Done => return,
            }
        }
    }

    /// One synchronous owner step, run on the blocking pool.
    fn step(&mut self) -> Next {
        match self.look() {
            Step::Again => Next::Tick,
            Step::Stopped => {
                // Dropped here rather than under the lock; dropping it tells
                // its readers to stop.
                self.released = true;
                drop(self.removed());
                Next::Done
            }
            Step::Ended(code, unpublished) => {
                self.report(code, unpublished);
                Next::Done
            }
            Step::Leaving => self.leave(),
            Step::Gone => Next::Done,
        }
    }

    /// What the registry says of this command now.
    fn seen(&self) -> Seen {
        let Ok(held) = self.held.lock() else {
            return Seen::Leaving;
        };
        if held.leaving {
            return Seen::Leaving;
        }
        held.left
            .iter()
            .find(|left| left.number == self.number)
            .map_or(Seen::Gone, |left| Seen::Here {
                accepting: left.accepting,
                drained: left.drained(),
            })
    }

    /// One look: an abandoned result's end, or a stop asked for, first, then,
    /// once its result is accepted, how it is doing. The process is taken out
    /// for the look and put back unless the look has a confirmed stop.
    ///
    /// An empty cell is not a stop. It can mean that an acceptance or a
    /// cleanup task still owns the process; the ask remains set and the next
    /// look gets another chance after that owner returns it.
    fn look(&mut self) -> Step {
        let (accepting, drained) = match self.seen() {
            Seen::Here { accepting, drained } => (accepting, drained),
            Seen::Leaving => {
                return Step::Leaving;
            }
            Seen::Gone => {
                return Step::Gone;
            }
        };
        let abandoned = self.asks.abandoned.load(Ordering::Acquire);
        let stopping = self.asks.stop.load(Ordering::Acquire);
        if accepting && !abandoned && !stopping {
            return Step::Again;
        }
        let Some(mut loan) = Loan::take(&self.process) else {
            return Step::Again;
        };
        let step = if abandoned {
            self.asks.abandoned.store(false, Ordering::Release);
            self.stopping(&mut loan, true)
        } else if stopping {
            self.asks.stop.store(false, Ordering::Release);
            self.stopping(&mut loan, false)
        } else if accepting {
            Step::Again
        } else {
            self.reaping(&mut loan, drained)
        };
        if matches!(step, Step::Stopped | Step::Ended(..) | Step::Gone) {
            loan.confirm();
        }
        step
    }

    /// Stops a command after an abandoned result or an explicit stop ask.
    fn stopping(&self, loan: &mut Loan, abandoned: bool) -> Step {
        let Some(process) = loan.as_mut() else {
            return Step::Again;
        };
        if !abandoned {
            let Ok(ended) = guarded(|| process.ended()) else {
                self.refuse();
                return Step::Again;
            };
            if ended {
                return Step::Again;
            }
        }
        if let Ok(()) = stop_lent(loan) {
            Step::Stopped
        } else {
            self.refuse();
            Step::Again
        }
    }

    /// Whether the command has ended on its own, and what it left running has
    /// been ended.
    fn reaping(&mut self, loan: &mut Loan, drained: bool) -> Step {
        // Asked of `try_wait` fresh on every look until `stopped` is `Some`;
        // from there the pair is read back rather than asked again, because
        // ending a command's descendants can itself resolve a status
        // `try_wait` had not yet reported, and asking again after that would
        // report that new status instead of the one already decided.
        let (code, unpublished) = if let Some(settled) = self.reaping.settled.clone() {
            settled
        } else {
            let Some(process) = loan.as_mut() else {
                return Step::Again;
            };
            let Ok(status) = guarded(|| process.try_wait()) else {
                self.refuse();
                return Step::Again;
            };
            let ended = match &status {
                Ok(Some(_)) => false,
                Ok(None) | Err(_) => {
                    if let Ok(ended) = guarded(|| process.ended()) {
                        ended
                    } else {
                        self.refuse();
                        return Step::Again;
                    }
                }
            };
            match status {
                Ok(Some(status)) => (status.code(), None),
                // From a command that has ended, an error is how its ending
                // went wrong: what it wrote was refused, most often. It is
                // reported like any other ending, with why, rather than kept
                // as though it still ran.
                Err(problem) if ended => (
                    None,
                    Some(super::output::excerpt(&problem.to_string(), SHARE)),
                ),
                // It has ended, and its writes are waiting their turn to
                // publish. Kept rather than stopped, because stopping it
                // discards them — but not for the whole run: what it waits for
                // can be held by another crucible of this user, and a command
                // nothing ever reports holds one of the few slots there are.
                Ok(None) if ended => {
                    let since = *self.reaping.publishing.get_or_insert_with(Instant::now);
                    if since.elapsed() < PUBLICATION {
                        return Step::Again;
                    }
                    (
                        None,
                        Some("its publication did not finish in time".to_owned()),
                    )
                }
                // Still running, or a wait that could not be made. A command
                // whose status cannot be read is kept rather than reported: it
                // is still holding resources, and a stop and the registry's end
                // are both still able to end it.
                Ok(None) | Err(_) => return Step::Again,
            }
        };

        // Ended, but what it printed last may still be in flight: the readers
        // are tasks of their own, and the bytes a command wrote as it died land
        // after the status does. The ending is worth nothing to the model
        // without them, so it is held back a tick at a time.
        let gone = *self.reaping.exited.get_or_insert_with(Instant::now);
        if !drained && gone.elapsed() < super::output::DRAIN {
            return Step::Again;
        }

        // The shell has gone; its descendants have not necessarily, and this
        // is the one path where nothing else will end them. Asked on every
        // look until it succeeds, and not again after that: once it has,
        // nothing is left running for a second ask to stop.
        let stopped = if let Some(when) = self.reaping.stopped {
            when
        } else {
            if stop_lent(loan).is_err() {
                self.refuse();
                return Step::Again;
            }
            let now = Instant::now();
            self.reaping.stopped = Some(now);
            self.reaping.settled = Some((code, unpublished.clone()));
            now
        };

        // Ending descendants above can itself be what lets a held-open pipe
        // reach its end, and its reader still needs a moment to notice and
        // post it. The same grace the first wait gave is given again from here,
        // read afresh, so that moment is actually given rather than judged by a
        // check made before the reader had it.
        let drained = match self.seen() {
            Seen::Here { drained, .. } => drained,
            Seen::Leaving => return Step::Leaving,
            Seen::Gone => return Step::Gone,
        };
        if !drained && stopped.elapsed() < super::output::DRAIN {
            return Step::Again;
        }

        Step::Ended(code, unpublished)
    }

    /// Files the ending for [`Background::reap`] to take.
    ///
    /// Whether both readers reached the end, and how many lines there were,
    /// are read first, because it is the last moment either is true: the
    /// readers are told to stop next, and a reader told to stop reads as
    /// ended whether it reached the end or not. They are joined with the lock
    /// let go, and what they kept is read once they have been.
    fn report(&self, code: Option<i32>, unpublished: Option<String>) {
        let Some((complete, lines, out, err)) = self.with_entry(|left| {
            let complete = left.drained();
            let (lines, _) = left.counted();
            (complete, lines, left.out.release(), left.err.release())
        }) else {
            return;
        };
        // Both, whatever the first says: each join is also the reader's end.
        let out = out.join().is_err();
        let err = err.join().is_err();
        self.with_entry(|left| {
            let printed = left.printed(out || err, complete);
            left.done = Some(Ended {
                tool: super::NAME,
                number: left.number,
                called: left.called.clone(),
                said: left.said.clone(),
                code,
                lines,
                printed: printed.into(),
                unpublished: unpublished.map(Into::into),
            });
        });
    }

    /// Takes this command's entry out of the registry, for a stop that left
    /// nothing running. Dropped by the caller with the lock let go; dropping
    /// it tells its readers to stop.
    fn removed(&self) -> Option<Left> {
        let mut held = self.held.lock().ok()?;
        let at = held
            .left
            .iter()
            .position(|left| left.number == self.number)?;
        Some(held.left.remove(at))
    }

    /// Runs `with` on this command's entry, under the registry's lock.
    fn with_entry<T>(&self, with: impl FnOnce(&mut Left) -> T) -> Option<T> {
        let mut held = self.held.lock().ok()?;
        held.left
            .iter_mut()
            .find(|left| left.number == self.number)
            .map(with)
    }

    /// Records that cleanup could not be confirmed. The entry and its process
    /// stay owned so another stop can retry them.
    fn refuse(&self) {
        self.with_entry(|left| left.refused = true);
    }

    /// Ends the command because the registry has been let go of.
    ///
    /// One that has ended is waiting its turn to publish what it wrote, and
    /// is let finish that first, a tick at a time, because ending it would
    /// discard it. What it waits for is another command's publication, which
    /// ends — but the wait is bounded, because that publication may belong to
    /// another crucible of this user and crucible itself is on its way out.
    /// One whose descendants were already stopped has nothing left to end.
    fn leave(&mut self) -> Next {
        let since = *self.leaving.get_or_insert_with(Instant::now);
        let Some(mut loan) = Loan::take(&self.process) else {
            // The process is still borrowed by an acceptance or another
            // cleanup task. Leaving is a request, not proof that the scope is
            // gone; keep the owner alive to try again.
            return Next::Tick;
        };
        if self.reaping.stopped.is_some() {
            loan.confirm();
            return Next::Done;
        }
        let Some(process) = loan.as_mut() else {
            return Next::Tick;
        };
        let publishing = match guarded(|| process.ended()) {
            Ok(true) => match guarded(|| process.try_wait()) {
                Ok(Ok(None)) => true,
                Ok(_) => false,
                Err(()) => {
                    self.refuse();
                    return Next::Tick;
                }
            },
            Ok(false) => false,
            Err(()) => {
                self.refuse();
                return Next::Tick;
            }
        };
        if publishing && since.elapsed() < PUBLICATION {
            return Next::Tick;
        }
        if let Ok(()) = stop_lent(&mut loan) {
            self.released = true;
            loan.confirm();
            Next::Done
        } else {
            self.refuse();
            Next::Tick
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        let (runtime, cell) = {
            let Ok(mut held) = self.held.lock() else {
                return;
            };
            if held.leaving {
                return;
            }
            let cell = {
                let Some(left) = held.left.iter_mut().find(|left| left.number == self.number)
                else {
                    return;
                };
                if left.done.is_some() {
                    return;
                }
                left.refused = true;
                Arc::clone(&left.process)
            };
            (held.runtime.clone(), cell)
        };
        if let Some(runtime) = runtime {
            schedule_cell_release(Some(runtime), cell, None);
        }
    }
}

/// Runs a potentially blocking process call on the owner step's blocking
/// thread, with a bound. A panic is a failed cleanup attempt, never proof
/// that the process is gone.
fn guarded<T>(call: impl FnOnce() -> T) -> Result<T, ()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(call)).map_err(|_| ())
}

/// Bounds a lifecycle future on the runtime that owns the call.
///
/// On a runtime the wait is timed. With no runtime there is no clock to time it
/// against, so the call is asked once and refused unless it answers on that
/// poll — the rule [`Bridge::cross`] is, which the runner's own result seam does
/// not follow: that one awaits its acceptance, so an executor that has to wait
/// to close its transition is waited for. A future that would have waited is
/// dropped there and the caller is handed the refusal with the process, which
/// is the whole point of the bound: a process is never left borrowed for a wait
/// nothing can end.
pub(super) async fn bounded<F: Future>(future: F, allowed: Duration) -> Result<F::Output, ()> {
    if Handle::try_current().is_err() {
        return Bridge::CommandAcceptance.cross(future).map_err(|_| ());
    }
    tokio::time::timeout(allowed, future).await.map_err(|_| ())
}

/// Asks for one stop, on the thread that already owns this command's process
/// work, and answers what one poll of it says.
///
/// The stop is a future and the thread is one the runtime's blocking pool
/// handed out, so the ask belongs here rather than on a runtime worker: a
/// backend that blocks inside its own contract blocks this one thread, and a
/// timer on the runtime still fires. One poll is the whole bound — the process
/// contract keeps its bounded stop work inside the contract, so the poll
/// answers — and a stop that would have had to wait is refused rather than
/// waited for, so the caller is told only that. Asking again is the caller's
/// own decision: a descendant stop, a registry's own end, and the release task
/// are asked again from their next attempt, and a stop asked for by a key or by
/// an abandoned result waits for that ask to be made again, which is what the
/// ask flag being consumed before this call means. The process stays lent
/// either way: only a confirmed stop, which the caller gives up, ends the
/// ownership.
fn stop_lent(loan: &mut Loan) -> io::Result<()> {
    let Some(process) = loan.as_mut() else {
        return Err(io::Error::other("the process was already released"));
    };
    Bridge::CommandStop
        .cross(super::output::end(process))
        .map_err(|unready| {
            io::Error::other(format!(
                "the stop was asked again rather than held: {unready}"
            ))
        })?
}
pub(super) struct Kept {
    standing: Arc<Registry>,
    number: usize,
    accepting: bool,
}

impl Kept {
    /// Stable number already reported by the application registry.
    pub(super) const fn number(&self) -> usize {
        self.number
    }

    /// Transfers cleanup into runner-owned durable result finalization.
    pub(super) fn acceptance(mut self) -> Option<Box<dyn CallResultAcceptance>> {
        if !self.accepting {
            return None;
        }
        self.accepting = false;
        Some(Box::new(Acceptance {
            standing: Arc::clone(&self.standing),
            number: self.number,
            armed: true,
        }))
    }
}

impl Drop for Kept {
    fn drop(&mut self) {
        if !self.accepting {
            return;
        }
        let Ok(mut standing) = self.standing.lock() else {
            return;
        };
        standing.abandon(self.number);
        self.accepting = false;
    }
}

/// Binds one admitted result to its process. The process is borrowed only for
/// this bounded lifecycle call; cancellation returns it through [`Loan`], and
/// an unanswered call is refused rather than held forever.
struct Acceptance {
    standing: Arc<Registry>,
    number: usize,
    armed: bool,
}

impl CallResultAcceptance for Acceptance {
    fn accept<'a>(
        mut self: Box<Self>,
        receipt: CallResultReceipt,
    ) -> BoxFuture<'a, Result<(), SandboxError>>
    where
        Self: 'a,
    {
        Box::pin(async move {
            let unavailable = || {
                SandboxError::Lifecycle(std::io::Error::other(
                    "background result owner is unavailable",
                ))
            };
            let cell = {
                let standing = self.standing.lock().map_err(|_| {
                    SandboxError::Lifecycle(std::io::Error::other(
                        "background registry ownership is unavailable",
                    ))
                })?;
                standing
                    .left
                    .iter()
                    .find(|left| left.number == self.number && left.accepting)
                    .map(|left| Arc::clone(&left.process))
                    .ok_or_else(unavailable)?
            };
            let Some(mut loan) = Loan::take(&cell) else {
                return Err(unavailable());
            };
            let completed = match loan.as_mut() {
                Some(process) => {
                    match bounded(process.complete_background_acceptance(receipt), ACCEPTANCE).await
                    {
                        Ok(completed) => completed,
                        Err(()) => Err(SandboxError::Lifecycle(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "background result acceptance did not finish within its bound",
                        ))),
                    }
                }
                None => Err(unavailable()),
            };
            drop(loan);
            if let Ok(mut standing) = self.standing.lock()
                && let Some(left) = standing
                    .left
                    .iter_mut()
                    .find(|left| left.number == self.number)
            {
                if completed.is_ok() {
                    left.accepting = false;
                } else {
                    standing.abandon(self.number);
                }
            }
            self.armed = false;
            completed
        })
    }
}

impl Drop for Acceptance {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let Ok(mut standing) = self.standing.lock() else {
            return;
        };
        standing.abandon(self.number);
        self.armed = false;
    }
}

/// One slot owned by the application registry before the workload can start.
pub(super) struct Lease {
    standing: Arc<Registry>,
    held: bool,
    number: usize,
}
impl Lease {
    /// Stable registry number reserved before the workload crosses GO.
    pub(super) const fn number(&self) -> usize {
        self.number
    }

    fn consume_locked(&mut self, standing: &mut Held) -> bool {
        if !self.held || standing.reserved == 0 {
            return false;
        }
        standing.reserved = standing.reserved.saturating_sub(1);
        self.held = false;
        true
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        if !self.held {
            return;
        }
        if let Ok(mut standing) = self.standing.lock() {
            standing.reserved = standing.reserved.saturating_sub(1);
            self.held = false;
        }
    }
}

/// Takes over cleanup for a process that has not yet been admitted to a live
/// registry entry. The task retries a failed stop and keeps any reservation
/// until a stop is confirmed; the dropping thread never waits for it.
pub(super) fn release_process(process: Box<dyn SandboxProcess>, lease: Option<Lease>) {
    let cell: Cell = Arc::new(Mutex::new(Some(process)));
    schedule_cell_release(Handle::try_current().ok(), cell, lease);
}

/// Schedules the bounded release owner for one cell. A cell can be empty while
/// an acceptance or an earlier blocking step still owns its process, so the
/// task waits for the cell rather than treating that gap as a stop.
///
/// For a command the registry kept, the stop it asks is given the one blocking
/// thread [`MOST`] already reserves for that command's owner step, the owner
/// this task takes over from, so it is that same reservation seen from a later
/// step and not a second one against the runtime's counted blocking threads. A
/// foreground command is not that case: it was never kept, so this shares no
/// reservation, and it outlives the tool call that made it, which leaves it
/// bounded by neither that step nor the turn's tool runs. Nothing here caps how
/// many such tasks are live; that retry demand is outside the runtime's
/// counted owners, and what it costs is that a stop is asked later — each ask
/// is awaited, so a saturated pool queues it holding no thread, and no stop is
/// lost, confirmed without its cleanup, or counted as capacity released.
///
/// A stop this process's backend keeps refusing is asked again with the whole
/// patience a stop is given on the way out, [`STOPPING`], between each pair of
/// asks, and never sooner: the first refusal costs a tick and each one after it
/// doubles that wait, so a backend that cannot stop costs a blocking thread once
/// every [`STOPPING`] at worst rather than once a tick. A cell nobody is holding
/// yet is not a refusal and is still waited for at the tick, because that gap is
/// another owner handing the process back and not a backend declining.
fn schedule_cell_release(runtime: Option<Handle>, cell: Cell, lease: Option<Lease>) {
    let Some(runtime) = runtime else {
        // A process reaches this path from a destructor or from a caller still
        // constructing its runtime. A stop is a future, and with no runtime
        // there is nothing to answer one, so the reservation is given back and
        // the handle is left with the operating system. That is the one thing
        // this cannot do twice: it is not a stop, nothing records one, and the
        // process contract is what a stop would have asked for.
        drop(cell);
        drop(lease);
        return;
    };
    runtime.spawn(async move {
        let lease = lease;
        let mut patience = super::TICK;
        loop {
            let Some(loan) = Loan::take(&cell) else {
                tokio::time::sleep(super::TICK).await;
                continue;
            };
            // This stop is asked on the blocking pool, taking the place the
            // doc above names: a kept command's own owner reservation, or, for
            // a foreground one, no reservation at all.
            let (returned, stopped) = stop_loan(loan).await;
            if let Some(loan) = returned {
                if stopped.is_ok() {
                    loan.confirm();
                    drop(lease);
                    return;
                }
                drop(loan);
            }
            tokio::time::sleep(patience).await;
            patience = patience.saturating_mul(2).min(STOPPING);
        }
    });
}

/// Runs one stop on the blocking pool while keeping the loan alive if the
/// blocking task is cancelled or comes apart.
async fn stop_loan(loan: Loan) -> (Option<Loan>, io::Result<()>) {
    match tokio::task::spawn_blocking(move || {
        let mut loan = loan;
        let result = if loan.as_mut().is_some() {
            stop_lent(&mut loan)
        } else {
            Err(io::Error::other("the process loan was already released"))
        };
        (loan, result)
    })
    .await
    {
        Ok((loan, result)) => (Some(loan), result),
        Err(_) => (
            None,
            Err(io::Error::other("the process cleanup task came apart")),
        ),
    }
}

/// A running command, on its way into the registry.
///
/// Named rather than passed as five arguments, because the ceiling on how many a
/// function takes is there to stop exactly this call from being unreadable — and
/// because they belong together: they are one command's lifetime, and how it
/// came to have one.
pub(super) struct Taking {
    pub(super) process: Option<Box<dyn SandboxProcess>>,
    pub(super) out: Option<Pipe>,
    pub(super) err: Option<Pipe>,
    pub(super) since: Instant,
    /// The reservation travels with the process until the registry takes both
    /// or a cleanup task confirms their release.
    pub(super) lease: Option<Lease>,
    /// Which of the two ways in let go of it, which is the one thing about a
    /// command left running that the model reads differently depending on the
    /// answer.
    pub(super) why: super::output::Why,
}

impl Drop for Taking {
    fn drop(&mut self) {
        let Some(process) = self.process.take() else {
            return;
        };
        release_process(process, self.lease.take());
    }
}

impl Taking {
    /// What the command printed before it was let go of.
    ///
    /// Bounded and cut the same way an answer is, because that is what it is:
    /// the only part of this command's output the model will be handed unless
    /// it asks for more. Built the way `output::joined` builds an
    /// answer's own text — bytes and dropped count together — so a command
    /// that printed more than a stream's head and tail before it was let go of
    /// says where the rest went instead of splicing over it in silence, and so
    /// the caller can carry that count on to
    /// [`crucible_tools::ToolOutput::with_capture_elision`] the same way a
    /// finished command's own answer does.
    pub(super) fn printed(&self) -> super::output::Captured {
        let (Some(out), Some(err)) = (&self.out, &self.err) else {
            return super::output::Captured {
                text: String::from("(no output yet)"),
                original: 0,
                omitted: 0,
            };
        };
        let said = super::output::gathered(out, err, super::output::CAPTURE_TEXT);

        if said.text.trim().is_empty() {
            return super::output::Captured {
                text: String::from("(no output yet)"),
                original: said.original,
                omitted: said.omitted,
            };
        }

        said
    }
}

impl Left {
    /// How many lines and how many bytes it has printed.
    fn counted(&self) -> (usize, usize) {
        let (out_lines, out_bytes) = self.out.counted();
        let (err_lines, err_bytes) = self.err.counted();

        (
            out_lines.saturating_add(err_lines),
            out_bytes.saturating_add(err_bytes),
        )
    }

    /// Whether both readers have reached the end of their pipes.
    fn drained(&self) -> bool {
        self.out.ended() && self.err.ended()
    }

    /// What it printed, for the note about its ending: bounded and cut the way
    /// an answer is, and saying so where `unread` says a reader failed before
    /// the end, or where `complete` says a reader had still not reached it.
    ///
    /// A reader that failed stops the way one that reached the end does, so
    /// what it kept is a prefix that looks whole; joining it is what tells the
    /// two apart. A reader still short of the end when its owner stopped
    /// waiting is told apart only by `complete`: releasing a reader stops it,
    /// after which it reads as ended like any other, so `complete` is read
    /// before its readers are released; see `Owner::report`. The read
    /// failure's own words are not carried — only that there was one, in a
    /// fixed phrase — as a foreground command's error names what could not be
    /// done and not what the operating system said.
    ///
    /// Built through [`super::output::gathered`] rather than
    /// [`super::output::excerpt`], so what is cut here counts every byte a
    /// reader ever let go — not only the slice this cut removed on top of that
    /// — the same gap `joined` reports for a command somebody waited for.
    fn printed(&self, unread: bool, complete: bool) -> String {
        if unread {
            return noted(&self.out, &self.err, Note::Unread);
        }
        if !complete {
            return noted(&self.out, &self.err, Note::Undrained);
        }
        super::output::gathered(&self.out, &self.err, SHARE).text
    }
}

/// Why a command's kept output stops short of the whole, for [`noted`].
///
/// An enum rather than the marker's own text, so a marker `noted` is asked to
/// append is always one the const assertions below have measured against
/// [`SHARE`]: passing a string neither covers is a case the type cannot
/// express, not a bound this subtraction could still get wrong.
#[derive(Clone, Copy)]
enum Note {
    /// A reader failed before reaching the end.
    Unread,
    /// A reader had still not reached the end once every hold on reporting
    /// this ending had run out.
    Undrained,
}

impl Note {
    const fn marker(self) -> &'static str {
        match self {
            Self::Unread => UNREAD,
            Self::Undrained => UNDRAINED,
        }
    }
}

/// What was kept, with the marker `note` names appended: how a reader's
/// failure, or reap no longer waiting for a reader, is said.
///
/// Shared by both notes so the one gap between "what was printed" and "why it
/// stops short" is measured once, whichever note it is.
fn noted(out: &Pipe, err: &Pipe, note: Note) -> String {
    let marker = note.marker();
    let kept = super::output::gathered(out, err, SHARE - marker.len() - BEFORE_NOTE.len()).text;
    if kept.is_empty() {
        marker.to_owned()
    } else {
        format!("{kept}{BEFORE_NOTE}{marker}")
    }
}

/// What the note says of a command whose output a read failed on before the
/// end.
///
/// Its room, and the blank line before it, come out of the share rather than
/// being added to it, so a failed read makes the note no longer than
/// `gathered(.., SHARE)` could already have made it.
const UNREAD: &str = "[output is incomplete: reading it failed before the end]";

/// What the note says of a command whose readers had still not reached the
/// end of its pipes once every hold on reporting its ending had run out.
///
/// Nothing failed here: either something that outlived the stop still holds a
/// pipe open, or the reader had not caught up by the time the second hold
/// its owner gives it also ran out. Either way what was kept is a
/// prefix, not the whole.
const UNDRAINED: &str =
    "[output is incomplete: it had not been read to the end when the command was reported]";

/// What parts a note from what was printed.
const BEFORE_NOTE: &str = "\n\n";

// `noted` relies on both to leave a share it can still subtract from without
// underflowing, and a share too small to leave any output beside a note would
// be a note about nothing.
const _: () = assert!(UNREAD.len() + BEFORE_NOTE.len() < SHARE);
const _: () = assert!(UNDRAINED.len() + BEFORE_NOTE.len() < SHARE);

#[cfg(test)]
mod tests;
