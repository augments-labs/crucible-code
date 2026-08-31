//! Persistent provider-side prompt-cache resource contracts.

use std::fmt;
use std::io;
use std::time::Instant;

use uuid::Uuid;

use crate::{Cancel, ProviderAttemptId, Request};

use super::{
    PromptCacheFingerprint, PromptCacheIsolation, PromptCacheRetention, PromptCacheScopeDigest,
};

/// Largest opaque provider handle retained in local metadata.
pub const MAX_PROMPT_CACHE_HANDLE_BYTES: usize = 512;

/// Largest model or model-revision word retained in local metadata.
pub const MAX_PROMPT_CACHE_RESOURCE_WORD_BYTES: usize = 256;

/// Largest number of persistent resource records one store may retain.
pub const MAX_PROMPT_CACHE_RESOURCES: usize = 128;

/// Why a bounded resource word was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PromptCacheResourceWordError {
    /// The required word was empty.
    #[error("prompt-cache resource metadata word is empty")]
    Empty,
    /// The word exceeded its retained-data ceiling.
    #[error("prompt-cache resource metadata word is too long")]
    TooLong,
    /// A control character would make the one-line metadata representation ambiguous.
    #[error("prompt-cache resource metadata word contains a control character")]
    Control,
    /// A local identifier was not Crucible's canonical UUID-v7 spelling.
    #[error("prompt-cache resource identifier is not a canonical UUID v7")]
    InvalidId,
}

fn bounded_word(
    value: impl Into<Box<str>>,
    maximum: usize,
) -> Result<Box<str>, PromptCacheResourceWordError> {
    let value = value.into();
    if value.is_empty() {
        return Err(PromptCacheResourceWordError::Empty);
    }
    if value.len() > maximum {
        return Err(PromptCacheResourceWordError::TooLong);
    }
    if value.chars().any(char::is_control) {
        return Err(PromptCacheResourceWordError::Control);
    }
    Ok(value)
}

/// Opaque local identity for a separately managed remote cached-content resource.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PromptCacheResourceId(Box<str>);

impl PromptCacheResourceId {
    /// Mints an identifier that can safely key one local metadata record.
    #[must_use]
    pub fn new() -> Self {
        Self(Uuid::now_v7().to_string().into())
    }

    /// Reads the canonical identifier stored in private metadata.
    ///
    /// # Errors
    ///
    /// Returns an error unless `value` is Crucible's canonical UUID-v7
    /// spelling.
    pub fn parse(value: &str) -> Result<Self, PromptCacheResourceWordError> {
        let uuid = Uuid::try_parse(value).map_err(|_| PromptCacheResourceWordError::InvalidId)?;
        if uuid.get_version_num() != 7 || uuid.to_string() != value {
            return Err(PromptCacheResourceWordError::InvalidId);
        }
        Ok(Self(value.into()))
    }

    /// The local opaque identifier. Ordinary diagnostics must keep this redacted.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl Default for PromptCacheResourceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for PromptCacheResourceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptCacheResourceId([redacted])")
    }
}

/// Provider authorization-bearing handle for one remote cached-content resource.
#[derive(Clone, PartialEq, Eq)]
pub struct PromptCacheResourceHandle(Box<str>);

impl PromptCacheResourceHandle {
    /// Validates one bounded opaque provider handle.
    ///
    /// # Errors
    ///
    /// Returns an error for an empty, oversized, or control-bearing handle.
    pub fn new(value: impl Into<Box<str>>) -> Result<Self, PromptCacheResourceWordError> {
        bounded_word(value, MAX_PROMPT_CACHE_HANDLE_BYTES).map(Self)
    }

    /// The provider spelling, available only to the lifecycle adapter and private store.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for PromptCacheResourceHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptCacheResourceHandle([redacted])")
    }
}

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

/// Digest of the effective policy that authorized a persistent resource.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptCachePolicyDigest([u8; 32]);

impl PromptCachePolicyDigest {
    /// Takes a domain-separated digest derived by the runner.
    #[must_use]
    pub const fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Bytes for durable private metadata, never ordinary diagnostics.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for PromptCachePolicyDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptCachePolicyDigest([redacted])")
    }
}

/// Ownership semantics for one persistent resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheResourceOwner {
    isolation: PromptCacheIsolation,
    exclusive: bool,
}

impl PromptCacheResourceOwner {
    /// Records the already-resolved sharing scope and deletion ownership.
    #[must_use]
    pub const fn new(isolation: PromptCacheIsolation, exclusive: bool) -> Self {
        Self {
            isolation,
            exclusive,
        }
    }

    /// Broadest scope across which this resource was authorized.
    #[must_use]
    pub const fn isolation(self) -> PromptCacheIsolation {
        self.isolation
    }

    /// Whether removing its owner authorizes remote deletion.
    #[must_use]
    pub const fn exclusive(self) -> bool {
        self.exclusive
    }
}

/// Exact non-plaintext binding that must match before a resource can be reused.
#[derive(Clone, PartialEq, Eq)]
pub struct PromptCacheResourceBinding {
    scope: PromptCacheScopeDigest,
    provider_scope: PromptCacheScopeDigest,
    owner_scope: PromptCacheScopeDigest,
    prefix: PromptCacheFingerprint,
    policy: PromptCachePolicyDigest,
    owner: PromptCacheResourceOwner,
    protocol: Box<str>,
    model: Box<str>,
    revision: Option<Box<str>>,
}

impl PromptCacheResourceBinding {
    /// Builds one exact resource binding from already-derived redacted identities.
    ///
    /// # Errors
    ///
    /// Returns an error when a retained protocol, model, or revision word is
    /// empty, oversized, or contains a control character.
    // These are the independent fields of the durable binding identity; a
    // wrapper would duplicate the same construction contract.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        scope: PromptCacheScopeDigest,
        provider_scope: PromptCacheScopeDigest,
        owner_scope: PromptCacheScopeDigest,
        prefix: PromptCacheFingerprint,
        policy: PromptCachePolicyDigest,
        owner: PromptCacheResourceOwner,
        protocol: impl Into<Box<str>>,
        model: impl Into<Box<str>>,
        revision: Option<&str>,
    ) -> Result<Self, PromptCacheResourceWordError> {
        Ok(Self {
            scope,
            provider_scope,
            owner_scope,
            prefix,
            policy,
            owner,
            protocol: bounded_word(protocol, MAX_PROMPT_CACHE_RESOURCE_WORD_BYTES)?,
            model: bounded_word(model, MAX_PROMPT_CACHE_RESOURCE_WORD_BYTES)?,
            revision: revision
                .map(|value| bounded_word(value, MAX_PROMPT_CACHE_RESOURCE_WORD_BYTES))
                .transpose()?,
        })
    }

    /// Authority/endpoint/credential/sharing-scope digest.
    #[must_use]
    pub const fn scope(&self) -> PromptCacheScopeDigest {
        self.scope
    }

    /// Provider route and credential identity used to constrain lifecycle calls.
    #[must_use]
    pub const fn provider_scope(&self) -> PromptCacheScopeDigest {
        self.provider_scope
    }

    /// Exact run/session/workspace/user owner permitted to retire this resource.
    #[must_use]
    pub const fn owner_scope(&self) -> PromptCacheScopeDigest {
        self.owner_scope
    }

    /// Exact provider-visible stable-prefix fingerprint.
    #[must_use]
    pub const fn prefix(&self) -> PromptCacheFingerprint {
        self.prefix
    }

    /// Policy digest that authorized creation or reuse.
    #[must_use]
    pub const fn policy(&self) -> PromptCachePolicyDigest {
        self.policy
    }

    /// Ownership and sharing scope.
    #[must_use]
    pub const fn owner(&self) -> PromptCacheResourceOwner {
        self.owner
    }

    /// Exact provider protocol whose lifecycle owns the remote handle.
    #[must_use]
    pub fn protocol(&self) -> &str {
        &self.protocol
    }

    /// Exact resolved model.
    #[must_use]
    pub fn model(&self) -> &str {
        &self.model
    }

    /// Exact published model revision, where one exists.
    #[must_use]
    pub fn revision(&self) -> Option<&str> {
        self.revision.as_deref()
    }
}

impl fmt::Debug for PromptCacheResourceBinding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptCacheResourceBinding")
            .field("scope", &"[redacted]")
            .field("provider_scope", &"[redacted]")
            .field("owner_scope", &"[redacted]")
            .field("prefix", &"[redacted]")
            .field("policy", &"[redacted]")
            .field("owner", &self.owner)
            .field("protocol", &self.protocol)
            .field("model", &"[redacted]")
            .field("revision", &self.revision.as_ref().map(|_| "[redacted]"))
            .finish()
    }
}

/// Durable lifecycle state of one remote resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheResourceState {
    /// Creation began but no conclusive provider result is stored yet.
    Creating,
    /// Provider resolution confirmed the resource is eligible for use.
    Ready,
    /// A renewal should be attempted before another use.
    Expiring,
    /// Deletion began but has not been confirmed.
    Deleting,
    /// Remote deletion was confirmed.
    Deleted,
    /// The recorded expiry has passed.
    Expired,
    /// A create, renew, or delete may have reached the provider.
    Ambiguous,
    /// Remote cleanup could not be confirmed within the bounded retry policy.
    Orphaned,
}

impl PromptCacheResourceState {
    /// Canonical private-metadata spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "creating",
            Self::Ready => "ready",
            Self::Expiring => "expiring",
            Self::Deleting => "deleting",
            Self::Deleted => "deleted",
            Self::Expired => "expired",
            Self::Ambiguous => "ambiguous",
            Self::Orphaned => "orphaned",
        }
    }

    /// Reads a canonical private-metadata spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "creating" => Some(Self::Creating),
            "ready" => Some(Self::Ready),
            "expiring" => Some(Self::Expiring),
            "deleting" => Some(Self::Deleting),
            "deleted" => Some(Self::Deleted),
            "expired" => Some(Self::Expired),
            "ambiguous" => Some(Self::Ambiguous),
            "orphaned" => Some(Self::Orphaned),
            _ => None,
        }
    }
}

/// Lifecycle operation whose outcome may need reconciliation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheResourceOperation {
    /// Create a new provider resource.
    Create,
    /// Extend a provider resource without exceeding current policy.
    Renew,
    /// Delete a provider resource.
    Delete,
}

/// Immutable bounded fact about one persistent-resource state change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheResourceFact {
    /// Provider attempt this preparation served, absent for explicit cleanup.
    pub attempt: Option<ProviderAttemptId>,
    /// Opaque local resource identity; its debug form is redacted.
    pub resource: PromptCacheResourceId,
    /// Lifecycle operation in progress or reconciled, where one applies.
    pub operation: Option<PromptCacheResourceOperation>,
    /// New durable state.
    pub state: PromptCacheResourceState,
    /// Provider expiry in Unix seconds, where known.
    pub expires_at: Option<u64>,
    /// Sharing and deletion ownership without the owner identity bytes.
    pub owner: PromptCacheResourceOwner,
}

impl PromptCacheResourceOperation {
    /// Canonical private-metadata spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Renew => "renew",
            Self::Delete => "delete",
        }
    }

    /// Reads a canonical private-metadata spelling.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "create" => Some(Self::Create),
            "renew" => Some(Self::Renew),
            "delete" => Some(Self::Delete),
            _ => None,
        }
    }
}

/// Bounded private metadata for one persistent remote resource.
#[derive(Clone)]
pub struct PromptCacheResourceRecord {
    id: PromptCacheResourceId,
    binding: PromptCacheResourceBinding,
    handle: Option<PromptCacheResourceHandle>,
    state: PromptCacheResourceState,
    pending: Option<PromptCacheResourceOperation>,
    created_at: u64,
    expires_at: Option<u64>,
    last_reconciled_at: Option<u64>,
}

impl PromptCacheResourceRecord {
    /// Starts a local idempotency record before remote creation is attempted.
    #[must_use]
    pub const fn creating(
        id: PromptCacheResourceId,
        binding: PromptCacheResourceBinding,
        created_at: u64,
    ) -> Self {
        Self {
            id,
            binding,
            handle: None,
            state: PromptCacheResourceState::Creating,
            pending: Some(PromptCacheResourceOperation::Create),
            created_at,
            expires_at: None,
            last_reconciled_at: None,
        }
    }

    /// Restores a fully validated record from private local metadata.
    ///
    /// # Errors
    ///
    /// Returns an error when the persisted lifecycle state, pending operation,
    /// handle, and expiry fields do not form a valid state-machine record.
    // Restoration deliberately names every persisted field so validation is
    // performed at this one boundary rather than spread across setters.
    #[allow(clippy::too_many_arguments)]
    pub fn restored(
        id: PromptCacheResourceId,
        binding: PromptCacheResourceBinding,
        handle: Option<PromptCacheResourceHandle>,
        state: PromptCacheResourceState,
        pending: Option<PromptCacheResourceOperation>,
        created_at: u64,
        expires_at: Option<u64>,
        last_reconciled_at: Option<u64>,
    ) -> Result<Self, PromptCacheResourceError> {
        let valid = match state {
            PromptCacheResourceState::Creating => {
                pending == Some(PromptCacheResourceOperation::Create)
                    && handle.is_none()
                    && expires_at.is_none()
            }
            PromptCacheResourceState::Ready => {
                pending.is_none() && handle.is_some() && expires_at.is_some()
            }
            PromptCacheResourceState::Expiring => {
                pending == Some(PromptCacheResourceOperation::Renew)
                    && handle.is_some()
                    && expires_at.is_some()
            }
            PromptCacheResourceState::Deleting => {
                pending == Some(PromptCacheResourceOperation::Delete)
            }
            PromptCacheResourceState::Ambiguous => pending.is_some(),
            PromptCacheResourceState::Deleted
            | PromptCacheResourceState::Expired
            | PromptCacheResourceState::Orphaned => pending.is_none(),
        };
        if !valid {
            return Err(PromptCacheResourceError::InvalidMetadata);
        }
        Ok(Self {
            id,
            binding,
            handle,
            state,
            pending,
            created_at,
            expires_at,
            last_reconciled_at,
        })
    }

    /// Marks successful creation or reconciliation.
    pub fn ready(
        &mut self,
        handle: PromptCacheResourceHandle,
        expires_at: u64,
        reconciled_at: u64,
    ) {
        self.handle = Some(handle);
        self.state = PromptCacheResourceState::Ready;
        self.pending = None;
        self.expires_at = Some(expires_at);
        self.last_reconciled_at = Some(reconciled_at);
    }

    /// Records a lifecycle state transition without changing sensitive fields.
    pub fn set_state(&mut self, state: PromptCacheResourceState, at: u64) {
        self.state = state;
        self.pending = match state {
            PromptCacheResourceState::Creating => Some(PromptCacheResourceOperation::Create),
            PromptCacheResourceState::Expiring => Some(PromptCacheResourceOperation::Renew),
            PromptCacheResourceState::Deleting => Some(PromptCacheResourceOperation::Delete),
            PromptCacheResourceState::Ready
            | PromptCacheResourceState::Deleted
            | PromptCacheResourceState::Expired
            | PromptCacheResourceState::Orphaned => None,
            PromptCacheResourceState::Ambiguous => self.pending,
        };
        self.last_reconciled_at = Some(at);
    }

    /// Records an operation whose provider outcome is unknown.
    pub fn ambiguous(&mut self, operation: PromptCacheResourceOperation, at: u64) {
        self.state = PromptCacheResourceState::Ambiguous;
        self.pending = Some(operation);
        self.last_reconciled_at = Some(at);
    }

    /// Whether this exact binding is confirmed ready before its expiry.
    #[must_use]
    pub fn can_reuse(&self, binding: &PromptCacheResourceBinding, now: u64) -> bool {
        self.binding == *binding
            && self.state == PromptCacheResourceState::Ready
            && self.handle.is_some()
            && self.expires_at.is_some_and(|expiry| now < expiry)
    }

    /// Local opaque record identity.
    #[must_use]
    pub const fn id(&self) -> &PromptCacheResourceId {
        &self.id
    }

    /// Exact redacted binding.
    #[must_use]
    pub const fn binding(&self) -> &PromptCacheResourceBinding {
        &self.binding
    }

    /// Provider handle, for private storage and lifecycle calls only.
    #[must_use]
    pub const fn handle(&self) -> Option<&PromptCacheResourceHandle> {
        self.handle.as_ref()
    }

    /// Current durable state.
    #[must_use]
    pub const fn state(&self) -> PromptCacheResourceState {
        self.state
    }

    /// Operation requiring reconciliation, where one exists.
    #[must_use]
    pub const fn pending(&self) -> Option<PromptCacheResourceOperation> {
        self.pending
    }

    /// Creation time as Unix seconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Provider expiry as Unix seconds.
    #[must_use]
    pub const fn expires_at(&self) -> Option<u64> {
        self.expires_at
    }

    /// Last bounded reconciliation attempt as Unix seconds.
    #[must_use]
    pub const fn last_reconciled_at(&self) -> Option<u64> {
        self.last_reconciled_at
    }
}

impl fmt::Debug for PromptCacheResourceRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptCacheResourceRecord")
            .field("id", &"[redacted]")
            .field("binding", &self.binding)
            .field("handle", &self.handle.as_ref().map(|_| "[redacted]"))
            .field("state", &self.state)
            .field("pending", &self.pending)
            .field("created_at", &self.created_at)
            .field("expires_at", &self.expires_at)
            .field("last_reconciled_at", &self.last_reconciled_at)
            .finish()
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

/// Bounded reason a persistent-resource operation failed.
#[derive(Debug, thiserror::Error)]
pub enum PromptCacheResourceError {
    /// Cancellation was observed before a conclusive result.
    #[error("prompt-cache resource operation was cancelled")]
    Cancelled,
    /// The operation's explicit deadline elapsed.
    #[error("prompt-cache resource operation reached its deadline")]
    Deadline,
    /// This provider/model exposes no matching lifecycle.
    #[error("prompt-cache persistent resources are unsupported")]
    Unsupported,
    /// Provider rejected the bounded lifecycle operation.
    #[error("provider rejected the prompt-cache resource operation")]
    Rejected,
    /// The provider may have applied this operation and must be reconciled.
    #[error("prompt-cache resource {0:?} outcome is ambiguous")]
    Ambiguous(PromptCacheResourceOperation),
    /// Private metadata was invalid or from another format.
    #[error("prompt-cache resource metadata is invalid")]
    InvalidMetadata,
    /// The bounded store has reached its record ceiling.
    #[error("prompt-cache resource metadata store is full")]
    StoreFull,
    /// A private filesystem operation failed.
    #[error("could not {operation} prompt-cache resource metadata: {source}")]
    Local {
        /// Stable operation name, without a path or resource handle.
        operation: &'static str,
        /// Operating-system failure.
        #[source]
        source: io::Error,
    },
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

/// Private bounded metadata store, implemented above core using a resolved user-home path.
pub trait PromptCacheResourceStore: Send + fmt::Debug {
    /// Finds the newest exact binding, including non-ready records for reconciliation.
    ///
    /// An older orphan may be retained for explicit cleanup after a replacement
    /// is created. Selecting the newest record prevents that cleanup evidence
    /// from shadowing the usable replacement after restart.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when private metadata cannot be read or
    /// validated.
    fn matching(
        &mut self,
        binding: &PromptCacheResourceBinding,
    ) -> Result<Option<PromptCacheResourceRecord>, PromptCacheResourceError>;

    /// Inserts or replaces one record by local id.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when the bound is reached or private
    /// metadata cannot be persisted.
    fn put(&mut self, record: &PromptCacheResourceRecord) -> Result<(), PromptCacheResourceError>;

    /// Removes one confirmed-deleted local record.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when private metadata cannot be updated.
    fn remove(&mut self, id: &PromptCacheResourceId) -> Result<(), PromptCacheResourceError>;

    /// Returns at most `maximum` records for bounded inspection or cleanup.
    ///
    /// # Errors
    ///
    /// Returns a typed store error when private metadata cannot be read or
    /// validated.
    fn inspect(
        &mut self,
        maximum: usize,
    ) -> Result<Vec<PromptCacheResourceRecord>, PromptCacheResourceError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PromptCacheFingerprint, PromptCacheIsolation, PromptCacheScopeDigest};

    fn binding() -> PromptCacheResourceBinding {
        PromptCacheResourceBinding::new(
            PromptCacheScopeDigest::new([1; 32]),
            PromptCacheScopeDigest::new([4; 32]),
            PromptCacheScopeDigest::new([5; 32]),
            PromptCacheFingerprint::new([2; 32]),
            PromptCachePolicyDigest::new([3; 32]),
            PromptCacheResourceOwner::new(PromptCacheIsolation::Session, false),
            "fixture",
            "model-a",
            Some("revision-a"),
        )
        .unwrap()
    }

    #[test]
    fn resource_words_are_bounded_and_redacted() {
        assert!(PromptCacheResourceHandle::new("h".repeat(MAX_PROMPT_CACHE_HANDLE_BYTES)).is_ok());
        assert!(
            PromptCacheResourceHandle::new("h".repeat(MAX_PROMPT_CACHE_HANDLE_BYTES + 1)).is_err()
        );
        assert!(PromptCacheResourceHandle::new("line\nbreak").is_err());

        let handle = PromptCacheResourceHandle::new("provider-secret-handle").unwrap();
        assert_eq!(
            format!("{handle:?}"),
            "PromptCacheResourceHandle([redacted])"
        );
    }

    #[test]
    fn reuse_requires_the_exact_binding_ready_and_unexpired() {
        let wanted = binding();
        let mut record =
            PromptCacheResourceRecord::creating(PromptCacheResourceId::new(), wanted.clone(), 100);
        assert!(!record.can_reuse(&wanted, 100));

        record.ready(
            PromptCacheResourceHandle::new("remote-1").unwrap(),
            200,
            110,
        );
        assert!(record.can_reuse(&wanted, 199));
        assert!(!record.can_reuse(&wanted, 200));

        let other = PromptCacheResourceBinding::new(
            PromptCacheScopeDigest::new([9; 32]),
            wanted.provider_scope(),
            wanted.owner_scope(),
            wanted.prefix(),
            wanted.policy(),
            wanted.owner(),
            wanted.protocol(),
            wanted.model(),
            wanted.revision(),
        )
        .unwrap();
        assert!(!record.can_reuse(&other, 150));
    }

    #[test]
    fn ambiguous_and_orphaned_operations_are_never_reused() {
        let wanted = binding();
        for state in [
            PromptCacheResourceState::Ambiguous,
            PromptCacheResourceState::Orphaned,
            PromptCacheResourceState::Deleting,
            PromptCacheResourceState::Deleted,
            PromptCacheResourceState::Expired,
        ] {
            let mut record = PromptCacheResourceRecord::creating(
                PromptCacheResourceId::new(),
                wanted.clone(),
                100,
            );
            record.set_state(state, 120);
            assert!(!record.can_reuse(&wanted, 121), "{state:?}");
        }
    }

    #[test]
    fn restored_records_enforce_lifecycle_state_invariants() {
        let invalid = PromptCacheResourceRecord::restored(
            PromptCacheResourceId::new(),
            binding(),
            Some(PromptCacheResourceHandle::new("remote-1").unwrap()),
            PromptCacheResourceState::Ready,
            Some(PromptCacheResourceOperation::Delete),
            100,
            Some(200),
            Some(110),
        );
        assert!(matches!(
            invalid,
            Err(PromptCacheResourceError::InvalidMetadata)
        ));

        let valid = PromptCacheResourceRecord::restored(
            PromptCacheResourceId::new(),
            binding(),
            Some(PromptCacheResourceHandle::new("remote-1").unwrap()),
            PromptCacheResourceState::Expiring,
            Some(PromptCacheResourceOperation::Renew),
            100,
            Some(200),
            Some(110),
        )
        .unwrap();
        assert_eq!(valid.state(), PromptCacheResourceState::Expiring);
    }
}
