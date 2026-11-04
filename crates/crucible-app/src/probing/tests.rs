//! What the intake keeps of an agent's answer is bounded, says when it cut
//! any of it, and reads a method by what the agent says it is.

use serde_json::{Value, json};

use super::*;

/// The value `recorded` holds at the JSON pointer `at`, null where it holds
/// none.
fn at<'a>(recorded: &'a Value, at: &str) -> &'a Value {
    recorded.pointer(at).unwrap_or(&Value::Null)
}

/// What is recorded of `result`, as an agent held in by a closed sandbox.
fn recorded_from(result: &Value) -> Value {
    let said = json!({"jsonrpc": "2.0", "id": 0, "result": result});
    let Value::Object(said) = said else {
        panic!("an object");
    };
    recorded(&Sandbox::Closed, &said).expect("recorded")
}

#[test]
fn an_answer_within_every_bound_is_recorded_whole_and_not_marked_incomplete() {
    let recorded = recorded_from(&json!({
        "agentInfo": {"name": "agent", "version": "1"},
        "authMethods": [{"id": "key", "name": "Key", "type": "env_var"}],
    }));

    assert_eq!(at(&recorded, "/authMethods/0/kind"), "env_var");
    assert_eq!(at(&recorded, "/agentInfo/version"), "1");
    assert_eq!(recorded.get("incomplete"), None);
}

#[test]
fn methods_past_the_most_kept_are_left_out_and_the_record_says_so() {
    let methods: Vec<Value> = (0..ITEMS + 4)
        .map(|n| json!({"id": format!("m{n}"), "name": "Method"}))
        .collect();

    let recorded = recorded_from(&json!({"authMethods": methods}));

    assert_eq!(
        at(&recorded, "/authMethods").as_array().map(Vec::len),
        Some(ITEMS)
    );
    assert_eq!(at(&recorded, "/incomplete"), true);
}

#[test]
fn a_name_longer_than_the_most_kept_is_cut_on_a_character_boundary() {
    let name = "é".repeat(TEXT_BYTES);

    let recorded = recorded_from(&json!({"agentInfo": {"name": name}}));

    let kept = at(&recorded, "/agentInfo/name").as_str().expect("text");
    assert!(kept.len() <= TEXT_BYTES, "{}", kept.len());
    assert!(name.starts_with(kept));
    assert_eq!(at(&recorded, "/incomplete"), true);
    assert_eq!(at(&recorded, "/agentInfo/version"), &Value::Null);
}

#[test]
fn capabilities_past_their_ceiling_are_left_out_and_the_record_says_so() {
    let big = json!({"experimental": "x".repeat(CAPABILITY_BYTES)});

    let recorded = recorded_from(&json!({"agentCapabilities": big}));

    assert_eq!(at(&recorded, "/capabilities"), &Value::Null);
    assert_eq!(at(&recorded, "/incomplete"), true);
}

#[test]
fn a_terminal_method_says_it_needs_a_real_terminal_only_where_the_agent_says_so() {
    let recorded = recorded_from(&json!({"authMethods": [
        {"id": "a", "type": "terminal", "_meta": {"tty": true, "label": "Log in"}},
        {"id": "b", "type": "terminal"},
        {"id": "c", "type": "mystery"},
    ]}));

    assert_eq!(at(&recorded, "/authMethods/0/tty"), true);
    assert_eq!(at(&recorded, "/authMethods/0/meta/label"), "Log in");
    assert_eq!(at(&recorded, "/authMethods/1/tty"), "unknown");
    assert_eq!(recorded.pointer("/authMethods/1/meta"), None);
    assert_eq!(at(&recorded, "/authMethods/2/kind"), "other");
    assert_eq!(recorded.pointer("/authMethods/2/tty"), None);
}

#[test]
fn an_agent_that_refuses_initialize_is_told_with_what_it_said() {
    let said = json!({"jsonrpc": "2.0", "id": 0, "error": {"code": -1, "message": "no"}});
    let Value::Object(said) = said else {
        panic!("an object");
    };

    let refused = recorded(&Sandbox::Closed, &said).expect_err("refused");

    assert_eq!(refused.to_string(), "the agent refused initialize: no");
}
