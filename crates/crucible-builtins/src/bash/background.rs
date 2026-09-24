//! Commands that go on running after their call has answered.
//!
//! The module the rest of `bash` was written against the absence of. A command
//! used to be ended on every exit path, and [`super::output`] said why: a handle
//! kept would make both the process's lifetime and its resources unbounded. This
//! is what makes keeping one bounded instead — a cap on how many, each one's
//! output held to the same figure a foreground command's is, and every process
//! group asked to end when this is let go of. Failed cleanup keeps its entry
//! while the registry lives, so the panel can retry without losing ownership.
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
//! names. That task is the only code that asks the command's process anything
//! once it is kept, besides an acceptance binding its receipt, which borrows the
//! process for that one call: it looks at its status on every tick, ends what
//! the command left running once it has exited, stops it when a key asks, and
//! ends it when the registry is let go of. Every one of those steps is
//! synchronous inside today's backends — a stop reaps and rolls back, a status
//! can publish — so each runs on the runtime's blocking threads, one at a time
//! for each command, and the task itself only waits between them. A process
//! whose backend stops answering therefore holds one blocking thread, never a
//! thread that polls the runtime's tasks or drives its timer, so a sandbox's
//! limit kill and status go on. The thread that draws only reads what the
//! tasks found, and asks for a stop without waiting for it: the stop's outcome
//! is read on a later frame, the row gone or marked as refused. The registry's
//! lock is never held across a call into a process. A registry that has been
//! named no runtime takes no command.
//!
//! **Nothing here consults the cancel.** <kbd>Esc</kbd> stops the turn, and a
//! command somebody deliberately let go of is not part of the turn that started
//! it. The only things that end one are being asked to, and the process leaving.
//!
//! What it cannot promise by itself: a signal that kills crucible outright runs
//! no destructor. The enforcing Linux backend binds its broker and PID namespace
//! to the host process so that loss still ends the workload; compatibility mode
//! has no equivalent kernel boundary, and the shipped documentation says so
//! rather than implying otherwise.

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
const STOPPING: Duration = Duration::from_secs(2);

/// How long letting the registry go waits for its commands' owners to end
/// them: the patience a command that has ended gets to publish, and then a
/// stop's. An owner not done by then is inside a call into its process that
/// has not come back, or was never given a thread to run on; the registry
/// ends what it can reach itself, and a call that has not come back is left
/// to the runtime's own bounded shutdown.
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
/// `None` while one of the two has taken it out to work on it: the owner on
/// every look, and an acceptance while it binds its receipt. Whoever takes it
/// calls into it with no lock held and puts it back, so neither the registry's
/// lock nor this one is ever held across a call into a process.
type Cell = Arc<Mutex<Option<Box<dyn SandboxProcess>>>>;

/// Takes the process out of `cell`, where nobody else has it.
fn taken(cell: &Cell) -> Option<Box<dyn SandboxProcess>> {
    cell.lock().ok()?.take()
}

/// Puts the process back into `cell`. Dropped instead where the cell's lock
/// is poisoned, which ends it the way every sandbox this ships ends a process
/// it drops.
fn returned(cell: &Cell, process: Box<dyn SandboxProcess>) {
    if let Ok(mut held) = cell.lock() {
        *held = Some(process);
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

        // And what no owner reached is ended here: a command whose owner was
        // never given a thread to run on, or was aborted between its steps,
        // is still in its cell. Taken out under the lock and ended outside
        // it, on this thread, each stop bounded as the backend bounds it.
        let unreached: Vec<Box<dyn SandboxProcess>> = {
            let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
            held.left
                .iter()
                .filter_map(|left| taken(&left.process))
                .collect()
        };
        for mut process in unreached {
            let _ = super::output::end(process.as_mut());
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
pub(super) struct Keep<'a> {
    pub(super) called: &'a str,
    pub(super) said: &'a str,
    pub(super) lease: Option<Lease>,
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
    /// on it. On every branch but the last it ends the child itself before
    /// answering, once the registry's lock is let go, because a command nobody
    /// can see or stop is the one outcome this module exists to prevent. On
    /// the lock-failure branch nothing calls `end`; the child ends only because
    /// dropping `taking` does, true of every sandbox this ships, though the
    /// contract behind it promises no such thing.
    pub(super) fn keep(&self, mut taking: Taking, plan: Keep<'_>) -> Option<Kept> {
        let Keep {
            called,
            said,
            mut lease,
            accepting,
        } = plan;
        let mut standing = self.standing.lock().ok()?;
        let admitted = match (standing.runtime.clone(), lease.as_mut()) {
            (None, _) => None,
            (Some(runtime), Some(lease)) => (Arc::ptr_eq(&self.standing, &lease.standing)
                && lease.consume_locked(&mut standing))
            .then_some((runtime, lease.number)),
            (Some(_), None) if standing.left.len().saturating_add(standing.reserved) >= MOST => {
                None
            }
            (Some(runtime), None) => {
                standing.counted = standing.counted.saturating_add(1);
                Some((runtime, standing.counted))
            }
        };
        let Some((runtime, number)) = admitted else {
            // Ended here rather than reported and forgotten. The caller has
            // already let go of it, so this is the last code that could, and a
            // command nobody can see or stop is the outcome the cap exists for.
            drop(standing);
            let _ = super::output::end(taking.process.as_mut());
            return None;
        };

        let process: Cell = Arc::new(Mutex::new(Some(taking.process)));
        let asks = Arc::new(Asks::default());
        let owner = runtime.spawn(
            Owner {
                number,
                held: Arc::clone(&self.standing.held),
                process: Arc::clone(&process),
                asks: Arc::clone(&asks),
                reaping: Reaping::default(),
                leaving: None,
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
            out: taking.out,
            err: taking.err,
            since: taking.since,
            accepting,
            refused: false,
            done: None,
        });

        Some(Kept {
            standing: Arc::clone(&self.standing),
            number,
            accepting,
        })
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

/// The task that owns one command from the moment it is kept.
///
/// The only code that asks the command's process anything from then on,
/// besides an acceptance binding its receipt. Each of its steps is one
/// synchronous look, run on the runtime's blocking threads and awaited, so a
/// status that takes a publication's turn to resolve, or a stop that reaps
/// and rolls back, holds up this command's step and neither a thread that
/// polls the runtime's tasks nor the thread that draws.
struct Owner {
    number: usize,
    /// What is behind the registry, and never the registry itself: see
    /// [`Registry`].
    held: Arc<Mutex<Held>>,
    process: Cell,
    asks: Arc<Asks>,
    reaping: Reaping,
    /// When it began ending the command because the registry was let go of.
    /// `None` until it has.
    leaving: Option<Instant>,
}

/// Whether an owner has more to do.
enum Next {
    /// Another step after the next tick.
    Tick,
    /// The command is over, or no longer this owner's.
    Done,
}

impl Owner {
    /// Takes one step on the blocking threads per tick until the command is
    /// over, or until the registry is let go of and it has been ended.
    ///
    /// The owner travels into each step and back out of it, so nothing of it
    /// is shared with the step. A step the runtime would not run, because it
    /// is shutting down, ends the owner there: what it did not reach is left
    /// in its cell, where the registry's own end finds it.
    async fn run(mut self) {
        loop {
            let stepped = tokio::task::spawn_blocking(move || {
                let next = self.step();
                (self, next)
            })
            .await;
            let Ok((owner, next)) = stepped else {
                return;
            };
            self = owner;
            match next {
                Next::Tick => tokio::time::sleep(super::TICK).await,
                Next::Done => return,
            }
        }
    }

    /// One synchronous step, on a blocking thread.
    fn step(&mut self) -> Next {
        if self.leaving.is_some() {
            return self.leave();
        }
        match self.look() {
            Step::Again => Next::Tick,
            Step::Stopped => {
                // Dropped here rather than under the lock; dropping it tells
                // its readers to stop.
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
    /// for the look and put back unless the look is its last.
    ///
    /// While its result is still being accepted, and nothing has asked for it
    /// to end, the process is left in its cell: that is where the acceptance
    /// borrows it from, and an acceptance that found it gone would fail and
    /// end the command.
    fn look(&mut self) -> Step {
        let (accepting, drained) = match self.seen() {
            Seen::Here { accepting, drained } => (accepting, drained),
            Seen::Leaving => return Step::Leaving,
            Seen::Gone => return Step::Gone,
        };
        let asked =
            self.asks.abandoned.load(Ordering::Acquire) || self.asks.stop.load(Ordering::Acquire);
        if accepting && !asked {
            return Step::Again;
        }
        // Lent to an acceptance binding its receipt; looked at next tick.
        let Some(mut process) = taken(&self.process) else {
            return Step::Again;
        };
        let step = if self.asks.abandoned.swap(false, Ordering::AcqRel) {
            // A stop that fails leaves it with the registry like any other.
            if super::output::end(process.as_mut()).is_ok() {
                Step::Stopped
            } else {
                Step::Again
            }
        } else if self.asks.stop.swap(false, Ordering::AcqRel) {
            self.stopping(process.as_mut())
        } else if accepting {
            Step::Again
        } else {
            self.reaping(process.as_mut(), drained)
        };
        match step {
            Step::Again | Step::Leaving => returned(&self.process, process),
            Step::Stopped | Step::Ended(..) | Step::Gone => drop(process),
        }
        step
    }

    /// Stops the command, as a key asked.
    ///
    /// One that has ended is waiting to be reported, or waiting its turn to
    /// publish what it wrote. Stopping it could cut short that publication, so
    /// it is left to the ending. A stop that fails is marked on its entry, for
    /// the panel to say so.
    fn stopping(&mut self, process: &mut (dyn SandboxProcess + 'static)) -> Step {
        if process.ended() {
            return Step::Again;
        }
        if super::output::end(process).is_ok() {
            return Step::Stopped;
        }
        self.with_entry(|left| left.refused = true);
        Step::Again
    }

    /// Whether the command has ended on its own, and what it left running has
    /// been ended.
    fn reaping(&mut self, process: &mut (dyn SandboxProcess + 'static), drained: bool) -> Step {
        // Asked of `try_wait` fresh on every look until `stopped` is `Some`;
        // from there the pair is read back rather than asked again, because
        // ending a command's descendants can itself resolve a status
        // `try_wait` had not yet reported, and asking again after that would
        // report that new status instead of the one already decided.
        let (code, unpublished) = if let Some(settled) = self.reaping.settled.clone() {
            settled
        } else {
            match process.try_wait() {
                Ok(Some(status)) => (status.code(), None),
                // From a command that has ended, an error is how its ending went
                // wrong: what it wrote was refused, most often. It is reported like
                // any other ending, with why, rather than kept as though it still
                // ran.
                Err(problem) if process.ended() => (
                    None,
                    Some(super::output::excerpt(&problem.to_string(), SHARE)),
                ),
                // It has ended, and its writes are waiting their turn to
                // publish. Kept rather than stopped, because stopping it
                // discards them — but not for the whole run: what it waits for
                // can be held by another crucible of this user, and a command
                // nothing ever reports holds one of the few slots there are.
                Ok(None) if process.ended() => {
                    let since = *self.reaping.publishing.get_or_insert_with(Instant::now);
                    if since.elapsed() < PUBLICATION {
                        return Step::Again;
                    }
                    (
                        None,
                        Some("its publication did not finish in time".to_owned()),
                    )
                }
                // Still running, or a wait that could not be made. A command whose
                // status cannot be read is kept rather than reported: it is still
                // holding resources, and a stop and the registry's end are both
                // still able to end it.
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
            if super::output::end(process).is_err() {
                return Step::Again;
            }
            let now = Instant::now();
            self.reaping.stopped = Some(now);
            self.reaping.settled = Some((code, unpublished.clone()));
            now
        };

        // Ending descendants above can itself be what lets a held-open pipe
        // reach its end, and its reader still needs a moment to notice and
        // post it. The same grace the first wait gave is given again from
        // here, read afresh, so that moment is actually given rather than
        // judged by a check made before the reader had it.
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

    /// One step of ending the command because the registry has been let go
    /// of.
    ///
    /// One that has ended is waiting its turn to publish what it wrote, and is
    /// let finish that first, a tick at a time, because ending it would
    /// discard it. What it waits for is another command's publication, which
    /// ends — but the wait is bounded, because that publication may belong to
    /// another crucible of this user and crucible itself is on its way out.
    /// One whose descendants were already stopped has nothing left to end.
    fn leave(&mut self) -> Next {
        let since = *self.leaving.get_or_insert_with(Instant::now);
        let Some(mut process) = taken(&self.process) else {
            return Next::Done;
        };
        if self.reaping.stopped.is_some() {
            return Next::Done;
        }
        if process.ended()
            && matches!(process.try_wait(), Ok(None))
            && since.elapsed() < PUBLICATION
        {
            returned(&self.process, process);
            return Next::Tick;
        }
        let _ = super::output::end(process.as_mut());
        Next::Done
    }
}

/// A command installed under its stable application-owned identity.
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

/// Stops a command dropped before runner finalization binds its receipt. A
/// binding that fails or would have had to wait disarms it instead, leaving the
/// command with the registry like any other background command.
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
            // Borrowed from its owner for the one call, with the registry
            // unlocked: binding waits, synchronously, and on Linux the in-tree
            // backend writes a durable record there and stops the process
            // when that record fails. Crossed rather than awaited until the
            // runner awaits a background result's acceptance.
            let mut process = taken(&cell).ok_or_else(unavailable)?;
            let completed = Bridge::BashSandbox
                .cross(process.complete_background_acceptance(receipt))
                .unwrap_or_else(|unready| {
                    Err(SandboxError::Lifecycle(std::io::Error::other(unready)))
                });
            returned(&cell, process);
            if let Ok(mut standing) = self.standing.lock()
                && let Some(left) = standing
                    .left
                    .iter_mut()
                    .find(|left| left.number == self.number)
            {
                left.accepting = false;
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

/// A running command, on its way into the registry.
///
/// Named rather than passed as five arguments, because the ceiling on how many a
/// function takes is there to stop exactly this call from being unreadable — and
/// because they belong together: they are one command's lifetime, and how it
/// came to have one.
pub(super) struct Taking {
    pub(super) process: Box<dyn SandboxProcess>,
    pub(super) out: Pipe,
    pub(super) err: Pipe,
    pub(super) since: Instant,
    /// Which of the two ways in let go of it, which is the one thing about a
    /// command left running that the model reads differently depending on the
    /// answer.
    pub(super) why: super::output::Why,
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
        let said = super::output::gathered(&self.out, &self.err, super::output::CAPTURE_TEXT);

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
