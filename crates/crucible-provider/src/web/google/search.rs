//! Google search response parser with suggestions and citation evidence.
//!
//! Enforces that every grounded answer carries associated search suggestions
//! and verified source citations, with clean failure boundaries.

use crucible_core::{
    ContinuationPart, ProviderContinuation, SearchResponse, SearchResult, SourceError,
};
use serde_json::Value;

use super::problem;

/// Parses steps into a complete grounded search response.
pub(super) fn response(
    text: &str,
    state: &ProviderContinuation,
) -> Result<SearchResponse, SourceError> {
    let mut call: Option<String> = None;
    let mut answered = false;
    let mut suggestions: Option<String> = None;
    let mut found: Vec<SearchResult> = Vec::new();

    for part in state.parts() {
        match part {
            ContinuationPart::Opaque(data) => {
                let value: Value = serde_json::from_str(data.as_str())
                    .map_err(|_| problem("invalid native retrieval step"))?;
                match value.get("type").and_then(Value::as_str) {
                    Some("thought") => {}
                    Some("google_search_call") => {
                        if call.is_some() {
                            return Err(problem("multiple Google search calls in side response"));
                        }
                        let queries = value
                            .pointer("/arguments/queries")
                            .and_then(Value::as_array)
                            .ok_or_else(|| problem("Google search called without queries"))?;
                        if queries.is_empty() {
                            return Err(problem("Google search called without queries"));
                        }
                        call = Some(
                            value
                                .get("id")
                                .and_then(Value::as_str)
                                .ok_or_else(|| problem("missing Google search call ID"))?
                                .to_string(),
                        );
                    }
                    Some("google_search_result") => {
                        let Some(expected_id) = call.as_deref() else {
                            return Err(problem("Google search result arrived before call"));
                        };
                        if answered {
                            return Err(problem("multiple Google search results in side response"));
                        }
                        let call_id = value
                            .get("call_id")
                            .and_then(Value::as_str)
                            .ok_or_else(|| problem("missing Google search result call ID"))?;
                        if call_id != expected_id {
                            return Err(problem("mismatched Google search call and result"));
                        }
                        if value.get("is_error").and_then(Value::as_bool) == Some(true) {
                            return Err(problem("Google search returned an error"));
                        }
                        let results = value
                            .get("result")
                            .and_then(Value::as_array)
                            .ok_or_else(|| problem("invalid Google search result shape"))?;
                        if results.is_empty() {
                            return Err(problem("empty Google search result payload"));
                        }

                        // Search suggestions are required by Google grounding terms.
                        for item in results {
                            if let Some(raw_sugg) =
                                item.get("search_suggestions").and_then(Value::as_str)
                            {
                                let rendered = render_suggestions(raw_sugg);
                                if !rendered.is_empty() {
                                    suggestions = Some(rendered);
                                    break;
                                }
                            }
                        }

                        if suggestions.is_none() {
                            return Err(problem("missing Google search suggestions"));
                        }
                        answered = true;
                    }
                    None if value.pointer("/output/type").and_then(Value::as_str)
                        == Some("model_output") => {}
                    _ => {
                        return Err(problem("unexpected native tool in Google search response"));
                    }
                }
            }
            ContinuationPart::Text { start, end, data } => {
                let said = text
                    .get(*start..*end)
                    .ok_or_else(|| problem("invalid retrieved text range"))?;
                let value: Value = serde_json::from_str(data.as_str())
                    .map_err(|_| problem("invalid retrieval annotations"))?;
                if let Some(annotations) = value.get("annotations").and_then(Value::as_array) {
                    for citation in annotations {
                        if citation.get("type").and_then(Value::as_str) != Some("url_citation") {
                            continue;
                        }
                        let url = citation
                            .get("url")
                            .and_then(Value::as_str)
                            .ok_or_else(|| problem("Google search citation missing URL"))?;
                        if !url.starts_with("http://") && !url.starts_with("https://") {
                            return Err(problem("invalid Google search citation URL"));
                        }
                        let title = citation.get("title").and_then(Value::as_str).unwrap_or("");

                        let index = |key| {
                            citation
                                .get(key)
                                .and_then(Value::as_u64)
                                .and_then(|n| usize::try_from(n).ok())
                        };
                        let extract = match (index("start_index"), index("end_index")) {
                            (Some(s), Some(e)) if s < e => said
                                .get(s..e)
                                .or_else(|| text.get(s..e))
                                .unwrap_or(said)
                                .to_string(),
                            _ => said.to_string(),
                        };

                        found.push(SearchResult {
                            url: url.into(),
                            title: if title.is_empty() {
                                url.into()
                            } else {
                                title.into()
                            },
                            extract: extract.trim().into(),
                        });
                    }
                }
            }
            ContinuationPart::Call { .. } => {
                return Err(problem(
                    "unexpected function call in Google search response",
                ));
            }
        }
    }

    if call.is_none() || !answered {
        return Err(problem("Google search response carried no search result"));
    }
    let Some(suggestions) = suggestions else {
        return Err(problem("missing Google search suggestions"));
    };
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(problem("missing Google search grounded answer"));
    }
    if found.is_empty() {
        return Err(problem("Google search response carried no citations"));
    }

    Ok(SearchResponse::grounded(trimmed, found, suggestions))
}

/// Transforms Google Search Suggestions HTML into terminal-ready text/links
/// while preserving exact search URLs without redirects.
pub(super) fn render_suggestions(html: &str) -> String {
    let mut links = Vec::new();
    let mut remainder = html;

    // Extract any <a href="...">...</a> links
    while let Some(start_tag) = remainder.find("<a") {
        let tag_rest = &remainder[start_tag..];
        let Some(tag_end) = tag_rest.find('>') else {
            break;
        };
        let tag_content = &tag_rest[..tag_end];
        let after_tag = &tag_rest[tag_end + 1..];
        let Some(close_tag) = after_tag.find("</a>") else {
            break;
        };
        let link_text = &after_tag[..close_tag];

        if let Some(href_start) = tag_content.find("href=\"") {
            let href_rest = &tag_content[href_start + 6..];
            if let Some(href_end) = href_rest.find('"') {
                let href = &href_rest[..href_end];
                let clean_text = strip_tags(link_text);
                if !clean_text.is_empty() && !href.is_empty() {
                    links.push(format!("- [{clean_text}]({href})"));
                }
            }
        }
        remainder = &after_tag[close_tag + 4..];
    }

    if !links.is_empty() {
        return links.join("\n");
    }

    // Fall back to stripped text if no anchor tags
    strip_tags(html)
}

fn strip_tags(input: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in input.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    // Unescape common HTML entities
    let unescaped = out
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'");
    unescaped.split_whitespace().collect::<Vec<_>>().join(" ")
}
