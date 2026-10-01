//! The `MiniMax` provider, as a dialect of Chat Completions.
//!
//! The wire is [`crate::completions`]; what is `MiniMax`'s is here: its two
//! sites, the name it is told crucible goes by, the fields its reference
//! names where the wire's others differ, how it reports a failure inside an
//! answer, what its cache is known to do, and that its models want their
//! reasoning back.
//!
//! Its models write their reasoning into the answer between tags unless
//! asked to keep it apart, so every request asks: read as the answer, it
//! would put the model's working in front of the user as though it were one.

use crucible_credentials::Outgoing;
use crucible_models::{Delta, PromptCacheCapabilities, PromptCacheProvenance, ProviderError};
use crucible_types::{Modalities, Modality};
use serde_json::Value;

use crate::completions::wire::Thought;
use crate::completions::{Chat, Dialect, Reasoning};
use crate::endpoint::Endpoint;
use crate::refusal::SILENT;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "minimax";

/// Where a key or plan of minimax.io, the international site, is served.
const IO: Endpoint = Endpoint::fixed("https://api.minimax.io/v1/chat/completions");

/// Where a key or plan of minimaxi.com, the mainland site, is served.
const CN: Endpoint = Endpoint::fixed("https://api.minimax.cn/v1/chat/completions");

/// `MiniMax`'s dialect of Chat Completions.
#[derive(Debug)]
pub struct MiniMaxChat;

impl Dialect for MiniMaxChat {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "MiniMax";
    const ADDRESSES: &'static [Endpoint] = &[IO, CN];
    const SHAPE: &'static str = "minimax-chat-completions-v1";
    const CEILING: &'static str = "max_completion_tokens";
    const FLAGS: &'static [(&'static str, bool)] = &[("reasoning_split", true)];
    // No code of its own is read as a request too large for the window, so
    // none is named under `OUTGREW`, and the integer it carries in
    // `base_resp.status_code` is not read for one. Its code table does list
    // `1039` "Token limit exceeded", but with "Please retry your requests
    // later", beside `1002` "rate limit": it cannot be told apart from a limit
    // on tokens over time, and compacting for a rate would shorten a session
    // for nothing. Another harness reads its overflow as `2013` "invalid
    // params" with words about the window, and `2013` is every refused
    // parameter.
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
        // Its pages disagree on the field it wants the reasoning back in;
        // the reference, which outweighs its guides, names this one beside
        // the flag that keeps it apart.
        Reasoning::Returned
    }

    fn failure(payload: &Value) -> Option<ProviderError> {
        // Every answer carries a status, and anything but zero is a failure.
        let status = payload.get("base_resp")?;
        let code = status.get("status_code").and_then(Value::as_i64)?;
        (code != 0).then(|| ProviderError::Upstream {
            provider: NAME,
            kind: code.to_string().into(),
            message: status
                .get("status_msg")
                .and_then(Value::as_str)
                .filter(|said| !said.is_empty())
                .unwrap_or(SILENT)
                .into(),
        })
    }

    fn prompt_cache(model: &str) -> PromptCacheCapabilities {
        let revision = match model {
            "MiniMax-M3" => "MiniMax-M3",
            "MiniMax-M2.7" => "MiniMax-M2.7",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        crate::completions::automatic(
            revision,
            PromptCacheProvenance::new(
                "https://platform.minimax.io/docs/api-reference/text-prompt-caching",
                "2026-10-01",
                "minimax-prompt-cache-2026-10-01",
            ),
            512,
        )
    }
}

/// `MiniMax`'s Chat Completions API.
pub type MiniMax = Chat<MiniMaxChat>;

impl Chat<MiniMaxChat> {
    /// Where a key or plan of minimax.io is served.
    pub const IO: Endpoint = IO;

    /// Where a key or plan of minimaxi.com is served. A key of one site is
    /// refused by the other's.
    pub const CN: Endpoint = CN;
}

#[cfg(test)]
mod tests;
