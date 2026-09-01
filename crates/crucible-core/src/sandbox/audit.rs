//! Bounded, fixed-attribution facts from one tool call's sandbox lifecycles.

use std::sync::{Arc, Mutex};

use crate::{Ancestry, SandboxId, ToolId};

use super::{
    SandboxCleanup, SandboxCommandStage, SandboxGuardrailDecision, SandboxInspection, SandboxUsage,
    SandboxViolation,
};

/// Most sandbox facts one admitted tool call may retain.
pub const MAX_SANDBOX_AUDIT_FACTS: usize = 128;

/// A lifecycle transition that contains no backend-controlled prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLifecycle {
    /// Effective policy and manifest identities were fixed.
    PolicyResolved,
    /// A session and its cleanup ownership were prepared.
    Prepared,
    /// The manifest transaction committed.
    Materialized,
    /// The governed command started.
    CommandStarted,
    /// The command leader exited and its descendant scope was emptied.
    CommandFinished,
}

/// Stable failure category retained without OS errors, paths, or command text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFailureKind {
    /// A requested capability was unsupported.
    Unsupported,
    /// No suitable backend could be prepared.
    BackendUnavailable,
    /// A command guardrail denied an image.
    Guardrail,
    /// The concurrent reservation was exhausted.
    Concurrency,
    /// Manifest or filesystem preparation failed.
    Materialization,
    /// Process creation failed.
    Spawn,
    /// A running process could not be controlled or reaped.
    Lifecycle,
    /// The bounded audit collector itself could not retain the fact.
    Audit,
    /// A command or environment record was structurally invalid.
    InvalidInput,
}

/// Lifecycle phase in which a typed failure occurred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxFailurePhase {
    /// Backend selection and capability negotiation.
    Prepare,
    /// Transactional manifest/workspace setup.
    Materialize,
    /// Guardrail evaluation and process creation.
    Start,
    /// Process control, accounting, or cleanup.
    Execute,
}

/// One redacted lifecycle fact. The surrounding record supplies attribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxFactKind {
    /// A deterministic lifecycle transition.
    Lifecycle(SandboxLifecycle),
    /// The immutable negotiated inspection snapshot.
    Negotiated(SandboxInspection),
    /// One requested/effective command-filter decision.
    Guardrail {
        /// Image evaluated.
        stage: SandboxCommandStage,
        /// Redacted allow/deny outcome.
        decision: SandboxGuardrailDecision,
    },
    /// A hard resource ceiling was crossed.
    Violation(SandboxViolation),
    /// Bounded current/final usage.
    Usage(SandboxUsage),
    /// Terminal cleanup state.
    Cleanup(SandboxCleanup),
    /// A typed phase failed without retaining its diagnostic text.
    Failed {
        /// Phase that could not complete.
        phase: SandboxFailurePhase,
        /// Stable failure class.
        kind: SandboxFailureKind,
    },
}

/// One fact tied to one stable sandbox identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SandboxFact {
    sandbox: SandboxId,
    kind: SandboxFactKind,
}

impl SandboxFact {
    /// Stable lifecycle identity.
    #[must_use]
    pub const fn sandbox(&self) -> SandboxId {
        self.sandbox
    }

    /// Redacted fact payload.
    #[must_use]
    pub const fn kind(&self) -> &SandboxFactKind {
        &self.kind
    }
}

/// A fact with attribution fixed by the host-created tool context.
#[derive(Clone, PartialEq, Eq)]
pub struct SandboxAuditRecord {
    ancestry: Ancestry,
    call: ToolId,
    fact: SandboxFact,
}

impl SandboxAuditRecord {
    /// Run ancestry that cannot be selected by a backend.
    #[must_use]
    pub const fn ancestry(&self) -> Ancestry {
        self.ancestry
    }

    /// Tool call that owns the lifecycle.
    #[must_use]
    pub const fn call(&self) -> &ToolId {
        &self.call
    }

    /// Redacted sandbox fact.
    #[must_use]
    pub const fn fact(&self) -> &SandboxFact {
        &self.fact
    }
}

impl std::fmt::Debug for SandboxAuditRecord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxAuditRecord")
            .field("ancestry", &self.ancestry)
            .field("call", &"[tool call]")
            .field("fact", &self.fact)
            .finish()
    }
}

/// Cloneable bounded collector shared by one tool context and its processes.
#[derive(Clone)]
pub struct SandboxAudit {
    ancestry: Ancestry,
    call: ToolId,
    facts: Arc<Mutex<Vec<SandboxAuditRecord>>>,
}

impl SandboxAudit {
    /// Fixes attribution before any backend receives the collector.
    #[must_use]
    pub fn new(ancestry: Ancestry, call: ToolId) -> Self {
        Self {
            ancestry,
            call,
            facts: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Retains one typed fact under the collector's fixed attribution.
    ///
    /// # Errors
    ///
    /// A poisoned or full collector fails closed instead of silently omitting
    /// a capability-claimed audit record.
    pub fn record(
        &self,
        sandbox: SandboxId,
        kind: SandboxFactKind,
    ) -> Result<(), SandboxAuditError> {
        let mut facts = self
            .facts
            .lock()
            .map_err(|_| SandboxAuditError::Unavailable)?;
        if facts.len() >= MAX_SANDBOX_AUDIT_FACTS {
            return Err(SandboxAuditError::Full);
        }
        facts.push(SandboxAuditRecord {
            ancestry: self.ancestry,
            call: self.call.clone(),
            fact: SandboxFact { sandbox, kind },
        });
        Ok(())
    }

    /// Stable snapshot in causal insertion order.
    pub fn records(&self) -> Result<Box<[SandboxAuditRecord]>, SandboxAuditError> {
        self.facts
            .lock()
            .map(|facts| facts.clone().into_boxed_slice())
            .map_err(|_| SandboxAuditError::Unavailable)
    }

    /// Takes every currently retained fact exactly once in causal order.
    ///
    /// # Errors
    ///
    /// The collector became unavailable.
    pub fn take_records(&self) -> Result<Box<[SandboxAuditRecord]>, SandboxAuditError> {
        self.facts
            .lock()
            .map(|mut facts| std::mem::take(&mut *facts).into_boxed_slice())
            .map_err(|_| SandboxAuditError::Unavailable)
    }

    pub(crate) fn belongs_to(&self, ancestry: Ancestry, call: &ToolId) -> bool {
        self.ancestry == ancestry && &self.call == call
    }
}

impl std::fmt::Debug for SandboxAudit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let retained = self.facts.lock().map_or(0, |facts| facts.len());
        f.debug_struct("SandboxAudit")
            .field("ancestry", &self.ancestry)
            .field("call", &"[tool call]")
            .field("retained", &retained)
            .finish()
    }
}

/// Why a bounded fact could not be retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SandboxAuditError {
    /// The fixed per-call fact ceiling was reached.
    #[error("sandbox audit reached its bounded fact ceiling")]
    Full,
    /// The collector could no longer be accessed safely.
    #[error("sandbox audit collector is unavailable")]
    Unavailable,
    /// A host attempted to attach a collector to another execution or call.
    #[error("sandbox audit attribution does not match its tool context")]
    AttributionMismatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_is_fixed_and_debug_redacts_the_call() {
        let ancestry = Ancestry::new();
        let audit = SandboxAudit::new(ancestry, ToolId::new("secret-provider-call"));
        audit
            .record(
                SandboxId::new(),
                SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
            )
            .expect("record");
        let records = audit.records().expect("records");

        assert_eq!(records.len(), 1);
        assert_eq!(records[0].ancestry(), ancestry);
        assert!(!format!("{:?}", records[0]).contains("secret-provider-call"));
    }

    #[test]
    fn collector_fails_closed_at_its_fixed_fact_ceiling() {
        let audit = SandboxAudit::new(Ancestry::new(), ToolId::new("bounded-call"));
        let sandbox = SandboxId::new();
        for _ in 0..MAX_SANDBOX_AUDIT_FACTS {
            audit
                .record(
                    sandbox,
                    SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
                )
                .expect("space below the ceiling");
        }

        assert_eq!(
            audit.record(
                sandbox,
                SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared)
            ),
            Err(SandboxAuditError::Full)
        );
        assert_eq!(
            audit.records().expect("bounded snapshot").len(),
            MAX_SANDBOX_AUDIT_FACTS
        );
    }
}
