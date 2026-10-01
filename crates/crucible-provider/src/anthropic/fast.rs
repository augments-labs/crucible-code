//! Anthropic's fast form: `speed: "fast"` with the fast mode beta header, on
//! the models the vendor serves fast, how an answer says the speed it was
//! served at, and the refusals taken for a refusal of fast.
//!
//! Read on 2026-09-30 from the vendor's fast mode page and API reference. The
//! vendor says only that a refused fast request "returns an error"; which
//! errors those are is taken from open harnesses that are not the vendor's,
//! which match the message a refusal carries.

use crucible_credentials::Outgoing;
use crucible_models::{Cost, FastForm, Served};
use serde_json::Value;

/// The beta a fast request carries.
const BETA: &str = "fast-mode-2026-02-01";

/// What `Fast` costs, in the vendor's words, for the model priced so, and
/// who may use it.
const PRICE: Cost = Cost {
    price: "$10 / $50 per million input / output tokens",
    speed: Some("up to 2.5x higher output tokens per second"),
    caveat: Some("Fast mode is a research preview; Anthropic turns it on per organization."),
};

/// What `Fast` costs on Claude Opus 5.5, read 2026-10-01: its own price,
/// lower than Opus 5's, with the same speed and the same preview.
const OPUS_55_PRICE: Cost = Cost {
    price: "$8 / $40 per million input / output tokens",
    ..PRICE
};

/// How `model` is asked to answer fast on the vendor's own address, and on
/// a configured one, `None`.
pub(super) fn form(vendor: bool, model: &str) -> FastForm {
    match (vendor, model) {
        (true, "claude-opus-5") => FastForm::Field(PRICE),
        (true, "claude-opus-5-5") => FastForm::Field(OPUS_55_PRICE),
        _ => FastForm::None,
    }
}

/// Adds the fast mode beta to whatever betas the request already carries,
/// joined rather than written over: the vendor reads one comma-separated
/// header.
pub(super) fn beta(outgoing: &mut Outgoing) {
    let joined = outgoing
        .headers()
        .iter()
        .find(|(name, _)| &**name == "anthropic-beta")
        .map_or_else(|| BETA.to_owned(), |(_, value)| format!("{value},{BETA}"));
    outgoing.set_header("anthropic-beta", joined);
}

/// The speed an event's usage says the answer was served at, where it says.
///
/// Looked for only in an event that mentions the field, so the events that
/// do not are not parsed twice.
pub(super) fn served(data: &str) -> Option<Served> {
    if !data.contains("\"speed\"") {
        return None;
    }
    let payload: Value = serde_json::from_str(data).ok()?;
    let usage = payload.get("usage").or_else(|| {
        payload
            .get("message")
            .and_then(|message| message.get("usage"))
    })?;
    match usage.get("speed").and_then(Value::as_str)? {
        "fast" => Some(Served::Fast),
        _ => Some(Served::Standard),
    }
}

/// Whether a refused body is a refusal of fast: on a request that asked for
/// it, a `400` `invalid_request_error` whose message names fast mode or the
/// `speed` parameter, or the `429` that says usage credits are required for
/// fast mode. Every other refusal of a fast request is the one it would have
/// been at standard speed.
pub(super) fn refused(status: u16, body: &str) -> bool {
    let Ok(payload) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let error = payload.get("error");
    let field = |name: &str| {
        error
            .and_then(|error| error.get(name))
            .and_then(Value::as_str)
    };
    let message = field("message").unwrap_or_default();
    match (status, field("type")) {
        (400, Some("invalid_request_error")) => names_fast(message),
        (429, _) => message == "Usage credits are required for fast mode.",
        _ => false,
    }
}

/// Whether `message` names fast mode, however spaced, or the word `speed`.
fn names_fast(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    let fast_mode = ["fast mode", "fast-mode", "fast_mode", "fastmode"]
        .iter()
        .any(|spelled| lower.contains(spelled));
    let speed = lower
        .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .any(|word| word == "speed");
    fast_mode || speed
}
