//! The execution-fact half of a durable checkpoint.
//!
//! What an unfinished execution needs in order to resume safely is owned by
//! `crucible-storage`. A checkpoint additionally binds the facts that only the
//! running system can state — the prompt-cache identity a resumed turn must
//! still match, and the sandbox lifecycles it must still find — so those parts
//! stay here with the fact models they name.

use std::fmt;

use crucible_storage::{
    ActionResolution, ApprovalDecision, CheckpointId, InterruptionError, InvocationRecord,
    MAX_CHECKPOINT_INVOCATIONS, MAX_CHECKPOINT_SANDBOXES, PendingActions, RecoveryAction,
    ResumeScope,
};
use crucible_types::Ancestry;

use crate::{
    PromptCacheFingerprint, PromptCacheResourceId, PromptCacheScopeDigest, SandboxCheckpoint,
};

fn bounded(field: &'static str, value: Box<str>) -> Result<Box<str>, InterruptionError> {
    InterruptionError::check_word(field, &value)?;
    Ok(value)
}

/// Minimal prompt-cache state allowed into an execution checkpoint.
#[derive(Clone)]
pub struct CacheCheckpoint {
    policy_version: Box<str>,
    capability_version: Box<str>,
    pricing_version: Option<Box<str>>,
    scope: PromptCacheScopeDigest,
    prefix: PromptCacheFingerprint,
    attempt: Option<crate::ProviderAttemptId>,
    resource: Option<PromptCacheResourceId>,
    expires_at: Option<u64>,
    reconcile: bool,
}

impl CacheCheckpoint {
    /// Builds cache resume evidence without request text, a routing key, or an
    /// authorization-bearing remote-resource handle.
    ///
    /// # Errors
    ///
    /// Version labels are bounded before retention.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        policy_version: impl Into<Box<str>>,
        capability_version: impl Into<Box<str>>,
        pricing_version: Option<impl Into<Box<str>>>,
        scope: PromptCacheScopeDigest,
        prefix: PromptCacheFingerprint,
        attempt: Option<crate::ProviderAttemptId>,
        resource: Option<PromptCacheResourceId>,
        expires_at: Option<u64>,
        reconcile: bool,
    ) -> Result<Self, InterruptionError> {
        let policy_version = bounded("cache policy version", policy_version.into())?;
        let capability_version = bounded("cache capability version", capability_version.into())?;
        let pricing_version = pricing_version
            .map(Into::into)
            .map(|word| bounded("cache pricing version", word))
            .transpose()?;
        Ok(Self {
            policy_version,
            capability_version,
            pricing_version,
            scope,
            prefix,
            attempt,
            resource,
            expires_at,
            reconcile,
        })
    }

    /// Prompt-cache policy semantics version.
    #[must_use]
    pub fn policy_version(&self) -> &str {
        &self.policy_version
    }

    /// Exact adapter/model capability-record version.
    #[must_use]
    pub fn capability_version(&self) -> &str {
        &self.capability_version
    }

    /// Exact pricing version, where cost was available.
    #[must_use]
    pub fn pricing_version(&self) -> Option<&str> {
        self.pricing_version.as_deref()
    }

    /// Redacted cache scope digest.
    #[must_use]
    pub const fn scope(&self) -> PromptCacheScopeDigest {
        self.scope
    }

    /// Redacted stable-prefix fingerprint.
    #[must_use]
    pub const fn prefix(&self) -> PromptCacheFingerprint {
        self.prefix
    }

    /// Exact provider send attempt, if one exists.
    #[must_use]
    pub const fn attempt(&self) -> Option<crate::ProviderAttemptId> {
        self.attempt
    }

    /// Local resource identity only; never its provider handle.
    #[must_use]
    pub const fn resource(&self) -> Option<&PromptCacheResourceId> {
        self.resource.as_ref()
    }

    /// Provider expiry in Unix seconds, where known.
    #[must_use]
    pub const fn expires_at(&self) -> Option<u64> {
        self.expires_at
    }

    /// Whether a remote state must be reconciled before reuse.
    #[must_use]
    pub const fn requires_reconciliation(&self) -> bool {
        self.reconcile
    }
}

impl fmt::Debug for CacheCheckpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CacheCheckpoint")
            .field("policy_version", &self.policy_version)
            .field("capability_version", &self.capability_version)
            .field("pricing_version", &self.pricing_version)
            .field("scope", &"[redacted]")
            .field("prefix", &"[redacted]")
            .field("attempt", &self.attempt)
            .field("resource", &self.resource.as_ref().map(|_| "[redacted]"))
            .field("expires_at", &self.expires_at)
            .field("reconcile", &self.reconcile)
            .finish()
    }
}

/// Version evidence supplied by the live runtime at resume.
#[derive(Debug, Clone)]
pub struct ResumeEvidence {
    scope: ResumeScope,
    policy_version: Box<str>,
    capability_version: Box<str>,
    pricing_version: Option<Box<str>>,
    cache_scope: Option<PromptCacheScopeDigest>,
    cache_prefix: Option<PromptCacheFingerprint>,
    sandboxes: Vec<SandboxCheckpoint>,
}

impl ResumeEvidence {
    /// Builds exact live evidence. Invalid labels remain a mismatch rather than
    /// becoming a partially trusted resume.
    #[must_use]
    pub fn new(
        scope: ResumeScope,
        policy_version: impl Into<Box<str>>,
        capability_version: impl Into<Box<str>>,
        pricing_version: Option<impl Into<Box<str>>>,
    ) -> Self {
        Self {
            scope,
            policy_version: policy_version.into(),
            capability_version: capability_version.into(),
            pricing_version: pricing_version.map(Into::into),
            cache_scope: None,
            cache_prefix: None,
            sandboxes: Vec::new(),
        }
    }

    /// Supplies a freshly derived cache scope and stable-prefix fingerprint.
    /// Matching them validates resume identity; it never claims provider cache
    /// activity, which remains known only from a provider usage report.
    #[must_use]
    pub const fn with_cache_identity(
        mut self,
        scope: PromptCacheScopeDigest,
        prefix: PromptCacheFingerprint,
    ) -> Self {
        self.cache_scope = Some(scope);
        self.cache_prefix = Some(prefix);
        self
    }

    /// Supplies a freshly probed effective sandbox/backend identity.
    ///
    /// Evidence may include multiple independently selected sandboxes; every
    /// checkpointed lifecycle must find one exact, non-weaker match.
    ///
    /// # Errors
    ///
    /// The fixed evidence collection is already full.
    pub fn with_sandbox(mut self, sandbox: SandboxCheckpoint) -> Result<Self, InterruptionError> {
        if self.sandboxes.len() >= MAX_CHECKPOINT_SANDBOXES {
            return Err(InterruptionError::TooMany {
                kind: "sandbox evidence",
                maximum: MAX_CHECKPOINT_SANDBOXES,
            });
        }
        self.sandboxes.push(sandbox);
        Ok(self)
    }
}

/// One distinct in-flight execution checkpoint.
#[derive(Debug, Clone)]
pub struct ExecutionCheckpoint {
    id: CheckpointId,
    ancestry: Ancestry,
    scope: ResumeScope,
    cache: Option<CacheCheckpoint>,
    created_at: u64,
    expires_at: u64,
    pending: PendingActions,
    invocations: Vec<InvocationRecord>,
    sandboxes: Vec<SandboxCheckpoint>,
}

impl ExecutionCheckpoint {
    /// Creates an empty execution checkpoint, separate from conversation and
    /// extension records.
    ///
    /// # Errors
    ///
    /// Expiry must be later than creation.
    // Identity, ancestry, authority, cache state, and both lifetime endpoints
    // are independent persisted fields; grouping them would add no invariant.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        id: CheckpointId,
        ancestry: Ancestry,
        scope: ResumeScope,
        cache: Option<CacheCheckpoint>,
        created_at: u64,
        expires_at: u64,
    ) -> Result<Self, InterruptionError> {
        if expires_at <= created_at {
            return Err(InterruptionError::InvalidExpiry);
        }
        Ok(Self {
            id,
            ancestry,
            scope,
            cache,
            created_at,
            expires_at,
            pending: PendingActions::new(),
            invocations: Vec::new(),
            sandboxes: Vec::new(),
        })
    }

    /// Stable checkpoint identity.
    #[must_use]
    pub const fn id(&self) -> CheckpointId {
        self.id
    }

    /// Execution tree attribution.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// Resume authority fingerprints.
    #[must_use]
    pub const fn scope(&self) -> ResumeScope {
        self.scope
    }

    /// Minimal cache revalidation state.
    #[must_use]
    pub const fn cache(&self) -> Option<&CacheCheckpoint> {
        self.cache.as_ref()
    }

    /// Creation time in Unix seconds.
    #[must_use]
    pub const fn created_at(&self) -> u64 {
        self.created_at
    }

    /// Checkpoint expiry in Unix seconds.
    #[must_use]
    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }

    /// Pending-action state.
    #[must_use]
    pub const fn pending(&self) -> &PendingActions {
        &self.pending
    }

    /// Mutable pending-action state for resolution.
    #[must_use]
    pub const fn pending_mut(&mut self) -> &mut PendingActions {
        &mut self.pending
    }

    /// Invocation recovery records.
    #[must_use]
    pub fn invocations(&self) -> &[InvocationRecord] {
        &self.invocations
    }

    /// Minimal redacted sandbox identities requiring resume revalidation.
    #[must_use]
    pub fn sandboxes(&self) -> &[SandboxCheckpoint] {
        &self.sandboxes
    }

    /// Adds one bounded sandbox checkpoint identity.
    ///
    /// # Errors
    ///
    /// Duplicate lifecycle identities and a full checkpoint are refused.
    pub fn add_sandbox(&mut self, sandbox: SandboxCheckpoint) -> Result<(), InterruptionError> {
        if self.sandboxes.len() >= MAX_CHECKPOINT_SANDBOXES {
            return Err(InterruptionError::TooMany {
                kind: "sandbox",
                maximum: MAX_CHECKPOINT_SANDBOXES,
            });
        }
        if self.sandboxes.iter().any(|held| held.id() == sandbox.id()) {
            return Err(InterruptionError::InvalidId);
        }
        self.sandboxes.push(sandbox);
        Ok(())
    }

    /// Adds one bounded invocation record.
    ///
    /// # Errors
    ///
    /// Duplicate identities and a full checkpoint are refused.
    pub fn add_invocation(
        &mut self,
        invocation: InvocationRecord,
    ) -> Result<(), InterruptionError> {
        invocation.validate()?;
        if self.invocations.len() >= MAX_CHECKPOINT_INVOCATIONS {
            return Err(InterruptionError::TooMany {
                kind: "invocation",
                maximum: MAX_CHECKPOINT_INVOCATIONS,
            });
        }
        if self
            .invocations
            .iter()
            .any(|held| held.id() == invocation.id())
        {
            return Err(InterruptionError::InvalidId);
        }
        if self
            .invocations
            .iter()
            .any(|held| held.call().id == invocation.call().id)
        {
            return Err(InterruptionError::DuplicateCall(
                invocation.call().id.clone(),
            ));
        }
        self.invocations.push(invocation);
        Ok(())
    }

    /// Replaces pending state after a protected checkpoint is decoded.
    pub fn set_pending(&mut self, pending: PendingActions) {
        self.pending = pending;
    }

    /// Validates the live endpoint, model, credential, authority, semantic
    /// versions, and expiry. A matching fingerprint is never returned as a
    /// cache-hit claim.
    ///
    /// # Errors
    ///
    /// Any mismatch or expiry fails closed.
    pub fn validate_resume(
        &self,
        evidence: &ResumeEvidence,
        now: u64,
    ) -> Result<ValidatedResume, InterruptionError> {
        if now >= self.expires_at
            || self
                .cache
                .as_ref()
                .and_then(CacheCheckpoint::expires_at)
                .is_some_and(|expiry| now >= expiry)
            || self
                .pending
                .entries()
                .any(|(action, resolution, completed)| {
                    !completed
                        && now >= action.expires_at()
                        && matches!(
                            resolution,
                            None | Some(ActionResolution::Approval(ApprovalDecision::Approved))
                        )
                })
        {
            return Err(InterruptionError::Expired);
        }
        if evidence.scope != self.scope {
            return Err(InterruptionError::ResumeMismatch);
        }
        if self.sandboxes.iter().any(|saved| {
            !evidence
                .sandboxes
                .iter()
                .any(|live| saved.is_compatible_with(live))
        }) {
            return Err(InterruptionError::ResumeMismatch);
        }
        let recovery = match &self.cache {
            Some(cache)
                if cache.policy_version() == &*evidence.policy_version
                    && cache.capability_version() == &*evidence.capability_version
                    && cache.pricing_version() == evidence.pricing_version.as_deref()
                    && evidence.cache_scope == Some(cache.scope())
                    && evidence.cache_prefix == Some(cache.prefix()) =>
            {
                if cache.requires_reconciliation() || cache.resource().is_some() {
                    RecoveryAction::Reconcile
                } else {
                    // Even an exact prefix/scope match is only permission to
                    // build another provider request and observe its report.
                    RecoveryAction::Retry
                }
            }
            Some(_) => return Err(InterruptionError::ResumeMismatch),
            None => RecoveryAction::Retry,
        };
        Ok(ValidatedResume { recovery })
    }
}

/// Successful scope/version validation with no speculative cache outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ValidatedResume {
    recovery: RecoveryAction,
}

impl ValidatedResume {
    /// Required next action.
    #[must_use]
    pub const fn recovery(self) -> RecoveryAction {
        self.recovery
    }
}

/// Persistence contract for execution checkpoints.
pub trait CheckpointStore {
    /// Store-owned error preserving its concrete boundary.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Durably replaces the checkpoint under its stable identity.
    ///
    /// # Errors
    ///
    /// Returns the implementation's error when validation or durable storage
    /// fails.
    fn save(&mut self, checkpoint: &ExecutionCheckpoint) -> Result<(), Self::Error>;

    /// Loads one typed checkpoint, if present.
    ///
    /// # Errors
    ///
    /// Returns the implementation's error when protected storage cannot be
    /// read or decoded safely.
    fn load(&self, id: CheckpointId) -> Result<Option<ExecutionCheckpoint>, Self::Error>;

    /// Removes one finished checkpoint. Repeating removal is idempotent.
    ///
    /// # Errors
    ///
    /// Returns the implementation's error when the protected file cannot be
    /// validated or removed.
    fn remove(&mut self, id: CheckpointId) -> Result<(), Self::Error>;
}
