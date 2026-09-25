//! What the application owns for the length of a run and lends to what it
//! assembles: today the runtime, the worker tools hand their blocking work
//! to, the shared HTTP service provider turns and web posts use, and the owner
//! of account renewals, plus whatever later needs to be owned once per run the
//! same way.
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
//! outlives it. A renewal still in flight is then given [`RENEWING`] to
//! finish and be written down. The runtime is the last thing shut down,
//! because everything else here may have work on it.

use std::fmt;
use std::num::NonZeroUsize;
use std::sync::OnceLock;
use std::time::Duration;

use crucible_auth::{Renewals, Unjoined};
use crucible_http::{Http, Lookups, Poison, ProxyEnv, Tls};
use crucible_provider::HttpTurns;
use crucible_tools::ToolWorker;

use crate::runtime::{BLOCKING, RuntimeOwner, Unstarted, Unstopped};

// Every shipped owner of the runtime's blocking threads, each at its most, and
// still at least one thread to spare: the tool worker's jobs, the shared HTTP
// client's two one-place lookup owners, account renewals and a login's requests
// and store work, each counting work it gave up on that is still running, and
// one step at a time for each command left running, whose owner asks its process
// everything there. An owner added to the blocking threads is added here.
const HTTP_LOOKUPS: usize = 2;
const _: () = assert!(
    ToolWorker::CAPACITY + HTTP_LOOKUPS + Renewals::BLOCKING + crucible_builtins::MOST < BLOCKING,
    "the tool worker, shared HTTP lookups, account requests and the commands left running together \
     would take every blocking thread the runtime has"
);

/// How long a renewal still in flight when the run is over is given to
/// finish and be written down.
///
/// A rotation the token service has answered has spent the old refresh token,
/// so one abandoned there leaves a store whose token no longer renews. Five
/// seconds covers a rotation whose answer is on its way and the write after
/// it; one still unanswered by then is abandoned, and said to be.
pub const RENEWING: Duration = Duration::from_secs(5);

/// What the application owns for the length of a run.
#[derive(Debug)]
pub struct Services {
    runtime: RuntimeOwner,
    tool_worker: OnceLock<ToolWorker>,
    http: OnceLock<HttpTurns>,
    renewals: Renewals,
}

impl Services {
    /// Services that have started nothing yet.
    pub(crate) fn new() -> Self {
        Self {
            runtime: RuntimeOwner::new(),
            tool_worker: OnceLock::new(),
            http: OnceLock::new(),
            renewals: Renewals::new(),
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

    /// The one asynchronous HTTP service provider turns and web posts use.
    ///
    /// It is built the first time a factory needs it, with one poisoned lookup
    /// place for targets and one plain place for proxy hosts. A TLS setup that
    /// cannot be built leaves the same transport refusal in place rather than
    /// starting a second client or sending plaintext.
    #[must_use]
    pub fn http(&self) -> &HttpTurns {
        self.http.get_or_init(|| {
            let Ok(tls) = Tls::new() else {
                return HttpTurns::unavailable();
            };
            let poison = Poison::default();
            let targets = Lookups::poisoned(NonZeroUsize::MIN, &poison);
            let proxies = Lookups::plain(NonZeroUsize::MIN);
            HttpTurns::new(Http::new(&tls, targets, proxies, ProxyEnv::capture()))
        })
    }

    /// The one owner of account renewals for the run, which every
    /// subscription login is built with.
    ///
    /// Cheap and inert until it is given the runtime, which
    /// [`crate::startup::assemble`] does once it has built it: until then a
    /// renewal is refused rather than run, and nothing is started for it.
    #[must_use]
    pub fn renewals(&self) -> &Renewals {
        &self.renewals
    }

    /// Shuts down everything here: renewals still in flight are given
    /// [`RENEWING`] to finish, and the runtime is shut down last.
    fn shutdown(self) -> Result<(), Unfinished> {
        let Services {
            runtime,
            tool_worker,
            http,
            renewals,
        } = self;
        let renewals = renewals.join_within(RENEWING).err();
        drop(http);
        drop(tool_worker);
        let runtime = runtime.shutdown().err();
        match (renewals, runtime) {
            (None, None) => Ok(()),
            (renewals, runtime) => Err(Unfinished { renewals, runtime }),
        }
    }
}

/// What a run's services had not finished when their bounds ran out: a
/// cleanup that failed, said whole.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unfinished {
    /// Renewals still in flight when their bound ran out, which were
    /// abandoned.
    renewals: Option<Unjoined>,
    /// Runtime threads still running when theirs did.
    runtime: Option<Unstopped>,
}

impl fmt::Display for Unfinished {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.renewals, &self.runtime) {
            (Some(renewals), Some(runtime)) => write!(f, "{renewals}; {runtime}"),
            (Some(renewals), None) => renewals.fmt(f),
            (None, Some(runtime)) => runtime.fmt(f),
            (None, None) => f.write_str("the run's services were not all shut down"),
        }
    }
}

impl std::error::Error for Unfinished {}

/// Runs `run` with the application's services, then shuts them down.
///
/// `run` is consumed by the call, so everything it owns — what it captured and
/// what it made — is dropped before the services are shut down; only what it
/// returns is not.
///
/// # Errors
///
/// The second half of what comes back is [`Unfinished`] where renewals still
/// in flight had not finished within [`RENEWING`], or the runtime's threads
/// had not all stopped within their bound. It is reported whatever `run`
/// answered, beside it, so a run that failed and then failed to clean up
/// says both.
pub fn serving<T>(run: impl FnOnce(&Services) -> T) -> (T, Result<(), Unfinished>) {
    let services = Services::new();
    let ran = run(&services);
    (ran, services.shutdown())
}

#[cfg(test)]
mod tests;
