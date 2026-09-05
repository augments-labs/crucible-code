//! A refused startup still owns its process cleanup outcome.

use super::*;

fn dialogue(name: &str) -> Vec<Value> {
    opening(name, &json!([offers("search")]))
}

fn bad_greeting() -> Vec<Value> {
    vec![json!({"jsonrpc":"2.0", "id":1, "result":{"protocolVersion":"unsupported"}})]
}

fn failed_preparation(script: Answers, required: bool, expected_cause: &str) {
    let sandbox = Pretend::new([
        Answers::Says(dialogue("docs")),
        script,
        Answers::Says(dialogue("later")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![
            chosen("docs"),
            chosen("broken").required(required),
            chosen("later"),
        ],
    );
    let context = lifecycle();
    let error = hosting
        .prepare(&context)
        .expect_err("uncertain cleanup is never optional");
    let message = error.to_string();
    assert!(message.contains("broken"), "{message}");
    assert!(message.contains(expected_cause), "{message}");
    assert!(message.contains("unconfirmed cleanup"), "{message}");
    assert_eq!(sandbox.started(), 2, "no later server can start");
    assert_eq!(sandbox.server(0).stops(), 1, "earlier peers are stopped");
    assert_eq!(sandbox.server(1).stop_attempts.load(Ordering::Relaxed), 1);
    assert!(hosting.snapshot(&context).unwrap().entries().is_empty());
    assert!(hosting.dispose(&context).is_err());
    assert!(hosting.dispose(&context).is_err());
    assert!(hosting.prepare(&context).is_err());
    assert_eq!(sandbox.started(), 2);
}

#[test]
fn optional_bad_greeting_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(bad_greeting()),
        false,
        "MCP version unsupported",
    );
}

#[test]
fn required_bad_greeting_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(bad_greeting()),
        true,
        "MCP version unsupported",
    );
}

#[test]
fn optional_bad_catalogue_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(opening("broken", &json!(false))),
        false,
        "without tools",
    );
}

#[test]
fn required_bad_catalogue_retains_unconfirmed_cleanup() {
    failed_preparation(
        Answers::Unreapable(opening("broken", &json!(false))),
        true,
        "without tools",
    );
}

#[test]
fn failed_restart_handshake_retains_unconfirmed_cleanup() {
    let sandbox = Pretend::new([
        Answers::Says(dialogue("docs")),
        Answers::Unreapable(bad_greeting()),
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
    assert!(error.to_string().contains("unconfirmed cleanup"), "{error}");
    assert!(
        error.to_string().contains("MCP version unsupported"),
        "{error}"
    );
    assert_eq!(sandbox.started(), 2);
    assert_eq!(sandbox.server(1).stop_attempts.load(Ordering::Relaxed), 1);
    assert!(hosting.dispose(&context).is_err());
    assert!(hosting.dispose(&context).is_err());
    assert!(hosting.prepare(&context).is_err());
    assert_eq!(sandbox.started(), 2);
}

#[test]
fn optional_missing_input_retains_unconfirmed_cleanup() {
    failed_preparation(Answers::MissingInput, false, "input");
}

#[test]
fn optional_missing_output_retains_unconfirmed_cleanup() {
    failed_preparation(Answers::MissingOutput, false, "output");
}

#[test]
fn optional_bad_greeting_with_confirmed_cleanup_still_allows_later_server() {
    let sandbox = Pretend::new([
        Answers::Says(bad_greeting()),
        Answers::Says(dialogue("later")),
    ]);
    let hosting = Hosting::new(
        builtin(&[]),
        sandbox.clone() as Arc<dyn SandboxService>,
        vec![optional("broken"), chosen("later")],
    );
    let context = lifecycle();
    hosting.prepare(&context).unwrap();
    assert_eq!(sandbox.started(), 2);
    assert_eq!(
        sandbox.server(0).stops(),
        1,
        "cleanup confirmed before continuing"
    );
    assert!(
        hosting
            .snapshot(&context)
            .unwrap()
            .find("mcp:later/search")
            .is_some()
    );
    hosting.dispose(&context).unwrap();
    hosting.dispose(&context).unwrap();
}
