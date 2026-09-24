//! What the application owns for the length of a run and lends to what it
//! assembles: today the runtime and the worker tools hand their blocking work
//! to, and whatever later needs to be owned once per run the same way.
//!
//! One value, [`Services`], made once by [`serving`] and lent to everything the
//! run builds. [`crate::startup::Startup`] carries it, so a factory reaches
//! what it needs through it rather than through a parameter of its own.
//!
//! **How it grows.** A service that has to live as long as the run — a worker
//! pool, a renewal owner, a status task's home — is one more field here, built
//! by a constructor in its own module, and reached through this value where a
//! factory needs it. It is never another `Startup` parameter and never another
//! value the command line has to hold, so the command line keeps making one
//! thing and the order that thing is taken apart in stays written in one place.
//!
//! **The order a run ends in.** [`serving`] lends the services to the run and
//! shuts them down only after the run has returned. Everything the run held —
//! its captures and its locals, the conversation, the registry of commands left
//! running and every command it still holds — has been dropped by then, so
//! anything their drops hand to the runtime lands on a runtime that is still
//! running rather than one that is draining. Only what the run returns
//! outlives it. The runtime is the last thing shut down, because everything
//! else here may have work on it.

use std::sync::OnceLock;

use crucible_tools::ToolWorker;

use crate::runtime::{BLOCKING, RuntimeOwner, Unstarted, Unstopped};

// Tool work may take every place the tool worker has and still leave the
// runtime blocking threads for its other owners.
const _: () = assert!(
    ToolWorker::CAPACITY < BLOCKING,
    "the tool worker would take every blocking thread the runtime has"
);

/// What the application owns for the length of a run.
#[derive(Debug)]
pub struct Services {
    runtime: RuntimeOwner,
    tool_worker: OnceLock<ToolWorker>,
}

impl Services {
    /// Services that have started nothing yet.
    pub(crate) fn new() -> Self {
        Self {
            runtime: RuntimeOwner::new(),
            tool_worker: OnceLock::new(),
        }
    }

    /// The application's runtime, built the first time a handle to it is
    /// asked for.
    #[must_use]
    pub fn runtime(&self) -> &RuntimeOwner {
        &self.runtime
    }

    /// The worker tools hand their blocking work to, on the application's
    /// runtime, built the first time it is asked for and the same one every
    /// time after, so every call it is lent to shares its one bound.
    ///
    /// # Errors
    ///
    /// [`Unstarted`] where the runtime had not been built and the operating
    /// system would not start it.
    pub fn tool_worker(&self) -> Result<&ToolWorker, Unstarted> {
        if let Some(worker) = self.tool_worker.get() {
            return Ok(worker);
        }
        let handle = self.runtime.handle()?;
        Ok(self.tool_worker.get_or_init(|| ToolWorker::new(handle)))
    }

    /// Shuts down everything here, the runtime last.
    fn shutdown(self) -> Result<(), Unstopped> {
        self.runtime.shutdown()
    }
}

/// Runs `run` with the application's services, then shuts them down.
///
/// `run` is consumed by the call, so everything it owns — what it captured and
/// what it made — is dropped before the services are shut down; only what it
/// returns is not.
///
/// # Errors
///
/// The second half of what comes back is [`Unstopped`] where the runtime's
/// threads had not all stopped within their bound. It is reported whatever
/// `run` answered, beside it, so a run that failed and then failed to clean up
/// says both.
pub fn serving<T>(run: impl FnOnce(&Services) -> T) -> (T, Result<(), Unstopped>) {
    let services = Services::new();
    let ran = run(&services);
    (ran, services.shutdown())
}

#[cfg(test)]
mod tests;
