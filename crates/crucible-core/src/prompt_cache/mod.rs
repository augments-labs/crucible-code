//! Provider-neutral prompt-cache contracts.
//!
//! Prompt caching reuses an identical provider-visible input prefix. It never
//! supplies a model response, skips a provider request, or stands in for a
//! provider's stateful response-continuation transport. Capabilities describe
//! only behavior an adapter can encode and an exact endpoint/model record has
//! verified; vendor field names stay in provider crates.

mod attempt;
mod capability;
mod policy;
mod pricing;
mod projection;
mod resource;
mod usage;

pub use attempt::{
    PromptCacheAttempt, PromptCacheEligibility, PromptCacheEncoding, PromptCacheFact,
    PromptCacheFingerprint, PromptCacheIdentity, PromptCacheIneligibleReason, PromptCacheKey,
    PromptCacheOutcome, PromptCachePlan, PromptCachePlanned, PromptCachePreparationError,
    PromptCacheRequest, PromptCacheRequestDisposition, PromptCacheRequestFact, PromptCacheRoute,
    PromptCacheScopeDigest, PromptCacheSelected, PromptCacheSelection, PromptCacheUsageFact,
};

pub use capability::{
    MAX_PROMPT_CACHE_MECHANISMS, PromptCacheBoundary, PromptCacheCapabilities,
    PromptCacheCapabilityWordError, PromptCacheContent, PromptCacheMechanism,
    PromptCacheMechanismCapability, PromptCacheProvenance, PromptCacheRetentionClass,
    PromptCacheSupport, PromptCacheUsageReporting, StatefulTransportCapability,
};
pub use policy::{
    MAX_PROMPT_CACHE_NAMESPACE_BYTES, MAX_PROMPT_CACHE_RETENTION_SECONDS, PromptCacheIsolation,
    PromptCacheMechanisms, PromptCacheMode, PromptCacheNamespace, PromptCachePersistentMode,
    PromptCachePolicy, PromptCachePolicyConflict, PromptCachePolicyError, PromptCachePolicySource,
    PromptCachePolicySources, PromptCachePolicyVersion, PromptCacheRetention,
};
pub use pricing::{
    CostAmount, PriceRate, PricingCurrency, PricingDate, PricingError, PricingQuery, PricingUnit,
    PromptCachePricing, PromptCacheRates, UsageCost, UsageRate, select_pricing,
};
pub use projection::{
    MAX_PROMPT_CACHE_BOUNDARIES, PromptCacheBoundaryPoint, PromptCacheContentSet,
    PromptCacheProjection, PromptCacheProjectionError,
};
pub use resource::{
    MAX_PROMPT_CACHE_HANDLE_BYTES, MAX_PROMPT_CACHE_RESOURCE_WORD_BYTES,
    MAX_PROMPT_CACHE_RESOURCES, PromptCachePolicyDigest, PromptCacheResourceBinding,
    PromptCacheResourceCreate, PromptCacheResourceCreated, PromptCacheResourceDeadline,
    PromptCacheResourceError, PromptCacheResourceFact, PromptCacheResourceHandle,
    PromptCacheResourceId, PromptCacheResourceLifecycle, PromptCacheResourceOperation,
    PromptCacheResourceOwner, PromptCacheResourceRecord, PromptCacheResourceReference,
    PromptCacheResourceRemote, PromptCacheResourceState, PromptCacheResourceStore,
    PromptCacheResourceWordError,
};
pub use usage::{
    InputTokenUsage, MAX_PROVIDER_USAGE_DETAIL_LABEL_BYTES, MAX_PROVIDER_USAGE_DETAILS,
    ProviderNumericDetail, ProviderUsage, UsageError,
};
