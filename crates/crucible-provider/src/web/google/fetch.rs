//! Retrieval evidence for a URL-context-only side request.
//!
//! An answer about a URL is not proof it was retrieved. Require the native
//! call, its successful result, and a valid citation to that exact URL. Search
//! steps or another destination are refused, never normalized into a page.

use crucible_core::{ContinuationPart, Page, ProviderContinuation, SourceError};
use serde_json::Value;

use super::problem;

pub(super) fn page(
    asked: &str,
    text: String,
    state: &ProviderContinuation,
) -> Result<Page, SourceError> {
    let mut call = None;
    let mut retrieved = false;
    let mut cited = false;
    let mut title = None;
    for part in state.parts() {
        match part {
            ContinuationPart::Opaque(data) => {
                let value: Value = serde_json::from_str(data.as_str())
                    .map_err(|_| problem("invalid native retrieval step"))?;
                match value.get("type").and_then(Value::as_str) {
                    Some("thought") => {}
                    Some("url_context_call") if call.is_none() => {
                        let urls = value
                            .pointer("/arguments/urls")
                            .and_then(Value::as_array)
                            .ok_or_else(|| problem("URL context called without URLs"))?;
                        if urls.len() != 1 || urls.first().and_then(Value::as_str) != Some(asked) {
                            return Err(problem(
                                "URL context called for an unexpected destination",
                            ));
                        }
                        call = Some(
                            value
                                .get("id")
                                .and_then(Value::as_str)
                                .filter(|id| !id.is_empty())
                                .ok_or_else(|| problem("URL context call has no identity"))?
                                .to_owned(),
                        );
                    }
                    Some("url_context_result") if !retrieved => {
                        if !matches!(value.get("is_error"), None | Some(Value::Bool(false))) {
                            return Err(problem("Google reported a URL context error"));
                        }
                        if call.as_deref().is_none()
                            || value.get("call_id").and_then(Value::as_str) != call.as_deref()
                        {
                            return Err(problem("URL context result has no matching call"));
                        }
                        let results = value
                            .get("result")
                            .and_then(Value::as_array)
                            .ok_or_else(|| problem("URL context returned no retrieval result"))?;
                        if results.len() != 1
                            || !results.first().is_some_and(|result| {
                                result.get("url").and_then(Value::as_str) == Some(asked)
                                    && result.get("status").and_then(Value::as_str)
                                        == Some("success")
                            })
                        {
                            return Err(problem("Google did not retrieve the requested URL"));
                        }
                        retrieved = true;
                    }
                    None if value.pointer("/output/type").and_then(Value::as_str)
                        == Some("model_output") => {}
                    _ => {
                        return Err(problem(
                            "unexpected native tool in URL-context-only response",
                        ));
                    }
                }
            }
            ContinuationPart::Text { start, end, data } => {
                let said = text
                    .get(*start..*end)
                    .ok_or_else(|| problem("invalid retrieved text range"))?;
                let value: Value = serde_json::from_str(data.as_str())
                    .map_err(|_| problem("invalid retrieval annotations"))?;
                if let Some(annotations) = value.get("annotations") {
                    for citation in annotations
                        .as_array()
                        .ok_or_else(|| problem("invalid retrieval annotations"))?
                    {
                        if citation.get("type").and_then(Value::as_str) != Some("url_citation") {
                            continue;
                        }
                        if citation.get("url").and_then(Value::as_str) != Some(asked) {
                            return Err(problem("URL context cited an unexpected destination"));
                        }
                        let index = |key| {
                            citation
                                .get(key)
                                .and_then(Value::as_u64)
                                .and_then(|n| usize::try_from(n).ok())
                        };
                        match (index("start_index"), index("end_index")) {
                            (None, None)
                                if citation.get("start_index").is_none()
                                    && citation.get("end_index").is_none() => {}
                            (Some(start), Some(end))
                                if start < end && said.get(start..end).is_some() => {}
                            _ => {
                                return Err(problem(
                                    "URL context returned an invalid citation range",
                                ));
                            }
                        }
                        cited = true;
                        if title.is_none() {
                            title = citation
                                .get("title")
                                .and_then(Value::as_str)
                                .map(Into::into);
                        }
                    }
                }
            }
            ContinuationPart::Call { .. } => {
                return Err(problem("unexpected function call in URL context"));
            }
        }
    }
    if !retrieved || !cited || text.is_empty() {
        return Err(problem("Google did not return a cited retrieved page"));
    }
    Ok(Page {
        url: asked.into(),
        title,
        text: text.into(),
    })
}
