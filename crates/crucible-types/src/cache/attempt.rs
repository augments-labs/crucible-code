//! What one provider prompt-cache attempt leaves behind to be recorded.
//!
//! A planned, encoded and reported attempt is a fact a session log and a
//! checkpoint keep after the request that produced it is gone. The request
//! side — the plan, the selection, the routing identity — belongs to
//! `crucible-models`, which builds these from it.

use std::fmt;

use crate::ProviderAttemptId;
use crate::usage::ProviderUsage;

use super::{PromptCacheMechanism, PromptCachePolicy, PromptCacheRetentionClass};

/// Cryptographic fingerprint of only the canonical stable provider prefix.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptCacheFingerprint([u8; 32]);

impl PromptCacheFingerprint {
    /// Takes the output of the domain-separated stable-prefix digest.
    #[must_use]
    pub const fn new(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Bytes for the second, scope-binding identity digest.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for PromptCacheFingerprint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptCacheFingerprint([redacted])")
    }
}

/// Digest of authority, endpoint, credential, model, and sharing scope.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptCacheScopeDigest([u8; 32]);

impl PromptCacheScopeDigest {
    /// Takes a domain-separated digest derived by the runner.
    #[must_use]
    pub const fn new(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// Bytes used to derive the bounded provider routing key.
    #[must_use]
    pub const fn bytes(self) -> [u8; 32] {
        self.0
    }
}

impl fmt::Debug for PromptCacheScopeDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptCacheScopeDigest([redacted])")
    }
}

/// Stable reason a candidate cannot be used for this request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheIneligibleReason {
    /// Explicit observe-only policy requested no controls.
    ObserveOnly,
    /// Capability record is unknown for this route/model.
    UnknownSupport,
    /// Capability record is a verified negative.
    Unsupported,
    /// The stable prefix is empty.
    EmptyPrefix,
    /// The estimated prefix is below the provider's documented threshold.
    BelowMinimum,
    /// Stable content includes a kind this mechanism does not accept.
    UnsupportedContent,
    /// No legal explicit boundary remains.
    UnsupportedBoundary,
    /// More breakpoints are needed than the endpoint permits.
    TooManyBreakpoints,
    /// Requested retention is outside the capability/policy intersection.
    DisallowedRetention,
    /// Policy filtered out every mechanism.
    MechanismDisallowed,
    /// A persistent resource was forbidden or unavailable.
    ResourceUnavailable,
    /// Privacy prohibition had no verified opt-out.
    OptOutUnavailable,
    /// Inherited policy was contradictory.
    PolicyConflict,
}

/// Prediction made before the provider responds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheEligibility {
    /// Request shape is eligible under the reviewed record.
    Eligible,
    /// Request shape is not eligible, for one stable reason.
    Ineligible(PromptCacheIneligibleReason),
}

/// One selected mechanism and retention class.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheSelected {
    mechanism: PromptCacheMechanism,
    retention: PromptCacheRetentionClass,
}

impl PromptCacheSelected {
    /// A reviewed mechanism selected under policy.
    #[must_use]
    pub const fn new(
        mechanism: PromptCacheMechanism,
        retention: PromptCacheRetentionClass,
    ) -> Self {
        Self {
            mechanism,
            retention,
        }
    }

    /// Neutral mechanism kind.
    #[must_use]
    pub const fn mechanism(self) -> PromptCacheMechanism {
        self.mechanism
    }

    /// Effective retention class.
    #[must_use]
    pub const fn retention(self) -> PromptCacheRetentionClass {
        self.retention
    }
}

/// What cache-specific metadata the adapter actually encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheEncoding {
    /// Policy intentionally requested no control.
    NoControlIntended,
    /// A selected automatic mechanism needs no extra request field.
    NoExtraControlEncoded,
    /// A stable routing or retention hint was encoded.
    AutomaticHintEncoded,
    /// Legal explicit boundary markers were encoded.
    BreakpointsEncoded(u8),
    /// A validated persistent resource was referenced.
    PersistentResourceReferenced,
    /// Encoding was refused before network I/O.
    Failed(PromptCacheIneligibleReason),
}

/// Whether the provider is known to have accepted the request shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheRequestDisposition {
    /// Preparation failed or cancellation occurred before send.
    NotSent,
    /// Transport may have accepted the request but no answer proved it.
    Unknown,
    /// Provider explicitly rejected the request/control.
    Rejected,
    /// Provider accepted the request shape; this says nothing about a cache hit.
    Accepted,
}

/// Provider-reported cache activity only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheOutcome {
    /// Provider supplied no cache detail.
    Unreported,
    /// Provider explicitly reported no read or write activity.
    NoActivity,
    /// Provider reported cache creation/write tokens.
    Write,
    /// Provider reported cache-read tokens.
    Read,
    /// Provider reported both categories for the attempt.
    ReadAndWrite,
}

/// One immutable, ancestry-stamped prompt-cache fact.
///
/// The event carries only typed bounded metadata. Digest-bearing fields redact
/// themselves, and neither request text nor provider control payloads have a
/// place in this type.
#[derive(Debug, Clone)]
pub enum PromptCacheFact {
    /// Capability, policy and eligibility were resolved before a send.
    Planned(Box<PromptCachePlanned>),
    /// The adapter's actual cache metadata and request disposition are known.
    RequestEncoded(PromptCacheRequestFact),
    /// Provider-reported normalized usage and cache outcome.
    UsageReported(Box<PromptCacheUsageFact>),
    /// Persistent-resource lifecycle state changed.
    ResourceChanged(super::PromptCacheResourceFact),
}

/// Actual adapter encoding and acceptance state for one attempt.
#[derive(Debug, Clone, Copy)]
pub struct PromptCacheRequestFact {
    /// The attempt this observation belongs to.
    pub attempt: ProviderAttemptId,
    /// What cache-specific metadata the adapter actually encoded.
    pub encoding: PromptCacheEncoding,
    /// Whether the request was sent and accepted.
    pub disposition: PromptCacheRequestDisposition,
}

/// One provider usage observation attached to its exact attempt.
#[derive(Debug, Clone)]
pub struct PromptCacheUsageFact {
    /// The attempt whose response reported these values.
    pub attempt: ProviderAttemptId,
    /// Cache activity derived only from provider-reported usage fields.
    pub outcome: PromptCacheOutcome,
    /// Normalized token categories with explicit unknowns.
    pub usage: ProviderUsage,
    /// Versioned cost categories with explicit unknowns.
    pub cost: super::UsageCost,
}

/// Bounded preparation fact for one provider attempt.
#[derive(Debug, Clone, Copy)]
pub struct PromptCachePlanned {
    /// Unique identity for this send or retry.
    pub attempt: ProviderAttemptId,
    /// Declared capability state for the exact route/model.
    pub support: super::PromptCacheSupport,
    /// Version of the reviewed capability record.
    pub capability_version: &'static str,
    /// Exact model revision where the provider publishes one.
    pub model_revision: Option<&'static str>,
    /// Version of policy merge/selection semantics.
    pub policy_version: super::PromptCachePolicyVersion,
    /// Fully narrowed policy. Its namespace redacts in diagnostics.
    pub policy: PromptCachePolicy,
    /// Predicted eligibility, never a provider-reported hit.
    pub eligibility: PromptCacheEligibility,
    /// Selected mechanism where one was eligible.
    pub selected: Option<PromptCacheSelected>,
    /// Scope digest; its bytes never print.
    pub scope: PromptCacheScopeDigest,
    /// Stable-prefix fingerprint; its bytes never print.
    pub prefix: PromptCacheFingerprint,
    /// Bounded canonical prefix size.
    pub stable_bytes: u64,
    /// Conservative eligibility estimate, not provider usage.
    pub estimated_tokens: u64,
    /// Static adapter request-shape version.
    pub request_shape_version: &'static str,
}
