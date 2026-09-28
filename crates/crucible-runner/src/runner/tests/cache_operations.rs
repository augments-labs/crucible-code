//! What explicit prompt-cache cleanup does: the records a persistent resource
//! leaves behind, the provider work it waits for, and which of them an
//! interrupted call may still have created.

use super::*;

#[test]
fn a_cleanup_waits_for_a_provider_delete_that_yields() {
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(
        Script::new(Vec::new()).delayed_delete(),
        Tools::new(),
        Verdict::Deny,
    )
    .storing(store);
    let provider_scope =
        prompt_cache::provider_scope(scripted.runner.provider.prompt_cache_route());
    records
        .lock()
        .unwrap()
        .push(ready_resource("script", provider_scope, 1));

    let cleaned = scripted
        .runner
        .clean_prompt_cache(&Cancel::new())
        .awaited()
        .expect("the delayed delete to finish");

    assert_eq!(cleaned.inspected, 1);
    assert_eq!(cleaned.deleted, 1);
    assert_eq!(cleaned.ambiguous, 0);
    assert!(records.lock().unwrap().is_empty());
}

#[test]
fn an_interrupted_persistent_create_is_waited_for_and_recorded_as_ambiguous() {
    let script = Script::new(vec![saying("done")]).interrupting_create();
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny).storing(store);
    scripted
        .runner
        .redefine(|agent| agent.telling("stable fixture instructions"));
    scripted.runner.policy.prompt_cache = scripted
        .runner
        .policy
        .prompt_cache
        .with_persistent_resources(PromptCachePersistentMode::Create);

    let problem = scripted.turn("go").unwrap_err();

    assert!(
        matches!(
            problem,
            TurnError::PromptCacheResource(PromptCacheResourceError::Cancelled)
        ),
        "the interrupted create's own answer must be waited for: {problem:?}"
    );
    assert!(scripted.sent.lock().unwrap().is_empty());
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("the interrupted create must retain its resource record");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ambiguous);
    assert_eq!(
        record.pending(),
        Some(PromptCacheResourceOperation::Create),
        "the unknown outcome must say which operation needs reconciliation"
    );
}

#[test]
fn persistent_resources_are_ready_before_wire_reference_and_explicit_cleanup_deletes_them() {
    let script = Script::new(vec![saying("done")]).persistent();
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny).storing(store);
    scripted
        .runner
        .redefine(|agent| agent.telling("stable fixture instructions"));
    scripted.runner.policy.prompt_cache = scripted
        .runner
        .policy
        .prompt_cache
        .with_persistent_resources(PromptCachePersistentMode::Create);

    scripted.turn("go").expect("the turn to finish");

    let sent_guard = scripted.sent.lock().unwrap();
    let [sent] = sent_guard.as_slice() else {
        panic!("persistent fixture must send exactly one request");
    };
    assert!(sent.cache_resource, "{:?}", sent.cache_selection);
    drop(sent_guard);
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("persistent fixture must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ready);
    drop(held);
    assert_eq!(
        scripted
            .runner
            .prompt_cache_attempt()
            .expect("an attempt")
            .encoding,
        PromptCacheEncoding::PersistentResourceReferenced
    );
    let lifecycle_states: Vec<_> = scripted
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::PromptCache {
                fact: PromptCacheFact::ResourceChanged(fact),
            } => Some((fact.operation, fact.state)),
            _ => None,
        })
        .collect();
    assert_eq!(
        lifecycle_states,
        [
            (
                Some(PromptCacheResourceOperation::Create),
                PromptCacheResourceState::Creating,
            ),
            (
                Some(PromptCacheResourceOperation::Create),
                PromptCacheResourceState::Ready,
            ),
        ]
    );

    let cleaned = scripted
        .runner
        .clean_prompt_cache(&Cancel::new())
        .awaited()
        .expect("bounded cleanup");
    assert_eq!(cleaned.deleted, 1);
    assert!(records.lock().unwrap().is_empty());
}

#[test]
fn retirement_deletes_only_the_current_exclusive_owner_scope() {
    let script = Script::new(vec![saying("done")]).persistent();
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(script, Tools::new(), Verdict::Deny).storing(store);
    scripted
        .runner
        .redefine(|agent| agent.telling("stable fixture instructions"));
    scripted.runner.policy.prompt_cache = scripted
        .runner
        .policy
        .prompt_cache
        .with_persistent_resources(PromptCachePersistentMode::Create);

    scripted.turn("go").expect("the turn to finish");

    let current = records
        .lock()
        .unwrap()
        .first()
        .cloned()
        .expect("the current owner resource");
    let binding = PromptCacheResourceBinding::new(
        PromptCacheScopeDigest::new([91; 32]),
        current.binding().provider_scope(),
        PromptCacheScopeDigest::new([92; 32]),
        PromptCacheFingerprint::new([93; 32]),
        PromptCachePolicyDigest::new([94; 32]),
        PromptCacheResourceOwner::new(PromptCacheIsolation::Session, true),
        current.binding().protocol(),
        "other-session-model",
        Some("script-revision-v1"),
    )
    .unwrap();
    let mut other_owner =
        PromptCacheResourceRecord::creating(PromptCacheResourceId::new(), binding, 100);
    other_owner.ready(
        PromptCacheResourceHandle::new("other-owner-handle").unwrap(),
        u64::MAX,
        110,
    );
    records.lock().unwrap().push(other_owner.clone());

    let retired = scripted
        .runner
        .retire_prompt_cache(&Cancel::new())
        .awaited()
        .expect("bounded retirement");

    assert_eq!(retired.inspected, 1);
    assert_eq!(retired.deleted, 1);
    let remaining = records.lock().unwrap();
    let [record] = remaining.as_slice() else {
        panic!("another owner scope must remain untouched");
    };
    assert_eq!(record.id(), other_owner.id());
}

fn ready_resource(
    protocol: &str,
    provider_scope: PromptCacheScopeDigest,
    seed: u8,
) -> PromptCacheResourceRecord {
    let binding = PromptCacheResourceBinding::new(
        PromptCacheScopeDigest::new([seed; 32]),
        provider_scope,
        PromptCacheScopeDigest::new([seed.saturating_add(3); 32]),
        PromptCacheFingerprint::new([seed.saturating_add(1); 32]),
        PromptCachePolicyDigest::new([seed.saturating_add(2); 32]),
        PromptCacheResourceOwner::new(PromptCacheIsolation::Session, true),
        protocol,
        "claude-test",
        Some("script-revision-v1"),
    )
    .unwrap();
    let mut record =
        PromptCacheResourceRecord::creating(PromptCacheResourceId::new(), binding, 100);
    record.ready(
        PromptCacheResourceHandle::new(format!("provider-handle-{seed}")).unwrap(),
        u64::MAX,
        110,
    );
    record
}

#[test]
fn cleanup_without_the_current_provider_lifecycle_fails_without_relabelling_records() {
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted =
        Scripted::new(Script::new(Vec::new()), Tools::new(), Verdict::Deny).storing(store);
    let provider_scope =
        prompt_cache::provider_scope(scripted.runner.provider.prompt_cache_route());
    records
        .lock()
        .unwrap()
        .push(ready_resource("script", provider_scope, 1));

    let problem = scripted
        .runner
        .clean_prompt_cache(&Cancel::new())
        .awaited()
        .unwrap_err();

    assert!(matches!(problem, PromptCacheResourceError::Unsupported));
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("unsupported cleanup must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ready);
}

#[test]
fn cleanup_is_provider_scoped_and_marks_a_conclusive_survivor_orphaned() {
    let store = SharedStore::default();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(
        Script::new(Vec::new()).surviving_delete(),
        Tools::new(),
        Verdict::Deny,
    )
    .storing(store);
    let provider_scope =
        prompt_cache::provider_scope(scripted.runner.provider.prompt_cache_route());
    records.lock().unwrap().extend([
        ready_resource("script", provider_scope, 1),
        ready_resource(
            "another-protocol",
            PromptCacheScopeDigest::new([90; 32]),
            10,
        ),
    ]);

    let cleaned = scripted
        .runner
        .clean_prompt_cache(&Cancel::new())
        .awaited()
        .unwrap();

    assert_eq!(cleaned.inspected, 1);
    assert_eq!(cleaned.orphaned, 1);
    let records = records.lock().unwrap();
    let [owned, other] = records.as_slice() else {
        panic!("protocol-scoped cleanup must retain both fixture records");
    };
    assert_eq!(owned.state(), PromptCacheResourceState::Orphaned);
    assert_eq!(other.state(), PromptCacheResourceState::Ready);
}

#[test]
fn ambiguous_delete_is_retained_for_reconciliation_and_pre_cancel_changes_nothing() {
    let store = SharedStore::default();
    let resumed_store = store.clone();
    let records = Arc::clone(&store.0);
    let mut scripted = Scripted::new(
        Script::new(Vec::new()).ambiguous_delete(),
        Tools::new(),
        Verdict::Deny,
    )
    .storing(store);
    let provider_scope =
        prompt_cache::provider_scope(scripted.runner.provider.prompt_cache_route());
    records
        .lock()
        .unwrap()
        .push(ready_resource("script", provider_scope, 1));
    let cancelled = Cancel::new();
    cancelled.request();

    assert!(matches!(
        scripted.runner.clean_prompt_cache(&cancelled).awaited(),
        Err(PromptCacheResourceError::Cancelled)
    ));
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("pre-cancelled cleanup must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ready);
    drop(held);

    let cleaned = scripted
        .runner
        .clean_prompt_cache(&Cancel::new())
        .awaited()
        .unwrap();
    assert_eq!(cleaned.ambiguous, 1);
    assert_eq!(
        cleaned
            .changes()
            .iter()
            .map(|change| change.state)
            .collect::<Vec<_>>(),
        [
            PromptCacheResourceState::Deleting,
            PromptCacheResourceState::Ambiguous,
        ]
    );
    let held = records.lock().unwrap();
    let [record] = held.as_slice() else {
        panic!("ambiguous cleanup must retain one resource");
    };
    assert_eq!(record.state(), PromptCacheResourceState::Ambiguous);
    assert_eq!(record.pending(), Some(PromptCacheResourceOperation::Delete));
    drop(held);

    let credential_scope = scripted
        .runner
        .provider
        .prompt_cache_route()
        .credential_scope;
    let mut resumed = Scripted::new(
        Script::new(Vec::new())
            .with_credential_scope(credential_scope)
            .persistent(),
        Tools::new(),
        Verdict::Deny,
    )
    .storing(resumed_store);
    let reconciled = resumed
        .runner
        .clean_prompt_cache(&Cancel::new())
        .awaited()
        .unwrap();

    assert_eq!(reconciled.deleted, 1);
    assert_eq!(
        reconciled
            .changes()
            .iter()
            .map(|change| (change.operation, change.state))
            .collect::<Vec<_>>(),
        [(
            Some(PromptCacheResourceOperation::Delete),
            PromptCacheResourceState::Deleted,
        )]
    );
    assert!(records.lock().unwrap().is_empty());
}
