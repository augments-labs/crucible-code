//! The Z.ai provider, as a dialect of Chat Completions.
//!
//! The wire is [`crate::completions`]; what is Z.ai's is here: its two
//! sites, the name it is told crucible goes by, what its cache is known to
//! do, the reasons to stop it has words of its own for, and the code it
//! refuses a request too large for the model with.
//!
//! What its references left unsaid is taken the way that asks least of it:
//! the counts are not asked for, since neither site documents the field that
//! asks and both send them unasked; text alone.

use crucible_credentials::Outgoing;
use crucible_models::{Delta, PromptCacheCapabilities, PromptCacheProvenance, ProviderError};
use crucible_types::{Modalities, Modality, StopReason};
use serde_json::Value;

use crate::completions::{Chat, Dialect};
use crate::endpoint::Endpoint;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "zai";

/// Where a key of z.ai, the international site, is served.
const ZAI: Endpoint = Endpoint::fixed("https://api.z.ai/api/paas/v4/chat/completions");

/// Where a key of bigmodel.cn, the mainland site, is served.
const BIGMODEL: Endpoint = Endpoint::fixed("https://open.bigmodel.cn/api/paas/v4/chat/completions");

/// Z.ai's dialect of Chat Completions.
#[derive(Debug)]
pub struct ZaiChat;

impl Dialect for ZaiChat {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "Z.ai";
    const ADDRESSES: &'static [Endpoint] = &[ZAI, BIGMODEL];
    const SHAPE: &'static str = "zai-chat-completions-v1";
    const USAGE_ASKED: bool = false;
    // `1261` (400) "Prompt too long", on the error-code page of both sites,
    // which print the code as a string under `error`.
    const OUTGREW: &'static [&'static str] = &["1261"];
    // Its reasoning is cleared by the vendor between turns unless asked to
    // keep it, so none is kept here to send back.
    type Kept = ();

    fn spells() -> Modalities {
        Modalities::empty().insert(Modality::Text)
    }

    fn headers(outgoing: &mut Outgoing) {
        crate::completions::identify(outgoing);
    }

    fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
        crate::completions::usage::inclusive(payload, NAME, None)
    }

    fn stopped(reason: &str) -> Option<StopReason> {
        match reason {
            // The answer was held back by the vendor's review.
            "sensitive" => Some(StopReason::Filtered),
            // The conversation no longer fits the model: its remedy is room
            // made in the window, not a longer answer.
            "model_context_window_exceeded" => Some(StopReason::WindowExceeded),
            // `network_error` is the model failing part way, which every
            // other unknown reason already reads as: unfinished.
            _ => None,
        }
    }

    fn prompt_cache(model: &str) -> PromptCacheCapabilities {
        let revision = match model {
            "glm-5.3" => "glm-5.3",
            "glm-5.3-flash" => "glm-5.3-flash",
            "glm-5.2" => "glm-5.2",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        crate::completions::automatic(
            revision,
            PromptCacheProvenance::new(
                "https://docs.z.ai/guides/capabilities/cache",
                "2026-10-01",
                "zai-prompt-cache-2026-10-01",
            ),
            // No smallest cached prefix is published; this assumes no hit
            // below the floor the wire's other vendors share.
            1_024,
        )
    }
}

/// Z.ai's Chat Completions API.
pub type Zai = Chat<ZaiChat>;

impl Chat<ZaiChat> {
    /// Where a key of z.ai is served.
    pub const ZAI: Endpoint = ZAI;

    /// Where a key of bigmodel.cn is served. A key of one site is refused by
    /// the other's.
    pub const BIGMODEL: Endpoint = BIGMODEL;
}

#[cfg(test)]
mod tests;
