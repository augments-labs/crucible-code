//! Subscription logins registered at the binary wiring boundary.
//!
//! The auth crate owns the open [`SubscriptionLogin`] interface and each
//! implementation owns its authorization and renewal protocol. This registry
//! is the sole closed list in the shipped binary: one provider implementation
//! is paired with the fixed audience its tokens are issued for. Adding a
//! provider does not add a branch to the store or TUI.

use std::sync::Arc;

use crucible_auth::{KimiOAuth, OpenAiOAuth, StoredCredentials, SubscriptionLogin};
use crucible_core::Credential;
use crucible_provider::{Endpoint, Moonshot, OpenAi};

/// Every subscription login this build can perform.
#[derive(Clone)]
pub(crate) struct Subscriptions {
    providers: Arc<[Registered]>,
}

struct Registered {
    login: Arc<dyn SubscriptionLogin>,
    endpoint: Endpoint,
}

/// A resolved subscription and the only address allowed to receive it.
///
/// One struct rather than two return values because the pair is a single
/// fact: a plan's token is issued against one address, and a caller handed
/// them separately could send it to another.
pub(crate) struct Resolved {
    pub(crate) credential: Box<dyn Credential>,
    pub(crate) endpoint: Endpoint,
}

impl Subscriptions {
    /// The registry compiled into this binary.
    #[must_use]
    pub(crate) fn production() -> Self {
        Self {
            providers: Arc::new([
                Registered {
                    login: Arc::new(OpenAiOAuth::new()),
                    endpoint: OpenAi::SUBSCRIPTION,
                },
                Registered {
                    login: Arc::new(KimiOAuth::new()),
                    endpoint: Moonshot::CODING,
                },
            ]),
        }
    }

    /// Resolves a stored subscription without exposing its token.
    #[must_use]
    pub(crate) fn credential(
        &self,
        provider: &str,
        stored: &StoredCredentials,
    ) -> Option<Resolved> {
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
