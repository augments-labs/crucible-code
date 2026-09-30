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
    KimiOAuth, KimiSite, Kind, LoginAttempt, LoginMethod, OAuthError, OpenAiOAuth, Renewals, Store,
    StoredCredentials, SubscriptionLogin, provider_of,
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
    /// The name the sign-in it starts is written under.
    name: &'static str,
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
                    login: Arc::new(KimiOAuth::at(renewals.clone(), KimiSite::Ai)),
                    endpoint: Moonshot::CODING_AI,
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
                    name: "openai",
                    method: OpenAiOAuth::BROWSER,
                    title: "Log in to ChatGPT",
                    shown: "Continue in browser",
                    says: "sign in to ChatGPT on this device",
                },
                Route {
                    name: "openai",
                    method: OpenAiOAuth::DEVICE,
                    title: "Log in to ChatGPT",
                    shown: "Use a device code",
                    says: "sign in to ChatGPT from another device",
                },
                Route {
                    name: KimiSite::Ai.name(),
                    method: KimiOAuth::DEVICE,
                    title: "Log in to Kimi Code",
                    shown: "Use a device code",
                    says: "authorize the Kimi Code plan in a browser",
                },
                Route {
                    name: KimiSite::Com.name(),
                    method: KimiOAuth::DEVICE,
                    title: "Log in to Kimi Code",
                    shown: "Use a device code",
                    says: "authorize the Kimi Code plan in a browser",
                },
            ]),
        }
    }

    /// A registry of `logins`, each paired with the one address its tokens
    /// may be sent to, started through `routes`.
    ///
    /// The seam an implementation other than this build's own comes in
    /// through, such as one a test stands up; [`Subscriptions::production`] is
    /// the registry this build ships.
    #[must_use]
    pub fn new(logins: Vec<(Arc<dyn SubscriptionLogin>, Endpoint)>, routes: Vec<Route>) -> Self {
        Self {
            providers: logins
                .into_iter()
                .map(|(login, endpoint)| Registered { login, endpoint })
                .collect(),
            accounts: Arc::new([]),
            routes: routes.into(),
        }
    }

    /// Every registered login's stored name and the one address it is sent
    /// to, for the test that holds the rows to this registry.
    #[cfg(test)]
    pub(crate) fn registered(&self) -> impl Iterator<Item = (&'static str, &Endpoint)> {
        self.providers
            .iter()
            .map(|registered| (registered.login.name(), &registered.endpoint))
    }

    /// Every route this registry offers, for the test that holds each to a
    /// registered login.
    #[cfg(test)]
    pub(crate) fn every_route(&self) -> impl Iterator<Item = Route> + '_ {
        self.routes.iter().copied()
    }

    /// Provider accounts in their stable display order.
    #[must_use]
    pub fn accounts(&self) -> &[Account] {
        &self.accounts
    }

    /// The login methods registered for the sign-in written under `name`.
    pub fn routes(&self, name: &str) -> Vec<Route> {
        self.routes
            .iter()
            .copied()
            .filter(move |route| route.name == name)
            .collect()
    }

    /// Whether this build can sign in to `provider` with a subscription.
    #[must_use]
    pub fn supports(&self, provider: &str) -> bool {
        self.providers
            .iter()
            .any(|registered| registered.login.provider() == provider)
    }

    /// Starts one route from this registry.
    ///
    /// # Errors
    ///
    /// [`OAuthError`] from the selected implementation.
    pub fn start(&self, route: Route, store: Store) -> Result<LoginAttempt, OAuthError> {
        self.find(route.name)
            .ok_or(OAuthError::Method)?
            .login
            .start(route.method, store)
    }

    /// Resolves the sign-in `provider` holds without exposing its token,
    /// paired with the one address its site serves it at.
    #[must_use]
    pub fn credential(&self, provider: &str, stored: &StoredCredentials) -> Option<Resolved> {
        let held = stored
            .held(provider)
            .filter(|held| held.kind == Kind::Account)?;
        let registered = self.find(&held.name)?;
        Some(Resolved {
            credential: registered.login.credential(stored)?,
            endpoint: registered.endpoint.clone(),
        })
    }

    fn find(&self, name: &str) -> Option<&Registered> {
        self.providers
            .iter()
            .find(|registered| registered.login.name() == name)
    }
}

impl Route {
    /// A way to start the sign-in written under `name`, as a registry of
    /// logins other than this build's own offers it.
    #[must_use]
    pub const fn new(
        name: &'static str,
        method: LoginMethod,
        title: &'static str,
        shown: &'static str,
        says: &'static str,
    ) -> Self {
        Self {
            name,
            method,
            title,
            shown,
            says,
        }
    }

    /// The name the sign-in it starts is written under.
    #[must_use]
    pub const fn name(self) -> &'static str {
        self.name
    }

    /// The provider selected after this route completes: the part of its
    /// name before any `@`.
    #[must_use]
    pub fn provider(self) -> &'static str {
        provider_of(self.name)
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
