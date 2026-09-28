//! The tasks a client spawns, owned by the client.
//!
//! hyper-util runs each connection as a task of its own, and two short waits
//! beside it: returning a connection to the pool once its response has been
//! read, and finishing a connection that lost the race to an idle one. Every
//! one of them is spawned through the executor the client is built with, and
//! none of them is to be spawned and forgotten: each is to have an owner that
//! ends it. So the executor here is an owner. Each task goes into
//! one set, held by the [`Http`](crate::Http) that spawned it, and the set is
//! dropped, aborting every task still in it, when the last handle to that
//! client is. The executor holds the set only weakly, since a task holding it
//! would keep it alive.
//!
//! What bounds the set is what hyper-util spawns: one task per open
//! connection, in use or idle in the pool; one per response whose connection
//! is still busy; and one per request whose connection lost that race, which
//! ends once that request is dropped or has its response head, or before
//! that when the connection is made or fails
//! ([`Setups`](crate::connect::Setups)). Finished tasks are reaped at each spawn, so
//! only live ones are held. How many idle connections the pool keeps is the
//! pool's setting, not this set's. Hostname lookups are not in the set: a
//! lookup already on its blocking worker cannot be ended, and runs until the
//! platform answers, holding a permit of its own owner.

use std::future::Future;
use std::sync::{Arc, Mutex, PoisonError, Weak};

use tokio::task::JoinSet;

/// Every task a client has spawned and not yet seen end.
#[derive(Debug)]
pub(crate) struct Tasks(Mutex<JoinSet<()>>);

/// Spawns into a client's [`Tasks`], while the client is alive.
#[derive(Clone, Debug)]
pub(crate) struct Spawner(Weak<Tasks>);

impl Tasks {
    pub(crate) fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(JoinSet::new())))
    }

    pub(crate) fn spawner(self: &Arc<Self>) -> Spawner {
        Spawner(Arc::downgrade(self))
    }

    /// How many of the tasks are still running, once the ended ones are
    /// reaped.
    #[cfg(test)]
    pub(crate) fn live(&self) -> usize {
        let mut set = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        while set.try_join_next().is_some() {}
        set.len()
    }
}

impl<F> hyper::rt::Executor<F> for Spawner
where
    F: Future<Output = ()> + Send + 'static,
{
    /// Runs `task` as the client's own, or not at all once the client is
    /// gone: nothing is left to hand its result to.
    fn execute(&self, task: F) {
        let Some(tasks) = self.0.upgrade() else {
            return;
        };
        let mut set = tasks.0.lock().unwrap_or_else(PoisonError::into_inner);
        while set.try_join_next().is_some() {}
        set.spawn(task);
    }
}
