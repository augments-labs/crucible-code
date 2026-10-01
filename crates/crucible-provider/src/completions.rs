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
pub(crate) mod usage;
pub(crate) mod wire;

use std::fmt;
use std::marker::PhantomData;

use crucible_credentials::{Credential, Outgoing};
use crucible_models::{
    Cost, Delta, DeltaStream, Effort, FastForm, PromptCacheCapabilities, PromptCacheRoute,
    Provider, ProviderError, Request,
};
use crucible_runtime::{BoxFuture, Cancel};
use crucible_types::{
    ContinuationScope, CredentialScopeId, Modalities, PromptCacheEncoding, StopReason,
};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::refusal::{Own, refused_worded};
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

    /// What `model` costs where it is itself a fast model, with no standard
    /// form to switch to. None, by default. Only such a model is named here:
    /// `Chat<D>` writes no field that asks for fast, so a form the request
    /// would have to carry cannot be declared, and none is offered unsent.
    fn own_fast(model: &str) -> Option<Cost> {
        let _ = model;
        None
    }

    /// What a response's reader keeps of its reasoning: `()` for a vendor
    /// that wants none back, [`wire::Thought`] for one that does.
    type Kept: wire::Keeps;

    /// What `model`'s reasoning, written before its answer, is to the vendor.
    /// Read past and never sent back, by default.
    fn reasoning(model: &str) -> Reasoning {
        let _ = model;
        Reasoning::Unread
    }

    /// Whether the counts of a response have to be asked for. A vendor that
    /// sends them unasked, and does not document the field that asks, is sent
    /// nothing it might refuse.
    const USAGE_ASKED: bool = true;

    /// The field the ceiling on generated tokens is written under.
    const CEILING: &'static str = "max_tokens";

    /// Fields the vendor needs on every request, each a flag set the same way
    /// whatever is asked.
    const FLAGS: &'static [(&'static str, bool)] = &[];

    /// A failure the vendor reports inside an event that otherwise reads as
    /// part of an answer, where it has a way of its own to say one.
    fn failure(payload: &Value) -> Option<ProviderError> {
        let _ = payload;
        None
    }

    /// What a refusal at `endpoint` means, as a line of crucible's own, where
    /// the vendor's code says more than its sentence: a key the vendor says
    /// belongs elsewhere names the row it was given on. `None`, by default,
    /// leaves the vendor's sentence as it is.
    fn refused(status: u16, body: &str, endpoint: &Endpoint) -> Option<String> {
        let _ = (status, body, endpoint);
        None
    }

    /// The codes the vendor refuses a request too large for the model's window
    /// with, beyond the ones every vendor's refusals are read for: a refusal
    /// with one of them is [`ProviderError::WindowExceeded`], which the session
    /// is compacted for and the question asked again. Matched against the
    /// code under `error` or at the top level, as text or as a whole number by
    /// its decimal spelling, never against the sentence beside it. None, by
    /// default.
    const OUTGREW: &'static [&'static str] = &[];

    /// Why the model stopped, for a reason the vendor has words of its own for.
    /// `None` leaves it to the reasons every vendor on this wire shares.
    fn stopped(reason: &str) -> Option<StopReason> {
        let _ = reason;
        None
    }
}

/// What a model's reasoning is to the vendor that wrote it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reasoning {
    /// Read past and never sent back: the vendor keeps none of it.
    Unread,
    /// Kept beside the answer it came with, and sent back on that message.
    Returned,
    /// Kept and sent back the same way, and sent as empty on a message that
    /// has none, once a request carries tools: the vendor refuses a turn with
    /// tools that leaves any earlier answer without it.
    Required,
}

/// What this harness is called, to a vendor on this wire.
///
/// Sent rather than left to whatever the HTTP client would say on its own: a
/// vendor that reads it reads the client that is calling.
const AGENT: &str = concat!("crucible/", env!("CARGO_PKG_VERSION"));

/// Names crucible to the vendor, for a dialect whose headers say who calls.
pub(crate) fn identify(outgoing: &mut Outgoing) {
    outgoing.set_header("user-agent", AGENT);
}

/// What a vendor's cache is known to do where it caches a prompt's prefix on
/// its own and reports what it read, with no field a request could set: the
/// record for `revision`, reviewed at `provenance`, assuming no hit below
/// `minimum` tokens.
pub(crate) fn automatic(
    revision: &'static str,
    provenance: crucible_models::PromptCacheProvenance,
    minimum: u32,
) -> PromptCacheCapabilities {
    let usage_only = crucible_models::PromptCacheMechanismCapability::provider_managed(
        minimum,
        &[
            crucible_models::PromptCacheContent::Text,
            crucible_models::PromptCacheContent::Tools,
        ],
    );
    PromptCacheCapabilities::supported(
        provenance.record_version(),
        Some(revision),
        provenance,
        crucible_models::StatefulTransportCapability::Unsupported,
        &[usage_only],
        crucible_types::PromptCacheUsageReporting::ReadTokens,
    )
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
        D::own_fast(model).map_or(FastForm::None, FastForm::Own)
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

    /// Where reasoning kept for `request` is bound, or nothing where the vendor
    /// wants none of it back. Worked out where it is used rather than held
    /// across the send, which would be room every request carries for it.
    fn keeping(&self, request: &Request<'_>) -> Option<ContinuationScope> {
        (D::reasoning(request.model) != Reasoning::Unread)
            .then(|| ContinuationScope::new(self.credential_scope, self.endpoint.as_str()))
    }

    /// Where an answer this wire kept reasoning for is bound, for a vendor
    /// that keeps any: what tells its own earlier answers, whichever model
    /// wrote them, from another vendor's or another credential's.
    fn owning(&self) -> Option<ContinuationScope> {
        <D::Kept as wire::Keeps>::KEEPS
            .then(|| ContinuationScope::new(self.credential_scope, self.endpoint.as_str()))
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
            Self::fast_at_vendor(model)
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
            let body = body::serialize_for::<D>(&request, self.owning());

            let response = self
                .transport
                .post(self.endpoint.as_str(), &mut outgoing, body, cancel)
                .await;
            let redactions = outgoing.redactions();
            let response =
                response.map_err(|problem| problem.for_provider(D::NAME).redacted(&redactions))?;

            if response.status() != 200 {
                return Err(refused_worded(
                    D::NAME,
                    response,
                    &redactions,
                    cancel,
                    Own {
                        outgrew: || D::OUTGREW,
                        worded: |status, body: &str| D::refused(status, body, &self.endpoint),
                    },
                )
                .await);
            }

            Ok(Box::new(Response::with_wire(
                response.into_reader(),
                cancel.clone(),
                redactions,
                wire::Completions::<D>::for_request(&request, self.keeping(&request)),
            )) as Box<dyn DeltaStream>)
        })
    }
}

#[cfg(test)]
mod reasoning_tests;
#[cfg(test)]
pub(crate) mod testing;
#[cfg(test)]
mod tests;
