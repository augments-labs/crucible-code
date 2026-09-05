//! Sandbox request facts must remain attached to the host lifecycle registry.

use crucible_core::{
    MAX_SANDBOX_AUDIT_LIFECYCLES, SandboxAudit, SandboxAuditRegistry, SandboxCleanup,
    SandboxFactKind, SandboxFailureKind, SandboxFailurePhase, SandboxLifecycle, TOOL_CALL_ID_BYTES,
    TOOL_SOURCE_ID_BYTES,
};

use super::*;

#[test]
fn disposed_snapshots_do_not_consume_live_audit_capacity() {
    let sandbox = Pretend::new(
        (0..=MAX_SANDBOX_AUDIT_LIFECYCLES)
            .map(|_| Answers::Says(opening("docs", &json!([offers("search")])))),
    );
    let registry = SandboxAuditRegistry::new();
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    let mut snapshots = Vec::new();
    for _ in 0..=MAX_SANDBOX_AUDIT_LIFECYCLES {
        let context = lifecycle().with_sandbox_audits(registry.clone());
        hosting
            .prepare(&context)
            .expect("disposed lifecycle releases its audit slot");
        snapshots.push(hosting.snapshot(&context).unwrap());
        hosting.dispose(&context).unwrap();
        assert!(registry.take_records().unwrap().is_empty());
    }
    assert_eq!(sandbox.started(), MAX_SANDBOX_AUDIT_LIFECYCLES + 1);
    for snapshot in &snapshots {
        let entry = snapshot.find("mcp:docs/search").unwrap();
        assert!(matches!(
            calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new()),
            Err(ToolError::StaleGeneration { .. })
        ));
    }
}

struct Recording {
    inner: Arc<Pretend>,
    seen: Mutex<Vec<(SandboxId, SandboxAudit)>>,
}

impl Recording {
    fn new(answers: impl IntoIterator<Item = Answers>) -> Arc<Self> {
        Arc::new(Self {
            inner: Pretend::new(answers),
            seen: Mutex::new(Vec::new()),
        })
    }
}

impl SandboxService for Recording {
    fn probe(&self) -> Result<(SandboxBackendIdentity, SandboxCapabilities), SandboxError> {
        self.inner.probe()
    }
    fn prepare(&self, request: SandboxRequest) -> Result<Box<dyn SandboxSession>, SandboxError> {
        let id = request.id();
        let audit = request.audit().clone();
        self.seen.lock().unwrap().push((id, audit.clone()));
        audit.record(
            id,
            SandboxFactKind::Lifecycle(SandboxLifecycle::PolicyResolved),
        )?;
        let index = self.inner.started();
        match self.inner.prepare(request) {
            Ok(session) => {
                *self.inner.server(index).audit.lock().unwrap() = Some((id, audit));
                Ok(session)
            }
            Err(error) => {
                audit.record(
                    id,
                    SandboxFactKind::Failed {
                        phase: SandboxFailurePhase::Prepare,
                        kind: SandboxFailureKind::Lifecycle,
                    },
                )?;
                Err(error)
            }
        }
    }
}

#[test]
fn hosted_audits_keep_attribution_through_restart_and_disposal() {
    let first = opening("docs", &json!([offers("search")]));
    let mut second = opening("docs", &json!([offers("search")]));
    second.push(produced("replacement", false));
    let sandbox = Recording::new([Answers::Says(first), Answers::Says(second)]);
    let registry = SandboxAuditRegistry::new();
    let context = lifecycle().with_sandbox_audits(registry.clone());
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(1)],
    );
    hosting.prepare(&context).unwrap();
    let initial = registry.take_records().unwrap();
    assert_eq!(
        initial.len(),
        1,
        "preparation fact must reach host registry"
    );
    let snapshot = hosting.snapshot(&context).unwrap();
    let entry = snapshot.find("mcp:docs/search").unwrap();
    sandbox.inner.server(0).departs();
    assert!(
        calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
            .unwrap()
            .text()
            .contains("replacement")
    );
    hosting.dispose(&context).unwrap();
    let rest = registry.take_records().unwrap();
    let facts: Vec<_> = initial.iter().chain(rest.iter()).collect();
    assert_eq!(facts.len(), 4);
    assert!(
        facts
            .iter()
            .all(|record| record.ancestry() == context.ancestry()
                && record.call().as_str() == "mcp:docs")
    );
    let requests = sandbox.seen.lock().unwrap();
    let [(first, _), (second, _)] = requests.as_slice() else {
        panic!("initial preparation and exactly one restart are required")
    };
    assert_ne!(first, second);
    assert_eq!(
        facts
            .iter()
            .map(|record| record.fact().sandbox())
            .collect::<Vec<_>>(),
        vec![*first, *first, *second, *second]
    );
    assert!(matches!(
        rest.last().unwrap().fact().kind(),
        SandboxFactKind::Cleanup(SandboxCleanup::Complete)
    ));
    assert!(registry.take_records().unwrap().is_empty());
}

#[test]
fn hosted_audits_retain_preparation_failure_facts() {
    let sandbox = Recording::new([Answers::Refuses]);
    let registry = SandboxAuditRegistry::new();
    let context = lifecycle().with_sandbox_audits(registry.clone());
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    assert!(hosting.prepare(&context).is_err());
    hosting.dispose(&context).unwrap();
    let records = registry.take_records().unwrap();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .all(|record| record.ancestry() == context.ancestry()
                && record.call().as_str() == "mcp:docs")
    );
    assert!(matches!(
        records.last().unwrap().fact().kind(),
        SandboxFactKind::Failed {
            phase: SandboxFailurePhase::Prepare,
            ..
        }
    ));
}

#[test]
fn hosted_audits_refuse_full_registry_before_backend_effects() {
    let registry = SandboxAuditRegistry::new();
    let context = lifecycle().with_sandbox_audits(registry.clone());
    let held: Vec<_> = (0..MAX_SANDBOX_AUDIT_LIFECYCLES)
        .map(|i| {
            context
                .sandbox_audit(ToolId::new(format!("held-{i}")))
                .unwrap()
        })
        .collect();
    let sandbox = Recording::new([Answers::Says(opening("docs", &json!([])))]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs")],
    );
    assert!(hosting.prepare(&context).is_err());
    assert!(sandbox.seen.lock().unwrap().is_empty());
    drop(held);
    hosting.prepare(&context).unwrap();
    hosting.dispose(&context).unwrap();
    assert_eq!(sandbox.seen.lock().unwrap().len(), 1);
}

#[test]
fn hosted_audits_validate_identity_before_backend_effects() {
    for length in [TOOL_SOURCE_ID_BYTES + 1, TOOL_CALL_ID_BYTES + 1] {
        let sandbox = Recording::new([Answers::Says(opening("docs", &json!([])))]);
        let hosting = Hosting::new(
            builtin(&[]),
            sandbox.clone() as Arc<dyn SandboxService>,
            vec![chosen(&"x".repeat(length))],
        );
        assert!(hosting.prepare(&lifecycle()).is_err());
        assert!(
            sandbox.seen.lock().unwrap().is_empty(),
            "identity must be validated before preparation"
        );
    }
}
