//! What a response cost, read the way most vendors on this wire report it.
//!
//! The prompt's count includes the part read from the cache, which is given
//! beside it, and the reasoning is a part of the output. A vendor that counts
//! another way reads its own counts in its dialect instead.

use crucible_models::{Delta, ProviderError};
use crucible_types::{InputTokenUsage, ProviderNumericDetail, ProviderUsage, UsageError};
use serde_json::Value;

/// The counts `payload` carries, if any, for `provider`.
///
/// The cached part is read from `cached` at the top of the counts where the
/// vendor names a field of its own for it, and from the standard
/// `prompt_tokens_details.cached_tokens` otherwise.
///
/// # Errors
///
/// [`ProviderError::Protocol`] where the counts contradict each other.
pub(crate) fn inclusive(
    payload: &Value,
    provider: &'static str,
    cached: Option<&str>,
) -> Result<Option<Delta>, ProviderError> {
    let Some(usage) = payload.get("usage").filter(|usage| usage.is_object()) else {
        return Ok(None);
    };
    let input_total = number(usage, "prompt_tokens");
    let cache_read = cached.and_then(|field| number(usage, field)).or_else(|| {
        usage
            .get("prompt_tokens_details")
            .and_then(|details| number(details, "cached_tokens"))
    });
    let output = number(usage, "completion_tokens");
    let reported_total = number(usage, "total_tokens");
    let reasoning = usage
        .get("completion_tokens_details")
        .and_then(|details| number(details, "reasoning_tokens"));
    if input_total.is_none() && output.is_none() && reported_total.is_none() {
        return Ok(None);
    }
    let problem = |problem: UsageError| ProviderError::Protocol {
        provider,
        problem: format!("invalid usage accounting: {problem}").into(),
    };
    let mut details = Vec::new();
    for (label, value) in [
        ("cached_tokens", cache_read),
        ("reasoning_tokens", reasoning),
    ] {
        if let Some(value) = value {
            details.push(ProviderNumericDetail::new(label, value).map_err(problem)?);
        }
    }
    let input = InputTokenUsage::inclusive_read(input_total, cache_read).map_err(problem)?;
    let usage =
        ProviderUsage::new(input, output, reasoning, reported_total, &details).map_err(problem)?;
    Ok(Some(Delta::Usage(usage)))
}

fn number(value: &Value, field: &str) -> Option<u64> {
    value.get(field).and_then(Value::as_u64)
}
