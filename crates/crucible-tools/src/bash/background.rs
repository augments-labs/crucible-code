//! Commands that go on running after their call has answered.
//!
//! The module the rest of `bash` was written against the absence of. A command
//! used to be ended on every exit path, and [`super::output`] said why: a handle
//! kept would make both the process's lifetime and its resources unbounded. This
//! is what makes keeping one bounded instead — a cap on how many, each one's
//! output held to the same figure a foreground command's is, and every process
//! group ended when this is let go of.
//!
//! **Bound to the run rather than to the session.** `/clear` starts a new session
//! and this is untouched by it, because a running dev server is a fact about the
//! machine rather than about the context — and unlike a forgotten transcript, a
//! killed server cannot be resumed. It is made in the binary, cloned into the
//! tool, and held by the binary for as long as the process lives; the last clone
//! going is what ends every group.
//!
//! **Nothing here consults the cancel.** <kbd>Esc</kbd> stops the turn, and a
//! command somebody deliberately let go of is not part of the turn that started
//! it. The only things that end one are being asked to, and the process leaving.
//!
//! What it cannot promise: a signal that kills crucible outright runs no
//! destructor, so the commands survive it. Catching a signal needs `unsafe`,
//! which this workspace denies, and the shipped documentation says so rather than
//! implying otherwise.

use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::output::Pipe;
use super::platform::Scope;

/// How many commands may be left running at once.
///
/// A server, a watcher and a tunnel with one spare. It is a budget rather than a
/// number picked to be generous: each one holds two reader threads and its own
/// bounded output for as long as it runs, and a fifth call is answered with a
/// refusal naming the four in the way — which the model can act on.
pub const MOST: usize = 4;

/// One command left running, and everything that ends it.
struct Left {
    /// What the panel calls it, in the words the call sent.
    called: Box<str>,
    /// The number the result gave the model, and the panel shows.
    number: usize,
    child: Child,
    scope: Scope,
    /// Still draining, so what it prints goes on being kept and bounded.
    out: Pipe,
    err: Pipe,
    since: Instant,
}

/// One row of what is running, for whatever is drawing it.
///
/// A copy of the facts rather than a borrow of the command: the thread that draws
/// asks between frames and must not hold a lock across one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
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
}

/// One command that ended while nobody was waiting for it.
///
/// Taken once and gone: the reader is told in a line and the model in the note
/// under the next turn, and a fact reported twice would be two servers falling
/// over.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    /// The number it was running as.
    pub number: usize,
    /// The command, as the call sent it.
    pub called: Box<str>,
    /// What it exited with, or `None` where a signal ended it.
    pub code: Option<i32>,
    /// How many lines it printed in total.
    pub lines: usize,
}

/// Everything left running, behind the one lock that owns it.
#[derive(Default)]
struct Held {
    left: Vec<Left>,
    ended: Vec<Ended>,
    counted: usize,
}

impl Drop for Held {
    fn drop(&mut self) {
        // Every group, on the way out. This is the promise the module's prose
        // makes, and the reason the panic strategy for this workspace is unwind:
        // an abort would run none of it and leave the commands behind.
        for left in &mut self.left {
            let _ = super::output::end(&left.scope, &mut left.child);
        }
    }
}

/// Every command left running, shared by the tool that starts them and the
/// binary that draws and ends them.
#[derive(Clone, Default)]
pub struct Background {
    standing: Arc<Mutex<Held>>,
    /// Set by the thread that reads keys and read by the one waiting on a
    /// command. A flag rather than a channel for the reason [`crucible_core`]'s
    /// cancel is one: it is asked about between two twenty-millisecond ticks, and
    /// nothing needs to be delivered.
    asked: Arc<AtomicBool>,
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
    /// Nothing running yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
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

    /// Forgets a request nobody took.
    ///
    /// A press that arrived while no command was running would otherwise be
    /// answered by whichever one started next, which is a command let go of by a
    /// keystroke aimed at something else.
    pub fn forget(&self) {
        self.asked.store(false, Ordering::Relaxed);
    }

    /// Takes a running command, answering with the number it is now known by.
    ///
    /// `None` where the cap is already met, and then the caller still owns the
    /// child — which it ends, because a command nobody can see or stop is the one
    /// outcome this module exists to prevent.
    pub(super) fn keep(&self, called: &str, mut taking: Taking) -> Option<usize> {
        let mut standing = self.standing.lock().ok()?;
        if standing.left.len() >= MOST {
            // Ended here rather than reported and forgotten. The caller has
            // already let go of it, so this is the last code that could, and a
            // command nobody can see or stop is the outcome the cap exists for.
            let _ = super::output::end(&taking.scope, &mut taking.child);
            return None;
        }

        standing.counted = standing.counted.saturating_add(1);
        let number = standing.counted;

        standing.left.push(Left {
            called: called.into(),
            number,
            child: taking.child,
            scope: taking.scope,
            out: taking.out,
            err: taking.err,
            since: taking.since,
        });

        Some(number)
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
                    number: left.number,
                    called: left.called.clone(),
                    running: left.since.elapsed(),
                    lines,
                    bytes,
                }
            })
            .collect()
    }

    /// The end of what a command printed, for the view that stands one whole.
    #[must_use]
    pub fn wrote(&self, number: usize) -> Option<String> {
        let standing = self.standing.lock().ok()?;

        standing
            .left
            .iter()
            .find(|left| left.number == number)
            .map(Left::text)
    }

    /// Ends the command running as `number`.
    ///
    /// Silent about a number nothing answers to: the panel is drawn from a list
    /// that may be a frame old, and a key pressed against a command that has just
    /// exited has got what it asked for.
    pub fn stop(&self, number: usize) {
        let Ok(mut standing) = self.standing.lock() else {
            return;
        };

        if let Some(at) = standing.left.iter().position(|left| left.number == number) {
            let mut left = standing.left.remove(at);
            let _ = super::output::end(&left.scope, &mut left.child);
        }
    }

    /// Reaps whatever has exited on its own, and says which.
    ///
    /// Called on the beat the row above the box already redraws on rather than on
    /// every frame: a command exits once, and asking sixty times a second whether
    /// it has costs four system calls a frame to learn nothing.
    ///
    /// Each one is reported exactly once. What is returned is owed to two
    /// audiences — a line for the reader now, and the note the model is given at
    /// the top of its next turn — so it is taken by the caller that has both.
    pub fn reap(&self) -> Vec<Ended> {
        let Ok(mut standing) = self.standing.lock() else {
            return Vec::new();
        };

        let mut ended = Vec::new();
        let mut still = Vec::with_capacity(standing.left.len());

        for mut left in standing.left.drain(..) {
            match left.child.try_wait() {
                Ok(Some(status)) => {
                    // The shell has gone; its descendants have not necessarily,
                    // and this is the one path where nothing else will end them.
                    let _ = super::output::end(&left.scope, &mut left.child);
                    let (lines, _) = left.counted();

                    ended.push(Ended {
                        number: left.number,
                        called: left.called.clone(),
                        code: status.code(),
                        lines,
                    });
                }
                // Still running, or a wait that could not be made. A command
                // whose status cannot be read is kept rather than reported: it is
                // still holding resources, and `stop` and this module's drop are
                // both still able to end it.
                Ok(None) | Err(_) => still.push(left),
            }
        }

        standing.left = still;
        standing.ended.append(&mut ended.clone());

        ended
    }
}

/// A running command, on its way into the registry.
///
/// Named rather than passed as five arguments, because the ceiling on how many a
/// function takes is there to stop exactly this call from being unreadable — and
/// because the five belong together: they are one command's lifetime.
pub(super) struct Taking {
    pub(super) child: Child,
    pub(super) scope: Scope,
    pub(super) out: Pipe,
    pub(super) err: Pipe,
    pub(super) since: Instant,
}

impl Taking {
    /// What the command printed before it was let go of.
    ///
    /// Bounded and cut the same way an answer is, because that is what it is: the
    /// only part of this command's output the model will be handed unless it asks
    /// for more.
    pub(super) fn printed(&self) -> String {
        let mut said = self.out.text();
        said.push_str(&self.err.text());

        if said.trim().is_empty() {
            return String::from("(no output yet)");
        }

        said.trim_end().to_owned()
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

    /// The end of what it has printed, both streams joined.
    fn text(&self) -> String {
        let mut said = self.out.text();
        said.push_str(&self.err.text());
        said
    }
}
