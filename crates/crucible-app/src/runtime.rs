//! The one runtime the application owns: built the first time something asks
//! for it, and shut down within a bound once the run is over.
//!
//! No crate below this one builds a runtime. A library that did would decide
//! for the application how many threads it gets and when they stop; instead a
//! library is handed a [`Handle`] to this one, through [`crate::services`], and
//! runs what it owns there.
//!
//! **Built on first use.** [`RuntimeOwner::handle`] builds the runtime the
//! first time it is called and hands back the same runtime's handle every time
//! after. A path that never asks — `--help` and `--version`, which end while
//! the arguments are parsed, and the listings that print and stop — starts no
//! thread for it. Assembling a conversation asks, because its turns are
//! waited for on it, every command its sandbox starts is watched there,
//! including the kill of one that breaks its time or output limit, and an
//! account's tokens are renewed there.
//!
//! **Multi-thread, because a waiting caller does not drive the runtime.** A
//! synchronous caller waits for a future by polling it on its own thread,
//! entered into this runtime (see `crucible_runtime::Bridge::wait`), which is
//! the shape of `Handle::block_on`. Tokio's own documentation of that method,
//! at `src/runtime/handle.rs:248-253` in the pinned 1.53.1 source, says that on
//! a `current_thread` runtime only `Runtime::block_on` can drive the IO and
//! timer drivers and `Handle::block_on` cannot, so anything relying on IO or
//! timers does not work unless another thread is inside `Runtime::block_on` on
//! the same runtime. A current-thread runtime would therefore leave every
//! future a caller waits on unwoken by the timer it waits for. The workers of
//! a multi-thread runtime run those drivers themselves, whoever is waiting.
//!
//! **A timer and an I/O driver.** The timer is what every timed wait on this
//! runtime is measured against. The I/O driver is what a hosted program's
//! pipes and an account request's sockets are waited on through: the tasks
//! that read and write the pipes run here, and on Unix the local backend
//! registers each pipe with the driver of the runtime polling it; account
//! requests put their sockets here too — a renewal as a task of its own, a
//! login's requests in the future its thread waits on. Its drivers are fixed
//! when it is built, and one built without I/O would fail the first of those
//! tasks, saying I/O is disabled.
//!
//! **Bounded threads.** [`WORKERS`] threads poll the tasks spawned onto the
//! runtime, and at most [`BLOCKING`] more run blocking work handed to it; both
//! are fixed rather than read off the machine, so a limit written against the
//! worker count elsewhere is written against a number that does not move.
//! Work handed to the blocking threads beyond that queues inside the runtime,
//! so whoever hands it over is who bounds how much.
//!
//! **A bounded shutdown that says when it ran out.** Once the run is over,
//! [`crate::services::serving`] shuts the runtime down, which drops every task
//! at its next await point and gives the runtime's threads [`SHUTDOWN`] to
//! stop. A thread still inside synchronous work by then cannot
//! be stopped by anyone, so it is left, and the shutdown answers
//! [`Unstopped`], a failed cleanup, rather than success or a hang. A runtime
//! owner dropped without being shut down — a run unwinding — gives its threads
//! the same bound and has nobody left to report to.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::runtime::{Builder, Handle, Runtime};

/// How many threads poll the tasks spawned onto the runtime.
///
/// A turn is not one of them: it is polled on the thread that takes it. What
/// runs here is work the application owns on the turn's behalf and between
/// turns — a process's status, a hosted program's streams, a credential's
/// renewal — each of which spends most of its life waiting. Four keeps a task
/// that holds a worker inside synchronous work from stopping the rest, and
/// costs four idle threads.
pub const WORKERS: usize = 4;

/// The most threads the runtime starts for blocking work handed to it.
///
/// Blocking work is disk and platform calls, the web source's requests,
/// account renewals' lock, file and lookup work, and the calls into a command
/// left running, which have no asynchronous form, each bounded by its owner.
/// What every owner may hold at once is checked against this in
/// [`crate::services`]: the tool worker's four jobs, the web source's two
/// requests and account work's two threads, each counting work it gave up on
/// that is still running, and one step at a time for each of the four commands
/// that may be left running, whose owner makes every call into its process
/// here; and one thread to spare, so an owner at its most never makes
/// another's job queue.
pub const BLOCKING: usize = 13;

/// How long the runtime's threads are given to stop once it is shut down.
///
/// By the time the runtime is shut down, everything that owned work on it has
/// been dropped and has asked that work to stop, so a task still running is
/// one inside synchronous work that no wait will end. Two seconds lets work
/// finishing its last step finish it, and keeps a stuck one from holding the
/// exit.
pub const SHUTDOWN: Duration = Duration::from_secs(2);

/// What the runtime's threads are called, so a reader of a thread listing can
/// tell whose they are.
const NAME: &str = "crucible-runtime";

/// The application's runtime, built the first time it is asked for.
pub struct RuntimeOwner {
    built: Mutex<Option<Built>>,
}

/// A runtime that has been built, and a count of its threads still running.
struct Built {
    runtime: Runtime,
    /// Raised by each thread as it starts and lowered as it stops, so a
    /// shutdown can tell whether its bound ran out with threads still inside
    /// work: the runtime itself does not say.
    running: Arc<AtomicUsize>,
}

impl std::fmt::Debug for RuntimeOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeOwner")
            .field("built", &self.is_built())
            .finish()
    }
}

impl RuntimeOwner {
    /// An owner that has built nothing yet.
    pub(crate) fn new() -> Self {
        Self {
            built: Mutex::new(None),
        }
    }

    /// A handle to the application's runtime, building it if nothing has
    /// asked before.
    ///
    /// # Errors
    ///
    /// [`Unstarted`] where the operating system would not start the runtime's
    /// threads. Nothing is kept, so a later call tries again.
    pub fn handle(&self) -> Result<Handle, Unstarted> {
        let mut built = self.built.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(built) = built.as_ref() {
            return Ok(built.runtime.handle().clone());
        }
        let runtime = build()?;
        let handle = runtime.runtime.handle().clone();
        *built = Some(runtime);
        Ok(handle)
    }

    /// Whether anything has asked for the runtime yet.
    pub(crate) fn is_built(&self) -> bool {
        self.built
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    /// Shuts the runtime down, giving its threads [`SHUTDOWN`] to stop.
    ///
    /// A runtime nothing asked for was never built, and there is nothing to
    /// stop.
    ///
    /// # Errors
    ///
    /// [`Unstopped`] where some of the runtime's threads were still running
    /// when the bound ran out. They are left running, and what they were doing
    /// is unconfirmed.
    pub(crate) fn shutdown(self) -> Result<(), Unstopped> {
        self.shutdown_within(SHUTDOWN)
    }

    /// [`RuntimeOwner::shutdown`], with the bound as a parameter so a test
    /// can run it out without spending the real one.
    fn shutdown_within(mut self, bound: Duration) -> Result<(), Unstopped> {
        let Some(Built { runtime, running }) = self.take() else {
            return Ok(());
        };
        runtime.shutdown_timeout(bound);
        match running.load(Ordering::Acquire) {
            0 => Ok(()),
            running => Err(Unstopped {
                running,
                waited: bound,
            }),
        }
    }

    fn take(&mut self) -> Option<Built> {
        self.built
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
    }
}

impl Drop for RuntimeOwner {
    /// Bounded as the shutdown [`crate::services::serving`] makes is, so a run
    /// unwinding past its owner does not wait on a thread nothing can stop;
    /// unlike that one, there is nobody to tell whether the bound ran out.
    fn drop(&mut self) {
        if let Some(Built { runtime, .. }) = self.take() {
            runtime.shutdown_timeout(SHUTDOWN);
        }
    }
}

/// The runtime, with its thread counts, its timer, its I/O driver and its
/// threads counted.
fn build() -> Result<Built, Unstarted> {
    let running = Arc::new(AtomicUsize::new(0));
    let started = Arc::clone(&running);
    let stopped = Arc::clone(&running);
    let runtime = Builder::new_multi_thread()
        .worker_threads(WORKERS)
        .max_blocking_threads(BLOCKING)
        .thread_name(NAME)
        .on_thread_start(move || {
            started.fetch_add(1, Ordering::AcqRel);
        })
        .on_thread_stop(move || {
            stopped.fetch_sub(1, Ordering::AcqRel);
        })
        .enable_io()
        .enable_time()
        .build()
        .map_err(Unstarted)?;
    Ok(Built { runtime, running })
}

/// The operating system would not start the application's runtime.
#[derive(Debug, thiserror::Error)]
#[error("the runtime crucible runs its work on could not be started: {0}")]
pub struct Unstarted(io::Error);

/// The application's runtime was shut down with some of its threads still
/// running when the bound ran out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "{running} of the threads crucible runs its work on had not stopped {} ms after they were \
     asked to, and were left running; what they were doing is unconfirmed",
    waited.as_millis()
)]
pub struct Unstopped {
    running: usize,
    waited: Duration,
}

#[cfg(test)]
mod tests;
