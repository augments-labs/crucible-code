//! Interactions output and thoughts are disjoint. Its raw total can additionally
//! include tool/internal prompts, so retain that total only as a numeric detail.

use super::protocol;
use crucible_core::{Delta, InputTokenUsage, ProviderError, ProviderNumericDetail, ProviderUsage};
use serde_json::Value;

pub(super) fn reported(payload: &Value) -> Result<Option<Delta>, ProviderError> {
    let Some(usage) = payload.pointer("/interaction/usage") else {
        return Ok(None);
    };
    if !usage.is_object() {
        return Err(protocol("invalid interaction usage"));
    }
    let input = count(usage, "total_input_tokens")?;
    let cached = count(usage, "total_cached_tokens")?;
    let visible = count(usage, "total_output_tokens")?;
    let thoughts = count(usage, "total_thought_tokens")?;
    let output = match (visible, thoughts) {
        (Some(a), Some(b)) => Some(
            a.checked_add(b)
                .ok_or_else(|| protocol("interaction usage overflow"))?,
        ),
        _ => None,
    };
    let mut details = Vec::new();
    for label in [
        "total_input_tokens",
        "total_cached_tokens",
        "total_output_tokens",
        "total_thought_tokens",
        "total_tool_use_tokens",
        "total_tokens",
    ] {
        if let Some(value) = count(usage, label)? {
            details.push(
                ProviderNumericDetail::new(label, value)
                    .map_err(|_| protocol("invalid interaction usage detail"))?,
            );
        }
    }
    let input = InputTokenUsage::inclusive_read(input, cached)
        .map_err(|_| protocol("invalid interaction cached usage"))?;
    ProviderUsage::new(input, output, thoughts, None, &details)
        .map(|u| Some(Delta::Usage(u)))
        .map_err(|_| protocol("invalid interaction token usage"))
}

fn count(value: &Value, key: &str) -> Result<Option<u64>, ProviderError> {
    value
        .get(key)
        .map(|v| {
            v.as_u64()
                .ok_or_else(|| protocol("invalid interaction token count"))
        })
        .transpose()
}
