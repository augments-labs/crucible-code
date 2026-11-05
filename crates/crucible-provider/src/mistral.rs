//! The Mistral provider, as a dialect of Chat Completions.
//!
//! The wire is [`crate::completions`]; what is Mistral's is here: its
//! address, the name it is told crucible goes by, what its cache is known to
//! do, and that its model's reasoning arrives as a thinking chunk inside the
//! answer's content and is wanted back the same way.
//!
//! What its pages left unsaid is taken the way that asks least of it: text
//! alone, since its reference spells a picture and a document in shapes of its
//! own that this wire does not write, and the ceiling under the field its
//! reference names, `max_tokens`, the wire's default.

use crucible_credentials::Outgoing;
use crucible_models::{
    Delta, PromptCacheCapabilities, PromptCacheContent, PromptCacheMechanismCapability,
    PromptCacheProvenance, ProviderError, StatefulTransportCapability,
};
use crucible_types::{Modalities, Modality, PromptCacheRetentionClass, PromptCacheUsageReporting};
use serde_json::Value;

use crate::completions::wire::Thought;
use crate::completions::{Chat, Dialect, Reasoning, ThoughtAt};
use crate::endpoint::Endpoint;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "mistral";

/// Where a key is served.
const VENDOR: Endpoint = Endpoint::fixed("https://api.mistral.ai/v1/chat/completions");

/// What the vendor's cache is read from.
const CACHE: &str = "https://docs.mistral.ai/inference/conversations/advanced/prompt-caching";

/// What a prefix the cache keeps may hold.
const CACHE_CONTENT: &[PromptCacheContent] = &[PromptCacheContent::Text, PromptCacheContent::Tools];

/// Mistral's dialect of Chat Completions.
#[derive(Debug)]
pub struct MistralChat;

impl Dialect for MistralChat {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "Mistral";
    const ADDRESSES: &'static [Endpoint] = &[VENDOR];
    const SHAPE: &'static str = "mistral-chat-completions-v1";
    // Its reasoning page prints the thinking as a chunk of the content list
    // and says to replay "the full assistant message (including ThinkChunk)".
    const THOUGHT_AT: ThoughtAt = ThoughtAt::ContentChunk;
    // Its reference names no field that asks for a stream's counts, and its
    // caching page prints them on a response that asked for none.
    const USAGE_ASKED: bool = false;
    type Kept = Thought;

    fn spells() -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn headers(outgoing: &mut Outgoing) {
        crate::completions::identify(outgoing);
    }

    fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
        crate::completions::usage::inclusive(payload, NAME, None)
    }

    fn reasoning(_model: &str) -> Reasoning {
        // "Do not strip ThinkChunk from assistant messages before replaying
        // them", and no word that a message without one is refused.
        Reasoning::Returned
    }

    fn prompt_cache(model: &str) -> PromptCacheCapabilities {
        // The page says a key "increases the chance of a cache hit, but it
        // doesn't guarantee one", that blocks hold 64 tokens and that cached
        // tokens cost 10% of the input price. It states no lifetime and does
        // not say whether a prompt is cached without the key, so the key is
        // sent whenever the session has one.
        let revision = match model {
            "mistral-large-4" => "mistral-large-4",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        let keyed =
            PromptCacheMechanismCapability::automatic_prefix(64, true, false, CACHE_CONTENT)
                .with_retentions(&[PromptCacheRetentionClass::ProviderDefault]);
        PromptCacheCapabilities::supported(
            "mistral-prompt-cache-2026-11-05",
            Some(revision),
            PromptCacheProvenance::new(CACHE, "2026-11-05", "mistral-prompt-cache-2026-11-05"),
            StatefulTransportCapability::Unsupported,
            &[keyed],
            PromptCacheUsageReporting::ReadTokens,
        )
    }
}

/// Mistral's Chat Completions API.
pub type Mistral = Chat<MistralChat>;

impl Chat<MistralChat> {
    /// Where a key is served.
    pub const VENDOR: Endpoint = VENDOR;
}

#[cfg(test)]
mod tests;
