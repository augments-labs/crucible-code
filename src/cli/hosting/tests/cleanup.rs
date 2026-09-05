//! Unconfirmed cleanup cannot authorize replacement or become a clean disposal.

use super::*;

fn dialogue(name: &str) -> Vec<Value> {
    opening(name, &json!([offers("search")]))
}

#[test]
fn unconfirmed_cleanup_refuses_server_replacement() {
    let mut replacement = dialogue("docs");
    replacement.push(produced("replacement", false));
    let sandbox = Pretend::new([
        Answers::Unreapable(dialogue("docs")),
        Answers::Says(replacement),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(3)],
    );
    let context = lifecycle();
    hosting.prepare(&context).unwrap();
    let snapshot = hosting.snapshot(&context).unwrap();
    let entry = snapshot.find("mcp:docs/search").unwrap();
    sandbox.server(0).departs();
    let error = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new())
        .expect_err("a replacement requires confirmed old-scope cleanup");
    assert!(error.to_string().contains("unconfirmed cleanup"), "{error}");
    assert_eq!(sandbox.started(), 1);
    assert!(matches!(
        calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new()),
        Err(ToolError::StaleGeneration { .. })
    ));
    assert!(hosting.dispose(&context).is_err());
    assert_eq!(sandbox.started(), 1);
    assert_eq!(sandbox.server(0).stops(), 0);
}

#[test]
fn unconfirmed_disposal_remains_failed_and_blocks_repreparation() {
    let sandbox = Pretend::new([
        Answers::Unreapable(dialogue("docs")),
        Answers::Says(dialogue("notes")),
        Answers::Says(dialogue("docs")),
        Answers::Says(dialogue("notes")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs"), chosen("notes")],
    );
    let context = lifecycle();
    hosting.prepare(&context).unwrap();
    assert!(hosting.dispose(&context).is_err());
    assert_eq!(sandbox.server(0).stops(), 0);
    assert_eq!(
        sandbox.server(1).stops(),
        1,
        "every server must be attempted"
    );
    assert!(hosting.snapshot(&context).unwrap().entries().is_empty());
    assert!(
        hosting.dispose(&context).is_err(),
        "uncertainty is not completion"
    );
    assert_eq!(sandbox.server(0).stop_attempts.load(Ordering::Relaxed), 1);
    assert!(hosting.prepare(&context).is_err());
    assert_eq!(
        sandbox.started(),
        2,
        "no new backend after unconfirmed cleanup"
    );
}

#[test]
fn unconfirmed_partial_preparation_cleanup_stays_owned() {
    let sandbox = Pretend::new([
        Answers::Unreapable(dialogue("docs")),
        Answers::Refuses,
        Answers::Says(dialogue("docs")),
        Answers::Says(dialogue("notes")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs"), chosen("notes")],
    );
    let context = lifecycle();
    let error = hosting.prepare(&context).unwrap_err();
    assert!(error.to_string().contains("notes"));
    assert_eq!(sandbox.server(0).stop_attempts.load(Ordering::Relaxed), 1);
    assert!(
        hosting.dispose(&context).is_err(),
        "failed cleanup must remain visible"
    );
    assert!(hosting.dispose(&context).is_err());
    assert!(hosting.prepare(&context).is_err());
    assert_eq!(sandbox.started(), 1);
    assert_eq!(sandbox.server(0).stop_attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn rejected_catalogue_preserves_unconfirmed_replacement_cleanup() {
    let sandbox = Pretend::new([
        Answers::Says(dialogue("docs")),
        Answers::Unreapable(opening("docs", &json!([offers("different")]))),
        Answers::Says(dialogue("docs")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![chosen("docs").restarting(3)],
    );
    let context = lifecycle();
    hosting.prepare(&context).unwrap();
    let snapshot = hosting.snapshot(&context).unwrap();
    let entry = snapshot.find("mcp:docs/search").unwrap();
    sandbox.server(0).departs();
    let error = calls(entry.tool(), "mcp:docs/search", "{}", &Cancel::new()).unwrap_err();
    let message = error.to_string();
    assert!(message.contains("came back without search"), "{message}");
    assert!(message.contains("unconfirmed cleanup"), "{message}");
    assert_eq!(sandbox.started(), 2);
    assert!(
        sandbox
            .server(1)
            .sent()
            .iter()
            .all(|frame| { frame.get("method").and_then(Value::as_str) != Some("tools/call") })
    );
    assert!(hosting.dispose(&context).is_err());
    assert!(hosting.dispose(&context).is_err());
    assert!(hosting.prepare(&context).is_err());
    assert_eq!(sandbox.started(), 2);
    assert_eq!(sandbox.server(1).stop_attempts.load(Ordering::Relaxed), 1);
}
