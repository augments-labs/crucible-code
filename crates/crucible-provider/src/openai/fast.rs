//! OpenAI's fast form: the Responses field `service_tier`, what each model's
//! fast form costs on each route, how an answer says the tier it was served
//! at, and the one refusal of it the vendor documents.
//!
//! Read on 2026-09-30 from the vendor's fast mode guide, error codes page,
//! API reference, pricing page, and the `ChatGPT` speed page.

use crucible_models::{Cost, FastForm, Served};
use serde_json::Value;

use super::Serving;

/// The value a fast request asks for. The vendor takes `"fast"` too and
/// evaluates it as this one.
pub(super) const TIER: &str = "priority";

/// What the vendor's fast mode guide says of a fast request with an API key.
const DOWNGRADED: &str = "OpenAI may serve a fast request at standard speed when fast capacity is short; it is then billed at the standard price.";

/// What `Fast` costs with an API key, for the models priced at twice the
/// standard rate.
const TWICE: Cost = Cost {
    price: "2x the price",
    caveat: Some(DOWNGRADED),
};

/// What `Fast` costs with an API key for `gpt-5.5`.
const TWO_AND_A_HALF: Cost = Cost {
    price: "2.5x the price",
    caveat: Some(DOWNGRADED),
};

/// What `Fast` costs under the `ChatGPT` sign-in. The speed page says nothing
/// of a fast request served at standard speed.
const PLAN: Cost = Cost {
    price: "2.5x your plan's usage; 2x purchased credits",
    caveat: None,
};

/// How `model` is asked to answer fast on `serving`, or on a configured
/// address, `None`: what a gateway does with the field is not the vendor's to
/// say.
pub(super) fn form(serving: Option<Serving>, model: &str) -> FastForm {
    match (serving, model) {
        (Some(Serving::Api), "gpt-6-astra" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna") => {
            FastForm::Field(TWICE)
        }
        (Some(Serving::Api), "gpt-5.5") => FastForm::Field(TWO_AND_A_HALF),
        (
            Some(Serving::Subscription),
            "gpt-6-astra" | "gpt-5.6-sol" | "gpt-5.6-terra" | "gpt-5.6-luna",
        ) => FastForm::Field(PLAN),
        _ => FastForm::None,
    }
}

/// The tier a response says it was served at, where the event carries one.
///
/// `priority` and `fast` are the fast tier; any other tier the vendor names
/// is not. Looked for only in an event that mentions the field, so the events
/// that do not are not parsed twice.
pub(super) fn served(data: &str) -> Option<Served> {
    if !data.contains("\"service_tier\"") {
        return None;
    }
    let payload: Value = serde_json::from_str(data).ok()?;
    let tier = payload
        .get("response")
        .and_then(|response| response.get("service_tier"))
        .and_then(Value::as_str)?;
    Some(if matches!(tier, "priority" | "fast") {
        Served::Fast
    } else {
        Served::Standard
    })
}

/// Whether a refused request's body is the vendor refusing the tier: a
/// `400` whose error is an `invalid_request_error` naming `service_tier` as
/// its parameter, which the vendor's error codes page gives for a tier the
/// project may not use. Asked only of a request that sent the fast tier with
/// an API key: no refusal of it is documented under the sign-in.
pub(super) fn refused(status: u16, body: &str) -> bool {
    if status != 400 {
        return false;
    }
    let Ok(payload) = serde_json::from_str::<Value>(body) else {
        return false;
    };
    let error = payload.get("error");
    let field = |name: &str| {
        error
            .and_then(|error| error.get(name))
            .and_then(Value::as_str)
    };
    field("type") == Some("invalid_request_error") && field("param") == Some("service_tier")
}
