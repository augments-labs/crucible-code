//! Renewable account credentials and the login methods that produce them.
//!
//! The shared boundary deliberately knows no authorization protocol. A login
//! method may wait on a loopback browser callback, poll a device code, or ask
//! the terminal for a provider-specific value; all of them report the same
//! bounded updates and persist a credential before reporting completion.
//! Provider implementations also own renewal and request headers, so the TUI
//! and store never branch on OpenAI, `MoonshotAI`, or a provider added later.
//!
//! Access and refresh tokens never cross this module as text. [`Tokens`] has a
//! redacted `Debug`, the protected store walks its fields by hand, and a
//! [`Credential`] applies the current access token only at the request boundary.

mod kimi;
mod openai;
mod renewal;

use std::collections::BTreeMap;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use crucible_core::{Credential, CredentialScopeId};
use crucible_runtime::BoxFuture;
use sha2::{Digest as _, Sha256};
use tokio::runtime::Handle;
use tokio::sync::mpsc::error::TrySendError;
use tokio::task::JoinHandle;

use crate::{AuthError, Store, StoredCredentials};

pub use kimi::{KimiCredential, KimiOAuth};
pub use openai::{OpenAiCredential, OpenAiOAuth};
pub use renewal::{Renewals, Unjoined};

/// A credential's own copy of its tokens, for the moment it takes to read or
/// replace them.
///
/// Never held across an `.await`: a renewal is awaited with the lock released,
/// so a poll on any thread — a runtime worker's among them — waits on this for
/// no longer than another poll takes to copy tokens in or out.
fn held(tokens: &Mutex<Tokens>) -> MutexGuard<'_, Tokens> {
    tokens.lock().unwrap_or_else(PoisonError::into_inner)
}

fn credential_scope(domain: &[u8], identity: Option<&str>) -> Option<CredentialScopeId> {
    let identity = identity.filter(|value| !value.is_empty())?;
    let mut digest = Sha256::new();
    digest.update(b"crucible.oauth-credential-scope.v1");
    digest.update(
        u64::try_from(domain.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    digest.update(domain);
    digest.update(
        u64::try_from(identity.len())
            .unwrap_or(u64::MAX)
            .to_be_bytes(),
    );
    digest.update(identity.as_bytes());
    Some(CredentialScopeId::from_digest(digest.finalize().into()))
}

/// One provider-owned way to authorize an account.
///
/// Values are declared by implementations and interpreted only by the same
/// implementation. The binary registry pairs them with visible product copy;
/// neither the store nor the TUI has a closed list of protocols.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct LoginMethod(&'static str);

impl LoginMethod {
    /// Declares a stable method name inside one login implementation.
    #[must_use]
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    /// The implementation-owned method name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Debug for LoginMethod {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_tuple("LoginMethod").field(&self.0).finish()
    }
}

/// One visible change while an account login is running.
#[derive(PartialEq, Eq)]
pub enum LoginUpdate {
    /// Authorization is ready in a browser.
    Authorize {
        /// The complete URI to open. It may carry transient PKCE state and is
        /// therefore redacted from `Debug`.
        browser_uri: Box<str>,
        /// A short page safe to leave visible and copy from a narrow terminal.
        shown_uri: Box<str>,
        /// A device-flow code to enter on that page, when the method uses one.
        user_code: Option<Box<str>>,
        /// Whether the callback URI or authorization code can be pasted back
        /// into the terminal as a fallback.
        manual: bool,
    },
    /// A non-secret progress sentence supplied by the implementation.
    Progress {
        /// What the login is doing now.
        message: &'static str,
    },
    /// The renewable credential was written to the protected store.
    Complete,
}

impl fmt::Debug for LoginUpdate {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Authorize { user_code, .. } => out
                .debug_struct("Authorize")
                .field("browser_uri", &"<redacted>")
                .field("shown_uri", &"<redacted>")
                .field("user_code", &user_code.as_ref().map(|_| "<redacted>"))
                .finish(),
            Self::Progress { message } => out
                .debug_struct("Progress")
                .field("message", message)
                .finish(),
            Self::Complete => out.write_str("Complete"),
        }
    }
}

/// How long a test waits for something a login is going to do.
///
/// A login here runs as a task on a runtime and answers through a channel, and
/// every wait in these tests is for an answer that is coming — so the number is
/// not a deadline anything is measured against, it is the point at which a test
/// that would otherwise hang gives up and says so. Which makes short the wrong
/// answer: a shared runner hands a thread out when it feels like it, and two
/// seconds bought nothing over thirty except a suite that failed on a busy
/// machine and passed on a quiet one. Nothing waits the whole thirty — a channel
/// hands over the moment the other end sends, and the suite takes the same
/// second and a half it always did.
#[cfg(test)]
pub(crate) const PATIENCE: Duration = Duration::from_secs(30);

/// How long dropping a [`LoginAttempt`] off the runtime waits for its login
/// to stop.
///
/// Stopping is dropping the login's future, which a runtime worker does the
/// next time it is free: its callback listener, its request in flight and its
/// pause go with it. A second covers a runtime busy with other work; the wait
/// ends as soon as the future is gone.
pub(crate) const STOPPING: Duration = Duration::from_secs(1);

/// How many updates a login can report before the attempt takes one.
///
/// Three is every update a browser or device login reports: the page to
/// authorize on, the progress after it and the outcome. A reader that falls
/// further behind than that is not following the login, which stops rather
/// than wait for it (see [`LoginUpdates::send`]).
const UPDATES: usize = 3;

/// A running login.
///
/// The login is a task on the application's runtime, and this owns it:
/// [`LoginAttempt::cancel`] aborts it, and so does dropping the attempt. The
/// abort is final at once — the login is never resumed after it, so it
/// answers no request it was not already answering — and the login's
/// resources go when a runtime worker next carries it out.
///
/// Dropped on a thread outside the runtime, as the terminal drops it, the
/// attempt also waits up to a second for that, so once the drop has returned —
/// unless the runtime was too busy to carry the abort out within the second —
/// a browser login's callback port is closed and the slot it ran on takes the
/// next login. Dropped on a thread inside the runtime — a worker, a blocking
/// thread, or one that has entered it — the drop returns without waiting,
/// since waiting there could hold the very thread the abort needs: for a
/// moment after it the port can still take a connection it never answers,
/// and the slot can still answer [`OAuthError::Busy`].
pub struct LoginAttempt {
    updates: mpsc::Receiver<Result<LoginUpdate, OAuthError>>,
    input: tokio::sync::mpsc::Sender<Box<str>>,
    task: JoinHandle<()>,
    /// Never sent on: disconnected once the login's future has been dropped,
    /// finished or aborted.
    stopped: mpsc::Receiver<()>,
}

impl LoginAttempt {
    /// Waits briefly for the next update, returning `None` while the login is
    /// still running.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] when login failed or its task stopped unexpectedly.
    pub fn wait(&self, patience: Duration) -> Result<Option<LoginUpdate>, OAuthError> {
        match self.updates.recv_timeout(patience) {
            Ok(update) => update.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(OAuthError::WorkerStopped),
        }
    }

    /// Stops the login at whatever it is waiting on: a callback, a request in
    /// flight or a pause between polls.
    pub fn cancel(&self) {
        self.task.abort();
    }

    /// Hands bounded manual authorization input to the running method.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] when the value is empty or oversized, an earlier value
    /// is still being checked, or the login has stopped.
    pub fn submit(&self, value: &str) -> Result<(), OAuthError> {
        let value = value.trim();
        if value.is_empty() || value.len() > 16 * 1024 {
            return Err(OAuthError::Invalid {
                step: "manual authorization",
            });
        }
        self.input
            .try_send(value.into())
            .map_err(|problem| match problem {
                TrySendError::Full(_) => OAuthError::InputBusy,
                TrySendError::Closed(_) => OAuthError::WorkerStopped,
            })
    }
}

impl Default for LoginSlot {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for LoginSlot {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("LoginSlot").finish_non_exhaustive()
    }
}

impl Drop for LoginAttempt {
    fn drop(&mut self) {
        self.task.abort();
        // The abort is carried out by a runtime worker. Waiting for it on a
        // thread a runtime runs could hold the very worker it needs, so there
        // the abort is left to happen without a wait.
        if Handle::try_current().is_err() {
            let _ = self.stopped.recv_timeout(STOPPING);
        }
    }
}

impl fmt::Debug for LoginAttempt {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("LoginAttempt").finish_non_exhaustive()
    }
}

/// Where a running login reports what it is doing, without ever waiting.
///
/// A login is a task on a runtime, and a task that waited on a send would hold
/// the worker it runs on. So this has no waiting send at all: an update the
/// attempt has no room for ends the login instead.
pub struct LoginUpdates(mpsc::SyncSender<Result<LoginUpdate, OAuthError>>);

impl LoginUpdates {
    /// Hands one update to the attempt, at once.
    ///
    /// # Errors
    ///
    /// [`OAuthError::Cancelled`] where the attempt is gone, or has left three
    /// updates untaken: either way nobody is following the login, and it
    /// stops rather than wait.
    pub fn send(&self, update: Result<LoginUpdate, OAuthError>) -> Result<(), OAuthError> {
        self.0.try_send(update).map_err(|_| OAuthError::Cancelled)
    }
}

impl fmt::Debug for LoginUpdates {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("LoginUpdates").finish_non_exhaustive()
    }
}

/// One account-login implementation in the binary's open registry.
///
/// A provider may expose several methods through one implementation: OpenAI,
/// for example, owns both browser PKCE and device authorization. Adding that
/// method does not add a branch to the TUI, store, or provider wire adapter.
///
/// The set is open outside this crate too. What an implementation needs is a
/// [`LoginSlot`] to run its method on, [`LoginUpdate`] to say what it is doing,
/// and [`Store`] to write the result down; all three are public, and none of
/// them hands over a token. The method is a future, run as a task on the
/// runtime the implementation hands the slot, and the store's work is blocking
/// work, handed to that runtime's blocking threads:
///
/// ```
/// use std::fmt;
///
/// use crucible_auth::{
///     LoginAttempt, LoginMethod, LoginSlot, LoginUpdate, OAuthError, Store, StoredCredentials,
///     SubscriptionLogin,
/// };
/// use crucible_core::{Credential, Header, HeaderKey};
/// use tokio::runtime::Handle;
///
/// #[derive(Debug)]
/// struct Ledger(LoginSlot, Handle);
///
/// impl SubscriptionLogin for Ledger {
///     fn provider(&self) -> &'static str {
///         "ledger"
///     }
///
///     fn start(&self, method: LoginMethod, store: Store) -> Result<LoginAttempt, OAuthError> {
///         if method != LoginMethod::new("paste") {
///             return Err(OAuthError::Method);
///         }
///         self.0.start_with_input(&self.1, move |updates, mut typed| async move {
///             let authorize = updates.send(Ok(LoginUpdate::Authorize {
///                 browser_uri: "https://example.invalid/authorize".into(),
///                 shown_uri: "example.invalid/authorize".into(),
///                 user_code: None,
///                 manual: true,
///             }));
///             if authorize.is_err() {
///                 return;
///             }
///             let Some(pasted) = typed.recv().await else { return };
///             let kept = tokio::task::spawn_blocking(move || store.keep("ledger", &pasted)).await;
///             let _ = match kept {
///                 Ok(Ok(())) => updates.send(Ok(LoginUpdate::Complete)),
///                 Ok(Err(_)) | Err(_) => updates.send(Err(OAuthError::WorkerStopped)),
///             };
///         })
///     }
///
///     fn credential(&self, stored: &StoredCredentials) -> Option<Box<dyn Credential>> {
///         let key = stored.get("ledger")?;
///         Some(Box::new(HeaderKey::new(key, Header::bearer())))
///     }
/// }
/// ```
///
/// What stays behind is the renewable token state. `Tokens` is not exported, so
/// an implementation outside this crate persists through [`Store`] and reads
/// back through [`StoredCredentials`] rather than holding access and refresh
/// values of its own.
///
/// ```compile_fail,E0432
/// use crucible_auth::Tokens;
/// ```
pub trait SubscriptionLogin: Send + Sync + fmt::Debug {
    /// The provider name used by configuration and the auth store.
    fn provider(&self) -> &'static str;

    /// Starts one implementation-owned authorization method.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] when the method is unknown, there is no runtime to run
    /// it on, or an earlier attempt has not stopped yet.
    fn start(&self, method: LoginMethod, store: Store) -> Result<LoginAttempt, OAuthError>;

    /// Resolves a stored credential without exposing its tokens.
    fn credential(&self, stored: &StoredCredentials) -> Option<Box<dyn Credential>>;
}

/// A login or renewal failure. No variant carries response text, callback
/// parameters, or submitted credential material.
#[derive(Debug, thiserror::Error)]
pub enum OAuthError {
    /// The selected method is not implemented by this provider.
    #[error("this account login method is unavailable")]
    Method,
    /// Another login on the same slot is still running, or has not finished
    /// stopping.
    #[error("an earlier account login is still stopping — try again")]
    Busy,
    /// One pasted callback is still waiting for the implementation to read it.
    #[error("the previous authorization input is still being checked")]
    InputBusy,
    /// The login was started before the application had given it a runtime
    /// to run on.
    #[error("account login could not start: there is no runtime to run it on")]
    NotStarted,
    /// The login ended without reporting a result.
    #[error("account login ended unexpectedly")]
    WorkerStopped,
    /// Cryptographic state could not be obtained from the operating system.
    #[error("account login could not obtain cryptographic state")]
    Random,
    /// The local browser callback could not be listened for safely.
    #[error("account login could not listen for its browser callback")]
    Callback,
    /// The authorization service could not be reached.
    #[error("account login could not reach the authorization service")]
    Unreachable,
    /// The authorization service refused one step.
    #[error("account login was refused (HTTP {status})")]
    Refused {
        /// The HTTP status, which contains no credential bytes.
        status: u16,
    },
    /// A successful response did not have the bounded shape the flow requires.
    #[error("account login returned an invalid {step} response")]
    Invalid {
        /// The protocol step being decoded.
        step: &'static str,
    },
    /// The browser returned without the state minted for this attempt.
    #[error("account login returned with the wrong state")]
    State,
    /// The authorization page returned a provider-owned refusal. Its text is
    /// deliberately not carried across the secret boundary.
    #[error("account login was not authorized")]
    Denied,
    /// The user did not finish within the authorization lifetime.
    #[error("account login expired before it was authorized")]
    Expired,
    /// The user cancelled the local attempt.
    #[error("account login was cancelled")]
    Cancelled,
    /// The stored account was removed before it could be renewed.
    #[error("the account is signed out — use /login to sign in again")]
    SignedOut,
    /// The protected store could not be updated.
    #[error(transparent)]
    Store(#[from] AuthError),
    /// The client account requests are sent through could not be made.
    #[error("account requests could not set up TLS")]
    Tls(#[source] rustls::Error),
    /// A renewal was due before the application had given its renewals a
    /// runtime to run on.
    #[error("the account could not be renewed: there is no runtime to renew it on")]
    NoRuntime,
    /// A renewal ended without an outcome: the runtime it ran on stopped it,
    /// or it came apart.
    #[error("the account renewal stopped before it finished")]
    Abandoned,
}

/// The provider-neutral portion of a renewable credential.
///
/// `details` contains bounded non-token values needed by one provider at the
/// request boundary, such as a selected workspace id. It is walked by hand in
/// the versioned store and remains redacted here because a later provider may
/// assign sensitive meaning to a field.
#[derive(Clone)]
pub(crate) struct Tokens {
    access: Box<str>,
    refresh: Box<str>,
    details: BTreeMap<String, String>,
    expires_at: u64,
    refreshed_at: u64,
}

impl Tokens {
    pub(crate) fn new(
        access: Box<str>,
        refresh: Box<str>,
        expires_at: u64,
        refreshed_at: u64,
    ) -> Self {
        Self {
            access,
            refresh,
            details: BTreeMap::new(),
            expires_at,
            refreshed_at,
        }
    }

    pub(crate) fn with_detail(mut self, name: &str, value: impl Into<String>) -> Self {
        self.details.insert(name.to_owned(), value.into());
        self
    }

    pub(crate) fn access(&self) -> &str {
        &self.access
    }

    pub(crate) fn refresh(&self) -> &str {
        &self.refresh
    }

    pub(crate) fn detail(&self, name: &str) -> Option<&str> {
        self.details.get(name).map(String::as_str)
    }

    pub(crate) fn details(&self) -> &BTreeMap<String, String> {
        &self.details
    }

    pub(crate) fn replace_details(&mut self, details: BTreeMap<String, String>) {
        self.details = details;
    }

    pub(crate) fn needs_refresh(&self, at: u64, skew: u64, maximum_age: u64) -> bool {
        self.expires_at <= at.saturating_add(skew)
            || (maximum_age > 0 && self.refreshed_at.saturating_add(maximum_age) <= at)
    }

    pub(crate) fn times(&self) -> (u64, u64) {
        (self.expires_at, self.refreshed_at)
    }
}

impl fmt::Debug for Tokens {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("Tokens(<redacted>)")
    }
}

/// One login at a time, shared by every method of one implementation.
///
/// A [`LoginAttempt`] is what [`SubscriptionLogin::start`] has to return, and
/// this is the only thing that makes one. It is public because the trait is:
/// an implementation living outside this crate can hold a slot, run its method
/// as a task on the runtime it hands the slot, and report the same bounded
/// updates every other method reports. Nothing about a token crosses here —
/// the method is handed [`LoginUpdates`] and, for a method with a manual
/// fallback, the pasted values a user typed. Where the secret goes afterwards
/// is [`Store`]'s answer, not this type's.
///
/// One slot runs one login at a time. A second start while the first login is
/// still running, or still stopping, is [`OAuthError::Busy`] rather than a
/// second task, so an implementation cannot leave two browser callbacks
/// listening at once.
pub struct LoginSlot(Mutex<Weak<Running>>);

impl LoginSlot {
    /// An implementation's empty slot, held for the life of the implementation.
    #[must_use]
    pub const fn new() -> Self {
        Self(Mutex::new(Weak::new()))
    }

    /// Runs one method that needs nothing typed back at it, as a task on
    /// `runtime`.
    ///
    /// # Errors
    ///
    /// [`OAuthError::Busy`] when an earlier attempt has not stopped yet.
    pub fn start<F>(
        &self,
        runtime: &Handle,
        run: impl FnOnce(LoginUpdates) -> F,
    ) -> Result<LoginAttempt, OAuthError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.start_with_input(runtime, move |updates, _| run(updates))
    }

    /// Runs one method that can also be finished by hand, as a task on
    /// `runtime`.
    ///
    /// The receiver hands over what [`LoginAttempt::submit`] accepted, already
    /// trimmed and bounded, for a method that announced itself with
    /// `manual: true`; it ends once the attempt is gone.
    ///
    /// # Errors
    ///
    /// [`OAuthError::Busy`] when an earlier attempt has not stopped yet.
    pub fn start_with_input<F>(
        &self,
        runtime: &Handle,
        run: impl FnOnce(LoginUpdates, tokio::sync::mpsc::Receiver<Box<str>>) -> F,
    ) -> Result<LoginAttempt, OAuthError>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut slot = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        if slot.strong_count() > 0 {
            return Err(OAuthError::Busy);
        }

        let (send, updates) = mpsc::sync_channel(UPDATES);
        let (input, submitted) = tokio::sync::mpsc::channel(1);
        let (signal, stopped) = mpsc::sync_channel(0);
        let running = Arc::new(Running { _stopped: signal });
        *slot = Arc::downgrade(&running);
        let task = runtime.spawn(Owned {
            login: Box::pin(run(LoginUpdates(send), submitted)),
            _running: running,
        });
        Ok(LoginAttempt {
            updates,
            input,
            task,
            stopped,
        })
    }
}

/// Held by a login's task for as long as its future lives: the slot sees the
/// login running through a weak reference to it, and the attempt sees it
/// stop when the channel it holds the sending end of disconnects.
struct Running {
    _stopped: mpsc::SyncSender<()>,
}

/// A login's future as its task holds it.
///
/// The fields are dropped in the order they are declared, so the login — its
/// listener, its request in flight — is gone before [`Running`] says it is.
struct Owned {
    login: BoxFuture<'static, ()>,
    _running: Arc<Running>,
}

impl Future for Owned {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
        self.login.as_mut().poll(context)
    }
}

#[cfg(test)]
mod tests;
