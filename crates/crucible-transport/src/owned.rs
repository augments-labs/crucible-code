//! How the transport holds the work it runs, and how a synchronous host waits
//! on it.
//!
//! A hosted program's output, its input and its standard error are each
//! handled by a task on the runtime the host hands over, never by a thread of
//! the transport's own: a thread nobody joins outlives the value that started
//! it, where a task is held here and ends with its holder. [`Owned`] is that
//! holding. It aborts its task when it is dropped, which takes the task at its
//! next wait — and every task here spends its life in one, on a pipe, on a
//! queue or on the runtime's clock — so dropping the value that holds one
//! ends it as soon as a worker next looks at it, without the dropping thread
//! waiting for that.
//!
//! A host takes what a task hands over in one of two ways, from the same
//! bounded queue. One awaiting it is woken by the queue itself, as any task
//! is. One waiting on its own thread looks at the queue and, finding nothing,
//! waits for the [`Doorbell`] the task rings each time it hands something over
//! and once more as it ends, for as long as a bound the host holds allows.
//! That wait polls no future, enters no runtime and drives nothing, so it works
//! on a thread that has never seen one — and the runtime's own workers are what
//! move the task meanwhile.

use std::future::Future;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::Duration;

use tokio::runtime::Handle;
use tokio::task::JoinHandle;

/// How long a task sleeps before asking a stream again when the stream said
/// it had nothing.
///
/// A waiting read never says that: it waits instead. A backend is still free
/// to answer an empty read, and a task that asked again at once would spin a
/// worker for as long as the backend kept saying it; this is the pause the
/// synchronous readers took before the transport had tasks.
pub(crate) const PAUSE: Duration = Duration::from_millis(5);

/// A task this crate started, ended when this is dropped.
pub(crate) struct Owned(JoinHandle<()>);

impl Owned {
    /// Starts `work` on `on`, held by the value returned.
    pub(crate) fn spawn(on: &Handle, work: impl Future<Output = ()> + Send + 'static) -> Self {
        Self(on.spawn(work))
    }
}

impl Drop for Owned {
    /// Aborts the task, without waiting for the abort to land.
    ///
    /// Waiting would hold whichever thread drops a conversation until a
    /// worker had got round to the task, and the one thing left for the task
    /// to do is let go of its stream, which the abort does.
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// What a task rings each time it hands its host something, so a host waiting
/// on its own thread is woken rather than left to find it at its next look.
#[derive(Debug, Default)]
pub(crate) struct Doorbell {
    /// How many times it has been rung.
    rung: Mutex<u64>,
    /// Where a waiting host is woken.
    bell: Condvar,
}

impl Doorbell {
    /// How many times it has been rung so far: read before a host looks at
    /// its queue, so a ring after the look is not missed.
    pub(crate) fn rung(&self) -> u64 {
        *self.rung.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Wakes every host waiting on it.
    fn ring(&self) {
        {
            let mut rung = self.rung.lock().unwrap_or_else(PoisonError::into_inner);
            *rung = rung.wrapping_add(1);
        }
        self.bell.notify_all();
    }

    /// Waits until it has been rung since `seen`, or `longest` has passed.
    pub(crate) fn wait(&self, seen: u64, longest: Duration) {
        let rung = self.rung.lock().unwrap_or_else(PoisonError::into_inner);
        // Poisoned or not, the wait is over: the caller looks at its queue
        // again either way.
        let _ = self
            .bell
            .wait_timeout_while(rung, longest, |rung| *rung == seen);
    }
}

/// A task's hold on its host's doorbell, which rings once more as it is
/// dropped: a host waiting on its own thread hears a task end however it ended,
/// by returning, by coming apart or by being aborted.
pub(crate) struct Ringing(pub(crate) Arc<Doorbell>);

impl Ringing {
    /// Rings the bell.
    pub(crate) fn ring(&self) {
        self.0.ring();
    }
}

impl Drop for Ringing {
    fn drop(&mut self) {
        self.0.ring();
    }
}
