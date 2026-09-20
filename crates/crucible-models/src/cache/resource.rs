//! The lifecycle of a provider-side prompt-cache resource.
//!
//! Creating, resolving, renewing and deleting one exact resource is a provider
//! adapter's to implement; the record it hands back is `crucible-types`, and
//! where that record is kept is `crucible-storage`.

use crate::Request;
use crucible_runtime::Cancel;
use crucible_types::{
    PromptCacheResourceBinding, PromptCacheResourceError, PromptCacheResourceHandle,
    PromptCacheResourceId, PromptCacheResourceRecord, PromptCacheResourceState,
    PromptCacheRetention,
};

use std::fmt;
use std::time::Instant;

/// Borrowed validated reference an adapter may lower into its wire request.
#[derive(Clone, Copy)]
pub struct PromptCacheResourceReference<'a> {
    id: &'a PromptCacheResourceId,
    handle: &'a PromptCacheResourceHandle,
}

impl<'a> PromptCacheResourceReference<'a> {
    /// Joins the local identity with the provider handle resolved for it.
    #[must_use]
    pub const fn new(id: &'a PromptCacheResourceId, handle: &'a PromptCacheResourceHandle) -> Self {
        Self { id, handle }
    }

    /// Local identity for attribution and store updates.
    #[must_use]
    pub const fn id(self) -> &'a PromptCacheResourceId {
        self.id
    }

    /// Provider handle for adapter wire lowering only.
    #[must_use]
    pub const fn handle(self) -> &'a PromptCacheResourceHandle {
        self.handle
    }
}

impl fmt::Debug for PromptCacheResourceReference<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptCacheResourceReference([redacted])")
    }
}

/// One absolute deadline for a blocking lifecycle operation.
#[derive(Debug, Clone, Copy)]
pub struct PromptCacheResourceDeadline(Instant);

impl PromptCacheResourceDeadline {
    /// Sets an absolute deadline.
    #[must_use]
    pub const fn new(deadline: Instant) -> Self {
        Self(deadline)
    }

    /// Absolute deadline used to derive a child cancellation token.
    #[must_use]
    pub const fn instant(self) -> Instant {
        self.0
    }

    /// Whether the operation has no time remaining.
    #[must_use]
    pub fn expired(self) -> bool {
        Instant::now() >= self.0
    }
}

/// Borrowed creation input. Prompt bytes live only for the blocking call.
#[derive(Clone, Copy)]
pub struct PromptCacheResourceCreate<'a> {
    /// Local idempotency identity minted before the call.
    pub id: &'a PromptCacheResourceId,
    /// Exact provider request whose stable prefix may be materialized remotely.
    pub request: &'a Request<'a>,
    /// Exact scope, prefix, policy, owner, and model binding.
    pub binding: &'a PromptCacheResourceBinding,
    /// Current user-authorized retention ceiling.
    pub retention: PromptCacheRetention,
    /// Absolute operation deadline.
    pub deadline: PromptCacheResourceDeadline,
}

impl fmt::Debug for PromptCacheResourceCreate<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptCacheResourceCreate")
            .field("id", &"[redacted]")
            .field("request", &"[provider-visible content redacted]")
            .field("binding", &self.binding)
            .field("retention", &self.retention)
            .field("deadline", &self.deadline)
            .finish()
    }
}

/// Successful remote creation result.
#[derive(Debug, Clone)]
pub struct PromptCacheResourceCreated {
    /// Opaque provider handle.
    pub handle: PromptCacheResourceHandle,
    /// Exact provider expiry as Unix seconds.
    pub expires_at: u64,
}

/// Provider-side status returned by resolve, renew, reconcile, or inspect.
#[derive(Debug, Clone)]
pub struct PromptCacheResourceRemote {
    /// Provider handle where reconciliation recovered or retained one.
    pub handle: Option<PromptCacheResourceHandle>,
    /// Provider-observed lifecycle state.
    pub state: PromptCacheResourceState,
    /// Provider-observed expiry where the resource still exists.
    pub expires_at: Option<u64>,
}

/// Blocking provider lifecycle for persistent cached-content resources.
///
/// Implementations use the supplied cancellation token and absolute deadline;
/// they retain neither the request nor any prompt bytes after returning.
pub trait PromptCacheResourceLifecycle: Send + Sync {
    /// Creates one resource under the local idempotency identity.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle error when the bounded operation is rejected,
    /// cancelled, times out, or has an ambiguous remote outcome.
    fn create(
        &self,
        request: PromptCacheResourceCreate<'_>,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceCreated, PromptCacheResourceError>;

    /// Resolves and validates an existing record before reuse.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle error when the bounded operation cannot
    /// establish the remote resource's state.
    fn resolve(
        &self,
        record: &PromptCacheResourceRecord,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError>;

    /// Renews one resource no later than the supplied policy ceiling.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle error when renewal is rejected, cancelled,
    /// times out, or has an ambiguous remote outcome.
    fn renew(
        &self,
        record: &PromptCacheResourceRecord,
        retention: PromptCacheRetention,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError>;

    /// Deletes one provider resource idempotently.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle error when deletion is rejected, cancelled,
    /// times out, or has an ambiguous remote outcome.
    fn delete(
        &self,
        record: &PromptCacheResourceRecord,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError>;

    /// Reconciles an ambiguous create, renew, or delete.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle error when the remote outcome still cannot be
    /// established safely.
    fn reconcile(
        &self,
        record: &PromptCacheResourceRecord,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError>;

    /// Inspects current remote state without changing it.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle error when inspection is rejected, cancelled,
    /// times out, or remains ambiguous.
    fn inspect(
        &self,
        record: &PromptCacheResourceRecord,
        deadline: PromptCacheResourceDeadline,
        cancel: &Cancel,
    ) -> Result<PromptCacheResourceRemote, PromptCacheResourceError>;
}
