//! What a prompt-cache attempt leaves behind, and the policy it was made under.
//!
//! A session log, a checkpoint and a cost line keep these after the request
//! that produced them is gone, so they are values rather than contracts: the
//! narrowed policy a turn ran under, the facts one attempt was planned, encoded
//! and reported with, the record of a persistent resource a provider holds, and
//! what the attempt cost. Building them from a live request is
//! `crucible-models`; keeping a resource record is `crucible-storage`.

mod attempt;
mod capability;
mod policy;
mod pricing;
mod resource;

pub use attempt::{
    PromptCacheEligibility, PromptCacheEncoding, PromptCacheFact, PromptCacheFingerprint,
    PromptCacheIneligibleReason, PromptCacheOutcome, PromptCachePlanned,
    PromptCacheRequestDisposition, PromptCacheRequestFact, PromptCacheScopeDigest,
    PromptCacheSelected, PromptCacheUsageFact,
};
pub use capability::{
    PromptCacheCapabilityWordError, PromptCacheMechanism, PromptCacheRetentionClass,
    PromptCacheSupport, PromptCacheUsageReporting,
};
pub use policy::{
    MAX_PROMPT_CACHE_NAMESPACE_BYTES, MAX_PROMPT_CACHE_RETENTION_SECONDS, PromptCacheIsolation,
    PromptCacheMechanisms, PromptCacheMode, PromptCacheNamespace, PromptCachePersistentMode,
    PromptCachePolicy, PromptCachePolicyConflict, PromptCachePolicyError, PromptCachePolicySource,
    PromptCachePolicySources, PromptCachePolicyVersion, PromptCacheRetention,
};
pub use pricing::{CostAmount, PricingCurrency, PricingDate, PricingError, PricingUnit, UsageCost};
pub use resource::{
    MAX_PROMPT_CACHE_HANDLE_BYTES, MAX_PROMPT_CACHE_RESOURCE_WORD_BYTES,
    MAX_PROMPT_CACHE_RESOURCES, PromptCachePolicyDigest, PromptCacheResourceBinding,
    PromptCacheResourceError, PromptCacheResourceFact, PromptCacheResourceHandle,
    PromptCacheResourceId, PromptCacheResourceOperation, PromptCacheResourceOwner,
    PromptCacheResourceRecord, PromptCacheResourceState, PromptCacheResourceWordError,
};
