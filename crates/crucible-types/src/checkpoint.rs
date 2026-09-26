//! The prompt-cache state a checkpoint is allowed to keep.
//!
//! A resumed turn must still match the prompt cache it was planned under, and
//! must never claim a cache hit from that match: the provider's usage report is
//! the only thing that says one happened. What a checkpoint therefore keeps is
//! identity — which policy and adapter versions, which scope, which stable
//! prefix, which attempt, which local resource, when it expires, and whether a
//! remote state must be reconciled first — and never the prompt, the routing
//! key, or an authorization-bearing remote handle.
//!
//! It is a value rather than a contract because it holds no record. It is the
//! one half of a checkpoint that reaches no other crate's record, which is why
//! it can live in the root crate every crate retaining one of these words
//! already names — and why the bound a checkpoint, a history line and a cache
//! fact all retain their words under is stated here beside it.

use std::fmt;

use crate::cache::{PromptCacheFingerprint, PromptCacheResourceId, PromptCacheScopeDigest};
use crate::ids::ProviderAttemptId;

/// Most bytes in one bounded word a durable record retains.
///
/// One figure and one rule for every writer and every reader of a checkpoint, a
/// history line and a cache fact. The words a build retains in a cache
/// checkpoint are the same words a journal line carries, so a bound that
/// admitted one and refused the other would let a build write a record it then
/// cannot read back. This crate is the leaf every crate that retains one of
/// those words already names, so the figure and the rule behind it are stated
/// here once and named elsewhere.
///
/// [`MAX_CACHE_CHECKPOINT_WORD_BYTES`](self) is this same figure under the name
/// a cache checkpoint's own surface exports, and `crucible-storage` exports it
/// again as its checkpoint and journal figure; no one of those is a second
/// bound.
pub const MAX_CHECKPOINT_WORD_BYTES: usize = 256;

/// Whether one bounded, control-free word may be retained in a durable record.
///
/// The single decision behind [`MAX_CHECKPOINT_WORD_BYTES`]. A crate that
/// reports a refusal in its own error type maps this answer rather than
/// restating the rule, so no two of those crates can disagree about which words
/// a record may hold.
#[must_use]
pub fn is_checkpoint_word(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_CHECKPOINT_WORD_BYTES
        && !value.chars().any(char::is_control)
}

/// Why a version label could not be retained in a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CacheCheckpointError {
    /// A version label was empty, oversized, or held a control character.
    #[error(
        "cache checkpoint {field} must be 1..={MAX_CHECKPOINT_WORD_BYTES} bytes with no control character"
    )]
    InvalidVersion {
        /// Which label was refused, named so the writer can say it.
        field: &'static str,
    },
}

impl CacheCheckpointError {
    /// Which retained label was refused.
    #[must_use]
    pub const fn field(&self) -> &'static str {
        match self {
            Self::InvalidVersion { field } => field,
        }
    }
}

/// Retains one bounded version label, refusing what a checkpoint must not keep.
fn bounded(field: &'static str, value: Box<str>) -> Result<Box<str>, CacheCheckpointError> {
    if !is_checkpoint_word(&value) {
        return Err(CacheCheckpointError::InvalidVersion { field });
    }
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
    attempt: Option<ProviderAttemptId>,
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
        attempt: Option<ProviderAttemptId>,
        resource: Option<PromptCacheResourceId>,
        expires_at: Option<u64>,
        reconcile: bool,
    ) -> Result<Self, CacheCheckpointError> {
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
    pub const fn attempt(&self) -> Option<ProviderAttemptId> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_version_label_is_bounded_and_names_itself_when_it_is_not() {
        let cache = |policy: Box<str>| {
            CacheCheckpoint::new(
                policy,
                "capabilities-v1",
                None::<Box<str>>,
                PromptCacheScopeDigest::new([1; 32]),
                PromptCacheFingerprint::new([2; 32]),
                None,
                None,
                None,
                false,
            )
        };
        assert!(cache("policy-v1".into()).is_ok());
        assert!(cache("x".repeat(MAX_CHECKPOINT_WORD_BYTES).into()).is_ok());
        for refused in [
            String::new(),
            "with\nnewline".to_owned(),
            "x".repeat(MAX_CHECKPOINT_WORD_BYTES + 1),
        ] {
            assert_eq!(
                cache(refused.into()).expect_err("an unbounded label is refused"),
                CacheCheckpointError::InvalidVersion {
                    field: "cache policy version"
                }
            );
        }
    }

    #[test]
    fn a_refused_label_renders_the_figure_the_refusal_names() {
        // A writer reads this string to find out which label was refused and
        // what it has to become, so both interpolations are observed rather than
        // assumed: the field names the label, and the bound is rendered as the
        // figure the rule actually applied. The expected side is built by
        // `format!` rather than by repeating the literal, so the figure stays
        // stated once in this crate and the comparison still fails if the
        // message ever stops naming what it enforced.
        let refused = CacheCheckpointError::InvalidVersion {
            field: "cache pricing source",
        };
        assert_eq!(
            refused.to_string(),
            format!(
                "cache checkpoint cache pricing source must be 1..={MAX_CHECKPOINT_WORD_BYTES} bytes with no control character"
            )
        );
    }
}
