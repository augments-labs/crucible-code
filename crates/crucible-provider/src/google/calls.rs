//! One response's call identities and server-side result obligations.
//!
//! Function calls await local execution after the response. Native calls must
//! instead have exactly one matching result before successful completion. Keep
//! returned identities to reject reuse; charge each owned key before insertion.

use super::protocol;
use crucible_core::{CONTINUATION_BYTES, CONTINUATION_PARTS, ProviderError, TOOL_CALL_ID_BYTES};
use serde_json::{Map, Value};
use std::collections::BTreeMap;

enum Waiting {
    Function,
    Native(&'static str),
    Returned,
}

#[derive(Default)]
pub(super) struct Calls {
    seen: BTreeMap<Box<str>, Waiting>,
    functions: usize,
}

impl Calls {
    pub(super) fn function(&mut self, id: &str, bytes: &mut usize) -> Result<(), ProviderError> {
        if self.functions == 128 {
            return Err(protocol("too many function calls"));
        }
        self.insert(id, Waiting::Function, bytes)?;
        self.functions += 1;
        Ok(())
    }

    pub(super) fn native(
        &mut self,
        fields: &Map<String, Value>,
        bytes: &mut usize,
    ) -> Result<(), ProviderError> {
        let kind = fields.get("type").and_then(Value::as_str).unwrap_or("");
        let expected = match kind {
            "google_search_call" => Some("google_search_result"),
            "url_context_call" => Some("url_context_result"),
            "code_execution_call" => Some("code_execution_result"),
            "processing_call" => Some("processing_result"),
            _ => None,
        };
        if let Some(expected) = expected {
            let id = fields
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| protocol("native call has no identity"))?;
            return self.insert(id, Waiting::Native(expected), bytes);
        }
        if matches!(
            kind,
            "google_search_result"
                | "url_context_result"
                | "code_execution_result"
                | "processing_result"
        ) {
            let id = fields
                .get("call_id")
                .and_then(Value::as_str)
                .ok_or_else(|| protocol("native result has no call identity"))?;
            let waiting = self
                .seen
                .get_mut(id)
                .ok_or_else(|| protocol("native result has no preceding call"))?;
            if !matches!(waiting, Waiting::Native(expected) if *expected == kind) {
                return Err(protocol("native result does not match an unanswered call"));
            }
            *waiting = Waiting::Returned;
        }
        Ok(())
    }

    pub(super) fn completed(&self) -> Result<(), ProviderError> {
        if self
            .seen
            .values()
            .any(|waiting| matches!(waiting, Waiting::Native(_)))
        {
            return Err(protocol("interaction has unanswered native calls"));
        }
        Ok(())
    }

    fn insert(
        &mut self,
        id: &str,
        waiting: Waiting,
        bytes: &mut usize,
    ) -> Result<(), ProviderError> {
        if id.is_empty()
            || id.len() > TOOL_CALL_ID_BYTES
            || id.chars().any(char::is_control)
            || self.seen.len() >= CONTINUATION_PARTS
            || self.seen.contains_key(id)
        {
            return Err(protocol("invalid, duplicate or excessive call identities"));
        }
        let next = bytes.saturating_add(id.len()).saturating_add(64);
        if next > CONTINUATION_BYTES {
            return Err(protocol(
                "interaction call identities exceed their resource limit",
            ));
        }
        *bytes = next;
        self.seen.insert(id.into(), waiting);
        Ok(())
    }
}
