//! One planned and observed provider prompt-cache attempt.

use std::fmt;

use crate::{CredentialScopeId, ProviderAttemptId};

use super::{
    PromptCacheCapabilities, PromptCacheContentSet, PromptCacheMechanism, PromptCachePolicy,
    PromptCacheProjection, PromptCacheResourceReference, PromptCacheRetentionClass,
};

/// Borrowed non-secret route facts needed to scope one cache identity.
#[derive(Clone, Copy)]
pub struct PromptCacheRoute<'a> {
    /// Exact provider protocol, not a marketing brand.
    pub protocol: &'static str,
    /// Endpoint/deployment route. Custom routes remain capability-unknown.
    pub endpoint: &'a str,
    /// Whether the address came from user configuration rather than the adapter.
    pub custom_endpoint: bool,
    /// Fail-closed identity of the credential-bearing adapter instance.
    pub credential_scope: CredentialScopeId,
    /// Provider account/project identifier where one is verified and non-secret.
    pub account: Option<&'a str>,
    /// Provider project/deployment identifier where one is verified.
    pub project: Option<&'a str>,
    /// Version of adapter lowering semantics.
    pub request_shape_version: &'static str,
}

impl fmt::Debug for PromptCacheRoute<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptCacheRoute")
            .field("protocol", &self.protocol)
            .field("endpoint", &"[redacted]")
            .field("custom_endpoint", &self.custom_endpoint)
            .field("credential_scope", &self.credential_scope)
            .field("account", &self.account.map(|_| "[redacted]"))
            .field("project", &self.project.map(|_| "[redacted]"))
            .field("request_shape_version", &self.request_shape_version)
            .finish()
    }
}

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

/// Complete local cache identity before provider-specific key encoding.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheIdentity {
    scope: PromptCacheScopeDigest,
    prefix: PromptCacheFingerprint,
    request_shape_version: &'static str,
}

impl PromptCacheIdentity {
    /// Binds a prefix to one exact authority/request-shape scope.
    #[must_use]
    pub const fn new(
        scope: PromptCacheScopeDigest,
        prefix: PromptCacheFingerprint,
        request_shape_version: &'static str,
    ) -> Self {
        Self {
            scope,
            prefix,
            request_shape_version,
        }
    }

    /// Redacted scope digest.
    #[must_use]
    pub const fn scope(self) -> PromptCacheScopeDigest {
        self.scope
    }

    /// Redacted stable-prefix fingerprint.
    #[must_use]
    pub const fn prefix(self) -> PromptCacheFingerprint {
        self.prefix
    }

    /// Adapter request-shape version bound into the identity.
    #[must_use]
    pub const fn request_shape_version(self) -> &'static str {
        self.request_shape_version
    }
}

impl fmt::Debug for PromptCacheIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptCacheIdentity")
            .field("scope", &"[redacted]")
            .field("prefix", &"[redacted]")
            .field("request_shape_version", &self.request_shape_version)
            .finish()
    }
}

/// A bounded provider routing key derived from an identity, never supplied by a project.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct PromptCacheKey {
    hex: [u8; 64],
    length: u8,
}

impl PromptCacheKey {
    /// Encodes a digest as lowercase hexadecimal under a provider's length limit.
    #[must_use]
    pub fn from_digest(digest: [u8; 32], maximum: usize) -> Self {
        let length = maximum.min(64);
        let mut hex = [0; 64];
        for (pair, byte) in hex.chunks_exact_mut(2).zip(digest) {
            let [high, low] = pair else {
                continue;
            };
            *high = nybble(byte >> 4);
            *low = nybble(byte & 0x0f);
        }
        Self {
            hex,
            length: u8::try_from(length).unwrap_or(64),
        }
    }

    /// Provider-visible opaque text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        let bytes = self.hex.get(..usize::from(self.length)).unwrap_or_default();
        std::str::from_utf8(bytes).unwrap_or_default()
    }
}

const fn nybble(value: u8) -> u8 {
    if value < 10 {
        b'0' + value
    } else {
        b'a' + value - 10
    }
}

impl fmt::Debug for PromptCacheKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PromptCacheKey([redacted])")
    }
}

/// Stable projection plus its fingerprint, ready for policy selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCachePlan {
    fingerprint: PromptCacheFingerprint,
    stable_bytes: u64,
    estimated_tokens: u64,
    boundaries: Box<[super::PromptCacheBoundaryPoint]>,
    content: PromptCacheContentSet,
}

impl PromptCachePlan {
    /// Joins a projection with the digest produced by its canonical byte stream.
    #[must_use]
    pub fn new(projection: &PromptCacheProjection, fingerprint: PromptCacheFingerprint) -> Self {
        Self {
            fingerprint,
            stable_bytes: projection.stable_bytes(),
            estimated_tokens: projection.estimated_tokens(),
            boundaries: projection.boundaries().into(),
            content: projection.content(),
        }
    }

    /// Redacted stable-prefix fingerprint.
    #[must_use]
    pub const fn fingerprint(&self) -> PromptCacheFingerprint {
        self.fingerprint
    }

    /// Exact bytes in Crucible's canonical projection.
    #[must_use]
    pub const fn stable_bytes(&self) -> u64 {
        self.stable_bytes
    }

    /// Conservative provider-neutral token estimate used only for eligibility.
    #[must_use]
    pub const fn estimated_tokens(&self) -> u64 {
        self.estimated_tokens
    }

    /// Legal neutral boundaries, in semantic order.
    #[must_use]
    pub fn boundaries(&self) -> &[super::PromptCacheBoundaryPoint] {
        &self.boundaries
    }

    /// Provider-visible content kinds in the stable prefix.
    #[must_use]
    pub const fn content(&self) -> PromptCacheContentSet {
        self.content
    }

    #[cfg(test)]
    fn fixture(
        stable_bytes: u64,
        estimated_tokens: u64,
        boundaries: &[super::PromptCacheBoundary],
        content: PromptCacheContentSet,
    ) -> Self {
        Self {
            fingerprint: PromptCacheFingerprint::new([0x11; 32]),
            stable_bytes,
            estimated_tokens,
            boundaries: boundaries
                .iter()
                .enumerate()
                .map(|(index, kind)| {
                    super::PromptCacheBoundaryPoint::fixture(
                        *kind,
                        u32::try_from(index + 1).expect("fixture boundary count is bounded"),
                    )
                })
                .collect(),
            content,
        }
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

/// Selection and predicted eligibility, kept distinct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheSelection {
    selected: Option<PromptCacheSelected>,
    eligibility: PromptCacheEligibility,
}

impl PromptCacheSelection {
    /// Selects the first reviewed mechanism that remains eligible under policy.
    ///
    /// `resource_ready` means a separately authorized lifecycle operation has
    /// already resolved or created a scope-matching persistent resource. This
    /// function never creates one as a side effect.
    ///
    /// # Errors
    ///
    /// Returns an error when merged policy is contradictory, required caching
    /// cannot be satisfied, or prohibition cannot be encoded safely.
    pub fn prepare(
        policy: PromptCachePolicy,
        capabilities: &PromptCacheCapabilities,
        plan: &PromptCachePlan,
        resource_ready: bool,
    ) -> Result<Self, PromptCachePreparationError> {
        if policy.conflict().is_some() || policy.validate().is_err() {
            return Err(PromptCachePreparationError::PolicyConflict);
        }

        if policy.mode() == super::PromptCacheMode::ObserveOnly {
            return Ok(Self::ineligible(PromptCacheIneligibleReason::ObserveOnly));
        }

        if policy.mode() == super::PromptCacheMode::Prohibit {
            return capabilities
                .mechanisms()
                .iter()
                .find(|mechanism| {
                    policy.allowed_mechanisms().contains(mechanism.mechanism())
                        && mechanism.supports_opt_out()
                })
                .map(|mechanism| {
                    Self::eligible(PromptCacheSelected::new(
                        mechanism.mechanism(),
                        PromptCacheRetentionClass::ProviderDefault,
                    ))
                })
                .ok_or(PromptCachePreparationError::ProhibitedWithoutOptOut);
        }

        let support_reason = match capabilities.support() {
            super::PromptCacheSupport::Unknown => Some(PromptCacheIneligibleReason::UnknownSupport),
            super::PromptCacheSupport::Unsupported => {
                Some(PromptCacheIneligibleReason::Unsupported)
            }
            super::PromptCacheSupport::Supported => None,
        };
        if let Some(reason) = support_reason {
            return finish_selection(policy, reason);
        }
        if plan.stable_bytes() == 0 || plan.content().is_empty() {
            return finish_selection(policy, PromptCacheIneligibleReason::EmptyPrefix);
        }
        if policy.allowed_mechanisms().is_empty() {
            return finish_selection(policy, PromptCacheIneligibleReason::MechanismDisallowed);
        }

        let mut last_reason = PromptCacheIneligibleReason::MechanismDisallowed;
        for mechanism in capabilities.mechanisms() {
            if !policy.allowed_mechanisms().contains(mechanism.mechanism()) {
                continue;
            }
            let reason = mechanism_ineligibility(policy, mechanism, plan, resource_ready);
            if let Some(reason) = reason {
                last_reason = reason;
                continue;
            }
            return Ok(Self::eligible(PromptCacheSelected::new(
                mechanism.mechanism(),
                policy.retention().class(),
            )));
        }

        finish_selection(policy, last_reason)
    }

    /// Records a selected eligible mechanism.
    #[must_use]
    pub const fn eligible(selected: PromptCacheSelected) -> Self {
        Self {
            selected: Some(selected),
            eligibility: PromptCacheEligibility::Eligible,
        }
    }

    /// Records that no eligible mechanism was selected.
    #[must_use]
    pub const fn ineligible(reason: PromptCacheIneligibleReason) -> Self {
        Self {
            selected: None,
            eligibility: PromptCacheEligibility::Ineligible(reason),
        }
    }

    /// Selected mechanism, if any.
    #[must_use]
    pub const fn selected(self) -> Option<PromptCacheSelected> {
        self.selected
    }

    /// Predicted request eligibility, never a provider-reported hit.
    #[must_use]
    pub const fn eligibility(self) -> PromptCacheEligibility {
        self.eligibility
    }
}

fn mechanism_ineligibility(
    policy: PromptCachePolicy,
    capability: &super::PromptCacheMechanismCapability,
    plan: &PromptCachePlan,
    resource_ready: bool,
) -> Option<PromptCacheIneligibleReason> {
    if plan.estimated_tokens() < u64::from(capability.minimum_prefix_tokens()) {
        return Some(PromptCacheIneligibleReason::BelowMinimum);
    }
    if !plan.content().is_subset_of(capability.content()) {
        return Some(PromptCacheIneligibleReason::UnsupportedContent);
    }
    if !capability
        .retentions()
        .contains(&policy.retention().class())
    {
        return Some(PromptCacheIneligibleReason::DisallowedRetention);
    }

    match capability.mechanism() {
        PromptCacheMechanism::ExplicitBreakpoints => {
            let legal = plan
                .boundaries()
                .iter()
                .filter(|point| capability.boundaries().contains(&point.kind()))
                .count();
            if legal == 0 {
                return Some(PromptCacheIneligibleReason::UnsupportedBoundary);
            }
            if capability.maximum_breakpoints() == 0 {
                return Some(PromptCacheIneligibleReason::TooManyBreakpoints);
            }
        }
        PromptCacheMechanism::PersistentContent => {
            if policy.persistent_resources() == super::PromptCachePersistentMode::Forbid
                || !resource_ready
            {
                return Some(PromptCacheIneligibleReason::ResourceUnavailable);
            }
        }
        PromptCacheMechanism::ProviderManagedUsageOnly | PromptCacheMechanism::AutomaticPrefix => {}
    }

    None
}

fn finish_selection(
    policy: PromptCachePolicy,
    reason: PromptCacheIneligibleReason,
) -> Result<PromptCacheSelection, PromptCachePreparationError> {
    if policy.mode() == super::PromptCacheMode::Require {
        Err(PromptCachePreparationError::Required(reason))
    } else {
        Ok(PromptCacheSelection::ineligible(reason))
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

/// A complete seven-state cache attempt with normalized usage and cost.
#[derive(Debug, Clone)]
pub struct PromptCacheAttempt {
    /// One identity per send/retry attempt.
    pub id: ProviderAttemptId,
    /// Effective declared capabilities.
    pub capabilities: PromptCacheCapabilities,
    /// Fully merged selected policy.
    pub policy: PromptCachePolicy,
    /// Predicted selection/eligibility.
    pub selection: PromptCacheSelection,
    /// Actual adapter encoding.
    pub encoding: PromptCacheEncoding,
    /// Network/request acceptance state.
    pub disposition: PromptCacheRequestDisposition,
    /// Only provider-reported cache activity.
    pub outcome: PromptCacheOutcome,
    /// Latest normalized provider usage for this attempt.
    pub usage: Option<super::ProviderUsage>,
    /// Cost derived from exact reviewed pricing, or explicitly unknown.
    pub cost: super::UsageCost,
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

impl PromptCachePlanned {
    /// Builds a fact from the exact neutral request handed to an adapter.
    #[must_use]
    pub fn from_request(request: &PromptCacheRequest<'_>) -> Self {
        Self {
            attempt: request.attempt,
            support: request.capabilities.support(),
            capability_version: request.capabilities.record_version(),
            model_revision: request.capabilities.model_revision(),
            policy_version: request.policy.version(),
            policy: request.policy,
            eligibility: request.selection.eligibility(),
            selected: request.selection.selected(),
            scope: request.identity.scope(),
            prefix: request.identity.prefix(),
            stable_bytes: request.plan.stable_bytes(),
            estimated_tokens: request.plan.estimated_tokens(),
            request_shape_version: request.identity.request_shape_version(),
        }
    }
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
    pub usage: super::ProviderUsage,
    /// Versioned cost categories with explicit unknowns.
    pub cost: super::UsageCost,
}

/// Preparation failure before a provider request can be sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum PromptCachePreparationError {
    /// Required caching had no eligible mechanism.
    #[error("prompt caching is required but no eligible reviewed mechanism is available")]
    Required(PromptCacheIneligibleReason),
    /// Privacy prohibition had no verified provider opt-out.
    #[error("prompt caching is prohibited but this provider/model has no verified opt-out")]
    ProhibitedWithoutOptOut,
    /// Inherited policy was contradictory.
    #[error("prompt-cache policy has an inherited conflict")]
    PolicyConflict,
    /// The selected mechanism could not be lowered at a reviewed wire boundary.
    #[error("prompt-cache control could not be encoded at a legal provider boundary")]
    Encoding(PromptCacheIneligibleReason),
}

/// One borrowed neutral cache plan attached to a provider request.
#[derive(Clone, Copy)]
pub struct PromptCacheRequest<'a> {
    /// Identity of this send/retry attempt.
    pub attempt: ProviderAttemptId,
    /// Fully merged policy.
    pub policy: PromptCachePolicy,
    /// Exact effective adapter/model capabilities.
    pub capabilities: &'a PromptCacheCapabilities,
    /// Canonical stable-prefix plan.
    pub plan: &'a PromptCachePlan,
    /// Scope-bound cache identity.
    pub identity: PromptCacheIdentity,
    /// Mechanism selected before sending.
    pub selection: PromptCacheSelection,
    /// Optional derived provider routing key; never project-supplied text.
    pub routing_key: Option<PromptCacheKey>,
    /// Optional validated persistent resource.
    pub resource: Option<PromptCacheResourceReference<'a>>,
}

impl fmt::Debug for PromptCacheRequest<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PromptCacheRequest")
            .field("attempt", &self.attempt)
            .field("policy", &self.policy)
            .field("support", &self.capabilities.support())
            .field("plan", &"[fingerprint redacted]")
            .field("identity", &self.identity)
            .field("selection", &self.selection)
            .field("routing_key", &self.routing_key.map(|_| "[redacted]"))
            .field("resource", &self.resource.map(|_| "[redacted]"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        PromptCacheBoundary, PromptCacheContent, PromptCacheMechanismCapability, PromptCacheMode,
        PromptCachePersistentMode, PromptCacheProvenance, PromptCacheSupport,
        PromptCacheUsageReporting, StatefulTransportCapability,
    };

    fn capabilities(mechanisms: &[PromptCacheMechanismCapability]) -> PromptCacheCapabilities {
        PromptCacheCapabilities::supported(
            "fixture-v1",
            None,
            PromptCacheProvenance::new("https://example.invalid", "2026-08-31", "fixture-v1"),
            StatefulTransportCapability::Unsupported,
            mechanisms,
            PromptCacheUsageReporting::ReadAndWriteTokens,
        )
    }

    fn plan(tokens: u64, boundaries: &[PromptCacheBoundary]) -> PromptCachePlan {
        PromptCachePlan::fixture(
            tokens.saturating_mul(4),
            tokens,
            boundaries,
            PromptCacheContentSet::NONE.with(PromptCacheContent::Text),
        )
    }

    #[test]
    fn observe_only_never_selects_a_control_even_when_support_is_known() {
        let caps = capabilities(&[PromptCacheMechanismCapability::automatic_prefix(
            1,
            true,
            true,
            &[PromptCacheContent::Text],
        )]);
        let policy = PromptCachePolicy::default().with_mode(PromptCacheMode::ObserveOnly);

        let selected = PromptCacheSelection::prepare(policy, &caps, &plan(10, &[]), false).unwrap();

        assert_eq!(selected.selected(), None);
        assert_eq!(
            selected.eligibility(),
            PromptCacheEligibility::Ineligible(PromptCacheIneligibleReason::ObserveOnly)
        );
    }

    #[test]
    fn prefer_selects_the_first_eligible_mechanism_in_reviewed_order() {
        let caps = capabilities(&[
            PromptCacheMechanismCapability::explicit_breakpoints(
                10,
                4,
                &[PromptCacheBoundary::AfterMessage],
                &[PromptCacheContent::Text],
            ),
            PromptCacheMechanismCapability::automatic_prefix(
                10,
                false,
                false,
                &[PromptCacheContent::Text],
            ),
        ]);

        let selected = PromptCacheSelection::prepare(
            PromptCachePolicy::default(),
            &caps,
            &plan(20, &[PromptCacheBoundary::AfterMessage]),
            false,
        )
        .unwrap();

        assert_eq!(
            selected.selected().map(PromptCacheSelected::mechanism),
            Some(PromptCacheMechanism::ExplicitBreakpoints)
        );
        assert_eq!(selected.eligibility(), PromptCacheEligibility::Eligible);
    }

    #[test]
    fn prefer_falls_back_unchanged_while_require_fails_before_send() {
        let caps = capabilities(&[PromptCacheMechanismCapability::automatic_prefix(
            1_024,
            false,
            false,
            &[PromptCacheContent::Text],
        )]);
        let too_short = plan(12, &[]);

        let preferred =
            PromptCacheSelection::prepare(PromptCachePolicy::default(), &caps, &too_short, false)
                .unwrap();
        let required = PromptCacheSelection::prepare(
            PromptCachePolicy::default().with_mode(PromptCacheMode::Require),
            &caps,
            &too_short,
            false,
        )
        .unwrap_err();

        assert_eq!(
            preferred.eligibility(),
            PromptCacheEligibility::Ineligible(PromptCacheIneligibleReason::BelowMinimum)
        );
        assert_eq!(
            required,
            PromptCachePreparationError::Required(PromptCacheIneligibleReason::BelowMinimum)
        );
    }

    #[test]
    fn prohibit_requires_a_real_verified_opt_out() {
        let ordinary = capabilities(&[PromptCacheMechanismCapability::automatic_prefix(
            1,
            false,
            false,
            &[PromptCacheContent::Text],
        )]);
        let opting_out = capabilities(&[PromptCacheMechanismCapability::automatic_prefix(
            1,
            false,
            false,
            &[PromptCacheContent::Text],
        )
        .with_opt_out()]);
        let policy = PromptCachePolicy::default().with_mode(PromptCacheMode::Prohibit);

        assert_eq!(
            PromptCacheSelection::prepare(policy, &ordinary, &plan(10, &[]), false),
            Err(PromptCachePreparationError::ProhibitedWithoutOptOut)
        );
        assert!(
            PromptCacheSelection::prepare(policy, &opting_out, &plan(10, &[]), false)
                .unwrap()
                .selected()
                .is_some()
        );
    }

    #[test]
    fn sdk_constructed_prohibit_with_resource_authority_fails_preparation() {
        let caps = capabilities(&[PromptCacheMechanismCapability::automatic_prefix(
            1,
            false,
            false,
            &[PromptCacheContent::Text],
        )
        .with_opt_out()]);
        let policy = PromptCachePolicy::default()
            .with_mode(PromptCacheMode::Prohibit)
            .with_persistent_resources(PromptCachePersistentMode::Create);

        assert_eq!(
            PromptCacheSelection::prepare(policy, &caps, &plan(10, &[]), false),
            Err(PromptCachePreparationError::PolicyConflict)
        );
    }

    #[test]
    fn persistent_content_needs_separate_resource_authority_and_readiness() {
        let caps = capabilities(&[PromptCacheMechanismCapability::persistent_content(
            1,
            &[PromptCacheContent::Text],
        )]);
        let forbidden = PromptCacheSelection::prepare(
            PromptCachePolicy::default(),
            &caps,
            &plan(10, &[]),
            false,
        )
        .unwrap();
        let reusable = PromptCacheSelection::prepare(
            PromptCachePolicy::default()
                .with_persistent_resources(PromptCachePersistentMode::Reuse),
            &caps,
            &plan(10, &[]),
            true,
        )
        .unwrap();

        assert_eq!(
            forbidden.eligibility(),
            PromptCacheEligibility::Ineligible(PromptCacheIneligibleReason::ResourceUnavailable)
        );
        assert_eq!(reusable.eligibility(), PromptCacheEligibility::Eligible);
        assert_eq!(caps.support(), PromptCacheSupport::Supported);
    }

    #[test]
    fn all_identity_bearing_values_redact_their_bytes() {
        let prefix = PromptCacheFingerprint::new([0xab; 32]);
        let scope = PromptCacheScopeDigest::new([0xcd; 32]);
        let identity = PromptCacheIdentity::new(scope, prefix, "fixture-v1");
        let key = PromptCacheKey::from_digest([0xef; 32], 64);

        for shown in [
            format!("{prefix:?}"),
            format!("{scope:?}"),
            format!("{identity:?}"),
            format!("{key:?}"),
        ] {
            assert!(shown.contains("redacted"), "{shown}");
            assert!(!shown.contains("abab"), "{shown}");
            assert!(!shown.contains("cdcd"), "{shown}");
            assert!(!shown.contains("efef"), "{shown}");
        }
        assert_eq!(key.as_str().len(), 64);
    }
}
