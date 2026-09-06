//! Cited sources from a completed native Google Search side answer.
//!
//! Citations live on model text, not necessarily in the native result array:
//! that array can contain only Search Suggestions. The shared wire parser
//! verifies native call/result identities before handing over continuation.
//! Thought, signatures and suggestions are never mistaken for source extracts.

use crucible_core::{ContinuationPart, Host, ProviderContinuation, SearchResult, SourceError};
use serde_json::Value;
use std::collections::BTreeSet;

use super::{host_of, problem};

pub(super) fn results(
    text: &str,
    state: &ProviderContinuation,
) -> Result<Vec<SearchResult>, SourceError> {
    let mut searched = false;
    let mut found = Vec::new();
    let mut seen = BTreeSet::new();
    let mut bytes = 0_usize;
    for part in state.parts() {
        match part {
            ContinuationPart::Opaque(data) => {
                let value: Value = serde_json::from_str(data.as_str())
                    .map_err(|_| problem("invalid native search step"))?;
                match value.get("type").and_then(Value::as_str) {
                    Some("thought" | "google_search_call") => {}
                    Some("google_search_result") => {
                        if !matches!(value.get("is_error"), None | Some(Value::Bool(false))) {
                            return Err(problem("Google reported a native search error"));
                        }
                        if !value.get("result").is_some_and(Value::is_array) {
                            return Err(problem("Google search returned an invalid result"));
                        }
                        searched = true;
                    }
                    None if value.pointer("/output/type").and_then(Value::as_str)
                        == Some("model_output") => {}
                    _ => return Err(problem("unexpected native tool in search-only response")),
                }
            }
            ContinuationPart::Text { start, end, data } => {
                let said = text
                    .get(*start..*end)
                    .ok_or_else(|| problem("invalid search text range"))?;
                let value: Value = serde_json::from_str(data.as_str())
                    .map_err(|_| problem("invalid search annotations"))?;
                let Some(annotations) = value.get("annotations") else {
                    continue;
                };
                for citation in annotations
                    .as_array()
                    .ok_or_else(|| problem("invalid search annotations"))?
                {
                    if citation.get("type").and_then(Value::as_str) != Some("url_citation") {
                        continue;
                    }
                    let Some(url) = citation.get("url").and_then(Value::as_str) else {
                        return Err(problem("search citation has no URL"));
                    };
                    if !matches!(host_of(url), Host::Named { .. }) {
                        continue;
                    }
                    let extract = extract(said, citation)?;
                    if seen.contains(url) {
                        continue;
                    }
                    let title = citation.get("title").and_then(Value::as_str).unwrap_or(url);
                    // Overlapping citations can repeat a large text span many
                    // times. Bound the projection before copying those spans.
                    bytes = bytes
                        .saturating_add(url.len())
                        .saturating_add(title.len())
                        .saturating_add(extract.len());
                    if bytes > super::super::MOST {
                        return Err(problem("Google search results exceeded their byte limit"));
                    }
                    seen.insert(url.to_owned());
                    found.push(SearchResult {
                        url: url.into(),
                        title: title.into(),
                        extract: extract.into(),
                    });
                }
            }
            ContinuationPart::Call { .. } => {
                return Err(problem("unexpected function call in native search"));
            }
        }
    }
    if !searched {
        return Err(problem("Google search returned no native results"));
    }
    Ok(found)
}

fn extract<'a>(text: &'a str, citation: &Value) -> Result<&'a str, SourceError> {
    let index = |key| {
        citation
            .get(key)
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
    };
    match (index("start_index"), index("end_index")) {
        (None, None)
            if citation.get("start_index").is_none() && citation.get("end_index").is_none() =>
        {
            Ok("")
        }
        (Some(start), Some(end)) if start < end => text
            .get(start..end)
            .ok_or_else(|| problem("Google search returned an invalid citation range")),
        _ => Err(problem("Google search returned an invalid citation range")),
    }
}
