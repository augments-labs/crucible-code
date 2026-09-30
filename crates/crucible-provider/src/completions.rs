//! Chat Completions, for every vendor that serves it.
//!
//! The request is written in [`body`], one event of a response is read in
//! [`wire`], and [`Chat`] is the provider that sends one and hands back the
//! other. None of the three has a case for any one vendor. What differs between
//! the vendors that serve this wire is a [`Dialect`]: what the provider is
//! called, the addresses the vendor serves, the headers it asks for beside the
//! credential, how it counts what a response cost, how it spells a rung of
//! effort, and what its prompt cache is known to do.
//!
//! A second vendor on this wire is a dialect and nothing else: a type that
//! implements [`Dialect`], and a name for `Chat` over it.
//!
//! This is not [`crate::openai`], which posts to Responses and reads a
//! narration this endpoint does not send. The two are the same vendor's
//! protocols in the way two releases of a format are the same format: the
//! fields, the framing and the shape of a transcript all differ.
//!
//! It names no HTTP client and no credential kind. A [`Transport`] is handed in
//! and so is a [`Credential`], which is what lets the whole protocol be tested
//! against recorded bytes.

pub(crate) mod body;
pub(crate) mod wire;

use std::fmt;
use std::marker::PhantomData;

use crucible_credentials::{Credential, Outgoing};
use crucible_models::{
    Delta, DeltaStream, Effort, FastForm, PromptCacheCapabilities, PromptCacheRoute, Provider,
    ProviderError, Request,
};
use crucible_runtime::{BoxFuture, Cancel};
use crucible_types::{CredentialScopeId, Modalities, PromptCacheEncoding};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::refusal::refused;
use crate::stream::Response;
use crate::transport::Transport;

/// What one vendor on this wire does its own way.
///
/// Everything here is about the vendor rather than about one request, so none
/// of it takes `self`: a dialect is a type, and [`Chat`] is generic over it.
pub trait Dialect: Send + Sync + 'static {
    /// What the provider is called, in errors and in the status line.
    const NAME: &'static str;

    /// What the vendor is called in a line a request carries in place of a
    /// file it cannot take, and what the provider is shown as in `Debug`.
    const TITLE: &'static str;

    /// The addresses the vendor serves. Any other is one a setting named, and
    /// nothing is assumed about what its cache does.
    const ADDRESSES: &'static [Endpoint];

    /// What the prompt cache records a request's shape as, so a change to the
    /// shape is not read as the same cache.
    const SHAPE: &'static str;

    /// The kinds of file the vendor's models take.
    fn spells() -> Modalities;

    /// Headers the vendor asks for beside the content type, the stream it
    /// answers in and the credential, which [`Chat`] sets itself.
    fn headers(outgoing: &mut Outgoing);

    /// What a response cost, from the payload of an event that carries it,
    /// or nothing where the event carries no count.
    ///
    /// # Errors
    ///
    /// [`ProviderError::Protocol`] where the counts contradict each other.
    fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError>;

    /// How the vendor spells `effort`.
    fn effort(effort: Effort) -> &'static str {
        effort.as_str()
    }

    /// What the vendor's cache is known to do for `model`, at one of its own
    /// addresses.
    fn prompt_cache(model: &str) -> PromptCacheCapabilities;

    /// How `model` is asked to answer fast, at one of the vendor's own
    /// addresses. None, by default: the answer of a vendor that serves no
    /// fast form on this wire.
    fn fast(model: &str) -> FastForm {
        let _ = model;
        FastForm::None
    }
}

/// A Chat Completions provider, speaking `D`'s dialect.
pub struct Chat<D: Dialect> {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
    credential_scope: CredentialScopeId,
    dialect: PhantomData<D>,
}

impl<D: Dialect> fmt::Debug for Chat<D> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct(D::TITLE)
            .field("credential", &self.credential)
            .field("transport", &self.transport)
            .field("endpoint", &self.endpoint)
            .field("credential_scope", &self.credential_scope)
            .finish()
    }
}

impl<D: Dialect> Chat<D> {
    /// How `model` is asked to answer fast at the vendor's own address:
    /// what a list of models says before any provider is set up.
    #[must_use]
    pub fn fast_at_vendor(model: &str) -> crucible_models::FastForm {
        D::fast(model)
    }

    /// A provider that authenticates with `credential`, sends over `transport`
    /// and posts to `endpoint`.
    ///
    /// The address is named by the caller rather than defaulted here, because
    /// the wiring is where a decision like that belongs and there is one
    /// constructor rather than a defaulting one beside an explicit one. It is
    /// an [`Endpoint`] rather than a string because what decides who receives
    /// the key is checked before it is one.
    #[must_use]
    pub fn at(
        endpoint: Endpoint,
        credential: Box<dyn Credential>,
        transport: Box<dyn Transport>,
    ) -> Self {
        let credential_scope = credential.scope();
        Self {
            credential,
            transport,
            endpoint,
            credential_scope,
            dialect: PhantomData,
        }
    }

    /// Whether requests go to one of the vendor's own addresses rather than
    /// one a setting named.
    fn vendor(&self) -> bool {
        D::ADDRESSES.contains(&self.endpoint)
    }

    /// The headers every request carries, including the secret.
    async fn headers(&self, cancel: &Cancel) -> Result<Outgoing, ProviderError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", "text/event-stream");
        D::headers(&mut outgoing);

        // Raced against the turn's cancel: a credential renewing its token
        // waits for a renewal that is the renewal's own work, so a turn
        // stopped meanwhile stops waiting here and leaves it to finish.
        cancel
            .race(self.credential.authorize(&mut outgoing))
            .await
            .ok_or(ProviderError::Cancelled(D::NAME))?
            .map_err(|source| ProviderError::Credential {
                provider: D::NAME,
                source,
            })?;

        Ok(outgoing)
    }
}

impl<D: Dialect> Provider for Chat<D> {
    fn name(&self) -> &'static str {
        D::NAME
    }

    fn spells(&self) -> Modalities {
        D::spells()
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        if !self.vendor() {
            return PromptCacheCapabilities::unknown("custom endpoint");
        }
        D::prompt_cache(model)
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "openai-chat-completions",
            endpoint: self.endpoint.as_str(),
            custom_endpoint: !self.vendor(),
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: D::SHAPE,
        }
    }

    fn fast(&self, model: &str) -> FastForm {
        if self.vendor() {
            D::fast(model)
        } else {
            FastForm::None
        }
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> PromptCacheEncoding {
        body::prompt_cache_encoding(request)
    }

    fn stream<'a>(
        &'a self,
        request: Request<'a>,
        cancel: &'a Cancel,
    ) -> BoxFuture<'a, Result<Box<dyn DeltaStream>, ProviderError>> {
        Box::pin(async move {
            // Nothing is sent for a turn the user has already abandoned. Once the
            // request is away, cancelling is the stream's business.
            if cancel.requested() {
                return Err(ProviderError::Cancelled(D::NAME));
            }

            let mut outgoing = self.headers(cancel).await?;
            let body = body::serialize::<D>(&request);

            let response = self
                .transport
                .post(self.endpoint.as_str(), &mut outgoing, body, cancel)
                .await;
            let redactions = outgoing.redactions();
            let response =
                response.map_err(|problem| problem.for_provider(D::NAME).redacted(&redactions))?;

            if response.status() != 200 {
                return Err(
                    refused(D::NAME, response.status(), response, &redactions, cancel).await,
                );
            }

            Ok(Box::new(Response::<wire::Completions<D>>::new(
                response.into_reader(),
                cancel.clone(),
                redactions,
            )) as Box<dyn DeltaStream>)
        })
    }
}

#[cfg(test)]
mod tests;
