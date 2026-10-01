//! A response, from OpenAI's shape: Responses as [`super::Gpt`] reads it.
//!
//! The reader is [`crate::responses::wire`], and nothing of it is OpenAI's but
//! the dialect. What is here is what OpenAI's replay reads it through, and the
//! name OpenAI's tests read one event with, which stay where they were.

use crucible_models::{Delta, ProviderError};
use serde_json::Value;

#[cfg(test)]
use crate::responses::wire::Narration;
#[cfg(test)]
pub(super) use crate::responses::wire::deltas;
use crate::responses::wire::usage as counted;
#[cfg(test)]
use crate::sse::SseEvent;
#[cfg(test)]
use crucible_types::{StopReason, ToolId};

pub(super) use crate::responses::wire::cut;

/// The Responses API, being narrated, as OpenAI narrates it.
#[cfg(test)]
pub(super) type Responses = Narration<super::Gpt>;

/// Normalizes the inclusive Responses usage object once at the wire boundary.
pub(super) fn usage(
    payload: &Value,
    cache_write_reporting: bool,
) -> Result<Option<Delta>, ProviderError> {
    counted::<super::Gpt>(payload, cache_write_reporting)
}

#[cfg(test)]
mod tests;
