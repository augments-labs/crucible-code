//! Bounded, fixed-attribution facts from one tool call's sandbox lifecycles.
//!
//! What a fact *is* is owned by `crucible-storage`, beside the journal and the
//! checkpoint that keep it: the lifecycle identity, the typed payload and the
//! record both of them make. This module fixes the attribution — which
//! execution, which tool call — and bounds how many facts one call may retain,
//! which is the half only the runtime that recorded them can know.

use std::sync::{Arc, Mutex};

use crucible_storage::{SandboxFact, SandboxFactKind};
use crucible_types::{Ancestry, SandboxId, ToolId};

/// Most sandbox facts one admitted tool call may retain.
pub const MAX_SANDBOX_AUDIT_FACTS: usize = 128;

/// Most live or detached sandbox lifecycles one runner may retain for late
/// journal delivery.
pub const MAX_SANDBOX_AUDIT_LIFECYCLES: usize = 128;

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

    /// The redacted record this fact is delivered as.
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
    /// An invalid call identity or a poisoned/full collector fails closed
    /// instead of retaining an undeliverable audit record.
    pub fn record(
        &self,
        sandbox: SandboxId,
        kind: SandboxFactKind,
    ) -> Result<(), SandboxAuditError> {
        validate_call(&self.call)?;
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
            fact: SandboxFact::new(sandbox, kind),
        });
        Ok(())
    }

    /// Stable snapshot in causal insertion order.
    ///
    /// # Errors
    ///
    /// The collector became unavailable.
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

    /// Whether this collector was created for exactly this execution and call.
    ///
    /// A caller handed a collector by someone else asks before writing to it:
    /// facts recorded under the wrong attribution are facts about the wrong
    /// call.
    #[must_use]
    pub fn belongs_to(&self, ancestry: Ancestry, call: &ToolId) -> bool {
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

/// Bounded owner of audit collectors whose sandbox processes may outlive the
/// tool call that created them.
///
/// Foreground facts can be drained immediately. A detached command keeps a
/// collector clone, so the registry retains its fixed attribution and drains
/// facts produced later. Once the lifecycle releases its last clone, the next
/// drain removes the empty registry entry.
#[derive(Clone, Default)]
pub struct SandboxAuditRegistry {
    audits: Arc<Mutex<Vec<SandboxAudit>>>,
}

impl SandboxAuditRegistry {
    /// An empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates and registers one fixed-attribution collector.
    ///
    /// # Errors
    ///
    /// The call identity is empty or oversized, the registry is unavailable,
    /// or its live-lifecycle ceiling is reached.
    pub fn collector(
        &self,
        ancestry: Ancestry,
        call: ToolId,
    ) -> Result<SandboxAudit, SandboxAuditError> {
        validate_call(&call)?;
        let mut audits = self
            .audits
            .lock()
            .map_err(|_| SandboxAuditError::Unavailable)?;
        let mut index = 0_usize;
        while let Some(audit) = audits.get(index) {
            let released = !audit.has_lifecycle_holder();
            let empty = audit
                .facts
                .lock()
                .map_err(|_| SandboxAuditError::Unavailable)?
                .is_empty();
            if released && empty {
                audits.remove(index);
            } else {
                index = index.saturating_add(1);
            }
        }
        if audits.len() >= MAX_SANDBOX_AUDIT_LIFECYCLES {
            return Err(SandboxAuditError::RegistryFull);
        }
        let audit = SandboxAudit::new(ancestry, call);
        audits.push(audit.clone());
        Ok(audit)
    }

    /// Takes every fact currently available, preserving collector creation and
    /// per-lifecycle insertion order, then retires released collectors.
    ///
    /// # Errors
    ///
    /// The registry or one collector became unavailable. No facts are consumed
    /// when any collector is unavailable.
    pub fn take_records(&self) -> Result<Box<[SandboxAuditRecord]>, SandboxAuditError> {
        let mut audits = self
            .audits
            .lock()
            .map_err(|_| SandboxAuditError::Unavailable)?;
        // Lock every bounded collector before consuming any facts. A later
        // unavailable collector must not discard earlier undelivered records.
        let mut facts = audits
            .iter()
            .map(|audit| {
                audit
                    .facts
                    .lock()
                    .map_err(|_| SandboxAuditError::Unavailable)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let records = facts
            .iter_mut()
            .flat_map(|records| std::mem::take(&mut **records))
            .collect::<Vec<_>>();
        drop(facts);
        audits.retain(SandboxAudit::has_lifecycle_holder);
        Ok(records.into_boxed_slice())
    }
}

impl SandboxAudit {
    fn has_lifecycle_holder(&self) -> bool {
        // One holder is the registry entry itself. Anything beyond it is a
        // request, tool context, session, process, or background lifecycle.
        Arc::strong_count(&self.facts) > 1
    }
}

impl std::fmt::Debug for SandboxAuditRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let retained = self.audits.lock().map_or(0, |audits| audits.len());
        f.debug_struct("SandboxAuditRegistry")
            .field("lifecycles", &retained)
            .finish()
    }
}

fn validate_call(call: &ToolId) -> Result<(), SandboxAuditError> {
    if call.as_str().is_empty() || call.as_str().len() > crucible_types::TOOL_CALL_ID_BYTES {
        Err(SandboxAuditError::InvalidCall)
    } else {
        Ok(())
    }
}

/// Why a bounded fact could not be retained.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SandboxAuditError {
    /// A call identity cannot cross the framework journal boundary.
    #[error("sandbox audit call identity is empty or oversized")]
    InvalidCall,
    /// The fixed per-call fact ceiling was reached.
    #[error("sandbox audit reached its bounded fact ceiling")]
    Full,
    /// The collector could no longer be accessed safely.
    #[error("sandbox audit collector is unavailable")]
    Unavailable,
    /// A host attempted to attach a collector to another execution or call.
    #[error("sandbox audit attribution does not match its tool context")]
    AttributionMismatch,
    /// Too many detached or live lifecycles are waiting for delivery.
    #[error("sandbox audit registry reached its bounded lifecycle ceiling")]
    RegistryFull,
}

#[cfg(test)]
mod tests {
    use crucible_storage::{SandboxCleanup, SandboxLifecycle};

    use super::*;

    #[test]
    fn invalid_call_identity_is_refused_before_registry_or_fact_retention() {
        let registry = SandboxAuditRegistry::new();
        for id in [
            String::new(),
            "x".repeat(crucible_types::TOOL_CALL_ID_BYTES + 1),
        ] {
            let call = ToolId::new(id);
            assert!(registry.collector(Ancestry::new(), call.clone()).is_err());
            assert!(registry.audits.lock().expect("registry").is_empty());
            let direct = SandboxAudit::new(Ancestry::new(), call);
            assert!(
                direct
                    .record(
                        SandboxId::new(),
                        SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
                    )
                    .is_err()
            );
            assert!(direct.records().expect("no undeliverable facts").is_empty());
        }
        let valid = registry
            .collector(
                Ancestry::new(),
                ToolId::new("x".repeat(crucible_types::TOOL_CALL_ID_BYTES)),
            )
            .expect("maximum valid identity");
        valid
            .record(
                SandboxId::new(),
                SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
            )
            .expect("valid fact");
        let records = registry.take_records().expect("valid drain");
        let [_record] = records.as_ref() else {
            panic!("exactly one valid fact")
        };
    }

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
        let record = records.first().expect("one record");
        assert_eq!(record.ancestry(), ancestry);
        assert!(!format!("{record:?}").contains("secret-provider-call"));
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

    #[test]
    fn registry_retains_late_facts_until_the_detached_lifecycle_releases_them() {
        let registry = SandboxAuditRegistry::new();
        let audit = registry
            .collector(Ancestry::new(), ToolId::new("background-call"))
            .expect("collector");
        let detached = audit.clone();
        drop(audit);
        assert!(registry.take_records().expect("empty drain").is_empty());

        detached
            .record(
                SandboxId::new(),
                SandboxFactKind::Lifecycle(SandboxLifecycle::CommandFinished),
            )
            .expect("late fact");
        let records = registry.take_records().expect("late drain");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records.first().expect("one late fact").call().as_str(),
            "background-call"
        );

        drop(detached);
        assert!(registry.take_records().expect("retired drain").is_empty());
    }

    #[test]
    fn unavailable_collector_cannot_discard_facts_drained_before_it() {
        let registry = SandboxAuditRegistry::new();
        let first = registry
            .collector(Ancestry::new(), ToolId::new("first-call"))
            .expect("first collector");
        let second = registry
            .collector(Ancestry::new(), ToolId::new("second-call"))
            .expect("second collector");
        let first_id = SandboxId::new();
        for lifecycle in [
            SandboxLifecycle::Prepared,
            SandboxLifecycle::CommandFinished,
        ] {
            first
                .record(first_id, SandboxFactKind::Lifecycle(lifecycle))
                .expect("first fact");
        }
        second
            .record(
                SandboxId::new(),
                SandboxFactKind::Cleanup(SandboxCleanup::Complete),
            )
            .expect("second fact");
        let first_records = first.records().expect("first snapshot");
        let mut expected = first_records.to_vec();
        expected.extend(second.records().expect("second snapshot"));

        let poisoned = Arc::clone(&second.facts);
        assert!(
            std::thread::spawn(move || {
                let _guard = poisoned.lock().expect("available before injected failure");
                panic!("injected collector owner failure");
            })
            .join()
            .is_err()
        );
        assert_eq!(registry.take_records(), Err(SandboxAuditError::Unavailable));
        assert_eq!(
            first.records().expect("first remains available"),
            first_records
        );

        // Recovery exists only in this fixture. A production poisoned collector
        // stays unavailable; a failed drain must never consume another's facts.
        second.facts.clear_poison();
        assert_eq!(
            registry.take_records().expect("recovered drain").as_ref(),
            expected
        );
        assert!(
            registry
                .take_records()
                .expect("no duplicate delivery")
                .is_empty()
        );
        drop(first);
        drop(second);
        assert!(registry.take_records().expect("retired drain").is_empty());
        assert!(registry.audits.lock().expect("registry").is_empty());
    }

    #[test]
    fn registering_a_new_call_cannot_prune_undelivered_late_facts() {
        let registry = SandboxAuditRegistry::new();
        let late = registry
            .collector(Ancestry::new(), ToolId::new("late-call"))
            .expect("late collector");
        late.record(
            SandboxId::new(),
            SandboxFactKind::Lifecycle(SandboxLifecycle::CommandFinished),
        )
        .expect("late fact");
        drop(late);

        let current = registry
            .collector(Ancestry::new(), ToolId::new("current-call"))
            .expect("current collector");
        let records = registry.take_records().expect("drain");
        assert_eq!(records.len(), 1);
        assert_eq!(
            records
                .first()
                .expect("one undelivered fact")
                .call()
                .as_str(),
            "late-call"
        );
        drop(current);
    }
}
