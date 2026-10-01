//! Anthropic's refusal of a request too large for the model.
//!
//! Anthropic gives it no code of its own: its context windows page says the
//! refusal is a `400` `invalid_request_error`, the type of every refused
//! request, saying "prompt is too long" (see `fixtures/SOURCES.md`). It is
//! told apart by that type and those words opening the message, so that the
//! same words quoted back from a request inside another refusal stay that
//! refusal. Where they are not met, the refusal ends the turn as it did, which
//! loses the turn but never compacts a session for the wrong reason.

use serde_json::Value;

/// How the message of that refusal opens.
const OPENS: &str = "prompt is too long";

/// Whether a refusal is Anthropic's of a request too large for the model.
pub(super) fn outgrew(status: u16, body: &Value) -> bool {
    let Some(error) = body.get("error") else {
        return false;
    };
    status == 400
        && error.get("type").and_then(Value::as_str) == Some("invalid_request_error")
        && error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|said| said.starts_with(OPENS))
}
