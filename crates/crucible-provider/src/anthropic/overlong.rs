//! Anthropic's refusal of a request too large for the model.
//!
//! Anthropic gives it no code of its own: it is a `400`
//! `invalid_request_error`, the type of every refused request, whose message
//! reads "prompt is too long: N tokens > M maximum". It is told apart by that
//! type and the whole of that form opening the message, so that the same
//! words quoted back from a request inside another refusal stay that refusal.
//! The form is not confirmed against a page of Anthropic's (see
//! `fixtures/SOURCES.md`). Where it is not met, the refusal ends the turn as it
//! did, which loses the turn but never compacts a session for the wrong
//! reason.

use serde_json::Value;

/// How the message of that refusal opens.
const OPENS: &str = "prompt is too long: ";

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
            .and_then(|said| said.strip_prefix(OPENS))
            .is_some_and(counts)
}

/// Whether `said` opens with "N tokens > M maximum", both counts in digits.
fn counts(said: &str) -> bool {
    let Some(said) = number(said).and_then(|rest| rest.strip_prefix(" tokens > ")) else {
        return false;
    };
    number(said).is_some_and(|rest| rest.starts_with(" maximum"))
}

/// What follows the run of ASCII digits `said` opens with, where it opens
/// with at least one.
fn number(said: &str) -> Option<&str> {
    let rest = said.trim_start_matches(|c: char| c.is_ascii_digit());
    (rest.len() < said.len()).then_some(rest)
}
