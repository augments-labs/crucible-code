//! The `MiMo` provider, Xiaomi's, as a dialect of Chat Completions.
//!
//! The wire is [`crate::completions`]; what is `MiMo`'s is here: its address,
//! the name it is told crucible goes by, the fields its reference names where
//! the wire's others differ, what its cache is known to do, and that its
//! models want their reasoning back.
//!
//! What its reference left unsaid is taken the way that asks least of it:
//! the ceiling goes under the one name the reference documents, the counts
//! are not asked for since it sends them unasked, and the files are text.

use crucible_credentials::Outgoing;
use crucible_models::{Delta, PromptCacheCapabilities, PromptCacheProvenance, ProviderError};
use crucible_types::{Modalities, Modality};
use serde_json::Value;

use crate::completions::wire::Thought;
use crate::completions::{Chat, Dialect, Reasoning};
use crate::endpoint::Endpoint;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "mimo";

/// Where a pay-as-you-go key is served.
const VENDOR: Endpoint = Endpoint::fixed("https://api.xiaomimimo.com/v1/chat/completions");

/// `MiMo`'s dialect of Chat Completions.
#[derive(Debug)]
pub struct MimoChat;

impl Dialect for MimoChat {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "MiMo";
    const ADDRESSES: &'static [Endpoint] = &[VENDOR];
    const SHAPE: &'static str = "mimo-chat-completions-v1";
    const USAGE_ASKED: bool = false;
    const CEILING: &'static str = "max_completion_tokens";
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
        // With thinking on, an earlier answer that called a tool and comes
        // back without its reasoning is refused with a 400, and the vendor
        // asks for it on every earlier answer.
        Reasoning::Required
    }

    fn prompt_cache(model: &str) -> PromptCacheCapabilities {
        let revision = match model {
            "mimo-v2.6-pro" => "mimo-v2.6-pro",
            "mimo-v2.6-flash" => "mimo-v2.6-flash",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        crate::completions::automatic(
            revision,
            PromptCacheProvenance::new(
                "https://mimo.mi.com/static/docs/price/pay-as-you-go.md",
                "2026-10-01",
                "mimo-prompt-cache-2026-10-01",
            ),
            // No smallest cached prefix is published; this assumes no hit
            // below the floor the wire's other vendors share.
            1_024,
        )
    }
}

/// `MiMo`'s Chat Completions API.
pub type Mimo = Chat<MimoChat>;

impl Chat<MimoChat> {
    /// Where a pay-as-you-go key is served.
    pub const VENDOR: Endpoint = VENDOR;
}

#[cfg(test)]
mod tests;
