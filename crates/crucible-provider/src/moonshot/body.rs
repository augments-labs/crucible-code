//! A request, in `MoonshotAI`'s shape: Chat Completions as [`super::Kimi`]
//! writes it.
//!
//! The writer is [`crate::completions::body`]. It is handed Kimi's dialect
//! here, and Kimi's tests of what a request carries stay beside it.

use crucible_models::Request;
#[cfg(test)]
use crucible_types::{Message, StopReason, ToolResult, ToolSchema};
#[cfg(test)]
use serde_json::{Value, json};

use super::Kimi;

/// The whole request body.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the provider writes through the shared wire")
)]
pub(super) fn serialize(request: &Request<'_>) -> String {
    crate::completions::body::serialize::<Kimi>(request)
}

#[cfg(test)]
fn build(request: &Request<'_>) -> Value {
    serde_json::from_str(&serialize(request)).expect("request body is JSON")
}

#[cfg(test)]
mod tests;
