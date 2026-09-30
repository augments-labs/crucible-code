//! Google's fast form: the Interactions field `service_tier: "priority"`, on
//! the models the vendor lists for it, and how a completed interaction says
//! the tier it was served at.
//!
//! Read on 2026-09-30 from the vendor's priority inference pages and the
//! Interactions API reference. No page says what a refused priority request
//! returns, so no refusal is taken for a refusal of fast: whatever comes back
//! is reported as it would be at standard speed. Congestion does not refuse;
//! it serves the request at the standard tier, which the answer then says.

use crucible_models::{Cost, FastForm, Served};
use serde_json::Value;

/// The value a fast request asks for.
pub(super) const TIER: &str = "priority";

/// What `Fast` costs, in the vendor's words, who may use it, and what
/// congestion does to it.
const PRICE: Cost = Cost {
    price: "75-100% more than Standard",
    caveat: Some(
        "For Tier 2 and Tier 3 accounts only. Google serves a priority request at standard speed when priority is congested, and bills it at the standard price.",
    ),
};

/// How `model` is asked to answer fast at the vendor's own address, and at a
/// configured one, `None`. The models are those the vendor's priority page
/// lists that crucible serves.
pub(super) fn form(vendor: bool, model: &str) -> FastForm {
    match (vendor, model) {
        (
            true,
            "gemini-3.8-flash" | "gemini-3.7-flash" | "gemini-3.6-flash" | "gemini-3.1-pro-preview",
        ) => FastForm::Field(PRICE),
        _ => FastForm::None,
    }
}

/// The tier a completed interaction says it was served at, where it says.
pub(super) fn served(payload: &Value) -> Option<Served> {
    let tier = payload
        .pointer("/interaction/service_tier")
        .and_then(Value::as_str)?;
    Some(if tier == TIER {
        Served::Fast
    } else {
        Served::Standard
    })
}
