//! The xAI provider: Grok, as a dialect of Responses.
//!
//! The wire is [`crate::responses`], which writes the request, reads the
//! response and sends one for the other, and `Responses<Grok>` is what ships.
//! What is xAI's is here: its one address, a tool declared without `strict`,
//! how it counts what a response cost, and the two shapes it words a failure
//! in.
//!
//! No `tool_choice` is ever sent, and of the cache's fields only
//! `prompt_cache_key`, which the vendor's reference lists. No reasoning is sent
//! back: the vendor keeps it for a client that does not.
//!
//! It names no HTTP client and no credential kind. A [`crate::Transport`] is
//! handed in and so is a [`crucible_credentials::Credential`], which is what
//! lets the whole protocol be tested against recorded bytes.

use crucible_credentials::Redactions;
use crucible_models::{Delta, ProviderError};
use crucible_types::{Modalities, Modality};
use serde_json::Value;

use crate::endpoint::Endpoint;
use crate::responses::wire::Counts;
use crate::responses::{Dialect, Plain, Responses};

/// What this provider is called, in errors and in the status line.
const NAME: &str = "xai";

/// Where a key from xAI's console is served.
const VENDOR: Endpoint = Endpoint::fixed("https://api.x.ai/v1/responses");

/// xAI's dialect of Responses.
#[derive(Debug)]
pub struct Grok;

impl Dialect for Grok {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "Xai";
    const ADDRESSES: &'static [Endpoint] = &[VENDOR];
    const PROTOCOL: &'static str = "xai-responses";
    const SHAPE: &'static str = "xai-responses-v1";

    // Not on the reference's function tool, and whether a field it does not
    // list is refused or ignored is not said; it is left off.
    const STRICT: bool = false;

    // The vendor's Chat Completions stream closes with this line; whether this
    // one does is not printed, so it is let pass rather than read.
    const SENTINEL: bool = true;

    type Route = ();
    type Replay = Plain;

    fn route(endpoint: &Endpoint) {
        let _ = endpoint;
    }

    fn spells() -> Modalities {
        // No part for a picture or a file sent inline is documented on this
        // route, so text alone until one is.
        Modalities::empty().insert(Modality::Text)
    }

    fn usage(payload: &Value, cache_writes: bool) -> Result<Option<Delta>, ProviderError> {
        let Some(counts) = Counts::of(payload) else {
            return Ok(None);
        };
        counted(counts).normalized::<Self>(cache_writes).map(Some)
    }

    fn failure(event: &Value) -> &Value {
        // The vendor prints its failure events with the code and the words
        // under `error`, beside the event's own `type`.
        event
            .get("error")
            .filter(|error| error.is_object())
            .unwrap_or(event)
    }

    fn refusal(error: ProviderError, redactions: &Redactions) -> ProviderError {
        match error {
            ProviderError::Refused {
                provider,
                status,
                message,
            } => match flat(&message) {
                Some(said) => ProviderError::Refused {
                    provider,
                    status,
                    message: said.into(),
                }
                .redacted(redactions),
                None => ProviderError::Refused {
                    provider,
                    status,
                    message,
                },
            },
            other => other,
        }
    }
}

/// What a response cost, as the rest of the wire counts it: reasoning inside
/// the output, and the total as the input and the output together.
///
/// The vendor's reference prints a usage whose reasoning is counted beside the
/// output rather than inside it, with a total of all three, and a stream
/// recorded from its API counts it inside, as the rest of the wire does. Both
/// are read: reasoning larger than the output, or a total that only adds up
/// with it counted a second time, is folded into the output. A total that
/// adds up neither way is left out rather than the turn failed over it.
fn counted(mut counts: Counts) -> Counts {
    if let (Some(output), Some(reasoning)) = (counts.output, counts.reasoning) {
        let beside = counts
            .input
            .and_then(|input| input.checked_add(output))
            .zip(counts.total)
            .is_some_and(|(inside, total)| {
                inside != total && inside.checked_add(reasoning) == Some(total)
            });
        if reasoning > output || beside {
            counts.output = output.checked_add(reasoning);
        }
    }
    if let (Some(input), Some(output), Some(total)) = (counts.input, counts.output, counts.total)
        && input.checked_add(output) != Some(total)
    {
        counts.total = None;
    }
    counts
}

/// The sentence a refusal in the flat shape carries: the words as `error`
/// itself, a string, with a code beside it.
fn flat(message: &str) -> Option<String> {
    serde_json::from_str::<Value>(message)
        .ok()?
        .get("error")?
        .as_str()
        .map(str::to_owned)
}

/// xAI's Responses API.
pub type Xai = Responses<Grok>;

impl Responses<Grok> {
    /// The address this API is served at, for a caller with no reason to send
    /// anywhere else.
    pub const VENDOR: Endpoint = VENDOR;
}

#[cfg(test)]
mod tests;
