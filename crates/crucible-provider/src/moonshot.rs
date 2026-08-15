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

use crucible_core::{Cancel, Credential, DeltaStream, Outgoing, Provider, ProviderError, Request};

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
        Self {
            credential,
            transport,
            endpoint,
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
        let body = body::serialize(&request);

        let response = self
            .transport
            .post(self.endpoint.as_str(), outgoing.headers(), &body)
            .map_err(|problem| ProviderError::Transport {
                provider: NAME,
                problem: problem.to_string().into(),
            })?;

        if response.status != 200 {
            return Err(refused(NAME, response.status, response.body));
        }

        Ok(Box::new(Stream::new(response.body, cancel.clone())))
    }
}

#[cfg(test)]
mod tests;
