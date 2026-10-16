//! Every update kind is read, and a turn's cost is what it added.

use crucible_execution::{About, Cost, Currency, Event};
use crucible_types::TurnId;
use serde_json::{Value, json};

use crate::Act;
use crate::acp::session::{Kind, Updates};

/// Every `sessionUpdate` the protocol's schema defines, written out here
/// rather than taken from the code under test.
const PROTOCOL_KINDS: [&str; 15] = [
    "user_message_chunk",
    "agent_message_chunk",
    "agent_thought_chunk",
    "tool_call",
    "tool_call_update",
    "plan",
    "plan_update",
    "plan_removed",
    "available_commands_update",
    "current_mode_update",
    "config_option_update",
    "session_info_update",
    "usage_update",
    "compaction_update",
    "compaction_summary_chunk",
];

/// A running cost of `amount` dollars.
fn usage(amount: f64) -> Value {
    json!({"sessionUpdate": "usage_update", "used": 1, "size": 2, "cost": {"amount": amount, "currency": "USD"}})
}

/// The cost a turn's usage reported, where it reported one.
fn spent(act: Option<Act>) -> Option<u64> {
    match act {
        Some(Act::Say(Event::Usage(observed))) => observed.value().spent().map(Cost::millionths),
        Some(other) => panic!("not usage: {other:?}"),
        None => None,
    }
}

/// Each kind the protocol defines is one this reader names, under its own
/// name, and reading one never fails.
#[test]
fn every_update_kind_the_protocol_defines_has_an_arm() {
    let named: Vec<&str> = Kind::ALL.iter().map(|kind| kind.name()).collect();
    assert_eq!(named, PROTOCOL_KINDS);
    for name in PROTOCOL_KINDS {
        let kind = Kind::named(name);
        assert_eq!(kind.map(Kind::name), Some(name));
        let mut updates = Updates::fresh();
        let _ = updates.heard(&json!({"sessionUpdate": name}));
    }
    assert_eq!(Kind::named("not_a_kind"), None);
    assert!(
        Updates::fresh()
            .heard(&json!({"sessionUpdate": "not_a_kind"}))
            .is_empty()
    );
}

/// A fresh session's first turn reports what it cost, and the second only
/// what it added.
#[test]
fn the_first_turn_reports_its_cost_and_the_second_the_difference() {
    let mut updates = Updates::fresh();
    let _ = updates.heard(&usage(0.25));
    let first = spent(updates.settled(TurnId::FIRST));
    let _ = updates.heard(&usage(0.4));
    let _ = updates.heard(&usage(0.75));
    let second = spent(updates.settled(TurnId::FIRST.next()));

    assert_eq!(first, Some(250_000));
    assert_eq!(second, Some(500_000));
}

/// A resumed session's cost so far is unknown, so its first turn reports no
/// figure, and the turn after it reports what it added.
#[test]
fn a_turn_after_resuming_with_no_remembered_figure_reports_none() {
    let mut updates = Updates::resumed();
    let _ = updates.heard(&usage(3.0));
    let first = spent(updates.settled(TurnId::FIRST));
    let _ = updates.heard(&usage(3.5));
    let second = spent(updates.settled(TurnId::FIRST.next()));

    assert_eq!(first, None);
    assert_eq!(second, Some(500_000));
}

/// A cost that is negative, not finite, in another currency or shrinking
/// gives no figure rather than a wrong one.
#[test]
fn a_cost_that_cannot_be_differenced_gives_no_figure() {
    let mut negative = Updates::fresh();
    let _ = negative.heard(&usage(-1.0));
    assert_eq!(spent(negative.settled(TurnId::FIRST)), None);

    let mut shrinking = Updates::fresh();
    let _ = shrinking.heard(&usage(2.0));
    let _ = shrinking.settled(TurnId::FIRST);
    let _ = shrinking.heard(&usage(1.0));
    assert_eq!(spent(shrinking.settled(TurnId::FIRST.next())), None);

    let mut changed = Updates::fresh();
    let _ = changed.heard(&usage(2.0));
    let _ = changed.settled(TurnId::FIRST);
    let _ = changed.heard(
        &json!({"sessionUpdate": "usage_update", "cost": {"amount": 3.0, "currency": "EUR"}}),
    );
    assert_eq!(spent(changed.settled(TurnId::FIRST.next())), None);
    assert!(Currency::new("EUR").is_some());
}

/// An update of a kind the protocol does not define, or of no kind, is
/// dropped and counted.
#[test]
fn an_update_of_a_kind_not_defined_is_dropped_and_counted() {
    let mut updates = Updates::fresh();
    let unknown = updates.heard(&json!({"sessionUpdate": "not_a_kind"}));
    let unnamed = updates.heard(&json!({"content": "text"}));
    let _ = updates.heard(&usage(0.25));

    assert!(unknown.is_empty() && unnamed.is_empty());
    assert_eq!(updates.unknown, 2);
}

/// A call is reported under the agent's id for it, and its end under the same
/// id with whether it failed, even where it said nothing; a call still
/// running reports no end.
#[test]
fn a_call_and_its_end_are_reported_under_the_call_s_id() {
    let mut updates = Updates::fresh();
    let begun = updates.heard(&json!({
        "sessionUpdate": "tool_call", "toolCallId": "call_1", "title": "Run ls", "kind": "execute", "status": "pending",
    }));
    let running = updates.heard(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "in_progress",
    }));
    let done = updates.heard(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call_1", "status": "completed",
        "content": [{"type": "content", "content": {"type": "text", "text": "a\nb"}}],
    }));
    let failed = updates.heard(&json!({
        "sessionUpdate": "tool_call_update", "toolCallId": "call_2", "status": "failed",
    }));

    assert!(
        matches!(
            begun.as_slice(),
            [Act::Say(Event::ToolActivity { call, tool, summary, about: About::Other })]
                if call.as_str() == "call_1" && tool.as_str() == "execute" && summary.as_str() == "Run ls"
        ),
        "{begun:?}"
    );
    assert!(running.is_empty(), "{running:?}");
    assert!(
        matches!(
            done.as_slice(),
            [Act::Say(Event::ToolReport { call, failed: false, report })]
                if call.as_str() == "call_1" && report.as_str() == "a\nb"
        ),
        "{done:?}"
    );
    assert!(
        matches!(
            failed.as_slice(),
            [Act::Say(Event::ToolReport { call, failed: true, report })]
                if call.as_str() == "call_2" && report.as_str().is_empty()
        ),
        "{failed:?}"
    );
}
