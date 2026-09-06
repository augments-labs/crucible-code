//! Safe facts from a response that can contain signed, private history.
//!
//! Refusal prose and transformation paths are never diagnostics. Static error
//! categories preserve retry guidance; fixed numeric details explain dropped
//! thinking without copying any private payload. Reports replace previous levels,
//! including a server-side fallback's final transformation counts.

use super::continuation::problem;
use crucible_core::{Delta, InputTokenUsage, ProviderError, ProviderNumericDetail, ProviderUsage};
use serde_json::Value;

pub(super) fn refusal(error: ProviderError) -> ProviderError {
    match error {
        ProviderError::Refused { status, .. } => ProviderError::Refused {
            provider: super::NAME,
            status,
            message: match status {
                401 | 403 => "check the Anthropic API key and its model access",
                404 => "check the Anthropic model name and endpoint",
                408 | 429 | 500..=599 => "Anthropic is temporarily unable to serve this request",
                _ => "check the Anthropic model, request settings and workspace retention; private response details omitted",
            }.into(),
        },
        // Cancellation and the typed context-window failure carry no prose.
        error => error,
    }
}

pub(super) fn upstream(value: &Value) -> ProviderError {
    let kind = match value.pointer("/error/type").and_then(Value::as_str) {
        Some("overloaded_error") => "overloaded_error",
        Some("api_error") => "api_error",
        Some("timeout_error") => "timeout_error",
        Some("rate_limit_error") => "rate_limit_error",
        _ => return problem("Anthropic reported a message failure; private details omitted"),
    };
    ProviderError::Upstream {
        provider: super::NAME,
        kind: kind.into(),
        message: "Anthropic could not finish this request; private details omitted".into(),
    }
}

pub(super) fn transformations(value: &Value) -> Result<Option<Delta>, ProviderError> {
    let Some(entries) = value
        .get("input_transformations")
        .filter(|value| !value.is_null())
    else {
        return Ok(None);
    };
    let entries = entries
        .as_array()
        .ok_or_else(|| problem("invalid input transformations"))?;
    // The SSE reader bounds the input before parsing. Count in place rather
    // than retain a second list, and ignore additive vendor check categories.
    let mut prefix = 0;
    let mut model = 0;
    for entry in entries {
        if entry.get("type").and_then(Value::as_str) == Some("thinking_dropped") {
            match entry.get("reason").and_then(Value::as_str) {
                Some("prefix_binding_mismatch") => prefix += 1,
                Some("model_binding_mismatch") => model += 1,
                _ => {}
            }
        }
    }
    let details = [
        ProviderNumericDetail {
            label: "thinking_dropped_prefix",
            value: prefix,
        },
        ProviderNumericDetail {
            label: "thinking_dropped_model",
            value: model,
        },
    ];
    ProviderUsage::new(InputTokenUsage::UNKNOWN, None, None, None, &details)
        .map(Delta::Usage)
        .map(Some)
        .map_err(|_| problem("invalid transformation counts"))
}

pub(super) fn completion(value: &Value) -> Result<Vec<Delta>, ProviderError> {
    let mut deltas = super::wire::ended(value)?;
    let Some(details) = value
        .pointer("/usage/output_tokens_details")
        .filter(|value| !value.is_null())
    else {
        return Ok(deltas);
    };
    let thinking = details
        .get("thinking_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| problem("invalid thinking token count"))?;
    let output = value
        .pointer("/usage/output_tokens")
        .and_then(Value::as_u64)
        .ok_or_else(|| problem("missing inclusive output token count"))?;
    // Thinking is already billed in output; never add it again or estimate it
    // from the summarized text the API returned.
    let report = ProviderUsage::new(
        InputTokenUsage::UNKNOWN,
        Some(output),
        Some(thinking),
        None,
        &[
            ProviderNumericDetail {
                label: "output_tokens",
                value: output,
            },
            ProviderNumericDetail {
                label: "thinking_tokens",
                value: thinking,
            },
        ],
    )
    .map_err(|_| problem("invalid thinking usage accounting"))?;
    deltas.retain(|delta| !matches!(delta, Delta::Usage(_)));
    deltas.insert(0, Delta::Usage(report));
    Ok(deltas)
}
