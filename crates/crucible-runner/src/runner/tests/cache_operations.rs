//! What explicit prompt-cache cleanup does with provider work that must wait.

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
