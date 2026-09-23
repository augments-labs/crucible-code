//! Hostname lookups, bounded and, for some callers, under a deadline.
//!
//! The platform lookup cannot be cancelled: once it starts, its worker is
//! gone until the operating system answers. So each lookup takes a permit
//! from a [`Lookups`] owner and carries it into the worker, where it stays
//! until that call returns whatever became of the request that asked for it;
//! an owner never has more lookups on workers than its count. An owner is
//! made either poisoned or plain and stays so, so a stalled lookup of one
//! kind can never hold a permit the other kind waits for; the counts of all
//! owners together must stay below the runtime's blocking-thread limit, since
//! a started lookup keeps its worker whatever becomes of its request or its
//! client.
//!
//! A poisoned owner gives each lookup's permit wait and lookup 5 s together.
//! One that outlives that raises the poison, and every lookup under the same
//! poison afterwards fails at once rather than queue behind it. A plain
//! owner has neither: it is bounded by the deadline of the request around
//! it, and its stalls never raise the poison. It is a type of its own,
//! [`PlainLookups`], so that the lookup of a proxy's host, which must be
//! plain, cannot be handed a poisoned owner.

use std::fmt;
use std::io;
use std::net::{SocketAddr, ToSocketAddrs};
use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use std::vec;

use crucible_runtime::BoxFuture;
use hyper_util::client::legacy::connect::dns::Name;
use tokio::sync::{AcquireError, Semaphore};
use tower_service::Service;

/// How long a poisoned lookup may take, from asking for its permit.
const TIMEOUT_RESOLVE: Duration = Duration::from_secs(5);

/// The blocking platform call, a seam so tests never do live DNS.
pub(crate) trait Lookup: Send + Sync {
    fn lookup(&self, host: &str) -> io::Result<Vec<SocketAddr>>;
}

/// The operating system's resolver. The port is filled in by the connector.
struct System;

impl Lookup for System {
    fn lookup(&self, host: &str) -> io::Result<Vec<SocketAddr>> {
        (host, 0).to_socket_addrs().map(Iterator::collect)
    }
}

/// An owner of hostname lookups: the bound on how many run at once, and, for
/// a poisoned owner, the poison they obey and raise. It is what a connector
/// resolves names with.
#[derive(Clone)]
pub struct Lookups {
    pub(crate) permits: Arc<Semaphore>,
    lookup: Arc<dyn Lookup>,
    poison: Option<Poison>,
}

/// An owner of plain lookups: no deadline or poison of its own, and no way
/// to be given either. A proxy's host is looked up only with one of these,
/// so a proxied request can neither obey nor raise a poison.
///
/// It can look targets up too, converted into the [`Lookups`] a client takes
/// for them; the permits stay shared.
///
/// ```compile_fail,E0308
/// # use std::num::NonZeroUsize;
/// # use crucible_http::{Http, Lookups, Poison, ProxyEnv, Tls};
/// fn build(tls: &Tls, poison: &Poison) -> Http {
///     let target = Lookups::poisoned(NonZeroUsize::MIN, poison);
///     Http::new(tls, target.clone(), target, ProxyEnv::capture())
/// }
/// ```
///
/// ```
/// # use std::num::NonZeroUsize;
/// # use crucible_http::{Http, Lookups, Poison, ProxyEnv, Tls};
/// fn build(tls: &Tls, poison: &Poison) -> Http {
///     let target = Lookups::poisoned(NonZeroUsize::MIN, poison);
///     Http::new(tls, target.clone(), Lookups::plain(NonZeroUsize::MIN), ProxyEnv::capture())
/// }
/// ```
#[derive(Clone, Debug)]
pub struct PlainLookups(Lookups);

/// Raised once a poisoned lookup outlives its deadline, and never lowered.
///
/// An owned value rather than a process-wide flag: whoever builds the
/// owners makes one and hands it to those that are to obey it.
#[derive(Clone, Debug, Default)]
pub struct Poison(Arc<AtomicBool>);

/// A lookup that did not produce addresses.
#[derive(Debug, thiserror::Error)]
pub enum LookupError {
    /// This lookup outlived its deadline, or an earlier one did.
    #[error("hostname resolution stalled")]
    Stalled,
    /// The platform lookup failed: the name does not resolve, or the platform
    /// could not answer.
    #[error("hostname lookup failed")]
    Failed(#[from] io::Error),
    /// The owner's permits were closed.
    #[error("hostname lookups were closed")]
    Closed(#[from] AcquireError),
    /// The worker the lookup ran on stopped without answering.
    #[error("the hostname lookup stopped without answering")]
    Abandoned(#[from] tokio::task::JoinError),
}

impl Lookups {
    /// An owner of `count` lookups at once, on the runtime's blocking
    /// workers, under the 5 s deadline, that obey and raise `poison`: for the
    /// requests a stuck resolver must not hold up.
    #[must_use]
    pub fn poisoned(count: NonZeroUsize, poison: &Poison) -> Self {
        Self::with(count, Some(poison.clone()), Arc::new(System))
    }

    /// An owner of `count` lookups at once with no deadline or poison of
    /// their own, for requests that are bounded by a deadline of their own
    /// and for proxy hosts.
    #[must_use]
    pub fn plain(count: NonZeroUsize) -> PlainLookups {
        PlainLookups::with(count, Arc::new(System))
    }

    pub(crate) fn with(
        count: NonZeroUsize,
        poison: Option<Poison>,
        lookup: Arc<dyn Lookup>,
    ) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(count.get())),
            lookup,
            poison,
        }
    }

    async fn run(&self, host: String) -> Result<vec::IntoIter<SocketAddr>, LookupError> {
        let permit = Arc::clone(&self.permits).acquire_owned().await?;
        if self.poison.as_ref().is_some_and(Poison::is_raised) {
            return Err(LookupError::Stalled);
        }
        let lookup = Arc::clone(&self.lookup);
        let found = tokio::task::spawn_blocking(move || {
            let _held = permit;
            lookup.lookup(&host)
        });
        Ok(found.await??.into_iter())
    }
}

impl PlainLookups {
    pub(crate) fn with(count: NonZeroUsize, lookup: Arc<dyn Lookup>) -> Self {
        Self(Lookups::with(count, None, lookup))
    }
}

impl From<PlainLookups> for Lookups {
    fn from(plain: PlainLookups) -> Self {
        plain.0
    }
}

impl Poison {
    /// Whether a lookup has outlived its deadline.
    #[must_use]
    pub fn is_raised(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn raise(&self) {
        self.0.store(true, Ordering::Release);
    }
}

impl Lookups {
    async fn resolve(self, host: String) -> Result<vec::IntoIter<SocketAddr>, LookupError> {
        let Some(poison) = &self.poison else {
            return self.run(host).await;
        };
        if poison.is_raised() {
            return Err(LookupError::Stalled);
        }
        let found = tokio::time::timeout(TIMEOUT_RESOLVE, self.run(host));
        found.await.unwrap_or_else(|_| {
            poison.raise();
            Err(LookupError::Stalled)
        })
    }
}

impl Service<Name> for Lookups {
    type Response = vec::IntoIter<SocketAddr>;
    type Error = LookupError;
    type Future = BoxFuture<'static, Result<Self::Response, LookupError>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), LookupError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        Box::pin(self.clone().resolve(name.as_str().to_owned()))
    }
}

impl Service<Name> for PlainLookups {
    type Response = vec::IntoIter<SocketAddr>;
    type Error = LookupError;
    type Future = BoxFuture<'static, Result<Self::Response, LookupError>>;

    fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), LookupError>> {
        Poll::Ready(Ok(()))
    }

    fn call(&mut self, name: Name) -> Self::Future {
        self.0.call(name)
    }
}

impl fmt::Debug for Lookups {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lookups")
            .field("available", &self.permits.available_permits())
            .field("poisoned", &self.poison.is_some())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
pub(crate) mod tests;
