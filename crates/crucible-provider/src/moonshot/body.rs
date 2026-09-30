//! A request, in `MoonshotAI`'s shape: Chat Completions as [`super::Kimi`]
//! writes it.
//!
//! The writer is [`crate::completions::body`]. What is here is for Kimi's tests
//! of what a request carries, which stay where they were and reach the shared
//! writer through Kimi's dialect.

#[cfg(test)]
use crucible_models::Request;
#[cfg(test)]
use crucible_types::{Message, StopReason, ToolResult, ToolSchema};
#[cfg(test)]
use serde_json::{Value, json};

#[cfg(test)]
use super::Kimi;

/// The whole request body.
#[cfg(test)]
pub(super) fn serialize(request: &Request<'_>) -> String {
    crate::completions::body::serialize::<Kimi>(request)
}

#[cfg(test)]
fn build(request: &Request<'_>) -> Value {
    serde_json::from_str(&serialize(request)).expect("request body is JSON")
}

#[cfg(test)]
mod tests;
