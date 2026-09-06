//! Borrowed native assistant blocks; incompatible history stays descriptive.
//!
//! A prefix rewrite removes stale thinking only from responses preceding that
//! rewrite. It never discards subsequently produced thinking or local tool pairs.

use crate::anthropic::continuation::{PROTOCOL, identity, private, problem};
use crate::json::{Array, Object};
use crucible_core::{
    ContinuationPart, ContinuationScope, PromptCacheRetentionClass, ProviderContinuation,
    ProviderError, TOOL_CALL_ID_BYTES, TOOL_NAME_BYTES, ToolCall,
};
use serde_json::{Map, Value};
use std::collections::BTreeSet;

pub(super) fn compatible(state: &ProviderContinuation, scope: ContinuationScope) -> bool {
    // This protocol is currently produced only by Fable 5.1. A future Claude
    // name is not evidence that this older model can read its signed blocks.
    state.protocol() == PROTOCOL
        && state.scope() == scope
        && state.model() == crate::anthropic::FABLE_51
}

pub(super) fn answered(pending: Option<&BTreeSet<&str>>) -> Result<(), ProviderError> {
    if pending.is_some_and(|waiting| !waiting.is_empty()) {
        Err(problem("history has unanswered tool calls"))
    } else {
        Ok(())
    }
}

/// One borrowed assistant response and the native references it owns.
pub(super) struct Agent<'a> {
    pub(super) state: &'a ProviderContinuation,
    pub(super) text: &'a str,
    pub(super) calls: &'a [ToolCall],
}

impl Agent<'_> {
    pub(super) fn write(
        &self,
        messages: &mut Array<'_>,
        rewritten: bool,
        retention: Option<PromptCacheRetentionClass>,
    ) -> Result<(), ProviderError> {
        let Self { state, text, calls } = *self;
        state
            .validate(text, calls.len())
            .map_err(|_| problem("invalid continuation references"))?;
        super::effort::recorded(state)?;
        if calls.len() > 128 {
            return Err(problem("too many replay calls"));
        }
        let mut seen = BTreeSet::new();
        for call in calls {
            identity(call.id.as_str(), TOOL_CALL_ID_BYTES)?;
            identity(&call.name, TOOL_NAME_BYTES)?;
            if !seen.insert(call.id.as_str()) {
                return Err(problem("duplicate replay call id"));
            }
        }
        // A thinking-only assistant becomes empty when its stale thinking is
        // removed. Omit that empty message, not an invented assistant prefill.
        let has_thinking = state
            .parts()
            .iter()
            .skip(1)
            .any(|part| matches!(part, ContinuationPart::Opaque(_)));
        if text.is_empty() && calls.is_empty() && (rewritten || !has_thinking) {
            return Ok(());
        }
        let last_cacheable = state.parts().iter().rposition(|part| match part {
            ContinuationPart::Text { start, end, .. } => start < end,
            ContinuationPart::Call { .. } => true,
            ContinuationPart::Opaque(_) => false,
        });
        let mut outcome = Ok(());
        messages.object(|message| {
            message.text("role", "assistant");
            message.array("content", |content| {
                for (index, part) in state.parts().iter().enumerate().skip(1) {
                    if outcome.is_err() {
                        break;
                    }
                    outcome = self.block(
                        content,
                        part,
                        rewritten,
                        retention.filter(|_| last_cacheable == Some(index)),
                    );
                }
            });
        });
        outcome
    }

    fn block(
        &self,
        content: &mut Array<'_>,
        part: &ContinuationPart,
        rewritten: bool,
        retention: Option<PromptCacheRetentionClass>,
    ) -> Result<(), ProviderError> {
        match part {
            ContinuationPart::Opaque(data) => {
                let fields = parse(data.as_str())?;
                private(&fields)?;
                if !rewritten {
                    content.object(|block| attributes(block, &fields));
                }
            }
            ContinuationPart::Text { start, end, data } => {
                let fields = parse(data.as_str())?;
                if fields.get("type").and_then(Value::as_str) != Some("text")
                    || fields.contains_key("text")
                {
                    return Err(problem("invalid replay text attributes"));
                }
                let text = self
                    .text
                    .get(*start..*end)
                    .ok_or_else(|| problem("invalid replay text range"))?;
                if text.is_empty() {
                    return Ok(());
                }
                content.object(|block| {
                    attributes(block, &fields);
                    block.text("text", text);
                    if let Some(retention) = retention {
                        super::write_cache_control(block, retention);
                    }
                });
            }
            ContinuationPart::Call { index, data } => {
                let fields = parse(data.as_str())?;
                if fields.get("type").and_then(Value::as_str) != Some("tool_use")
                    || ["id", "name", "input"]
                        .iter()
                        .any(|key| fields.contains_key(*key))
                {
                    return Err(problem("invalid replay tool attributes"));
                }
                let call = self
                    .calls
                    .get(*index)
                    .ok_or_else(|| problem("invalid replay call reference"))?;
                let input: Value = serde_json::from_str(call.args.as_str())
                    .map_err(|_| problem("invalid replay tool input"))?;
                if !input.is_object() {
                    return Err(problem("invalid replay tool input"));
                }
                content.object(|block| {
                    attributes(block, &fields);
                    block.text("id", call.id.as_str());
                    block.text("name", &call.name);
                    block.value("input", &input);
                    if let Some(retention) = retention {
                        super::write_cache_control(block, retention);
                    }
                });
            }
        }
        Ok(())
    }
}

fn parse(data: &str) -> Result<Map<String, Value>, ProviderError> {
    match serde_json::from_str(data) {
        Ok(Value::Object(fields)) => Ok(fields),
        _ => Err(problem("invalid private block JSON")),
    }
}

fn attributes(block: &mut Object<'_>, fields: &Map<String, Value>) {
    for (key, value) in fields {
        block.value(key, value);
    }
}
