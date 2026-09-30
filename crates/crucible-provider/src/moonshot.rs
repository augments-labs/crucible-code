//! The `MoonshotAI` provider: Kimi, as a dialect of Chat Completions.
//!
//! The wire is [`crate::completions`], which writes the request, reads the
//! response and sends one for the other, and `Chat<Kimi>` is what ships. What
//! is Kimi's is here: its addresses, the name it is told crucible goes by, how
//! it counts a cached prompt (in [`wire`]), and what its cache is known to do.
//! [`body`] and [`stream`] hold nothing that ships: they keep Kimi's tests
//! where they were, reaching the shared wire through this dialect.
//!
//! It names no HTTP client and no credential kind. A [`crate::Transport`] is
//! handed in and so is a [`crucible_credentials::Credential`], which is what
//! lets the whole protocol be tested against recorded bytes.

mod body;
mod stream;
mod wire;

use crucible_credentials::Outgoing;
use crucible_models::{
    Delta, PromptCacheCapabilities, PromptCacheContent, PromptCacheMechanismCapability,
    PromptCacheProvenance, ProviderError, StatefulTransportCapability,
};
#[cfg(test)]
use crucible_models::{Provider, Request};
#[cfg(test)]
use crucible_runtime::Cancel;
use crucible_types::{Modalities, Modality, PromptCacheRetentionClass, PromptCacheUsageReporting};
use serde_json::Value;

use crate::completions::{Chat, Dialect};
use crate::endpoint::Endpoint;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "moonshot";

/// Where a Kimi Code key is served.
const CODING: Endpoint = Endpoint::fixed("https://api.kimi.com/coding/v1/chat/completions");

/// Where an Open Platform key is served.
const PLATFORM: Endpoint = Endpoint::fixed("https://api.moonshot.ai/v1/chat/completions");

/// Where a Kimi Code key or sign-in of kimi.ai, the global site, is served.
const CODING_AI: Endpoint = Endpoint::fixed("https://api.kimi.ai/coding/v1/chat/completions");

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

/// What Kimi Code's highspeed model costs, in the vendor's words.
const HIGHSPEED: &str = "6x the speed for 3x the quota";

/// Kimi's dialect of Chat Completions.
#[derive(Debug)]
pub struct Kimi;

impl Dialect for Kimi {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "Moonshot";
    const ADDRESSES: &'static [Endpoint] = &[CODING, CODING_AI, PLATFORM];
    const SHAPE: &'static str = "moonshot-chat-completions-v1";

    fn spells() -> Modalities {
        // `chat/completions` carries pictures and videos as nested URL parts,
        // each holding a base64 `data:` URL. They are the two attachment shapes
        // this module writes, so they are the two it offers.
        Modalities::empty()
            .insert(Modality::Text)
            .insert(Modality::Image)
            .insert(Modality::Video)
    }

    fn headers(outgoing: &mut Outgoing) {
        outgoing.set_header("user-agent", AGENT);
    }

    fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
        wire::usage(payload)
    }

    fn fast(model: &str) -> crucible_models::FastForm {
        // A fast model of its own rather than a switch: nothing in the request
        // asks for it but its id, and it has no standard form to go back to.
        match model {
            "kimi-for-coding-highspeed" => crucible_models::FastForm::Own(HIGHSPEED),
            _ => crucible_models::FastForm::None,
        }
    }

    fn prompt_cache(model: &str) -> PromptCacheCapabilities {
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
}

/// `MoonshotAI`'s Chat Completions API.
pub type Moonshot = Chat<Kimi>;

impl Chat<Kimi> {
    /// Where a key from the Kimi Code console is served.
    ///
    /// Two addresses rather than one, and which of them a key belongs to is
    /// decided when the key is issued rather than by anything visible in it.
    /// Sent to the other, a key is refused in the vendor's own words, and those
    /// words do not mention that the key was fine and the address was not, so
    /// the pairing is named here, where whoever wires a key up can see both.
    pub const CODING: Endpoint = CODING;

    /// Where a key from the Open Platform console is served.
    pub const PLATFORM: Endpoint = PLATFORM;

    /// Where a Kimi Code key or sign-in of kimi.ai is served: the global site
    /// has a coding address of its own, and a credential of one site is
    /// refused by the other's.
    pub const CODING_AI: Endpoint = CODING_AI;
}

#[cfg(test)]
mod differential;
#[cfg(test)]
mod fast_tests;
#[cfg(test)]
mod identity;
#[cfg(test)]
mod tests;
