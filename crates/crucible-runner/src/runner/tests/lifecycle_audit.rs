//! Toolset lifecycle audit delivery on successful and failed turn exits.

use crucible_core::{SandboxFactKind, SandboxId, SandboxLifecycle, ToolsetContext, ToolsetError};

use super::*;

struct Auditing {
    snapshot: ToolSnapshot,
    fails: Option<&'static str>,
}

impl Auditing {
    fn stage(&self, context: &ToolsetContext, stage: &'static str) -> Result<(), ToolsetError> {
        context
            .sandbox_audit(ToolId::new(stage))
            .unwrap()
            .record(
                SandboxId::new(),
                SandboxFactKind::Lifecycle(SandboxLifecycle::Prepared),
            )
            .unwrap();
        if self.fails == Some(stage) {
            Err(ToolsetError::Source {
                id: stage.into(),
                problem: "injected lifecycle failure".into(),
            })
        } else {
            Ok(())
        }
    }
}

impl Toolset for Auditing {
    fn prepare(&self, context: &ToolsetContext) -> Result<(), ToolsetError> {
        self.stage(context, "prepare")
    }
    fn snapshot(&self, context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError> {
        self.stage(context, "snapshot")?;
        Ok(self.snapshot.clone())
    }
    fn refresh(&self, context: &ToolsetContext) -> Result<ToolSnapshot, ToolsetError> {
        self.stage(context, "refresh")?;
        Ok(self.snapshot.clone())
    }
    fn dispose(&self, context: &ToolsetContext) -> Result<(), ToolsetError> {
        self.stage(context, "dispose")
    }
}

struct Observing {
    sent: Sent,
    facts: Mutex<Vec<(String, Ancestry, usize)>>,
}

impl Post for Observing {
    fn post(&self, envelope: EventEnvelope) {
        if let Event::Sandbox { call, .. } = envelope.event() {
            self.facts.lock().unwrap().push((
                call.as_str().into(),
                envelope.ancestry(),
                self.sent.lock().unwrap().len(),
            ));
        }
    }
}

fn audit_exit(fails: Option<&'static str>) {
    let root = std::env::temp_dir().join(format!("crucible-lifecycle-audit-{}", SandboxId::new()));
    std::fs::create_dir(&root).unwrap();
    let store = Recording::started("lifecycle audits");
    let recording: Arc<dyn JournalStore> = store.clone();
    let script = Script::new(vec![calling("a", "read", "{}"), saying("done")]);
    let events = Observing {
        sent: script.sent(),
        facts: Mutex::new(Vec::new()),
    };
    let toolset = Auditing {
        snapshot: tools([Fixed::new("read")]).snapshot().unwrap(),
        fails,
    };
    let mut runner = Runner::with_toolset(
        Box::new(script),
        toolset,
        Agent::new(
            AgentId::new("test"),
            Model {
                name: "test".into(),
                max_tokens: 64,
                window: None,
                accepts: None,
                effort: None,
            },
        ),
        ContextInputs::new(&root)
            .dated(std::time::UNIX_EPOCH + std::time::Duration::from_hours(496_824)),
        recording,
    );
    let cancel = Cancel::new();
    let steer = Steer::new();
    let aside = Aside::new();
    let mut asks = Says::new(Verdict::Allow);
    let context = runner.starting(&events, &cancel, &steer, &aside);
    let ancestry = context.ancestry();
    let result = runner.turn("go", Box::new([]), &mut asks, &context);
    assert_eq!(result.is_err(), fails.is_some(), "{fails:?}: {result:?}");
    drop(runner);
    let expected = match fails {
        Some("prepare") => vec![("prepare", 0), ("dispose", 0)],
        Some("snapshot") => vec![("prepare", 0), ("snapshot", 0), ("dispose", 0)],
        Some("refresh") => vec![
            ("prepare", 0),
            ("snapshot", 0),
            ("refresh", 1),
            ("dispose", 1),
        ],
        _ => vec![
            ("prepare", 0),
            ("snapshot", 0),
            ("refresh", 1),
            ("dispose", 2),
        ],
    };
    let facts = events.facts.lock().unwrap();
    let observed: Vec<_> = facts
        .iter()
        .map(|(stage, _, sent)| (stage.as_str(), *sent))
        .collect();
    assert_eq!(
        observed, expected,
        "{fails:?}: provider request counts at each audit event"
    );
    assert!(facts.iter().all(|(_, actual, _)| *actual == ancestry));
    // The same facts as the run record holds them, which is what a later
    // reader gets. How a store spells them is its own business; what matters
    // here is that every audit the reader watched was also written down,
    // attributed to the call it belongs to and to the run that made it.
    let recorded: Vec<_> = store
        .journaled()
        .into_iter()
        .filter_map(|item| match item {
            RunItem::Sandbox { ancestry, call, .. } => Some((call, ancestry)),
            _ => None,
        })
        .collect();
    assert_eq!(recorded.len(), expected.len());
    for ((call, recorded), (stage, _)) in recorded.iter().zip(&expected) {
        assert_eq!(call.as_str(), *stage);
        assert_eq!(recorded.run(), ancestry.run());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn toolset_audit_delivery_success() {
    audit_exit(None);
}

#[test]
fn toolset_audit_delivery_prepare_failure() {
    audit_exit(Some("prepare"));
}

#[test]
fn toolset_audit_delivery_snapshot_failure() {
    audit_exit(Some("snapshot"));
}

#[test]
fn toolset_audit_delivery_refresh_failure() {
    audit_exit(Some("refresh"));
}

#[test]
fn toolset_audit_delivery_dispose_failure() {
    audit_exit(Some("dispose"));
}

#[test]
fn sandbox_audit_failure_preserves_all_four_lifecycle_causes() {
    let prepared = combine_sandbox_audit::<()>(
        Err(TurnError::Toolset(ToolsetError::Source {
            id: "prepare".into(),
            problem: "source unavailable".into(),
        })),
        Err(ToolError::Unknown("prepare audit".into())),
    );
    let cleanup = TurnError::ToolsetCleanup {
        primary: Box::new(prepared.unwrap_err()),
        cleanup: ToolsetError::Source {
            id: "dispose".into(),
            problem: "cleanup unconfirmed".into(),
        },
    };
    let failed = combine_sandbox_audit::<()>(
        Err(cleanup),
        Err(ToolError::Unknown("dispose audit".into())),
    )
    .unwrap_err();
    let TurnError::SandboxAudit {
        primary,
        audit: ToolError::Unknown(audit),
    } = failed
    else {
        panic!("final audit failure replaced the lifecycle errors")
    };
    assert_eq!(audit.as_ref(), "dispose audit");
    let TurnError::ToolsetCleanup {
        primary,
        cleanup: ToolsetError::Source { id, problem },
    } = *primary
    else {
        panic!("cleanup failure or its primary cause was lost")
    };
    assert_eq!(
        (id.as_ref(), problem.as_ref()),
        ("dispose", "cleanup unconfirmed")
    );
    let TurnError::SandboxAudit {
        primary,
        audit: ToolError::Unknown(audit),
    } = *primary
    else {
        panic!("preparation audit failure or its primary cause was lost")
    };
    assert_eq!(audit.as_ref(), "prepare audit");
    assert!(
        matches!(*primary, TurnError::Toolset(ToolsetError::Source { id, problem })
        if id.as_ref() == "prepare" && problem.as_ref() == "source unavailable")
    );
}

#[test]
fn sandbox_audit_failure_does_not_hide_success_or_invent_another_cause() {
    assert_eq!(combine_sandbox_audit(Ok(42), Ok(())).unwrap(), 42);
    assert!(matches!(
        combine_sandbox_audit::<()>(Err(TurnError::Refused("primary".into())), Ok(())),
        Err(TurnError::Refused(name)) if name.as_ref() == "primary"
    ));
    assert!(matches!(
        combine_sandbox_audit(Ok(42), Err(ToolError::Unknown("audit".into()))),
        Err(TurnError::Tool(ToolError::Unknown(name))) if name.as_ref() == "audit"
    ));
}
