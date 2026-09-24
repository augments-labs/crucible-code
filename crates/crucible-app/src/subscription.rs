//! Subscription logins registered at the binary wiring boundary.
//!
//! The auth crate owns the open [`SubscriptionLogin`] interface and each
//! implementation owns its authorization and renewal protocol. This registry
//! is the sole closed list in the shipped binary: one provider implementation
//! is paired with its fixed credential audience and one or more visible login
//! routes. Adding a provider does not add a branch to the store or TUI.
//!
//! Every implementation here is built with the run's one [`Renewals`], so the
//! credentials any of them resolves — the turn's, a web source's — renew one
//! account's tokens once between them, on the run's runtime.

use std::fmt;
use std::sync::Arc;

use crucible_auth::{
    KimiOAuth, LoginAttempt, LoginMethod, OAuthError, OpenAiOAuth, Renewals, Store,
    StoredCredentials, SubscriptionLogin,
};
use crucible_credentials::Credential;
use crucible_provider::{Endpoint, Moonshot, OpenAi};

/// Every subscription login this build can perform.
#[derive(Clone)]
pub struct Subscriptions {
    providers: Arc<[Registered]>,
    accounts: Arc<[Account]>,
    routes: Arc<[Route]>,
}

struct Registered {
    login: Arc<dyn SubscriptionLogin>,
    endpoint: Endpoint,
}

/// One row the account picker can start.
#[derive(Debug, Clone, Copy)]
pub struct Account {
    provider: &'static str,
    /// The provider name at the left of the picker.
    pub shown: &'static str,
    /// The plan the account is billed under, at the right.
    pub plan: &'static str,
}

/// One authorization method inside a provider account.
#[derive(Debug, Clone, Copy)]
pub struct Route {
    provider: &'static str,
    method: LoginMethod,
    title: &'static str,
    /// The short name at the left of the picker.
    pub shown: &'static str,
    /// The billing source and interaction at the right.
    pub says: &'static str,
}

/// A resolved subscription and the only address allowed to receive it.
///
/// One struct rather than two return values because the pair is a single
/// fact: a plan's token is issued against one address, and a caller handed
/// them separately could send it to another.
pub struct Resolved {
    /// The renewable token, behind the contract that redacts it.
    pub credential: Box<dyn Credential>,
    /// The one address the token may be sent to.
    pub endpoint: Endpoint,
}

impl fmt::Debug for Resolved {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_struct("Resolved")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Subscriptions {
    /// The registry compiled into this binary, renewing through `renewals`:
    /// the run's own, from [`crate::services::Services::renewals`].
    #[must_use]
    pub fn production(renewals: &Renewals) -> Self {
        Self {
            providers: Arc::new([
                Registered {
                    login: Arc::new(OpenAiOAuth::new(renewals.clone())),
                    endpoint: OpenAi::SUBSCRIPTION,
                },
                Registered {
                    login: Arc::new(KimiOAuth::new(renewals.clone())),
                    endpoint: Moonshot::CODING,
                },
            ]),
            accounts: Arc::new([
                Account {
                    provider: "openai",
                    shown: "OpenAI",
                    plan: "ChatGPT plan",
                },
                Account {
                    provider: "moonshot",
                    shown: "MoonshotAI",
                    plan: "Kimi Code plan",
                },
            ]),
            routes: Arc::new([
                Route {
                    provider: "openai",
                    method: OpenAiOAuth::BROWSER,
                    title: "Log in to ChatGPT",
                    shown: "Continue in browser",
                    says: "sign in to ChatGPT on this device",
                },
                Route {
                    provider: "openai",
                    method: OpenAiOAuth::DEVICE,
                    title: "Log in to ChatGPT",
                    shown: "Use a device code",
                    says: "sign in to ChatGPT from another device",
                },
                Route {
                    provider: "moonshot",
                    method: KimiOAuth::DEVICE,
                    title: "Log in to Kimi Code",
                    shown: "Use a device code",
                    says: "authorize the Kimi Code plan in a browser",
                },
            ]),
        }
    }

    /// Provider accounts in their stable display order.
    #[must_use]
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// The login methods registered for one provider.
    pub fn routes(&self, provider: &str) -> Vec<Route> {
        self.routes
            .iter()
            .copied()
            .filter(move |route| route.provider == provider)
            .collect()
    }

    /// Whether this build can sign in to `provider` with a subscription.
    #[must_use]
    pub fn supports(&self, provider: &str) -> bool {
        self.find(provider).is_some()
    }

    /// Starts one route from this registry.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] from the selected implementation.
    pub fn start(&self, route: Route, store: Store) -> Result<LoginAttempt, OAuthError> {
        self.find(route.provider)
            .ok_or(OAuthError::Method)?
            .login
            .start(route.method, store)
    }

    /// Resolves a stored subscription without exposing its token.
    #[must_use]
    pub fn credential(&self, provider: &str, stored: &StoredCredentials) -> Option<Resolved> {
        let registered = self.find(provider)?;
        Some(Resolved {
            credential: registered.login.credential(stored)?,
            endpoint: registered.endpoint.clone(),
        })
    }

    fn find(&self, provider: &str) -> Option<&Registered> {
        self.providers
            .iter()
            .find(|registered| registered.login.provider() == provider)
    }
}

impl Route {
    /// The provider selected after this route completes.
    #[must_use]
    pub const fn provider(self) -> &'static str {
        self.provider
    }

    /// The account product named above a running login.
    #[must_use]
    pub const fn title(self) -> &'static str {
        self.title
    }
}

impl Account {
    /// The provider selected by this account row.
    #[must_use]
    pub const fn provider(self) -> &'static str {
        self.provider
    }
}

impl std::fmt::Debug for Subscriptions {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let providers: Vec<_> = self
            .providers
            .iter()
            .map(|entry| entry.login.provider())
            .collect();
        out.debug_struct("Subscriptions")
            .field("providers", &providers)
            .finish_non_exhaustive()
    }
}
