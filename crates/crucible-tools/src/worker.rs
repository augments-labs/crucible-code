//! The worker a tool hands its blocking work to.
//!
//! Some of what a tool does has no asynchronous form: a descriptor-relative
//! read, a walk of a directory tree, a search over the files it finds. Done
//! inside a tool's run, that work holds whichever thread polls the run for as
//! long as it takes. [`ToolWorker`] is where such work goes instead: it runs on
//! the blocking threads of the runtime the application owns and lends, and a
//! tool reaches it through its [`ToolContext`](crate::ToolContext), which says
//! whether one was lent.
//!
//! **Bounded.** At most [`ToolWorker::CAPACITY`] jobs run on a worker at once,
//! however many calls ask; a call that finds no room waits for it. Every copy
//! of a worker shares the one bound, so a run lending the same worker to every
//! call bounds the whole run. The bound is below the runtime's own limit on
//! blocking threads, so tool work never takes every one of them from the
//! runtime's other owners.
//!
//! **A job ends before its place is given back.** A job cannot be stopped from
//! outside once it has started — no thread can be — so the place a job takes
//! is carried into the job and given back only when the job returns, whatever
//! became of the call that asked for it. Capacity counts jobs that are running,
//! never jobs someone has merely stopped waiting for.
//!
//! **Cancelling.** A call cancelled while it waits for room stops waiting,
//! within [`NOTICED`] of the request, and its job is dropped without having
//! started. A call cancelled once its job is running asks the job to stop — the
//! job is handed a child of the call's [`Cancel`], and looks at it between the
//! steps of what it does — and still waits for it to return, so the call does
//! not end while work it started goes on.
//!
//! **What a dropped call leaves.** A call whose future is dropped while its job
//! runs — a turn abandoning the call, or a crossing that asks a run once and
//! drops what would have waited — raises its job's token as it goes, and leaves
//! the job to the runtime alone: nobody waits for it and nobody hears what it
//! answers. Until its next look at the token the job goes on working, with
//! whatever it was given, after whoever dropped the call has reported it; it
//! keeps its place for as long as it runs, and the place comes back when it
//! returns. So a job looks at its token before each step whose effect
//! outlives the process — a write, a rename, a removal — and not only between
//! long ones.
//!
//! A job that never looks at its token runs to its end and keeps its place
//! until then. That is the whole of what the worker cannot promise, and it is
//! why a job looks.

use std::fmt;
use std::future::{Future, poll_fn};
use std::pin::pin;
use std::sync::Arc;
use std::task::Poll;

use crucible_runtime::{Cancel, NOTICED};
use tokio::runtime::Handle;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Sleep;

/// The bounded worker blocking tool work runs on.
///
/// Cloning shares the bound rather than copying it.
#[derive(Clone)]
pub struct ToolWorker {
    handle: Handle,
    capacity: Arc<Semaphore>,
}

impl ToolWorker {
    /// How many jobs run on a worker at once.
    ///
    /// Four: enough for a wave of reads, writes and searches to overlap, and
    /// a share of the application runtime's blocking threads that leaves room
    /// for the runtime's other owners however much tool work is waiting: the
    /// application's budget check keeps what every owner may hold at once,
    /// together, below the threads the runtime has, with one to spare.
    pub const CAPACITY: usize = 4;

    /// A worker that runs its jobs on the blocking threads of `handle`'s
    /// runtime.
    ///
    /// That runtime needs a timer, and needs to be running while a call waits
    /// for room: the wait sets a timer to look at its cancellation by, and
    /// tokio panics setting one on a runtime built without a timer, or polling
    /// one after its runtime has shut down. The application's runtime is built
    /// with a timer and shut down only once the run is over.
    #[must_use]
    pub fn new(handle: Handle) -> Self {
        Self {
            handle,
            capacity: Arc::new(Semaphore::new(Self::CAPACITY)),
        }
    }

    /// Runs `job` on the worker once it has room, and answers what the job
    /// answered.
    ///
    /// The job is handed a child of `cancel`, raised with it and whenever the
    /// returned future is dropped before the job has returned. While the future
    /// is polled, a job that has started is always waited for: cancelling asks
    /// it to stop and it answers whatever it answers on the way out. Dropping
    /// the future instead leaves the job running until it next looks at its
    /// token, holding its place until it returns, as the module documentation
    /// says.
    ///
    /// The future may be polled from any thread, inside a runtime or not; it
    /// is the worker's runtime that runs the job and fires the timer.
    ///
    /// # Errors
    ///
    /// [`Unrun::Cancelled`] where `cancel` was raised before the job started,
    /// [`Unrun::Panicked`] where the job came apart, and [`Unrun::Stopped`]
    /// where the runtime would not start the job because it was shutting down.
    pub async fn run<T, F>(&self, cancel: &Cancel, job: F) -> Result<T, Unrun>
    where
        F: FnOnce(&Cancel) -> T + Send + 'static,
        T: Send + 'static,
    {
        let permit = self.room(cancel).await?;
        let stop = StopOnDrop(cancel.child());
        let told = stop.0.clone();
        let running = self.handle.spawn_blocking(move || {
            // Held for as long as the job runs, and given back as it returns
            // or unwinds: the place is the job's, not the call's.
            let _place = permit;
            job(&told)
        });
        let ended = running.await;
        drop(stop);
        ended.map_err(|join| {
            if join.is_panic() {
                Unrun::Panicked
            } else {
                Unrun::Stopped
            }
        })
    }

    /// A place on the worker, or [`Unrun::Cancelled`] once `cancel` is raised
    /// first.
    ///
    /// Waiting for a permit is woken only by a permit, and a [`Cancel`] wakes
    /// nobody, so the wait is also woken every [`NOTICED`] to look.
    async fn room(&self, cancel: &Cancel) -> Result<OwnedSemaphorePermit, Unrun> {
        let mut permit = pin!(Arc::clone(&self.capacity).acquire_owned());
        let mut tick = pin!(self.tick());
        poll_fn(|context| {
            loop {
                if cancel.requested() {
                    return Poll::Ready(Err(Unrun::Cancelled));
                }
                if let Poll::Ready(permit) = permit.as_mut().poll(context) {
                    return Poll::Ready(permit.map_err(|_| Unrun::Stopped));
                }
                if tick.as_mut().poll(context).is_pending() {
                    return Poll::Pending;
                }
                let now = tokio::time::Instant::now();
                tick.as_mut().reset(now.checked_add(NOTICED).unwrap_or(now));
            }
        })
        .await
    }

    /// A timer [`NOTICED`] from now on the worker's own runtime, so that it
    /// fires whichever thread is polling the wait.
    fn tick(&self) -> Sleep {
        let _entered = self.handle.enter();
        tokio::time::sleep(NOTICED)
    }
}

impl fmt::Debug for ToolWorker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolWorker")
            .field("capacity", &Self::CAPACITY)
            .field("available", &self.capacity.available_permits())
            .finish_non_exhaustive()
    }
}

/// Raises the job's token when the call stops waiting for it, however it
/// stops.
struct StopOnDrop(Cancel);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.request();
    }
}

/// Why work handed to a [`ToolWorker`] did not answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unrun {
    /// The call was cancelled before the worker had room for the work, which
    /// never started.
    #[error("the call was cancelled before the tool worker had room for its work")]
    Cancelled,
    /// The work unwound. What it came apart with is not carried: it is
    /// whatever the work's author wrote, and it has no place in an error a
    /// model or a log may read.
    #[error("the work handed to the tool worker came apart")]
    Panicked,
    /// The runtime the worker runs on was shutting down and would not start
    /// the work.
    #[error("the tool worker stopped taking work before this work started")]
    Stopped,
}

#[cfg(test)]
mod tests;
