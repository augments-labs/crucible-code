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

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Mutex;
use std::sync::mpsc::{self, RecvTimeoutError, TrySendError};
use std::thread;
use std::time::Duration;

use crucible_core::{Cancel, Credential};

use crate::{AuthError, Store, StoredCredentials};

pub use kimi::{KimiCredential, KimiOAuth};

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

/// A running login.
pub struct LoginAttempt {
    updates: mpsc::Receiver<Result<LoginUpdate, OAuthError>>,
    input: mpsc::SyncSender<Box<str>>,
    cancel: Cancel,
}

impl LoginAttempt {
    /// Waits briefly for the next update, returning `None` while the worker is
    /// still running.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] when login failed or its worker stopped unexpectedly.
    pub fn wait(&self, patience: Duration) -> Result<Option<LoginUpdate>, OAuthError> {
        match self.updates.recv_timeout(patience) {
            Ok(update) => update.map(Some),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(OAuthError::WorkerStopped),
        }
    }

    /// Asks the login worker to stop before its next request or pause.
    pub fn cancel(&self) {
        self.cancel.request();
    }

    /// Hands bounded manual authorization input to the running method.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] when the value is empty or oversized, an earlier value
    /// is still being checked, or the worker has stopped.
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
                TrySendError::Disconnected(_) => OAuthError::WorkerStopped,
            })
    }
}

impl Drop for LoginAttempt {
    fn drop(&mut self) {
        self.cancel.request();
    }
}

impl fmt::Debug for LoginAttempt {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("LoginAttempt").finish_non_exhaustive()
    }
}

/// One account-login implementation in the binary's open registry.
///
/// A provider may expose several methods through one implementation: OpenAI,
/// for example, owns both browser PKCE and device authorization. Adding that
/// method does not add a branch to the TUI, store, or provider wire adapter.
pub trait SubscriptionLogin: Send + Sync + fmt::Debug {
    /// The provider name used by configuration and the auth store.
    fn provider(&self) -> &'static str;

    /// Starts one implementation-owned authorization method.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] when the method is unknown, a worker cannot start, or an
    /// earlier attempt has not stopped yet.
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
    /// Another login worker has not finished stopping.
    #[error("an earlier account login is still stopping — try again")]
    Busy,
    /// One pasted callback is still waiting for the implementation to read it.
    #[error("the previous authorization input is still being checked")]
    InputBusy,
    /// The worker thread could not be created.
    #[error("account login could not start: {0}")]
    Worker(std::io::Error),
    /// The worker ended without reporting a result.
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

/// One bounded worker slot shared by every method of one implementation.
pub(crate) struct LoginSlot(Mutex<Option<thread::JoinHandle<()>>>);

impl LoginSlot {
    pub(crate) const fn new() -> Self {
        Self(Mutex::new(None))
    }

    pub(crate) fn start(
        &self,
        name: &str,
        run: impl FnOnce(Cancel, mpsc::SyncSender<Result<LoginUpdate, OAuthError>>) + Send + 'static,
    ) -> Result<LoginAttempt, OAuthError> {
        self.start_with_input(name, move |cancel, updates, _| run(cancel, updates))
    }

    pub(crate) fn start_with_input(
        &self,
        name: &str,
        run: impl FnOnce(
            Cancel,
            mpsc::SyncSender<Result<LoginUpdate, OAuthError>>,
            mpsc::Receiver<Box<str>>,
        ) + Send
        + 'static,
    ) -> Result<LoginAttempt, OAuthError> {
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(worker) = slot.take() {
            if !worker.is_finished() {
                *slot = Some(worker);
                return Err(OAuthError::Busy);
            }
            worker.join().map_err(|_| OAuthError::WorkerStopped)?;
        }

        let cancel = Cancel::new();
        let stopping = cancel.clone();
        let (send, updates) = mpsc::sync_channel(3);
        let (input, submitted) = mpsc::sync_channel(1);
        let worker = thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || run(stopping, send, submitted))
            .map_err(OAuthError::Worker)?;
        *slot = Some(worker);
        Ok(LoginAttempt {
            updates,
            input,
            cancel,
        })
    }
}

#[cfg(test)]
mod tests;
