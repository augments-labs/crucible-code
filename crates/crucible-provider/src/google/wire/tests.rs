//! Independent wire fixtures assert semantic order, not network arrival order.

use super::*;
use crucible_core::{ContinuationPart, StopReason};
use serde_json::{Value, json};

fn parser() -> Interactions {
    Interactions::new("gemini-3.8-flash", ContinuationScope::from_digest([7; 32])).unwrap()
}

fn send(wire: &mut Interactions, event: &Value) -> Result<Vec<Delta>, ProviderError> {
    wire.deltas(&SseEvent {
        name: event.get("event_type").unwrap().as_str().unwrap().into(),
        data: event.to_string(),
    })
}

#[test]
fn documented_gateway_timeouts_are_transient_without_exposing_error_payloads() {
    for (code, transient) in [
        ("gateway_timeout", true),
        ("invalid_request", false),
        ("private-code-canary", false),
    ] {
        let error = send(
            &mut parser(),
            &json!({"event_type":"error","error":{
                "code":code,"message":"private-signature-canary"
            }}),
        )
        .unwrap_err();
        assert_eq!(error.transient(), transient, "{code}");
        assert!(!format!("{error:?} {error}").contains("private-signature-canary"));
        assert!(!format!("{error:?} {error}").contains("private-code-canary"));
    }
}

#[test]
fn initial_text_is_streamed_before_stop_and_thought_is_private() {
    let mut wire = parser();
    let start = send(&mut wire, &json!({"event_type":"step.start","index":0,"step":{"type":"model_output","content":[{"type":"text","text":"hé"}]}})).unwrap();
    assert!(matches!(&start[..], [Delta::Text(text)] if text.as_ref() == "hé"));
    let more = send(
        &mut wire,
        &json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"llo"}}),
    )
    .unwrap();
    assert!(matches!(&more[..], [Delta::Text(text)] if text.as_ref() == "llo"));
    send(&mut wire, &json!({"event_type":"step.stop","index":0})).unwrap();
    let thought = send(&mut wire, &json!({"event_type":"step.start","index":1,"step":{"type":"thought","signature":"secret-signature","summary":[{"type":"text","text":"private-summary"}]}})).unwrap();
    assert!(thought.iter().all(|d| matches!(d, Delta::Progress)));
    send(&mut wire, &json!({"event_type":"step.stop","index":1})).unwrap();
    let ended = send(
        &mut wire,
        &json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
    )
    .unwrap();
    assert!(matches!(
        ended.last(),
        Some(Delta::Stopped(StopReason::Yielded))
    ));
    let state = ended
        .into_iter()
        .find_map(|d| {
            if let Delta::Continuation(c) = d {
                Some(c)
            } else {
                None
            }
        })
        .unwrap()
        .finish("héllo", 0, Some(StopReason::Yielded))
        .unwrap();
    assert!(matches!(
        state.parts().get(1).unwrap(),
        ContinuationPart::Text {
            start: 0,
            end: 6,
            ..
        }
    ));
    assert!(
        matches!(state.parts().last(),Some(ContinuationPart::Opaque(d)) if serde_json::from_str::<Value>(d.as_str()).unwrap() == json!({"type":"thought","signature":"secret-signature","summary":[{"type":"text","text":"private-summary"}]}))
    );
    assert!(!format!("{state:?}").contains("secret-signature"));
}

#[test]
fn interleaved_calls_are_delivered_with_complete_arguments_in_index_order() {
    let mut wire = parser();
    let mut deltas = vec![];
    for event in [
        json!({"event_type":"step.start","index":0,"step":{"type":"function_call","id":"one","name":"read","arguments":{}}}),
        json!({"event_type":"step.start","index":1,"step":{"type":"function_call","id":"two","name":"list","arguments":{}}}),
        json!({"event_type":"step.delta","index":1,"delta":{"type":"arguments_delta","arguments":"{\"path\":\"b\"}"}}),
        json!({"event_type":"step.stop","index":1}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":"{\"path\":"}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"arguments_delta","arguments":"\"a\"}"}}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"interaction.completed","interaction":{"status":"requires_action"}}),
    ] {
        deltas.extend(send(&mut wire, &event).unwrap());
    }
    let calls: Vec<_> = deltas
        .iter()
        .filter_map(|d| match d {
            Delta::ToolStarted { id, name } => Some((id.as_str(), name.as_ref())),
            _ => None,
        })
        .collect();
    assert_eq!(calls, [("one", "read"), ("two", "list")]);
    let args: Vec<Value> = deltas
        .iter()
        .filter_map(|d| {
            if let Delta::ToolArgs(s) = d {
                Some(serde_json::from_str(s).unwrap())
            } else {
                None
            }
        })
        .collect();
    assert_eq!(args, [json!({"path":"a"}), json!({"path":"b"})]);
    let state = deltas
        .into_iter()
        .find_map(|d| {
            if let Delta::Continuation(c) = d {
                Some(c)
            } else {
                None
            }
        })
        .unwrap()
        .finish("", 2, Some(StopReason::WantsTools))
        .unwrap();
    assert_eq!(state.parts().len(), 2);
}

#[test]
fn multipart_text_and_native_tool_arguments_survive_without_regrouping() {
    let mut wire = parser();
    for event in [
        json!({"event_type":"step.start","index":0,"step":{"type":"model_output","content":[{"type":"text","text":"first"},{"type":"text","text":"second"}]}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"text","text":"!"}}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"step.start","index":1,"step":{"type":"google_search_call","id":"native","arguments":{"queries":["one"]},"signature":"native-signature"}}),
        json!({"event_type":"step.stop","index":1}),
        json!({"event_type":"step.start","index":2,"step":{"type":"google_search_result","call_id":"native","result":[]}}),
        json!({"event_type":"step.stop","index":2}),
    ] {
        send(&mut wire, &event).unwrap();
    }
    let deltas = send(
        &mut wire,
        &json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
    )
    .unwrap();
    let state = deltas
        .into_iter()
        .find_map(|d| {
            if let Delta::Continuation(c) = d {
                Some(c)
            } else {
                None
            }
        })
        .unwrap()
        .finish("firstsecond!", 0, Some(StopReason::Yielded))
        .unwrap();
    assert!(matches!(
        state.parts().get(1).unwrap(),
        ContinuationPart::Text {
            start: 0,
            end: 5,
            ..
        }
    ));
    assert!(matches!(
        state.parts().get(2).unwrap(),
        ContinuationPart::Text {
            start: 5,
            end: 12,
            ..
        }
    ));
    assert!(
        matches!(state.parts().get(3).unwrap(),ContinuationPart::Opaque(data) if serde_json::from_str::<Value>(data.as_str()).unwrap().get("arguments") == Some(&json!({"queries":["one"]})))
    );
}

#[test]
fn google_usage_adds_disjoint_thought_once_and_keeps_unknown_fields_unknown() {
    let deltas = send(&mut parser(),&json!({"event_type":"interaction.completed","interaction":{"status":"completed","usage":{"total_input_tokens":100,"total_cached_tokens":60,"total_output_tokens":7,"total_thought_tokens":11,"total_tool_use_tokens":3,"total_tokens":121}}})).unwrap();
    let usage = deltas
        .into_iter()
        .find_map(|d| {
            if let Delta::Usage(u) = d {
                Some(u)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(usage.input.total, Some(100));
    assert_eq!(usage.input.uncached, Some(40));
    assert_eq!(usage.output, Some(18));
    assert_eq!(usage.reasoning, Some(11));
    assert_eq!(
        usage.total,
        Some(118),
        "Google's raw total can include internal/tool prompt tokens"
    );
    let deltas = send(&mut parser(),&json!({"event_type":"interaction.completed","interaction":{"status":"completed","usage":{"total_output_tokens":7}}})).unwrap();
    let usage = deltas
        .into_iter()
        .find_map(|d| {
            if let Delta::Usage(u) = d {
                Some(u)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(usage.output, None, "missing thought count is not zero");
    assert_eq!(usage.input.total, None);
    for usage in [
        json!({"total_output_tokens":u64::MAX,"total_thought_tokens":1}),
        json!({"total_input_tokens":3,"total_cached_tokens":4}),
        json!({"total_input_tokens":-1}),
    ] {
        assert!(send(&mut parser(),&json!({"event_type":"interaction.completed","interaction":{"status":"completed","usage":usage}})).is_err());
    }
}

#[test]
fn function_arguments_share_one_exact_response_budget() {
    use crucible_core::TOOL_ARGUMENT_BYTES;

    for streamed in [false, true] {
        for excess in [0, 1] {
            let mut wire = parser();
            // Two individually valid calls exhaust the aggregate byte budget.
            // JSON punctuation is eight bytes per object, not retained-tree weight.
            let first = "x".repeat(TOOL_ARGUMENT_BYTES / 2 - 8);
            let second = "y".repeat(TOOL_ARGUMENT_BYTES / 2 - 8 + excess);
            for (index, value) in [first, second].into_iter().enumerate() {
                let arguments = json!({"v":value});
                let result = send(
                    &mut wire,
                    &json!({"event_type":"step.start","index":index,"step":{
                        "type":"function_call","id":format!("call-{index}"),"name":"read",
                        "arguments":if streamed { json!({}) } else { arguments.clone() }
                    }}),
                );
                let result = if streamed {
                    result.unwrap();
                    send(
                        &mut wire,
                        &json!({"event_type":"step.delta","index":index,"delta":{
                            "type":"arguments_delta","arguments":arguments.to_string()
                        }}),
                    )
                } else {
                    result
                };
                if excess == 1 && index == 1 {
                    assert!(
                        result.is_err(),
                        "cap+1 must fail before retaining the event"
                    );
                } else {
                    result.unwrap();
                    send(&mut wire, &json!({"event_type":"step.stop","index":index})).unwrap();
                }
            }
            if excess == 0 {
                let deltas = send(&mut wire, &json!({"event_type":"interaction.completed","interaction":{"status":"requires_action"}})).unwrap();
                assert!(deltas.iter().any(|d| matches!(d, Delta::Continuation(_))));
            }
        }
    }
}

#[test]
fn function_call_admission_rejects_the_129th_call() {
    let mut wire = parser();
    for index in 0..128 {
        send(
            &mut wire,
            &json!({"event_type":"step.start","index":index,"step":{
                "type":"function_call","id":format!("call-{index}"),"name":"read","arguments":{}
            }}),
        )
        .unwrap();
        send(&mut wire, &json!({"event_type":"step.stop","index":index})).unwrap();
    }
    assert!(
        send(
            &mut wire,
            &json!({"event_type":"step.start","index":128,"step":{
                "type":"function_call","id":"one-too-many","name":"read","arguments":{}
            }})
        )
        .is_err()
    );
}

#[test]
fn function_call_admission_rejects_duplicate_identities() {
    for close_first in [false, true] {
        let mut wire = parser();
        send(
            &mut wire,
            &json!({"event_type":"step.start","index":0,"step":{
                "type":"function_call","id":"same-call","name":"read","arguments":{}
            }}),
        )
        .unwrap();
        if close_first {
            send(&mut wire, &json!({"event_type":"step.stop","index":0})).unwrap();
        }
        assert!(
            send(
                &mut wire,
                &json!({"event_type":"step.start","index":1,"step":{
                    "type":"function_call","id":"same-call","name":"write","arguments":{}
                }})
            )
            .is_err(),
            "duplicate identities must not reach another tool"
        );
    }
}

#[test]
fn native_identity_deltas_cannot_change_the_call_they_belong_to() {
    for (kind, field, payload) in [
        (
            "url_context_call",
            "id",
            json!({"arguments":{"urls":["https://example.com"]}}),
        ),
        ("url_context_result", "call_id", json!({"result":[]})),
    ] {
        let mut step = payload;
        let fields = step.as_object_mut().unwrap();
        fields.insert("type".into(), json!(kind));
        fields.insert(field.into(), json!("original"));
        let mut wire = parser();
        send(
            &mut wire,
            &json!({"event_type":"step.start","index":0,"step":step}),
        )
        .unwrap();
        let mut delta = json!({"type":kind});
        delta
            .as_object_mut()
            .unwrap()
            .insert(field.into(), json!("different"));
        assert!(
            send(
                &mut wire,
                &json!({"event_type":"step.delta","index":0,"delta":delta})
            )
            .is_err()
        );
    }
}

#[test]
fn repeated_native_identity_is_not_concatenated_like_a_signature() {
    let mut wire = parser();
    for event in [
        json!({"event_type":"step.start","index":0,"step":{"type":"url_context_call","id":"native","arguments":{"urls":[]},"signature":"first-"}}),
        json!({"event_type":"step.delta","index":0,"delta":{"type":"url_context_call","id":"native","arguments":{"urls":["https://example.com"]},"signature":"second"}}),
        json!({"event_type":"step.stop","index":0}),
        json!({"event_type":"step.start","index":1,"step":{"type":"url_context_result","call_id":"native","result":[]}}),
        json!({"event_type":"step.stop","index":1}),
    ] {
        send(&mut wire, &event).unwrap();
    }
    let deltas = send(
        &mut wire,
        &json!({"event_type":"interaction.completed","interaction":{"status":"completed"}}),
    )
    .unwrap();
    let state = deltas
        .into_iter()
        .find_map(|delta| {
            if let Delta::Continuation(state) = delta {
                Some(state)
            } else {
                None
            }
        })
        .unwrap()
        .finish("", 0, Some(StopReason::Yielded))
        .unwrap();
    let Some(ContinuationPart::Opaque(data)) = state.parts().first() else {
        panic!("native tool state is missing");
    };
    assert_eq!(
        serde_json::from_str::<Value>(data.as_str()).unwrap(),
        json!({
            "type":"url_context_call","id":"native","arguments":{"urls":["https://example.com"]},"signature":"first-second"
        })
    );
}
