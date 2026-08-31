//! Effective adapter and model prompt-cache capabilities.

/// Maximum mechanisms one reviewed endpoint/model record can advertise.
pub const MAX_PROMPT_CACHE_MECHANISMS: usize = 8;

/// Whether the exact adapter/endpoint/model combination is known to cache.
///
/// `Unknown` is deliberately not a permissive answer. It is used for custom
/// compatible endpoints and model revisions whose behavior has not been
/// verified; no cache-specific request field may be inferred from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptCacheSupport {
    /// No reviewed record establishes either answer.
    Unknown,
    /// A reviewed record establishes that no supported mechanism exists.
    Unsupported,
    /// At least one reviewed adapter/model mechanism remains after intersection.
    Supported,
}

/// Stateful response continuation, kept separate from prompt caching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatefulTransportCapability {
    /// The route has not been verified.
    Unknown,
    /// Every logical request must carry its complete provider transcript.
    Unsupported,
    /// The provider has a distinct continuation transport.
    Supported,
}

impl StatefulTransportCapability {
    const fn intersection(self, other: Self) -> Self {
        match (self, other) {
            (Self::Unknown, _) | (_, Self::Unknown) => Self::Unknown,
            (Self::Unsupported, _) | (_, Self::Unsupported) => Self::Unsupported,
            (Self::Supported, Self::Supported) => Self::Supported,
        }
    }
}

/// Provider-neutral cache mechanisms, in reviewed preference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptCacheMechanism {
    /// The provider may cache automatically and only reports usage.
    ProviderManagedUsageOnly,
    /// The provider automatically reuses an eligible stable prefix.
    AutomaticPrefix,
    /// The request marks legal stable-prefix boundaries.
    ExplicitBreakpoints,
    /// The request references a separately managed cached-content resource.
    PersistentContent,
}

impl PromptCacheMechanism {
    /// Canonical configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderManagedUsageOnly => "providerManagedUsageOnly",
            Self::AutomaticPrefix => "automaticPrefix",
            Self::ExplicitBreakpoints => "explicitBreakpoints",
            Self::PersistentContent => "persistentContent",
        }
    }
}

impl std::str::FromStr for PromptCacheMechanism {
    type Err = PromptCacheCapabilityWordError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "providerManagedUsageOnly" => Ok(Self::ProviderManagedUsageOnly),
            "automaticPrefix" => Ok(Self::AutomaticPrefix),
            "explicitBreakpoints" => Ok(Self::ExplicitBreakpoints),
            "persistentContent" => Ok(Self::PersistentContent),
            _ => Err(PromptCacheCapabilityWordError),
        }
    }
}

/// Classes of provider-visible content a mechanism can retain.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptCacheContent {
    /// Standing instructions and text message blocks.
    Text,
    /// Ordered visible tool descriptors.
    Tools,
    /// Images already present in the provider request.
    Images,
    /// Documents already present in the provider request.
    Documents,
    /// Audio already present in the provider request.
    Audio,
    /// Video already present in the provider request.
    Video,
}

/// Neutral positions an explicit-breakpoint adapter may map to its wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptCacheBoundary {
    /// After the standing system instructions.
    AfterSystem,
    /// After the exact ordered visible tool snapshot.
    AfterTools,
    /// After a complete provider-visible message.
    AfterMessage,
    /// After one legal content block within a message.
    AfterContent,
}

/// Retention classes policy may select without naming a vendor TTL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptCacheRetentionClass {
    /// Do not ask the provider to alter its ordinary retention.
    ProviderDefault,
    /// A verified short-lived cache class.
    Ephemeral,
    /// A verified longer-lived class requiring user authority.
    Extended,
}

impl PromptCacheRetentionClass {
    /// Canonical configuration spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProviderDefault => "providerDefault",
            Self::Ephemeral => "ephemeral",
            Self::Extended => "extended",
        }
    }
}

impl std::str::FromStr for PromptCacheRetentionClass {
    type Err = PromptCacheCapabilityWordError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "providerDefault" => Ok(Self::ProviderDefault),
            "ephemeral" => Ok(Self::Ephemeral),
            "extended" => Ok(Self::Extended),
            _ => Err(PromptCacheCapabilityWordError),
        }
    }
}

/// A non-canonical capability/policy word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("unknown prompt-cache capability value")]
pub struct PromptCacheCapabilityWordError;

/// Which cache-specific input buckets a provider can report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PromptCacheUsageReporting {
    /// No cache-specific usage is documented.
    None,
    /// Cache reads can be distinguished.
    ReadTokens,
    /// Cache reads and writes/creation can both be distinguished.
    ReadAndWriteTokens,
}

/// Immutable source metadata for a reviewed capability record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptCacheProvenance {
    source_url: &'static str,
    reviewed_on: &'static str,
    record_version: &'static str,
}

impl PromptCacheProvenance {
    /// Records one official source and the date/version reviewed into the build.
    #[must_use]
    pub const fn new(
        source_url: &'static str,
        reviewed_on: &'static str,
        record_version: &'static str,
    ) -> Self {
        Self {
            source_url,
            reviewed_on,
            record_version,
        }
    }

    /// Official documentation used for the record.
    #[must_use]
    pub const fn source_url(self) -> &'static str {
        self.source_url
    }

    /// ISO date on which the source was reviewed.
    #[must_use]
    pub const fn reviewed_on(self) -> &'static str {
        self.reviewed_on
    }

    /// Version of the compiled record.
    #[must_use]
    pub const fn record_version(self) -> &'static str {
        self.record_version
    }
}

/// One mechanism after adapter and model constraints are applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheMechanismCapability {
    mechanism: PromptCacheMechanism,
    minimum_prefix_tokens: u32,
    maximum_breakpoints: u8,
    routing_key: bool,
    retention_hint: bool,
    opt_out: bool,
    content: Box<[PromptCacheContent]>,
    boundaries: Box<[PromptCacheBoundary]>,
    retentions: Box<[PromptCacheRetentionClass]>,
}

impl PromptCacheMechanismCapability {
    /// A provider-managed automatic prefix cache.
    #[must_use]
    pub fn automatic_prefix(
        minimum_prefix_tokens: u32,
        routing_key: bool,
        retention_hint: bool,
        content: &[PromptCacheContent],
    ) -> Self {
        Self {
            mechanism: PromptCacheMechanism::AutomaticPrefix,
            minimum_prefix_tokens,
            maximum_breakpoints: 0,
            routing_key,
            retention_hint,
            opt_out: false,
            content: unique(content),
            boundaries: Box::new([]),
            retentions: Box::new([
                PromptCacheRetentionClass::ProviderDefault,
                PromptCacheRetentionClass::Ephemeral,
            ]),
        }
    }

    /// Automatic cache usage reporting with no request control.
    #[must_use]
    pub fn provider_managed(minimum_prefix_tokens: u32, content: &[PromptCacheContent]) -> Self {
        Self {
            mechanism: PromptCacheMechanism::ProviderManagedUsageOnly,
            minimum_prefix_tokens,
            maximum_breakpoints: 0,
            routing_key: false,
            retention_hint: false,
            opt_out: false,
            content: unique(content),
            boundaries: Box::new([]),
            retentions: Box::new([PromptCacheRetentionClass::ProviderDefault]),
        }
    }

    /// A request that marks explicit legal prefix boundaries.
    #[must_use]
    pub fn explicit_breakpoints(
        minimum_prefix_tokens: u32,
        maximum_breakpoints: u8,
        boundaries: &[PromptCacheBoundary],
        content: &[PromptCacheContent],
    ) -> Self {
        Self {
            mechanism: PromptCacheMechanism::ExplicitBreakpoints,
            minimum_prefix_tokens,
            maximum_breakpoints,
            routing_key: false,
            retention_hint: true,
            opt_out: false,
            content: unique(content),
            boundaries: unique(boundaries),
            retentions: Box::new([
                PromptCacheRetentionClass::ProviderDefault,
                PromptCacheRetentionClass::Ephemeral,
                PromptCacheRetentionClass::Extended,
            ]),
        }
    }

    /// A separately managed cached-content resource.
    #[must_use]
    pub fn persistent_content(minimum_prefix_tokens: u32, content: &[PromptCacheContent]) -> Self {
        Self {
            mechanism: PromptCacheMechanism::PersistentContent,
            minimum_prefix_tokens,
            maximum_breakpoints: 0,
            routing_key: false,
            retention_hint: true,
            opt_out: false,
            content: unique(content),
            boundaries: Box::new([]),
            retentions: Box::new([
                PromptCacheRetentionClass::ProviderDefault,
                PromptCacheRetentionClass::Extended,
            ]),
        }
    }

    /// Marks that this mechanism can encode a documented cache opt-out.
    #[must_use]
    pub const fn with_opt_out(mut self) -> Self {
        self.opt_out = true;
        self
    }

    /// Replaces the reviewed retention classes for this exact mechanism.
    #[must_use]
    pub fn with_retentions(mut self, retentions: &[PromptCacheRetentionClass]) -> Self {
        self.retentions = unique(retentions);
        self
    }

    /// The neutral mechanism kind.
    #[must_use]
    pub const fn mechanism(&self) -> PromptCacheMechanism {
        self.mechanism
    }

    /// Smallest eligible stable prefix in tokens.
    #[must_use]
    pub const fn minimum_prefix_tokens(&self) -> u32 {
        self.minimum_prefix_tokens
    }

    /// Greatest explicit markers accepted; zero for other mechanisms.
    #[must_use]
    pub const fn maximum_breakpoints(&self) -> u8 {
        self.maximum_breakpoints
    }

    /// Whether a stable routing key can be encoded.
    #[must_use]
    pub const fn supports_routing_key(&self) -> bool {
        self.routing_key
    }

    /// Whether a retention hint can be encoded.
    #[must_use]
    pub const fn supports_retention_hint(&self) -> bool {
        self.retention_hint
    }

    /// Whether a documented cache opt-out can be encoded.
    #[must_use]
    pub const fn supports_opt_out(&self) -> bool {
        self.opt_out
    }

    /// Provider-visible content kinds eligible under this mechanism.
    #[must_use]
    pub fn content(&self) -> &[PromptCacheContent] {
        &self.content
    }

    /// Neutral boundary positions the adapter may encode.
    #[must_use]
    pub fn boundaries(&self) -> &[PromptCacheBoundary] {
        &self.boundaries
    }

    /// Retention classes the model/endpoint record permits.
    #[must_use]
    pub fn retentions(&self) -> &[PromptCacheRetentionClass] {
        &self.retentions
    }

    fn intersection(&self, model: &Self) -> Option<Self> {
        (self.mechanism == model.mechanism).then(|| Self {
            mechanism: self.mechanism,
            minimum_prefix_tokens: self.minimum_prefix_tokens.max(model.minimum_prefix_tokens),
            maximum_breakpoints: self.maximum_breakpoints.min(model.maximum_breakpoints),
            routing_key: self.routing_key && model.routing_key,
            retention_hint: self.retention_hint && model.retention_hint,
            opt_out: self.opt_out && model.opt_out,
            content: common(&self.content, &model.content),
            boundaries: common(&self.boundaries, &model.boundaries),
            retentions: common(&self.retentions, &model.retentions),
        })
    }
}

fn common<T: Copy + PartialEq>(ordered: &[T], allowed: &[T]) -> Box<[T]> {
    ordered
        .iter()
        .copied()
        .filter(|item| allowed.contains(item))
        .collect()
}

fn unique<T: Copy + PartialEq>(values: &[T]) -> Box<[T]> {
    let mut unique = Vec::new();
    for value in values.iter().copied() {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    unique.into_boxed_slice()
}

/// The exact effective cache capabilities for one provider request route.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PromptCacheCapabilities {
    support: PromptCacheSupport,
    record_version: &'static str,
    model_revision: Option<&'static str>,
    provenance: Option<PromptCacheProvenance>,
    transport: StatefulTransportCapability,
    mechanisms: Box<[PromptCacheMechanismCapability]>,
    usage: PromptCacheUsageReporting,
}

impl PromptCacheCapabilities {
    /// No verified record exists, so no request control may be guessed.
    #[must_use]
    pub fn unknown(reason: &'static str) -> Self {
        Self {
            support: PromptCacheSupport::Unknown,
            record_version: reason,
            model_revision: None,
            provenance: None,
            transport: StatefulTransportCapability::Unknown,
            mechanisms: Box::new([]),
            usage: PromptCacheUsageReporting::None,
        }
    }

    /// A verified negative record.
    #[must_use]
    pub fn unsupported(
        record_version: &'static str,
        provenance: PromptCacheProvenance,
        transport: StatefulTransportCapability,
    ) -> Self {
        Self {
            support: PromptCacheSupport::Unsupported,
            record_version,
            model_revision: None,
            provenance: Some(provenance),
            transport,
            mechanisms: Box::new([]),
            usage: PromptCacheUsageReporting::None,
        }
    }

    /// A reviewed, ordered set of mechanisms.
    ///
    /// Records are compiled code rather than runtime input. A record exceeding
    /// the small fixed mechanism bound is therefore a developer defect and is
    /// rejected immediately instead of being truncated into a different claim.
    ///
    /// # Panics
    ///
    /// Panics when a compiled supported record has no mechanism or exceeds the
    /// fixed mechanism bound.
    // Each argument is an independent field in one reviewed capability record;
    // grouping them would only add a second constructor for the same contract.
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub fn supported(
        record_version: &'static str,
        model_revision: Option<&'static str>,
        provenance: PromptCacheProvenance,
        transport: StatefulTransportCapability,
        mechanisms: &[PromptCacheMechanismCapability],
        usage: PromptCacheUsageReporting,
    ) -> Self {
        assert!(
            !mechanisms.is_empty() && mechanisms.len() <= MAX_PROMPT_CACHE_MECHANISMS,
            "a supported prompt-cache record needs 1..={MAX_PROMPT_CACHE_MECHANISMS} mechanisms"
        );
        Self {
            support: PromptCacheSupport::Supported,
            record_version,
            model_revision,
            provenance: Some(provenance),
            transport,
            mechanisms: mechanisms.into(),
            usage,
        }
    }

    /// Intersects what the adapter can encode with what the exact model/route supports.
    #[must_use]
    pub fn intersection(&self, model: &Self) -> Self {
        if self.support == PromptCacheSupport::Unknown {
            return self.clone();
        }
        if model.support == PromptCacheSupport::Unknown {
            return model.clone();
        }
        let transport = self.transport.intersection(model.transport);
        if self.support == PromptCacheSupport::Unsupported {
            let mut answer = self.clone();
            answer.transport = transport;
            return answer;
        }
        if model.support == PromptCacheSupport::Unsupported {
            let mut answer = model.clone();
            answer.transport = transport;
            return answer;
        }

        let mechanisms: Box<[_]> = self
            .mechanisms
            .iter()
            .filter_map(|adapter| {
                model
                    .mechanisms
                    .iter()
                    .find(|candidate| candidate.mechanism == adapter.mechanism)
                    .and_then(|candidate| adapter.intersection(candidate))
            })
            .filter(|mechanism| {
                !mechanism.content.is_empty()
                    && !mechanism.retentions.is_empty()
                    && (mechanism.mechanism != PromptCacheMechanism::ExplicitBreakpoints
                        || (!mechanism.boundaries.is_empty() && mechanism.maximum_breakpoints > 0))
            })
            .collect();

        if mechanisms.is_empty() {
            return Self {
                support: PromptCacheSupport::Unsupported,
                record_version: model.record_version,
                model_revision: model.model_revision,
                provenance: model.provenance,
                transport,
                mechanisms,
                usage: self.usage.min(model.usage),
            };
        }

        Self {
            support: PromptCacheSupport::Supported,
            record_version: model.record_version,
            model_revision: model.model_revision,
            provenance: model.provenance,
            transport,
            mechanisms,
            usage: self.usage.min(model.usage),
        }
    }

    /// Declared support state.
    #[must_use]
    pub const fn support(&self) -> PromptCacheSupport {
        self.support
    }

    /// Exact ordered effective mechanisms.
    #[must_use]
    pub fn mechanisms(&self) -> &[PromptCacheMechanismCapability] {
        &self.mechanisms
    }

    /// Stateful response-continuation capability, on its own axis.
    #[must_use]
    pub const fn transport(&self) -> StatefulTransportCapability {
        self.transport
    }

    /// Cache-specific usage detail available from the response.
    #[must_use]
    pub const fn usage(&self) -> PromptCacheUsageReporting {
        self.usage
    }

    /// Version of the exact compiled endpoint/model record.
    #[must_use]
    pub const fn record_version(&self) -> &'static str {
        self.record_version
    }

    /// Resolved model revision, when the provider publishes one.
    #[must_use]
    pub const fn model_revision(&self) -> Option<&'static str> {
        self.model_revision
    }

    /// Source and review date behind this claim, absent for unknown routes.
    #[must_use]
    pub const fn provenance(&self) -> Option<PromptCacheProvenance> {
        self.provenance
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: PromptCacheProvenance = PromptCacheProvenance::new(
        "https://provider.example/prompt-caching",
        "2026-08-31",
        "fixture-v1",
    );

    fn automatic(minimum: u32) -> PromptCacheMechanismCapability {
        PromptCacheMechanismCapability::automatic_prefix(
            minimum,
            false,
            false,
            &[PromptCacheContent::Text, PromptCacheContent::Tools],
        )
    }

    #[test]
    fn unknown_and_unsupported_are_different_effective_answers() {
        let adapter = PromptCacheCapabilities::supported(
            "fixture-adapter-v1",
            None,
            SOURCE,
            StatefulTransportCapability::Unsupported,
            &[automatic(1_024)],
            PromptCacheUsageReporting::ReadTokens,
        );

        assert_eq!(
            adapter.intersection(&PromptCacheCapabilities::unknown("custom endpoint")),
            PromptCacheCapabilities::unknown("custom endpoint")
        );
        assert_eq!(
            adapter.intersection(&PromptCacheCapabilities::unsupported(
                "fixture-model-v1",
                SOURCE,
                StatefulTransportCapability::Unsupported,
            )),
            PromptCacheCapabilities::unsupported(
                "fixture-model-v1",
                SOURCE,
                StatefulTransportCapability::Unsupported,
            )
        );
    }

    #[test]
    fn intersection_keeps_adapter_order_and_the_tighter_model_limits() {
        let adapter = PromptCacheCapabilities::supported(
            "fixture-adapter-v1",
            None,
            SOURCE,
            StatefulTransportCapability::Supported,
            &[
                automatic(1_024),
                PromptCacheMechanismCapability::explicit_breakpoints(
                    512,
                    4,
                    &[
                        PromptCacheBoundary::AfterSystem,
                        PromptCacheBoundary::AfterMessage,
                    ],
                    &[PromptCacheContent::Text, PromptCacheContent::Tools],
                ),
            ],
            PromptCacheUsageReporting::ReadAndWriteTokens,
        );
        let model = PromptCacheCapabilities::supported(
            "fixture-model-v2",
            Some("2026-08-31"),
            SOURCE,
            StatefulTransportCapability::Unsupported,
            &[
                PromptCacheMechanismCapability::explicit_breakpoints(
                    2_048,
                    2,
                    &[PromptCacheBoundary::AfterMessage],
                    &[PromptCacheContent::Text],
                ),
                automatic(4_096),
            ],
            PromptCacheUsageReporting::ReadTokens,
        );

        let effective = adapter.intersection(&model);

        assert_eq!(effective.support(), PromptCacheSupport::Supported);
        assert_eq!(
            effective.transport(),
            StatefulTransportCapability::Unsupported,
            "stateful continuation is intersected separately from prompt caching"
        );
        assert_eq!(effective.usage(), PromptCacheUsageReporting::ReadTokens);
        let [automatic, explicit] = effective.mechanisms() else {
            panic!("expected the two intersected fixture mechanisms");
        };
        assert_eq!(automatic.mechanism(), PromptCacheMechanism::AutomaticPrefix);
        assert_eq!(automatic.minimum_prefix_tokens(), 4_096);
        assert_eq!(explicit.boundaries(), &[PromptCacheBoundary::AfterMessage]);
        assert_eq!(explicit.maximum_breakpoints(), 2);
        assert_eq!(explicit.content(), &[PromptCacheContent::Text]);
    }

    #[test]
    fn an_empty_mechanism_intersection_is_unsupported_not_supported() {
        let adapter = PromptCacheCapabilities::supported(
            "fixture-adapter-v1",
            None,
            SOURCE,
            StatefulTransportCapability::Unsupported,
            &[automatic(1_024)],
            PromptCacheUsageReporting::ReadTokens,
        );
        let model = PromptCacheCapabilities::supported(
            "fixture-model-v1",
            None,
            SOURCE,
            StatefulTransportCapability::Unsupported,
            &[PromptCacheMechanismCapability::persistent_content(
                1_024,
                &[PromptCacheContent::Text],
            )],
            PromptCacheUsageReporting::ReadTokens,
        );

        assert_eq!(
            adapter.intersection(&model).support(),
            PromptCacheSupport::Unsupported
        );
    }

    #[test]
    fn capability_sets_are_canonical_and_bounded_at_construction() {
        let repeated_content = vec![PromptCacheContent::Text; 100_000];
        let repeated_boundaries = vec![PromptCacheBoundary::AfterMessage; 100_000];
        let repeated_retentions = vec![PromptCacheRetentionClass::Extended; 100_000];

        let capability = PromptCacheMechanismCapability::explicit_breakpoints(
            1,
            4,
            &repeated_boundaries,
            &repeated_content,
        )
        .with_retentions(&repeated_retentions);

        assert_eq!(capability.content(), &[PromptCacheContent::Text]);
        assert_eq!(
            capability.boundaries(),
            &[PromptCacheBoundary::AfterMessage]
        );
        assert_eq!(
            capability.retentions(),
            &[PromptCacheRetentionClass::Extended]
        );
    }
}
