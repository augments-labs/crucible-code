//! The `MoonshotAI` provider.
//!
//! Four parts, and the split is the direction data travels: [`body`] builds a
//! request, [`wire`] reads one event of a response, [`stream`] delivers a whole
//! one, and this file is the request itself — the headers and the status.
//!
//! Only two of those are this vendor's. Delivering a response is the same job
//! whoever sent it, so the loop that does it lives in `crate::stream` and
//! [`stream`] is where this protocol is handed to it.
//!
//! Chat Completions, and a module of its own rather than a second address for
//! [`crate::openai`], which posts to Responses and reads a narration this
//! endpoint does not send. The two are OpenAI's protocols in the way two
//! releases of a format are the same format: the fields, the framing and the
//! shape of a transcript all differ, and a reader for one finds nothing it
//! recognises in the other. This is the endpoint the compatible vendors serve.
//!
//! It names no HTTP client and no credential kind. A [`Transport`] is handed in
//! and so is a [`Credential`], which is what lets the whole protocol be tested
//! against recorded bytes.

mod body;
mod stream;
mod wire;

use crucible_core::{
    Cancel, Credential, CredentialScopeId, DeltaStream, Modalities, Modality, Outgoing,
    PromptCacheCapabilities, PromptCacheContent, PromptCacheMechanismCapability,
    PromptCacheProvenance, PromptCacheRetentionClass, PromptCacheRoute, PromptCacheUsageReporting,
    Provider, ProviderError, Request, StatefulTransportCapability,
};

use crate::endpoint::Endpoint;
use crate::moonshot::stream::Stream;
use crate::refusal::refused;
use crate::transport::Transport;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "moonshot";

/// Where a Kimi Code key is served.
const CODING: Endpoint = Endpoint::fixed("https://api.kimi.com/coding/v1/chat/completions");

/// Where an Open Platform key is served.
const PLATFORM: Endpoint = Endpoint::fixed("https://api.moonshot.ai/v1/chat/completions");

const MOONSHOT_CACHE_CONTENT: &[PromptCacheContent] = &[
    PromptCacheContent::Text,
    PromptCacheContent::Tools,
    PromptCacheContent::Images,
    PromptCacheContent::Video,
];

/// What this harness is called, to a vendor that asks to be told.
///
/// `MoonshotAI`'s terms require a client to identify itself truthfully and treat
/// a tampered identifier as a violation, so this is sent rather than left to
/// whatever the HTTP client would say on its own. It names crucible because
/// crucible is what is calling.
const AGENT: &str = concat!("crucible/", env!("CARGO_PKG_VERSION"));

/// `MoonshotAI`'s Chat Completions API.
#[derive(Debug)]
pub struct Moonshot {
    credential: Box<dyn Credential>,
    transport: Box<dyn Transport>,
    endpoint: Endpoint,
    credential_scope: CredentialScopeId,
}

impl Moonshot {
    /// Where a key from the Kimi Code console is served.
    ///
    /// Two addresses rather than one, and which of them a key belongs to is
    /// decided when the key is issued rather than by anything visible in it.
    /// Sent to the other, a key is refused in the vendor's own words, and those
    /// words do not mention that the key was fine and the address was not — so
    /// the pairing is named here, where whoever wires a key up can see both.
    pub const CODING: Endpoint = CODING;

    /// Where a key from the Open Platform console is served.
    pub const PLATFORM: Endpoint = PLATFORM;

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
        }
    }

    /// The headers every request carries, including the secret.
    fn headers(&self) -> Result<Outgoing, ProviderError> {
        let mut outgoing = Outgoing::new();
        outgoing.set_header("content-type", "application/json");
        outgoing.set_header("accept", "text/event-stream");
        outgoing.set_header("user-agent", AGENT);

        self.credential
            .authorize(&mut outgoing)
            .map_err(|source| ProviderError::Credential {
                provider: NAME,
                source,
            })?;

        Ok(outgoing)
    }
}

impl Provider for Moonshot {
    fn name(&self) -> &'static str {
        NAME
    }

    fn spells(&self) -> Modalities {
        // `chat/completions` carries pictures and videos as nested URL parts,
        // each holding a base64 `data:` URL. They are the two attachment shapes
        // this module writes, so they are the two it offers.
        Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video)
    }

    fn prompt_cache_capabilities(&self, model: &str) -> PromptCacheCapabilities {
        if self.endpoint != CODING && self.endpoint != PLATFORM {
            return PromptCacheCapabilities::unknown("custom endpoint");
        }
        let revision = match model {
            "k3" => "k3",
            "k3-256k" => "k3-256k",
            "kimi-for-coding" => "kimi-for-coding",
            "kimi-for-coding-highspeed" => "kimi-for-coding-highspeed",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        let automatic = PromptCacheMechanismCapability::automatic_prefix(
            257,
            true,
            false,
            MOONSHOT_CACHE_CONTENT,
        )
        .with_retentions(&[PromptCacheRetentionClass::ProviderDefault]);
        PromptCacheCapabilities::supported(
            "kimi-prompt-cache-2026-08-31",
            Some(revision),
            PromptCacheProvenance::new(
                "https://platform.kimi.ai/docs/guide/use-context-caching-feature-of-kimi-api",
                "2026-08-31",
                "kimi-prompt-cache-2026-08-31",
            ),
            StatefulTransportCapability::Unsupported,
            &[automatic],
            PromptCacheUsageReporting::ReadTokens,
        )
    }

    fn prompt_cache_route(&self) -> PromptCacheRoute<'_> {
        PromptCacheRoute {
            protocol: "openai-chat-completions",
            endpoint: self.endpoint.as_str(),
            custom_endpoint: self.endpoint != CODING && self.endpoint != PLATFORM,
            credential_scope: self.credential_scope,
            account: None,
            project: None,
            request_shape_version: "moonshot-chat-completions-v1",
        }
    }

    fn prompt_cache_encoding(&self, request: &Request<'_>) -> crucible_core::PromptCacheEncoding {
        body::prompt_cache_encoding(request)
    }

    fn stream(
        &self,
        request: Request<'_>,
        cancel: &Cancel,
    ) -> Result<Box<dyn DeltaStream>, ProviderError> {
        // Nothing is sent for a turn the user has already abandoned. Once the
        // request is away, cancelling is the stream's business.
        if cancel.requested() {
            return Err(ProviderError::Cancelled(NAME));
        }

        let outgoing = self.headers()?;
        let redactions = outgoing.redactions();
        let body = body::serialize(&request);

        let response = self
            .transport
            .post(self.endpoint.as_str(), outgoing, body, cancel)
            .map_err(|problem| problem.for_provider(NAME).redacted(&redactions))?;

        if response.status != 200 {
            return Err(refused(
                NAME,
                response.status,
                response.body,
                &redactions,
                cancel,
            ));
        }

        Ok(Box::new(Stream::new(
            response.body,
            cancel.clone(),
            redactions,
        )))
    }
}

#[cfg(test)]
mod tests;
