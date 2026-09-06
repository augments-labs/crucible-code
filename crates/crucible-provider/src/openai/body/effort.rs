//! Reconstruct documented configuration updates without a second history.
//!
//! Request facts beside each native answer locate changes before their user
//! input. The original request-level control stays fixed across tool passes.
//! An unset effort after an explicit choice has no documented reset item; a
//! resumed request in that state uses its ordinary request-level control and
//! no historical updates. That gives up prefix caching, never the user's choice.

use super::replay::compatible;
use crate::json::Array;
use crate::openai::continuation::problem;
use crucible_core::{
    ContinuationPart, ContinuationScope, Effort, Message, ProviderContinuation, ProviderError,
    Request, RequestPurpose,
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
    changes: Changes<'a>,
    next: Option<(usize, Effort)>,
    initial: Option<Effort>,
}

impl<'a> Efforts<'a> {
    pub(super) fn new(
        request: &Request<'a>,
        scope: ContinuationScope,
    ) -> Result<Option<Self>, ProviderError> {
        if request.purpose != RequestPurpose::Turn {
            return Ok(None);
        }
        let mut initial = request.effort;
        for message in request.transcript.messages() {
            if let Message::Agent {
                continuation: Some(state),
                ..
            } = message
                && compatible(state, scope)
            {
                initial = recorded(state)?;
                break;
            }
        }
        // Check encodability before writing any controls. In particular, an
        // effort changed while resuming a tool pass has no next user boundary.
        // Both scans borrow the transcript; only one pending update is retained.
        let mut check = Changes::new(request, scope, initial);
        while let Some(change) = check.advance()? {
            if change.before.is_none() || change.effort.is_none() {
                return Ok(None);
            }
        }
        let mut changes = Changes::new(request, scope, initial);
        let next = changes
            .advance()?
            .and_then(|change| change.before.zip(change.effort));
        Ok(Some(Self {
            changes,
            next,
            initial,
        }))
    }

    pub(super) const fn initial(&self) -> Option<Effort> {
        self.initial
    }

    pub(super) fn before(
        &mut self,
        input: &mut Array<'_>,
        index: usize,
    ) -> Result<(), ProviderError> {
        if let Some((at, effort)) = self.next
            && at == index
        {
            input.object(|item| {
                item.text("type", "configuration_update");
                item.object("reasoning", |reasoning| {
                    reasoning.text("effort", effort.as_str());
                });
            });
            self.next = self
                .changes
                .advance()?
                .and_then(|change| change.before.zip(change.effort));
        }
        Ok(())
    }
}

struct Changes<'a> {
    remaining: std::iter::Enumerate<std::slice::Iter<'a, Message>>,
    scope: ContinuationScope,
    active: Option<Effort>,
    requested: Option<Effort>,
    finished: bool,
}

struct Change {
    before: Option<usize>,
    effort: Option<Effort>,
}

impl<'a> Changes<'a> {
    fn new(request: &Request<'a>, scope: ContinuationScope, initial: Option<Effort>) -> Self {
        Self {
            remaining: request.transcript.messages().iter().enumerate(),
            scope,
            active: initial,
            requested: request.effort,
            finished: false,
        }
    }

    fn advance(&mut self) -> Result<Option<Change>, ProviderError> {
        let mut user = None;
        for (index, message) in self.remaining.by_ref() {
            if matches!(message, Message::User { .. }) {
                user.get_or_insert(index);
            }
            if let Message::Agent {
                continuation: Some(state),
                ..
            } = message
                && compatible(state, self.scope)
            {
                let effort = recorded(state)?;
                if effort != self.active {
                    self.active = effort;
                    return Ok(Some(Change {
                        before: user,
                        effort,
                    }));
                }
                user = None;
            }
        }
        if !self.finished && self.requested != self.active {
            self.finished = true;
            return Ok(Some(Change {
                before: user,
                effort: self.requested,
            }));
        }
        self.finished = true;
        Ok(None)
    }
}
