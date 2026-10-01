//! The Qwen provider, Alibaba Cloud's Model Studio, as a dialect of Chat
//! Completions.
//!
//! The wire is [`crate::completions`]; what is Qwen's is here: the six
//! addresses its keys and plans are served at, the name it is told crucible
//! goes by, what its cache is known to do, which of its models want their
//! reasoning back, and what a key refused for belonging to the other site is
//! told: the row it was given on, by the address it was sent to.
//!
//! A key is bound to the region that issued it, and a plan's key to its
//! plan's address, so each row of `/login` names one of these.

use crucible_credentials::Outgoing;
use crucible_models::{Delta, PromptCacheCapabilities, PromptCacheProvenance, ProviderError};
use crucible_types::{Modalities, Modality};
use serde_json::Value;

use crate::completions::wire::Thought;
use crate::completions::{Chat, Dialect, Reasoning};
use crate::endpoint::Endpoint;

/// What this provider is called, in errors and in the status line.
const NAME: &str = "qwen";

/// Where a pay-as-you-go key of alibabacloud.com, the international site, is
/// served.
const KEY_INTL: Endpoint =
    Endpoint::fixed("https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions");

/// Where a pay-as-you-go key of aliyun.com, the mainland site, is served.
const KEY_CN: Endpoint =
    Endpoint::fixed("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions");

/// Where a Coding Plan key of alibabacloud.com is served.
const CODING_INTL: Endpoint =
    Endpoint::fixed("https://coding-intl.dashscope.aliyuncs.com/v1/chat/completions");

/// Where a Coding Plan key of aliyun.com is served.
const CODING_CN: Endpoint =
    Endpoint::fixed("https://coding.dashscope.aliyuncs.com/v1/chat/completions");

/// Where a Token Plan key of alibabacloud.com is served.
const TOKEN_INTL: Endpoint = Endpoint::fixed(
    "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
);

/// Where a Token Plan key of aliyun.com is served.
const TOKEN_CN: Endpoint = Endpoint::fixed(
    "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
);

/// What the vendor's cache is read from.
const CACHE: PromptCacheProvenance = PromptCacheProvenance::new(
    "https://www.alibabacloud.com/help/en/model-studio/context-cache",
    "2026-10-01",
    "qwen-prompt-cache-2026-10-01",
);

/// Qwen's dialect of Chat Completions.
#[derive(Debug)]
pub struct QwenChat;

impl Dialect for QwenChat {
    const NAME: &'static str = NAME;
    const TITLE: &'static str = "Qwen";
    const ADDRESSES: &'static [Endpoint] = &[
        KEY_INTL,
        KEY_CN,
        CODING_INTL,
        CODING_CN,
        TOKEN_INTL,
        TOKEN_CN,
    ];
    const SHAPE: &'static str = "qwen-chat-completions-v1";
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

    fn reasoning(model: &str) -> Reasoning {
        // The 3.8 models keep the earlier turns' reasoning by default and
        // read it back on each answer; the vendor answers a request without
        // it all the same, so it goes back only where it was kept.
        if model.starts_with("qwen3.8-") {
            Reasoning::Returned
        } else {
            Reasoning::Unread
        }
    }

    fn refused(status: u16, body: &str, endpoint: &Endpoint) -> Option<String> {
        // Decided by the code, which both of the vendor's guides print; the
        // sentence beside it is worded two ways.
        let error = serde_json::from_str::<Value>(body)
            .ok()?
            .get("error")?
            .clone();
        if status != 401 || error.get("code")?.as_str()? != "invalid_api_key" {
            return None;
        }
        let row = Qwen::row(endpoint)?;
        let said = error
            .get("message")
            .and_then(Value::as_str)
            .map_or("invalid_api_key", str::trim);
        Some(format!(
            "the {row} key was refused: a Qwen key belongs to the site it was made on, and to \
             that site's region, and is refused at the other's; give it on its own site's row. \
             Qwen said: {said}"
        ))
    }

    fn prompt_cache(model: &str) -> PromptCacheCapabilities {
        let revision = match model {
            "qwen3.8-max" => "qwen3.8-max",
            "qwen3.8-flash" => "qwen3.8-flash",
            "qwen3.7-plus" => "qwen3.7-plus",
            // On neither of the implicit cache's lists, though its own page
            // says it takes both kinds; nothing is asked of a cache here, so
            // only the reading of what it reports is claimed.
            "qwen3.6-plus" => "qwen3.6-plus",
            _ => return PromptCacheCapabilities::unknown("unreviewed model"),
        };
        crate::completions::automatic(revision, CACHE, 1_024)
    }
}

/// Qwen's Chat Completions API.
pub type Qwen = Chat<QwenChat>;

impl Chat<QwenChat> {
    /// Where a pay-as-you-go key of alibabacloud.com is served.
    pub const KEY_INTL: Endpoint = KEY_INTL;
    /// Where a pay-as-you-go key of aliyun.com is served.
    pub const KEY_CN: Endpoint = KEY_CN;
    /// Where a Coding Plan key of alibabacloud.com is served.
    pub const CODING_INTL: Endpoint = CODING_INTL;
    /// Where a Coding Plan key of aliyun.com is served.
    pub const CODING_CN: Endpoint = CODING_CN;
    /// Where a Token Plan key of alibabacloud.com is served.
    pub const TOKEN_INTL: Endpoint = TOKEN_INTL;
    /// Where a Token Plan key of aliyun.com is served.
    pub const TOKEN_CN: Endpoint = TOKEN_CN;

    /// The `/login` row whose key is sent to `endpoint`, as the row is shown,
    /// for a line about that key; none for an address a setting named.
    ///
    /// The rows are the application's; a test there holds these names to
    /// them.
    #[must_use]
    pub fn row(endpoint: &Endpoint) -> Option<&'static str> {
        [
            (KEY_INTL, "Qwen · alibabacloud.com"),
            (KEY_CN, "Qwen · aliyun.com"),
            (CODING_INTL, "Qwen Coding Plan · alibabacloud.com"),
            (CODING_CN, "Qwen Coding Plan · aliyun.com"),
            (TOKEN_INTL, "Qwen Token Plan · alibabacloud.com"),
            (TOKEN_CN, "Qwen Token Plan · aliyun.com"),
        ]
        .into_iter()
        .find_map(|(address, row)| (address == *endpoint).then_some(row))
    }
}

#[cfg(test)]
mod tests;
