//! What a model is asked and what it answers with.
//!
//! A provider adapter translates one request into its vendor's wire shape and
//! the response back into [`Delta`]s. This crate is the contract that adapter is
//! written against — [`Provider`], [`Request`], the record of what one model
//! accepts, the provider-neutral prompt-cache capabilities, projection,
//! selection and pricing, and the decision about which recorded results may be
//! sent to a vendor that did not produce them — and it holds no adapter. Each vendor's capability
//! tables, prices and wire fields stay beside that vendor's adapter.
//!
//! What an attempt leaves behind to be recorded — its usage, its cost, the
//! cache facts a session log keeps and the narrowed policy those facts name —
//! is shared data rather than a contract, and belongs to `crucible-types`. Where
//! a persistent cache resource is remembered is `crucible-storage`.
//!
//! An adapter outside this tree implements the contract against this crate and
//! the values beneath it, without compiling anything that runs a turn:
//!
//! ```
//! use crucible_models::{
//!     DeltaStream, PromptCacheCapabilities, PromptCacheRoute, Provider, ProviderError, Request,
//! };
//! use crucible_runtime::Cancel;
//! use crucible_types::{CredentialScopeId, Modalities, Modality, PromptCacheEncoding};
//!
//! struct Offline {
//!     scope: CredentialScopeId,
//! }
//!
//! impl Provider for Offline {
//!     fn name(&self) -> &'static str {
//!         "offline"
//!     }
//!
//!     fn spells(&self) -> Modalities {
//!         Modalities::empty().insert(Modality::Text)
//!     }
//!
//!     /// A route nobody has reviewed claims no caching at all.
//!     fn prompt_cache_capabilities(&self, _model: &str) -> PromptCacheCapabilities {
//!         PromptCacheCapabilities::unknown("an unreviewed adapter")
//!     }
//!
//!     fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
//!         PromptCacheRoute {
//!             protocol: "offline",
//!             endpoint: "offline",
//!             custom_endpoint: true,
//!             credential_scope: self.scope,
//!             account: None,
//!             project: None,
//!             request_shape_version: "offline-v1",
//!         }
//!     }
//!
//!     fn prompt_cache_encoding(&self, _request: &Request<'_>) -> PromptCacheEncoding {
//!         PromptCacheEncoding::NoControlIntended
//!     }
//!
//!     fn stream(
//!         &self,
//!         _request: Request<'_>,
//!         _cancel: &Cancel,
//!     ) -> Result<Box<dyn DeltaStream>, ProviderError> {
//!         Err(ProviderError::Unconfigured("this adapter answers nothing".into()))
//!     }
//! }
//!
//! let provider: Box<dyn Provider> = Box::new(Offline {
//!     scope: CredentialScopeId::new(),
//! });
//! assert_eq!(provider.restricts_results(), None);
//! ```

mod cache;
mod model;
mod provider;
mod transfer;

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
pub use transfer::{Transfer, transfer};
