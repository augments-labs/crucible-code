//! How each provider is reached: the one resolver every credential question
//! is answered through.
//!
//! A provider is reached by a subscription `/login` signed in, by an API key,
//! or through an installed agent. `providers.<name>.access` may say which, and
//! then nothing falls back from it: a key exported in the shell does not
//! answer for a provider whose access says subscription, and an account held
//! in the store does not answer for one whose access says apiKey. Left off, the
//! order crucible has always resolved in decides, unchanged: a subscription
//! where one is signed in and no address is configured, then the variable's
//! key, then the stored key.
//!
//! Construction, the content-use hold, `/login`, `/model` and `doctor` all ask
//! this module and none decides the order on its own, so none can say a
//! provider is reached one way while requests go another. What it hands back is
//! a [`Route`], which names where a credential is and never holds one.
//! Construction alone asks for a settled route, which carries the variable's
//! key where the route reads one: the variable is read once, while the route
//! is decided, so a request is signed with the value the decision was made on.
//!
//! This is also the one place that says which variable a provider's key is
//! taken from, so a setting that renames it is honoured everywhere a key is
//! looked for, or nowhere. `/login` asks the configuration only whether one is
//! renamed, to say so.

use crucible_auth::{Held, Kind};
use crucible_config::{Access, Driver, Settings};
use crucible_credentials::{ApiKey, CredentialError};

use crate::providers::Served;
use crate::startup::ProviderAuth;

/// How one provider's requests will be sent, resolved from what is configured
/// and what is held.
///
/// Holds no secret. A key's variable and a stored credential's name are where
/// to read it, and nothing here reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The account `/login` signed in, at the vendor's own address.
    Subscription {
        /// The provider it signs in to.
        provider: &'static str,
        /// The name the store holds the account under, which is its row.
        account: Box<str>,
    },
    /// An API key.
    ApiKey {
        /// The provider it is sent to.
        provider: &'static str,
        /// Where the key is read from.
        source: KeySource,
    },
    /// An installed agent, started with its own sign-in.
    External {
        /// The provider whose turns it runs.
        provider: &'static str,
        /// The `externalAgents` entry it is.
        instance: Box<str>,
        /// Which agent.
        driver: Driver,
        /// Its program, as configured: absolute, or a bare name for `PATH`.
        executable: Box<str>,
        /// The profile directory its driver's key names, where one is written.
        profile: Option<Box<str>>,
        /// Whether new work may be sent through it. Carried rather than
        /// refused here, because a disabled agent is still the route the user
        /// chose: what to say about it belongs to whoever would have started it.
        enabled: bool,
    },
}

/// A route as provider construction applies it.
///
/// Built here alone, so the key it carries is always the one the route's
/// variable held, and only where the route is the variable's.
#[derive(Debug)]
pub(crate) struct Settled {
    /// The route.
    route: Route,
    /// The key the variable held, where the route is the variable's: read
    /// while the route was decided, and applied rather than read again.
    exported: Option<ApiKey>,
}

impl Settled {
    /// The route.
    pub(crate) fn route(&self) -> &Route {
        &self.route
    }

    /// The route, and the key the variable held where the route reads it.
    pub(crate) fn into_parts(self) -> (Route, Option<ApiKey>) {
        (self.route, self.exported)
    }
}

/// Where an API key is read from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    /// An environment variable, by name.
    Environment {
        /// The variable's name; never its value.
        variable: Box<str>,
    },
    /// The key `/login` stored, under this name.
    Stored {
        /// The name the store holds it under, which is its row.
        name: Box<str>,
    },
}

/// Why a provider has no route.
///
/// Every sentence names the setting and the one method its access chose. A
/// provider whose access was written down is never told to use the other
/// method: that would be crucible overruling a choice the file made.
#[derive(Debug, thiserror::Error)]
pub enum AccessError {
    /// Nothing is held for a provider whose access was left off.
    #[error(transparent)]
    Absent(CredentialError),

    /// A subscription is held for a provider whose access was left off, and an
    /// address is configured that a subscription is never sent to.
    #[error("providers.{provider}.baseUrl cannot be used with a subscription login")]
    SubscriptionAddress {
        /// The provider whose subscription was found.
        provider: Box<str>,
    },

    /// A subscription access with an address configured.
    #[error(
        "providers.{provider}.access is subscription, which signs in only at {provider}'s own \
         address, and providers.{provider}.baseUrl sends requests elsewhere; remove baseUrl"
    )]
    SubscriptionElsewhere {
        /// The provider.
        provider: Box<str>,
    },

    /// A subscription access for a provider this build has no subscription
    /// sign-in for.
    #[error(
        "providers.{provider}.access is subscription, and this build has no subscription \
         sign-in for {provider}"
    )]
    NoSubscription {
        /// The provider.
        provider: Box<str>,
    },

    /// A subscription access with no account signed in.
    #[error(
        "providers.{provider}.access is subscription, and no account is signed in for \
         {provider}; sign in with /login"
    )]
    NotSignedIn {
        /// The provider.
        provider: Box<str>,
    },

    /// An API key access with no key set or stored.
    #[error(
        "providers.{provider}.access is apiKey, and no key was found: set {variable}, or store \
         one with /login"
    )]
    NoKey {
        /// The provider.
        provider: Box<str>,
        /// The variable a key would have been read from.
        variable: Box<str>,
    },

    /// An agent access beside a key that only a request crucible sends reads.
    #[error(
        "providers.{provider}.access is externalAgent, which the agent signs in to and sends \
         for itself, and providers.{provider}.{key} is set; remove {key}"
    )]
    AgentConflict {
        /// The provider.
        provider: Box<str>,
        /// The key written beside it.
        key: &'static str,
    },

    /// An agent access naming an instance the document does not have.
    #[error(
        "providers.{provider}.access names the instance {instance}, and externalAgents has no \
         entry {instance}"
    )]
    UnknownInstance {
        /// The provider.
        provider: Box<str>,
        /// The name it gave.
        instance: Box<str>,
    },
}

/// The variable `one`'s key is read from: the one the configuration names, or
/// the provider's own.
#[must_use]
pub fn variable(one: Served, settings: &Settings) -> &str {
    settings.api_key_env(one.name).unwrap_or(one.key)
}

/// Every variable `one`'s key could be read from: its own, and the one the
/// configuration names in its place, where it names one.
///
/// Both, because naming another does not empty the usual variable, and a key
/// exported before the setting was written is still a key.
pub fn variables(one: Served, settings: &Settings) -> impl Iterator<Item = &str> {
    std::iter::once(one.key).chain(settings.api_key_env(one.name))
}

/// The route `one`'s requests take, from the credentials `auth` holds now.
///
/// A subscription counts as signed in only where the login this build has for
/// it can produce its credential from the store, which is what construction
/// will ask for.
///
/// # Errors
///
/// [`AccessError`], naming the setting and what it is missing.
pub fn resolve(one: Served, auth: ProviderAuth<'_>) -> Result<Route, AccessError> {
    settle(one, auth).map(|settled| settled.route)
}

/// [`resolve`], keeping the variable's key for construction to apply.
pub(crate) fn settle(one: Served, auth: ProviderAuth<'_>) -> Result<Settled, AccessError> {
    settled(
        one,
        auth.settings,
        auth.from,
        auth.stored.held(one.name).as_ref(),
        Signing {
            offered: auth.subscriptions.supports(one.name),
            usable: auth
                .subscriptions
                .credential(one.name, auth.stored)
                .is_some(),
        },
    )
}

/// What this build knows about signing in to one provider by subscription.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Signing {
    /// Whether this build has a subscription sign-in for it at all.
    pub(crate) offered: bool,
    /// Whether the account held for it can sign a request. Where the store
    /// cannot be asked, as where only its inventory is read, an account held
    /// for a provider that is offered.
    pub(crate) usable: bool,
}

/// [`resolve`], from what the store holds for `one` rather than from the
/// store: `held` is the credential it holds, by map and name, however that was
/// found.
pub(crate) fn resolved(
    one: Served,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    held: Option<&Held>,
    signing: Signing,
) -> Result<Route, AccessError> {
    settled(one, settings, from, held, signing).map(|settled| settled.route)
}

fn settled(
    one: Served,
    settings: &Settings,
    from: &dyn Fn(&str) -> Option<String>,
    held: Option<&Held>,
    signing: Signing,
) -> Result<Settled, AccessError> {
    let provider = one.name;
    let account = held
        .filter(|held| held.kind == Kind::Account && signing.offered && signing.usable)
        .map(|held| Route::Subscription {
            provider,
            account: held.name.as_str().into(),
        });
    let variable = variable(one, settings);
    // The variable is read here at most once, and only where a key could
    // answer: a subscription chosen first leaves it unread.
    let key = || {
        if let Ok(exported) = ApiKey::from_lookup(variable, from) {
            return Some(Settled {
                route: Route::ApiKey {
                    provider,
                    source: KeySource::Environment {
                        variable: variable.into(),
                    },
                },
                exported: Some(exported),
            });
        }
        held.filter(|held| held.kind == Kind::Key)
            .map(|held| Settled {
                route: Route::ApiKey {
                    provider,
                    source: KeySource::Stored {
                        name: held.name.as_str().into(),
                    },
                },
                exported: None,
            })
    };
    let routed = |route| Settled {
        route,
        exported: None,
    };
    let addressed = settings.base_url(provider).is_some();

    match settings.access(provider) {
        None => {
            if !addressed && let Some(account) = account.clone() {
                return Ok(routed(account));
            }
            if let Some(found) = key() {
                return Ok(found);
            }
            if account.is_some() {
                return Err(AccessError::SubscriptionAddress {
                    provider: provider.into(),
                });
            }
            Err(AccessError::Absent(CredentialError::NotInEnvironment(
                variable.into(),
            )))
        }
        Some(Access::Subscription) => {
            if addressed {
                return Err(AccessError::SubscriptionElsewhere {
                    provider: provider.into(),
                });
            }
            if !signing.offered {
                return Err(AccessError::NoSubscription {
                    provider: provider.into(),
                });
            }
            account.map(routed).ok_or_else(|| AccessError::NotSignedIn {
                provider: provider.into(),
            })
        }
        Some(Access::ApiKey) => key().ok_or_else(|| AccessError::NoKey {
            provider: provider.into(),
            variable: variable.into(),
        }),
        Some(Access::ExternalAgent { instance }) => {
            let conflict = if addressed {
                Some("baseUrl")
            } else if settings.api_key_env(provider).is_some() {
                Some("apiKeyEnv")
            } else {
                None
            };
            if let Some(key) = conflict {
                return Err(AccessError::AgentConflict {
                    provider: provider.into(),
                    key,
                });
            }
            let agent = settings
                .external_agents()
                .into_iter()
                .find(|agent| agent.name() == &*instance)
                .ok_or_else(|| AccessError::UnknownInstance {
                    provider: provider.into(),
                    instance: instance.clone(),
                })?;
            Ok(routed(Route::External {
                provider,
                instance,
                driver: agent.driver(),
                executable: agent.executable().into(),
                profile: agent.profile().map(Into::into),
                enabled: agent.enabled(),
            }))
        }
    }
}

/// The fragment of a provider block that names the variable its key is read
/// from, for a test that writes one.
///
/// Here so the setting is spelled in this module alone.
#[cfg(test)]
pub(crate) fn naming(variable: &str) -> String {
    format!(r#""apiKeyEnv": "{variable}""#)
}

#[cfg(test)]
mod tests;
