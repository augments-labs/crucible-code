//! What a Kimi response cost, which is where Kimi's events differ from the
//! rest of Chat Completions.
//!
//! Everything else in an event is read by [`crate::completions::wire`].

use crucible_models::{Delta, ProviderError};
use crucible_types::{InputTokenUsage, ProviderNumericDetail, ProviderUsage, UsageError};
use serde_json::Value;

use crate::moonshot::NAME;

/// Normalizes Kimi's inclusive prompt total and cached-token subset.
pub(super) fn usage(payload: &Value) -> Result<Option<Delta>, ProviderError> {
    let Some(usage) = payload.get("usage") else {
        return Ok(None);
    };
    let input_total = number(usage, "prompt_tokens");
    let cache_read = number(usage, "cached_tokens");
    let output = number(usage, "completion_tokens");
    let reported_total = number(usage, "total_tokens");
    let reasoning = usage
        .get("completion_tokens_details")
        .and_then(|details| number(details, "reasoning_tokens"));
    if input_total.is_none()
        && cache_read.is_none()
        && output.is_none()
        && reported_total.is_none()
        && reasoning.is_none()
    {
        return Ok(None);
    }
    let mut details = Vec::new();
    for (label, value) in [
        ("cached_tokens", cache_read),
        ("reasoning_tokens", reasoning),
    ] {
        if let Some(value) = value {
            details.push(ProviderNumericDetail::new(label, value).map_err(usage_problem)?);
        }
    }
    let input = InputTokenUsage::inclusive_read(input_total, cache_read).map_err(usage_problem)?;
    let usage = ProviderUsage::new(input, output, reasoning, reported_total, &details)
        .map_err(usage_problem)?;
    Ok(Some(Delta::Usage(usage)))
}

fn number(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}

fn usage_problem(problem: UsageError) -> ProviderError {
    ProviderError::Protocol {
        provider: NAME,
        problem: format!("invalid usage accounting: {problem}").into(),
    }
}
