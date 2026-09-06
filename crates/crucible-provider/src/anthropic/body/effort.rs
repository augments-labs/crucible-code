//! Reconstruct append-only effort changes from request facts beside each answer.
//!
//! The first request's top-level control stays fixed. Later controls precede
//! their user/tool-result group, preserving the prefix across tool passes and
//! save/resume without adding another transcript or global provider state.

use super::replay::compatible;
use crate::anthropic::continuation::problem;
use crate::json::Array;
use crucible_core::{
    ContinuationPart, ContinuationScope, Effort, Message, ProviderContinuation, ProviderError,
    Request,
};
use serde_json::Value;

pub(super) fn recorded(state: &ProviderContinuation) -> Result<Option<Effort>, ProviderError> {
    let Some(ContinuationPart::Opaque(data)) = state.parts().first() else {
        return Err(problem("missing request effort record"));
    };
    let value: Value = serde_json::from_str(data.as_str())
        .map_err(|_| problem("invalid request effort record"))?;
    let fields = value
        .as_object()
        .filter(|fields| fields.len() == 1)
        .ok_or_else(|| problem("invalid request effort record"))?;
    match fields.get("request_effort") {
        Some(Value::Null) => Ok(None),
        Some(Value::String(effort)) => match effort.as_str() {
            "low" => Ok(Some(Effort::Low)),
            "medium" => Ok(Some(Effort::Medium)),
            "high" => Ok(Some(Effort::High)),
            "xhigh" => Ok(Some(Effort::Xhigh)),
            "max" => Ok(Some(Effort::Max)),
            _ => Err(problem("invalid request effort record")),
        },
        _ => Err(problem("invalid request effort record")),
    }
}

pub(super) struct Efforts<'a> {
    remaining: std::iter::Enumerate<std::slice::Iter<'a, Message>>,
    scope: ContinuationScope,
    next: Option<(usize, Option<Effort>)>,
    initial: Option<Effort>,
    active: Effort,
    requested: Option<Effort>,
}

impl<'a> Efforts<'a> {
    pub(super) fn new(
        request: &Request<'a>,
        scope: ContinuationScope,
    ) -> Result<Self, ProviderError> {
        let mut this = Self {
            remaining: request.transcript.messages().iter().enumerate(),
            scope,
            next: None,
            initial: None,
            active: Effort::High,
            requested: request.effort,
        };
        this.advance()?;
        this.initial = this.next.map_or(request.effort, |(_, effort)| effort);
        this.active = this.initial.unwrap_or(Effort::High);
        Ok(this)
    }

    pub(super) const fn initial(&self) -> Option<Effort> {
        self.initial
    }

    fn advance(&mut self) -> Result<(), ProviderError> {
        self.next = None;
        for (index, message) in self.remaining.by_ref() {
            if let Message::Agent {
                continuation: Some(state),
                ..
            } = message
                && compatible(state, self.scope)
            {
                self.next = Some((index, recorded(state)?));
                break;
            }
        }
        Ok(())
    }

    pub(super) fn before(
        &mut self,
        messages: &mut Array<'_>,
        index: usize,
        message: &Message,
    ) -> Result<(), ProviderError> {
        if self.next.is_some_and(|(at, _)| at == index) {
            return self.advance();
        }
        if matches!(message, Message::Agent { .. }) {
            return Ok(());
        }
        // None stays absent initially. After an explicit override, restoring
        // the documented Fable default needs a high control: omission would
        // incorrectly inherit the last override instead of restoring default.
        let effort = self
            .next
            .map_or(self.requested, |(_, effort)| effort)
            .unwrap_or(Effort::High);
        if effort != self.active {
            messages.object(|message| {
                message.text("role", "system");
                message.array("content", |_| {});
                message.object("output_config", |config| {
                    config.text("effort", effort.as_str());
                });
            });
            self.active = effort;
        }
        Ok(())
    }
}
