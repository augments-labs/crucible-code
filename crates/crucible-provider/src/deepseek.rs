//! The `DeepSeek` provider, as a dialect of Chat Completions.
//!
//! The wire is [`crate::completions`]; what is `DeepSeek`'s is here: its
//! address, the name it is told crucible goes by, where it counts a cached
//! prompt, what its cache is known to do, and that a model thinking with tools
//! wants every earlier answer's reasoning back.
//!
//! What its API reference left unsaid is taken the way that asks least of it:
//! no cache key, which the reference does not name; text alone, since one of
//! its two models takes no image and the files a provider takes are declared
//! for all its models at once.

use crucible_credentials::Outgoing;
use crucible_models::{Delta, PromptCacheCapabilities, PromptCacheProvenance, ProviderError};
use crucible_types::{Modalities, Modality};
use serde_json::Value;

use crate::completions::wire::Thought;
use crate::completions::{Chat, Dialect, Reasoning};
use crate::endpoint::Endpoint;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "deepseek";

/// Where a key is served.
const VENDOR: Endpoint = Endpoint::fixed("https://api.deepseek.com/chat/completions");

/// What the vendor's cache is read from.
const CACHE: &str = "https://api-docs.deepseek.com/guides/kv_cache";

/// `DeepSeek`'s dialect of Chat Completions.
#[derive(Debug)]
pub struct DeepSeekChat;

impl Dialect for DeepSeekChat {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "DeepSeek";
    const ADDRESSES: &'static [Endpoint] = &[VENDOR];
    const SHAPE: &'static str = "deepseek-chat-completions-v1";
    type Kept = Thought;

    fn spells() -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn headers(outgoing: &mut Outgoing) {
        crate::completions::identify(outgoing);
    }

    fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
        crate::completions::usage::inclusive(payload, NAME, Some("prompt_cache_hit_tokens"))
    }

    fn reasoning(_model: &str) -> Reasoning {
        // Its thinking guide has every earlier answer's reasoning sent back
        // once a request carries tools, even an answer that called none, and
        // a request without it is refused with a 400. Both models think by
        // default.
        Reasoning::Required
    }

    fn prompt_cache(model: &str) -> PromptCacheCapabilities {
        let revision = match model {
            "deepseek-flash" => "deepseek-flash",
            "deepseek-v4-pro" => "deepseek-v4-pro",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        crate::completions::automatic(
            revision,
            PromptCacheProvenance::new(CACHE, "2026-10-01", "deepseek-prompt-cache-2026-10-01"),
            // No smallest cached prefix is published; this assumes no hit
            // below the floor the wire's other vendors share.
            1_024,
        )
    }
}

/// `DeepSeek`'s Chat Completions API.
pub type DeepSeek = Chat<DeepSeekChat>;

impl Chat<DeepSeekChat> {
    /// Where a key is served.
    pub const VENDOR: Endpoint = VENDOR;
}

#[cfg(test)]
mod tests;
