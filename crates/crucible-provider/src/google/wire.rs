//! Indexed Interactions events. Private steps never become visible deltas.
//!
//! A later step can arrive before an earlier one closes. Delivery stays in step
//! order, and replay state is offered only after every step closes cleanly.

pub(super) mod step;
mod usage;

use super::protocol;
use crate::{sse::SseEvent, stream::Wire};
use crucible_core::{
    CONTINUATION_PARTS, Continuation, ContinuationScope, Delta, ProviderError, StopReason,
};
use serde_json::Value;
use std::collections::BTreeMap;
use step::{Budget, Step};

#[derive(Default)]
pub(crate) struct Interactions {
    steps: BTreeMap<usize, Step>,
    next: usize,
    started: usize,
    text: usize,
    calls: usize,
    state: Option<Continuation>,
    completed: bool,
    done: bool,
    budget: Budget,
}

impl Interactions {
    pub(crate) fn new(model: &str, scope: ContinuationScope) -> Result<Self, ProviderError> {
        Ok(Self {
            state: Some(
                Continuation::new(super::PROTOCOL, model, scope)
                    .map_err(|_| protocol("invalid continuation identity"))?,
            ),
            ..Self::default()
        })
    }

    fn step(&mut self, kind: &str, mut payload: Value) -> Result<Vec<Delta>, ProviderError> {
        let index = payload
            .get("index")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| *n < CONTINUATION_PARTS)
            .ok_or_else(|| protocol("invalid interaction step index"))?;
        if index < self.next {
            return Err(protocol("interaction step was already closed"));
        }
        if kind == "step.start" {
            if self.steps.contains_key(&index) {
                return Err(protocol("duplicate interaction step"));
            }
            let value = payload
                .get_mut("step")
                .map(Value::take)
                .ok_or_else(|| protocol("missing interaction step"))?;
            let step = Step::new(value, &mut self.budget)?;
            self.steps.insert(index, step);
            self.started += 1;
        } else {
            let step = self
                .steps
                .get_mut(&index)
                .ok_or_else(|| protocol("interaction delta has no open step"))?;
            if step.stopped {
                return Err(protocol("interaction step was already closed"));
            }
            if kind == "step.stop" {
                step.stopped = true;
            } else {
                let delta = payload
                    .get_mut("delta")
                    .map(Value::take)
                    .ok_or_else(|| protocol("missing interaction delta"))?;
                step.apply(delta, &mut self.budget)?;
            }
        }
        let mut deltas = Vec::new();
        while let Some(step) = self.steps.get_mut(&self.next) {
            step.flush(&mut self.text, &mut deltas);
            if !step.stopped {
                break;
            }
            let step = self
                .steps
                .remove(&self.next)
                .ok_or_else(|| protocol("missing interaction step"))?;
            step.finish(
                self.state
                    .as_mut()
                    .ok_or_else(|| protocol("missing continuation context"))?,
                &mut self.calls,
                &mut deltas,
                &mut self.budget,
            )?;
            self.next += 1;
        }
        if deltas.is_empty() {
            deltas.push(Delta::Progress);
        }
        Ok(deltas)
    }

    fn completed(&mut self, payload: &Value) -> Result<Vec<Delta>, ProviderError> {
        if !self.steps.is_empty() || self.next != self.started {
            return Err(protocol("interaction completed with unfinished steps"));
        }
        let stop = match payload
            .pointer("/interaction/status")
            .and_then(Value::as_str)
        {
            Some("completed") if self.calls == 0 => StopReason::Yielded,
            Some("requires_action") if self.calls > 0 => StopReason::WantsTools,
            Some("incomplete") => StopReason::OutOfTokens,
            Some("cancelled") => StopReason::Cancelled,
            _ => return Err(protocol("interaction did not complete successfully")),
        };
        self.completed = true;
        if matches!(stop, StopReason::Yielded | StopReason::WantsTools) {
            self.budget.completed()?;
        }
        let mut deltas = Vec::new();
        if let Some(usage) = usage::reported(payload)? {
            deltas.push(usage);
        }
        if self.started > 0 && matches!(stop, StopReason::Yielded | StopReason::WantsTools) {
            deltas.push(Delta::Continuation(
                self.state
                    .take()
                    .ok_or_else(|| protocol("missing continuation context"))?,
            ));
        }
        deltas.push(Delta::Stopped(stop));
        Ok(deltas)
    }
}

impl Wire for Interactions {
    const PROVIDER: &'static str = super::NAME;
    fn deltas(&mut self, event: &SseEvent) -> Result<Vec<Delta>, ProviderError> {
        if event.name == "ping" {
            return Ok(Vec::new());
        }
        // Google's REST stream can append this non-JSON marker. It confirms
        // an already completed interaction, never substitutes for completion.
        // Continue draining so a later contradictory event still fails.
        if event.name == "done" && event.data.trim() == "[DONE]" {
            if !self.completed || self.done {
                return Err(protocol("unexpected interaction done marker"));
            }
            self.done = true;
            return Ok(Vec::new());
        }
        let payload: Value = serde_json::from_str(&event.data)
            .map_err(|_| protocol("invalid interaction event JSON"))?;
        let kind = payload
            .get("event_type")
            .and_then(Value::as_str)
            .ok_or_else(|| protocol("missing interaction event type"))?;
        if !event.name.is_empty() && event.name != "message" && event.name != kind {
            return Err(protocol("conflicting interaction event types"));
        }
        if self.completed {
            return Err(protocol("interaction event after completion"));
        }
        match kind {
            "interaction.created" | "interaction.status_update" => Ok(Vec::new()),
            "step.start" => self.step("step.start", payload),
            "step.delta" => self.step("step.delta", payload),
            "step.stop" => self.step("step.stop", payload),
            "interaction.completed" => self.completed(&payload),
            // Vendor error text can contain private thought or signatures. It
            // has no safe user-visible subset; the typed failure is sufficient.
            "error"
                if payload.pointer("/error/code").and_then(Value::as_str)
                    == Some("gateway_timeout") =>
            {
                Err(ProviderError::Upstream {
                    provider: super::NAME,
                    kind: "gateway_timeout".into(),
                    message: "Google did not finish the interaction in time".into(),
                })
            }
            "error" | "interaction.failed" => {
                Err(protocol("Google reported an interaction failure"))
            }
            _ => Err(protocol("unsupported interaction event")),
        }
    }
}

#[cfg(test)]
mod tests;
