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

pub use attempt::{
    PromptCacheAttempt, PromptCacheIdentity, PromptCacheKey, PromptCachePlan,
    PromptCachePreparationError, PromptCacheRequest, PromptCacheRoute, PromptCacheSelection,
};
pub use capability::{
    MAX_PROMPT_CACHE_MECHANISMS, PromptCacheBoundary, PromptCacheCapabilities, PromptCacheContent,
    PromptCacheMechanismCapability, PromptCacheProvenance, StatefulTransportCapability,
};
pub use policy::narrow_policy;
pub use pricing::{
    PriceRate, PricingQuery, PromptCachePricing, PromptCacheRates, UsageRate, select_pricing,
};
pub use projection::{
    MAX_PROMPT_CACHE_BOUNDARIES, PromptCacheBoundaryPoint, PromptCacheContentSet,
    PromptCacheProjection, PromptCacheProjectionError,
};
pub use resource::{
    PromptCacheResourceCreate, PromptCacheResourceCreated, PromptCacheResourceDeadline,
    PromptCacheResourceLifecycle, PromptCacheResourceReference, PromptCacheResourceRemote,
};
