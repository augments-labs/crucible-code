//! What the application owns for the length of a run and lends to what it
//! assembles: today the runtime, the worker tools hand their blocking work
//! to, the shared HTTP service provider turns and web posts use, the release
//! check and its cached answer, and the owner of account renewals, plus whatever
//! later needs to be owned once per run the same way.
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
//! outlives it. A release check still in flight is asked to stop and then given
//! [`crucible_update::SHUTDOWN`], while a renewal is given [`RENEWING`] to
//! finish and be written down. The runtime is the last thing shut down, because
//! everything else here may have work on it.

use std::fmt;
use std::num::NonZeroUsize;
use std::sync::OnceLock;
use std::time::Duration;

use crucible_auth::{Renewals, Unjoined};
use crucible_http::{Lookups, Poison};
use crucible_provider::HttpTurns;
use crucible_tools::ToolWorker;
use crucible_update::{
    SHUTDOWN as RELEASE_SHUTDOWN, Unjoined as ReleaseUnjoined, UpdateCrateReleaseCheck,
};

use crate::runtime::{BLOCKING, RuntimeOwner, Unstarted, Unstopped};

// Every shipped owner of the runtime's blocking threads, each at its most, and
// still at least one thread to spare: the tool worker's jobs, the two one-place
// lookup owners shared by the poisoned HTTP targets and the plain HTTP/release
// lookups, account renewals and a login's requests and store work, each counting
// work it gave up on that is still running, the release check's one cache write,
// one step at a time for each command left running, whose owner asks its process
// everything there, and the stop and reap of each foreground command being
// stopped, as many as a turn may have tool runs at once. An owner added to the
// blocking threads is added here.
//
// That is the reach of the assertion below, and two of those owners are there
// because the owner decided on 2026-09-25 to widen `BLOCKING` by one for each
// rather than let a newly counted owner take the thread in spare. No other
// owner's bound changed by either decision.
//
// A kept command's release task is inside that reach: it asks the very process
// the `crucible_builtins::MOST` step was already reserved for, and that
// command's owner is gone before the release task runs, so the stop holds that
// step's one place rather than taking a second one. A foreground command's
// release task is not inside it. That command was never kept, so it shares no
// reservation, and its release task outlives the tool call that made it, which
// is what leaves it bounded by neither that step nor the turn's tool runs;
// nothing caps how many such tasks are live at once. Retry demand of that kind
// sits outside this count, bounded in rate by the release task's backoff, and
// what it costs is that a stop is asked later: each ask is awaited, so a
// saturated pool holds no thread while it waits, and no stop is lost, confirmed
// without its cleanup, or counted as capacity released.
const HTTP_LOOKUPS: usize = 2;
const RELEASE_WRITE: usize = 1;
const FOREGROUND_STOPS: usize = 2;
const _: () = assert!(
    ToolWorker::CAPACITY
        + HTTP_LOOKUPS
        + Renewals::BLOCKING
        + crucible_builtins::MOST
        + RELEASE_WRITE
        + FOREGROUND_STOPS
        < BLOCKING,
    "the tool worker, shared HTTP lookups, account requests, the release cache write, the \
     commands left running and the foreground stops together would take every \
     blocking thread the runtime has"
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
    release: OnceLock<UpdateCrateReleaseCheck>,
}

impl Services {
    /// Services that have started nothing yet.
    pub(crate) fn new() -> Self {
        Self {
            runtime: RuntimeOwner::new(),
            tool_worker: OnceLock::new(),
            http: OnceLock::new(),
            renewals: Renewals::new(),
            release: OnceLock::new(),
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

    /// The release owner for this run.
    ///
    /// It is built lazily with the same TLS, proxy and plain-lookup decisions
    /// the application's HTTP client uses. The release client keeps its own
    /// pool, while the owner itself is what joins or aborts the check at exit.
    #[must_use]
    pub fn release(&self) -> &UpdateCrateReleaseCheck {
        self.release.get_or_init(UpdateCrateReleaseCheck::new)
    }

    /// The one asynchronous HTTP service provider turns and web posts use.
    ///
    /// It is built the first time a factory needs it, with one poisoned lookup
    /// place for targets and the release owner's plain place for proxy hosts.
    /// The release owner supplies the TLS and proxy decisions, so a separate
    /// pool never means a separate trust or environment decision.
    #[must_use]
    pub fn http(&self) -> &HttpTurns {
        self.http.get_or_init(|| {
            let poison = Poison::default();
            let targets = Lookups::poisoned(NonZeroUsize::MIN, &poison);
            self.release()
                .client(targets)
                .map_or_else(HttpTurns::unavailable, HttpTurns::new)
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

    /// Shuts down everything here: the release check and renewals still in
    /// flight are given their bounds, and the runtime is shut down last.
    fn shutdown(self) -> Result<(), Unfinished> {
        let Services {
            runtime,
            tool_worker,
            http,
            renewals,
            release,
        } = self;
        let release_owner = release;
        let release = release_owner
            .get()
            .and_then(|owner| owner.join_within(RELEASE_SHUTDOWN).err());
        let renewals = renewals.join_within(RENEWING).err();
        drop(release_owner);
        drop(http);
        drop(tool_worker);
        let runtime = runtime.shutdown().err();
        match (renewals, release, runtime) {
            (None, None, None) => Ok(()),
            (renewals, release, runtime) => Err(Unfinished {
                renewals,
                release,
                runtime,
            }),
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
    /// A release check still in flight when its bound ran out, which was
    /// aborted.
    release: Option<ReleaseUnjoined>,
    /// Runtime threads still running when theirs did.
    runtime: Option<Unstopped>,
}

impl fmt::Display for Unfinished {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.renewals, &self.release, &self.runtime) {
            (Some(renewals), Some(release), Some(runtime)) => {
                write!(f, "{renewals}; {release}; {runtime}")
            }
            (Some(renewals), Some(release), None) => write!(f, "{renewals}; {release}"),
            (Some(renewals), None, Some(runtime)) => write!(f, "{renewals}; {runtime}"),
            (Some(renewals), None, None) => renewals.fmt(f),
            (None, Some(release), Some(runtime)) => write!(f, "{release}; {runtime}"),
            (None, Some(release), None) => release.fmt(f),
            (None, None, Some(runtime)) => runtime.fmt(f),
            (None, None, None) => f.write_str("the run's services were not all shut down"),
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
/// The second half of what comes back is [`Unfinished`] where the release check
/// or renewals still in flight had not finished within their bounds, or the
/// runtime's threads had not all stopped within its bound. It is reported
/// whatever `run` answered, beside it, so a run that failed and then failed to
/// clean up says both.
pub fn serving<T>(run: impl FnOnce(&Services) -> T) -> (T, Result<(), Unfinished>) {
    let services = Services::new();
    let ran = run(&services);
    (ran, services.shutdown())
}

#[cfg(test)]
mod tests;
