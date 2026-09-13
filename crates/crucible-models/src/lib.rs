//! What a model is asked and what it answers with.
//!
//! A provider adapter translates one request into its vendor's wire shape and
//! the response back into [`Delta`]s. This crate is the contract that adapter is
//! written against — [`Provider`], [`Request`], the record of what one model
//! accepts, and the provider-neutral prompt-cache capabilities, projection,
//! selection and pricing — and it holds no adapter. Each vendor's capability
//! tables, prices and wire fields stay beside that vendor's adapter.
//!
//! What an attempt leaves behind to be recorded — its usage, its cost, the
//! cache facts a session log keeps and the narrowed policy those facts name —
//! is shared data rather than a contract, and belongs to `crucible-types`. Where
//! a persistent cache resource is remembered is `crucible-storage`.

mod cache;
mod model;
mod provider;

pub use cache::{
    MAX_PROMPT_CACHE_BOUNDARIES, MAX_PROMPT_CACHE_MECHANISMS, PriceRate, PricingQuery,
    PromptCacheAttempt, PromptCacheBoundary, PromptCacheBoundaryPoint, PromptCacheCapabilities,
    PromptCacheContent, PromptCacheContentSet, PromptCacheIdentity, PromptCacheKey,
    PromptCacheMechanismCapability, PromptCachePlan, PromptCachePreparationError,
    PromptCachePricing, PromptCacheProjection, PromptCacheProjectionError, PromptCacheProvenance,
    PromptCacheRates, PromptCacheRequest, PromptCacheResourceCreate, PromptCacheResourceCreated,
    PromptCacheResourceDeadline, PromptCacheResourceLifecycle, PromptCacheResourceReference,
    PromptCacheResourceRemote, PromptCacheRoute, PromptCacheSelection, StatefulTransportCapability,
    UsageRate, select_pricing,
};
pub use model::{MODEL_NAME_BYTES, ModelCapabilities, ModelError, ModelLimits};
pub use provider::{
    Attached, Content, Delta, DeltaStream, Effort, EffortError, Provider, ProviderError,
    ProviderLimit, Request, RequestPurpose,
};
