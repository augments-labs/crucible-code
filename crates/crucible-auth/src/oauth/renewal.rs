//! The one renewal of an account's tokens in flight for each provider's
//! credential scope, run as work of its own, and the client account requests
//! are sent through.
//!
//! **Owned, not borrowed from whoever asked.** A credential that finds its
//! tokens due starts a rotation here, or joins the one already running for
//! its provider and scope, and awaits its outcome. The rotation is a task on
//! the application's runtime, held by [`Renewals`]; a waiter only watches it.
//! A waiter whose authorization future is dropped — a turn or a web call
//! stopped while it waits, which races the wait against its cancel, or a
//! crossing that polls it once and gives up — drops its watch and nothing
//! else, and the rotation still finishes and is written down. That matters
//! because a rotation the token service has already answered has spent the
//! old refresh token: abandoning it half way would leave the store holding a
//! token that no longer renews.
//!
//! **One per account in this process, and one across processes.** Every
//! credential built from the same login implementation shares one
//! [`Renewals`], so two credentials for the same account — the turn's and a
//! web source's — meet here, under their provider and scope, and present the
//! refresh token once. Other processes are met at the store's lock,
//! `auth.lock`: a rotation takes it, rereads the store inside it, and uses a
//! rotation another process already wrote rather than spending the one it
//! read. The lock is held across the request and released only after the new
//! rotation is written, so no other process can present the old token while
//! it is in flight.
//!
//! **The one blocking adapter, in one place.** Taking the lock (up to 5 s,
//! retried every 20 ms, then `Busy`), rereading the store and writing it
//! have no asynchronous form, so each runs on the runtime's blocking
//! threads, and so does looking up the host of an account request. All of
//! that waits for one
//! place the owner holds: a rotation takes it for the whole of its work, a
//! login request for the whole of its exchange and a login's store work until
//! that work returns, so rotations run one at a time in this process and
//! never beside a login's requests or its store work. At most
//! [`Renewals::BLOCKING`] blocking threads are held for account work at once:
//! one for the work holding the place, and one for a lookup it gave up on.
//! The lock itself is a file held open, not a guard of a mutex:
//! holding it across the request blocks no thread, and dropping it — when the
//! rotation fails, or is aborted at shutdown — releases it.
//!
//! **Bounded.** A request is given 30 s end to end, from waiting for a
//! connection slot to the last byte of its answer; another process waits at
//! most 5 s for the lock; and [`Renewals::join_within`] gives rotations still
//! running a bound of the caller's choosing before it aborts them.
//!
//! **Login borrows it too.** An account login is a task of its own on the
//! application's runtime, owned by its attempt, and it awaits its requests and
//! its store work through this owner, which is what holds them to the one
//! place.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::num::NonZeroUsize;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, PoisonError};
use std::time::{Duration, Instant};

use crucible_core::{CredentialScopeId, Outgoing};
use crucible_http::{BodyError, Http, Lookups, ProxyEnv, Tls, read_limited};
use crucible_runtime::BoxFuture;
use hyper::Method;
use tokio::runtime::Handle;
use tokio::sync::{Semaphore, watch};
use tokio::task::JoinHandle;

use super::{OAuthError, Tokens};
use crate::Store;
use crate::store::Taken;

/// The most bytes an account response may have. One more is read, to tell a
/// body that ended at the limit from one that was cut there.
const MAX_BODY: usize = 64 * 1024;

/// What a rotation came to, as each of its waiters is handed it.
type Outcome = Result<Tokens, Arc<OAuthError>>;

/// One account's rotation as a credential asks for it: whose tokens, where
/// they are written down, when they are due, and the request that replaces
/// them.
pub(crate) struct Due {
    /// The account's scope, which with the provider keys the one rotation in
    /// flight.
    pub(crate) scope: CredentialScopeId,
    /// The provider the store holds the rotation under.
    pub(crate) provider: &'static str,
    pub(crate) store: Store,
    /// Whether the rotation the store holds is still due, decided inside the
    /// store's lock.
    pub(crate) needs_refresh: fn(&Tokens, u64) -> bool,
    /// The one request that replaces the rotation, handed the one it
    /// replaces.
    pub(crate) refresh:
        Box<dyn FnOnce(Tokens) -> BoxFuture<'static, Result<Tokens, OAuthError>> + Send>,
}

/// The renewals of account tokens in flight, one per provider and credential
/// scope, and the runtime and client account requests run on.
///
/// Cheap to make and to clone: clones share everything, which is what makes
/// one rotation per account hold across every credential and login built with
/// one owner. It is given its runtime separately, once the application has
/// one ([`Renewals::runs_on`]); until then a renewal is refused with
/// [`OAuthError::NoRuntime`] and a login with [`OAuthError::NotStarted`].
#[derive(Clone)]
pub struct Renewals(Arc<Inner>);

struct Inner {
    runtime: OnceLock<Handle>,
    client: OnceLock<Http>,
    /// The one place this owner's blocking work takes: held by a rotation
    /// for the whole of its work and by a login request for the whole of its
    /// exchange.
    place: Arc<Semaphore>,
    /// The last rotation of each account, by provider and scope: a scope is
    /// one provider's, so two providers' credentials never share an entry
    /// even where their scopes are equal.
    ///
    /// Taken before [`Inner::running`] where both are held, and never while
    /// that is held.
    rotations: Mutex<HashMap<(&'static str, CredentialScopeId), Rotation>>,
    /// How many rotations have been started and not yet ended, for
    /// [`Renewals::join_within`] to wait on.
    running: Mutex<usize>,
    ended: Condvar,
}

/// One rotation, as the owner holds it.
struct Rotation {
    outcome: watch::Receiver<Option<Outcome>>,
    task: JoinHandle<()>,
}

impl Renewals {
    /// The most of the runtime's blocking threads account work holds at once.
    ///
    /// Two. Every blocking job of this owner's — a rotation's lock and file
    /// work, a login's store work, and the hostname lookup of the request a
    /// rotation or a login sends — runs while its rotation, login request or
    /// login store work holds the owner's one place, and each holder runs its
    /// jobs one after another: one thread. A login's store work keeps the
    /// place until it returns, even once its login has been dropped. A lookup
    /// whose request was given up before the platform answered — its deadline
    /// passed, its login was cancelled, its rotation was aborted — cannot be
    /// stopped, and runs on until the platform answers after the place has
    /// been let go: a second. Target and proxy hosts share one lookup
    /// place of the client's, which that lookup keeps until it returns, so
    /// there is never a third. That ceiling holds while the run lasts: between
    /// [`Renewals::join_within`] aborting the rotations still running and the
    /// runtime's own shutdown, a lookup an aborted rotation gave up on can
    /// briefly add one more.
    pub const BLOCKING: usize = 2;

    /// An owner with nothing in flight and no runtime yet.
    #[must_use]
    pub fn new() -> Self {
        Self(Arc::new(Inner {
            runtime: OnceLock::new(),
            client: OnceLock::new(),
            place: Arc::new(Semaphore::new(1)),
            rotations: Mutex::new(HashMap::new()),
            running: Mutex::new(0),
            ended: Condvar::new(),
        }))
    }

    /// Gives this owner the runtime its rotations and every account request
    /// run on. The first runtime given is the one kept.
    pub fn runs_on(&self, runtime: Handle) {
        let _ = self.0.runtime.set(runtime);
    }

    /// Waits up to `bound` for every rotation in flight to end, then aborts
    /// whatever is still running.
    ///
    /// Called once the run is over, before the runtime is shut down, so a
    /// rotation whose answer is on its way is written down rather than
    /// dropped with the runtime.
    ///
    /// # Errors
    ///
    /// [`Unjoined`] where rotations were still running when `bound` ran out.
    /// They were aborted: a rotation aborted at its request writes nothing and
    /// releases the store's lock, and one aborted inside its lock or file work
    /// runs on until that work returns.
    pub fn join_within(&self, bound: Duration) -> Result<(), Unjoined> {
        let until = Instant::now().checked_add(bound);
        // Read and released before the rotations are: starting a rotation
        // takes the rotations and then the count, so holding the count while
        // waiting for the rotations would wait on a start that waits on this.
        let running = {
            let mut running = lock(&self.0.running);
            while *running > 0 {
                let left = until.map_or(bound, |until| {
                    until.saturating_duration_since(Instant::now())
                });
                if left.is_zero() {
                    break;
                }
                running = self
                    .0
                    .ended
                    .wait_timeout(running, left)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
            }
            *running
        };
        if running == 0 {
            return Ok(());
        }
        for rotation in lock(&self.0.rotations).values() {
            rotation.task.abort();
        }
        Err(Unjoined {
            running,
            waited: bound,
        })
    }

    /// The runtime this owner was given, if it has been.
    pub(crate) fn runtime(&self) -> Option<&Handle> {
        self.0.runtime.get()
    }

    /// What the last rotation of `provider`'s account under `scope` came to,
    /// where it has ended and was written down.
    ///
    /// A credential whose own copy is due reads this before it starts a
    /// rotation, so one whose waiter was dropped — or another credential for
    /// the same account — finds a rotation already done at its first poll
    /// rather than starting another it would have to wait for.
    pub(crate) fn latest(
        &self,
        provider: &'static str,
        scope: CredentialScopeId,
    ) -> Option<Tokens> {
        let rotations = lock(&self.0.rotations);
        let outcome = rotations.get(&(provider, scope))?.outcome.borrow();
        match &*outcome {
            Some(Ok(tokens)) => Some(tokens.clone()),
            Some(Err(_)) | None => None,
        }
    }

    /// Renews what `due` names, joining the rotation already running for its
    /// provider and scope if there is one, and hands back the rotation it
    /// came to.
    ///
    /// Dropping the returned future stops only this wait.
    ///
    /// # Errors
    ///
    /// The rotation's own failure, shared by every waiter;
    /// [`OAuthError::NoRuntime`] where this owner has no runtime yet; and
    /// [`OAuthError::Abandoned`] where the rotation ended without an outcome.
    pub(crate) async fn renew(&self, due: Due) -> Outcome {
        let mut outcome = self.rotation(due).map_err(Arc::new)?;
        let answered = outcome
            .wait_for(Option::is_some)
            .await
            .map_err(|_| Arc::new(OAuthError::Abandoned))?;
        answered
            .clone()
            .unwrap_or_else(|| Err(Arc::new(OAuthError::Abandoned)))
    }

    /// The rotation in flight for `due`'s provider and scope, started if
    /// there is none.
    fn rotation(&self, due: Due) -> Result<watch::Receiver<Option<Outcome>>, OAuthError> {
        let key = (due.provider, due.scope);
        let mut rotations = lock(&self.0.rotations);
        if let Some(running) = rotations.get(&key)
            && running.outcome.borrow().is_none()
        {
            return Ok(running.outcome.clone());
        }
        let runtime = self.runtime().ok_or(OAuthError::NoRuntime)?;
        let (answer, outcome) = watch::channel(None);
        let running = Running::begin(&self.0);
        let place = Arc::clone(&self.0.place);
        let task = runtime.spawn(async move {
            let _running = running;
            let rotated = rotate(&place, due).await;
            let _ = answer.send(Some(rotated.map_err(Arc::new)));
        });
        rotations.insert(
            key,
            Rotation {
                outcome: outcome.clone(),
                task,
            },
        );
        Ok(outcome)
    }

    /// Sends one account request and reads its answer, all within `within`.
    ///
    /// The deadline covers the whole exchange: waiting for one of the
    /// client's connection slots, connecting, the response head and the
    /// body. Every status comes back as it is; no redirect is followed.
    ///
    /// # Errors
    ///
    /// [`OAuthError::Unreachable`] where no whole answer arrived in time or
    /// the exchange failed, [`OAuthError::Invalid`] where the body was over
    /// its limit, and [`OAuthError::Tls`] where the client could not be made.
    pub(crate) async fn post(
        &self,
        url: &str,
        mut headers: Outgoing,
        body: String,
        within: Duration,
    ) -> Result<(u16, String), OAuthError> {
        let client = self.client()?;
        let exchange = async {
            let response = client
                .send(Method::POST, url, &mut headers, body)
                .await
                .map_err(|_| OAuthError::Unreachable)?;
            let status = response.status().as_u16();
            let body = match read_limited(response.into_body(), MAX_BODY, within).await {
                Ok(body) => body,
                Err(BodyError::TooLarge) => {
                    return Err(OAuthError::Invalid {
                        step: "oversized authorization",
                    });
                }
                Err(_) => return Err(OAuthError::Unreachable),
            };
            let text = String::from_utf8(body).map_err(|_| OAuthError::Unreachable)?;
            Ok((status, text))
        };
        tokio::time::timeout(within, exchange)
            .await
            .map_err(|_| OAuthError::Unreachable)?
    }

    /// Sends one request of a login's, holding the owner's one place while it
    /// runs.
    ///
    /// Dropping the returned future — the login's task aborted — drops the
    /// request and lets the place go.
    ///
    /// # Errors
    ///
    /// The request's own failure.
    pub(crate) async fn login_request<T>(
        &self,
        request: impl Future<Output = Result<T, OAuthError>>,
    ) -> Result<T, OAuthError> {
        let _place = self
            .0
            .place
            .acquire()
            .await
            .map_err(|_| OAuthError::WorkerStopped)?;
        request.await
    }

    /// Runs a login's store work on one of the runtime's blocking threads,
    /// holding the owner's one place until that work returns.
    ///
    /// Work on a blocking thread cannot be stopped, so the place goes with it
    /// rather than with the future: a login dropped while its store work runs
    /// leaves the place held until the work is done, and nothing else of this
    /// owner's takes a blocking thread beside it.
    ///
    /// # Errors
    ///
    /// The work's own failure, and [`OAuthError::WorkerStopped`] where the
    /// work ended without an answer.
    pub(crate) async fn login_store<T: Send + 'static, E: Into<OAuthError> + Send + 'static>(
        &self,
        work: impl FnOnce() -> Result<T, E> + Send + 'static,
    ) -> Result<T, OAuthError> {
        let place = Arc::clone(&self.0.place)
            .acquire_owned()
            .await
            .map_err(|_| OAuthError::WorkerStopped)?;
        tokio::task::spawn_blocking(move || {
            let _place = place;
            work()
        })
        .await
        .map_err(|_| OAuthError::WorkerStopped)?
        .map_err(Into::into)
    }

    /// The client account requests are sent through, made the first time
    /// one is: TLS over the compiled-in roots, one hostname lookup at a time
    /// for targets and proxies together, and the proxy this process's
    /// environment names.
    fn client(&self) -> Result<&Http, OAuthError> {
        if let Some(client) = self.0.client.get() {
            return Ok(client);
        }
        let tls = Tls::new().map_err(OAuthError::Tls)?;
        // One lookup place, shared: a proxy's host and a target's are looked
        // up one at a time between them, and each request is bounded by its
        // own deadline rather than by a lookup's.
        let lookups = Lookups::plain(NonZeroUsize::MIN);
        let client = Http::new(&tls, lookups.clone().into(), lookups, ProxyEnv::capture());
        Ok(self.0.client.get_or_init(|| client))
    }
}

impl Default for Renewals {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for Renewals {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Renewals")
            .field("runtime", &self.0.runtime.get().is_some())
            .field("running", &*lock(&self.0.running))
            .finish_non_exhaustive()
    }
}

/// One rotation, from taking the store's lock to writing what replaced it,
/// holding the owner's one place throughout.
async fn rotate(place: &Semaphore, due: Due) -> Result<Tokens, OAuthError> {
    let Due {
        store,
        provider,
        needs_refresh,
        refresh,
        ..
    } = due;
    let _place = place.acquire().await.map_err(|_| OAuthError::Abandoned)?;
    let held = match blocking(move || store.take_rotation(provider, needs_refresh)).await? {
        Taken::Fresh(tokens) => return Ok(tokens),
        Taken::Due(held) => held,
    };
    let fresh = refresh(held.current().clone()).await?;
    blocking(move || held.persist(fresh)).await
}

/// Runs `work` on one of the runtime's blocking threads.
async fn blocking<T: Send + 'static, E: Into<OAuthError> + Send + 'static>(
    work: impl FnOnce() -> Result<T, E> + Send + 'static,
) -> Result<T, OAuthError> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|_| OAuthError::Abandoned)?
        .map_err(Into::into)
}

/// Counts a rotation as running from before it is spawned until its task
/// ends, however it ends: dropped with the task, it is lowered on an abort
/// as on an answer.
struct Running(Arc<Inner>);

impl Running {
    fn begin(inner: &Arc<Inner>) -> Self {
        *lock(&inner.running) += 1;
        Self(Arc::clone(inner))
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let mut running = lock(&self.0.running);
        *running = running.saturating_sub(1);
        self.0.ended.notify_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Renewals were still running when the owner stopped waiting for them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error(
    "{running} of the account renewals in flight had not finished {} ms after the run ended, and \
     were abandoned; whether their new tokens were written down is unconfirmed",
    waited.as_millis()
)]
pub struct Unjoined {
    running: usize,
    waited: Duration,
}

#[cfg(test)]
mod tests;
