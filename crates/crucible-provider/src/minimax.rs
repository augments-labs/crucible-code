//! The `MiniMax` provider, as a dialect of Chat Completions.
//!
//! The wire is [`crate::completions`]; what is `MiniMax`'s is here: its two
//! sites, the name it is told crucible goes by, the fields its reference
//! names where the wire's others differ, how it reports a failure inside an
//! answer and which of its failures is a request too large for the model,
//! what its cache is known to do, and that its models want their reasoning
//! back.
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
    // none is named under `OUTGREW`. Its code table does list `1039` "Token
    // limit exceeded", but with "Please retry your requests later", beside
    // `1002` "rate limit": it cannot be told apart from a limit on tokens over
    // time, and compacting for a rate would shorten a session for nothing.
    // Its overflow is `2013` "invalid params" with words about the window, and
    // `2013` is every refused parameter, so it is read as that code and those
    // words together: see [`refused_outgrown`], and [`outgrew`] behind it.
    const OVERLONG: Option<fn(u16, &Value) -> bool> = Some(refused_outgrown);
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
        // The one that says the request was too large is not about the moment,
        // and sending it again would be refused the same way.
        if outgrew(payload) {
            return Some(ProviderError::WindowExceeded { provider: NAME });
        }
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

/// How `MiniMax`'s refusal of a request too large for the model opens.
///
/// No page of `MiniMax`'s prints this refusal. The wording is taken from
/// another client's handling of `MiniMax`'s overflow (the Pi coding agent's
/// overflow matcher) and is unconfirmed: if the vendor words it otherwise,
/// the refusal is not recognised and ends the turn as any other `2013` does,
/// which loses the turn but never compacts a session for the wrong reason.
const OUTGROWN: &str = "invalid params, context window exceeds limit";

/// Whether a status `MiniMax` reports is its refusal of a request too large
/// for the model.
///
/// It sends no code for that refusal alone, so it is read as the code every
/// refused parameter has, `2013` under `base_resp.status_code`, together with
/// a `status_msg` that opens with [`OUTGROWN`], exactly and in this case.
/// Reading only the opening keeps a refusal that quotes those words back from
/// what was sent from reading as this one. The same code with other words,
/// such as the refusal of thinking switched off, and the same words under any
/// other code, stay the failures they were.
fn outgrew(payload: &Value) -> bool {
    let Some(status) = payload.get("base_resp") else {
        return false;
    };
    status.get("status_code").and_then(Value::as_i64) == Some(2013)
        && status
            .get("status_msg")
            .and_then(Value::as_str)
            .is_some_and(|said| said.starts_with(OUTGROWN))
}

/// [`outgrew`], for a refused body: the status it came with says nothing the
/// body does not.
fn refused_outgrown(_status: u16, body: &Value) -> bool {
    outgrew(body)
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
