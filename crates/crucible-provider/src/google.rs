//! Stateless Google Gemini Interactions, with credential ownership at composition.
//!
//! The body owns input steps; the wire owns streamed step assembly. Private
//! continuation is scoped to the checked recipient and key, never remote state.

mod body;
mod cache;
mod calls;
#[cfg(test)]
mod pair_tests;
mod shape;
#[cfg(test)]
mod shape_tests;
pub(crate) mod wire;

const NAME: &str = "google";
const PROTOCOL: &str = "google-interactions-v1";
const VENDOR_URL: &str = "https://generativelanguage.googleapis.com/v1beta/interactions?alt=sse";
const VENDOR: crate::Endpoint = crate::Endpoint::fixed(VENDOR_URL);

use crate::{Endpoint, Transport};
use crucible_core::{
    Cancel, ContinuationScope, Credential, CredentialScopeId, DeltaStream, Provider, ProviderError,
    Request,
};

/// Google's stateless Interactions API, authenticated by a caller-owned key.
pub struct Google {
    endpoint: Endpoint,
    credential: Box<dyn Credential>,
    credential_scope: CredentialScopeId,
    transport: Box<dyn Transport>,
}

impl std::fmt::Debug for Google {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Transport implementations may retain private request/response data.
        formatter
            .debug_struct("Google")
            .field("endpoint", &self.endpoint)
            .finish_non_exhaustive()
    }
}

impl Google {
    /// Default Google Developer API SSE route.
    pub const VENDOR: Endpoint = VENDOR;

    /// Constructs a provider at an already checked recipient.
    #[must_use]
    pub fn at(
        endpoint: Endpoint,
        credential: Box<dyn Credential>,
        transport: Box<dyn Transport>,
    ) -> Self {
        let credential_scope = credential.scope();
        Self {
            endpoint,
            credential,
            credential_scope,
            transport,
        }
    }
}

impl Provider for Google {
    fn name(&self) -> &'static str {
        NAME
    }
    fn spells(&self) -> crucible_core::Modalities {
        crucible_core::Modality::EVERY.into_iter().fold(
            crucible_core::Modalities::empty(),
            crucible_core::Modalities::insert,
        )
    }
    fn prompt_cache_capabilities(&self, model: &str) -> crucible_core::PromptCacheCapabilities {
        if self.endpoint != VENDOR {
            return crucible_core::PromptCacheCapabilities::unknown("custom endpoint");
        }
        cache::capabilities(model)
    }
    fn prompt_cache_pricing(
        &self,
        model: &str,
        revision: Option<&str>,
        tokens: Option<u64>,
        retention: crucible_core::PromptCacheRetentionClass,
        at: crucible_core::PricingDate,
    ) -> Result<Option<crucible_core::PromptCachePricing>, crucible_core::PricingError> {
        Ok(if self.endpoint == VENDOR {
            cache::pricing(model, revision, tokens, retention, at)
        } else {
            None
        })
    }
    fn prompt_cache_route(&self) -> crucible_core::PromptCacheRoute<'_> {
        crucible_core::PromptCacheRoute {
            protocol: PROTOCOL,
            endpoint: self.endpoint.as_str(),
            custom_endpoint: self.endpoint != VENDOR,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "google-interactions-stateless-v1",
        }
    }
    fn prompt_cache_encoding(&self, request: &Request<'_>) -> crucible_core::PromptCacheEncoding {
        cache::encoding(request)
    }
    fn stream(
        &self,
        request: Request<'_>,
        cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        if cancel.requested() {
            return Err(ProviderError::Cancelled(NAME));
        }
        let scope = ContinuationScope::new(self.credential_scope, self.endpoint.as_str());
        let body = body::serialize(&request, scope)?;
        let wire = wire::Interactions::new(request.model, scope)?;
        let mut outgoing = crucible_core::Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", "text/event-stream");
        self.credential
            .authorize(&mut outgoing)
            .map_err(|source| ProviderError::Credential {
                provider: NAME,
                source,
            })?;
        let redactions = outgoing.redactions();
        let response = self
            .transport
            .post(self.endpoint.as_str(), outgoing, body, cancel)
            .map_err(|error| error.for_provider(NAME).redacted(&redactions))?;
        if response.status != 200 {
            let error =
                crate::refusal::refused(NAME, response.status, response.body, &redactions, cancel);
            // A rejected stateless request can echo its signed history. Keep
            // bounded reading, cancellation and typed window recovery, but do
            // not expose Google's arbitrary refusal prose as a diagnostic.
            return Err(match error {
                ProviderError::Refused { status, .. } => ProviderError::Refused {
                    provider: NAME,
                    status,
                    message: match status {
                        401 | 403 => "check the Google API key and its model access",
                        404 => "check the Google model name and endpoint",
                        408 | 429 | 500..=599 => "Google is temporarily unable to serve this request",
                        _ => "check the Google model and request settings; private response details omitted",
                    }.into(),
                },
                error => error,
            });
        }
        Ok(Box::new(crate::stream::Response::with_wire(
            response.body,
            cancel.clone(),
            redactions,
            wire,
        )))
    }
}

fn protocol(problem: &'static str) -> crucible_core::ProviderError {
    crucible_core::ProviderError::Protocol {
        provider: NAME,
        problem: problem.into(),
    }
}

#[cfg(test)]
mod tests;
