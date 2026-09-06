//! Closed native-step validation shared by streaming finalization and replay.
//!
//! Required identities and known field types are checked before accepting usable
//! state. Unknown attributes remain intact within the continuation budget: they
//! may be signed vendor metadata and are never interpreted as local authority.

use super::protocol;
use crucible_core::{ProviderError, TOOL_CALL_ID_BYTES};
use serde_json::{Map, Value};

pub(super) fn native(fields: &Map<String, Value>) -> Result<(), ProviderError> {
    let valid = strings(fields, &["signature"])
        && optional(fields, "is_error", Value::is_boolean)
        && match fields.get("type").and_then(Value::as_str) {
            Some("thought") => optional(fields, "summary", |value| {
                value
                    .as_array()
                    .is_some_and(|items| items.iter().all(summary))
            }),
            Some("google_search_call") => {
                identity(fields, "id")
                    && strings(fields, &["search_type"])
                    && arguments(fields, |args| string_array(args, "queries"))
            }
            Some("url_context_call") => {
                identity(fields, "id") && arguments(fields, |args| string_array(args, "urls"))
            }
            Some("code_execution_call") => {
                identity(fields, "id")
                    && arguments(fields, |args| strings(args, &["code", "language"]))
            }
            Some("google_search_result") => {
                identity(fields, "call_id")
                    && results(fields, |item| {
                        strings(item, &["search_suggestions", "url", "title", "snippet"])
                    })
            }
            Some("url_context_result") => {
                identity(fields, "call_id")
                    && results(fields, |item| strings(item, &["url", "status"]))
            }
            Some("code_execution_result") => {
                identity(fields, "call_id") && fields.get("result").is_some_and(Value::is_string)
            }
            Some("processing_call") => identity(fields, "id"),
            Some("processing_result") => identity(fields, "call_id"),
            _ => false,
        };
    if valid {
        Ok(())
    } else {
        Err(protocol("invalid native interaction step"))
    }
}

fn identity(fields: &Map<String, Value>, key: &str) -> bool {
    fields.get(key).and_then(Value::as_str).is_some_and(|id| {
        !id.is_empty() && id.len() <= TOOL_CALL_ID_BYTES && !id.chars().any(char::is_control)
    })
}

fn optional(fields: &Map<String, Value>, key: &str, check: impl FnOnce(&Value) -> bool) -> bool {
    fields.get(key).is_none_or(check)
}

fn strings(fields: &Map<String, Value>, keys: &[&str]) -> bool {
    keys.iter()
        .all(|key| optional(fields, key, Value::is_string))
}

fn string_array(fields: &Map<String, Value>, key: &str) -> bool {
    optional(fields, key, |value| {
        value
            .as_array()
            .is_some_and(|items| items.iter().all(Value::is_string))
    })
}

fn arguments(fields: &Map<String, Value>, check: impl FnOnce(&Map<String, Value>) -> bool) -> bool {
    fields
        .get("arguments")
        .and_then(Value::as_object)
        .is_some_and(check)
}

fn results(fields: &Map<String, Value>, check: impl Fn(&Map<String, Value>) -> bool) -> bool {
    fields
        .get("result")
        .and_then(Value::as_array)
        .is_some_and(|items| {
            items
                .iter()
                .all(|item| item.as_object().is_some_and(&check))
        })
}

fn summary(value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    match fields.get("type").and_then(Value::as_str) {
        Some("text") => fields.get("text").is_some_and(Value::is_string),
        Some("image") => strings(fields, &["data", "uri", "mime_type", "resolution"]),
        _ => false,
    }
}
