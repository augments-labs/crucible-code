//! Function results resolve the immediately preceding native call group once.

use crucible_core::{
    Continuation, ContinuationData, ContinuationPart, ContinuationScope, Message, Request,
    RequestPurpose, StopReason, ToolArgs, ToolCall, ToolId, ToolOutput, ToolResult, Transcript,
};
use serde_json::Value;

fn scope() -> ContinuationScope {
    ContinuationScope::from_digest([1; 32])
}

fn calling(ids: &[&str]) -> Message {
    let mut state = Continuation::new(super::super::PROTOCOL, "gemini-3.8-flash", scope()).unwrap();
    let calls = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            state
                .push(ContinuationPart::Call {
                    index,
                    data: ContinuationData::new(r#"{"type":"function_call"}"#).unwrap(),
                })
                .unwrap();
            ToolCall {
                id: ToolId::new(*id),
                name: "read".into(),
                args: ToolArgs::new("{}"),
            }
        })
        .collect();
    Message::Agent {
        text: "".into(),
        calls,
        stop: Some(StopReason::WantsTools),
        continuation: Some(
            state
                .finish("", ids.len(), Some(StopReason::WantsTools))
                .unwrap(),
        ),
    }
}

fn results(ids: &[&str]) -> Message {
    Message::ToolResults(
        ids.iter()
            .map(|id| ToolResult {
                id: ToolId::new(*id),
                output: ToolOutput::ok("private-result-canary"),
            })
            .collect(),
    )
}

fn serialize(
    messages: Vec<Message>,
    purpose: RequestPurpose,
    recipient: ContinuationScope,
) -> Result<String, crucible_core::ProviderError> {
    let mut transcript = Transcript::new();
    for message in messages {
        transcript.push(message).unwrap();
    }
    super::serialize(
        &Request {
            purpose,
            model: "gemini-3.1-pro-preview",
            transcript: &transcript,
            tools: &[],
            max_tokens: 1024,
            system: None,
            effort: None,
            attached: &[],
            prompt_cache: None,
        },
        recipient,
    )
}

#[test]
fn native_function_results_must_answer_the_whole_call_group_exactly_once() {
    let cases = [
        vec![calling(&["a"]), results(&["a", "a"])],
        vec![calling(&["a"]), results(&["a"]), results(&["a"])],
        vec![calling(&["a"]), results(&["other"])],
        vec![calling(&["a", "b"]), results(&["a"])],
        vec![calling(&["a"])],
        vec![
            calling(&["a"]),
            Message::said("interrupted"),
            results(&["a"]),
        ],
        vec![calling(&["a"]), calling(&["b"]), results(&["b"])],
    ];
    let mut accepted = Vec::new();
    for (case, messages) in cases.into_iter().enumerate() {
        match serialize(messages, RequestPurpose::Turn, scope()) {
            Ok(_) => accepted.push(case),
            Err(error) => assert!(!format!("{error:?}").contains("private-result-canary")),
        }
    }
    assert!(
        accepted.is_empty(),
        "invalid function results replayed: {accepted:?}"
    );
}

#[test]
fn native_function_results_allow_split_reordered_groups_and_reused_ids_in_later_turns() {
    let body = serialize(
        vec![
            calling(&["a", "b"]),
            results(&["b"]),
            results(&["a"]),
            Message::said("again"),
            calling(&["a"]),
            results(&["a"]),
        ],
        RequestPurpose::Turn,
        scope(),
    )
    .unwrap();
    let value: Value = serde_json::from_str(&body).unwrap();
    let steps = value.get("input").unwrap().as_array().unwrap();
    let ids: Vec<_> = steps
        .iter()
        .filter_map(|step| step.get("call_id").and_then(Value::as_str))
        .collect();
    assert_eq!(ids, ["b", "a", "a"]);
}

#[test]
fn incomplete_foreign_and_recap_call_history_stays_neutral_context() {
    for (purpose, recipient) in [
        (RequestPurpose::Recap, scope()),
        (
            RequestPurpose::Turn,
            ContinuationScope::from_digest([2; 32]),
        ),
    ] {
        let body = serialize(
            vec![calling(&["a"]), results(&["other"])],
            purpose,
            recipient,
        )
        .unwrap();
        let value: Value = serde_json::from_str(&body).unwrap();
        assert!(
            value
                .get("input")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .all(|step| { step.get("type").and_then(Value::as_str) == Some("user_input") })
        );
    }
}

#[test]
fn unknown_gemini_names_do_not_make_private_state_compatible() {
    for model in ["gemini-unknown", "gemini-2.5-flash"] {
        let mut state = Continuation::new(super::super::PROTOCOL, model, scope()).unwrap();
        state
            .push(ContinuationPart::Opaque(
                ContinuationData::new(
                    r#"{"type":"thought","signature":"unreviewed-model-private-state"}"#,
                )
                .unwrap(),
            ))
            .unwrap();
        let body = serialize(
            vec![Message::Agent {
                text: "".into(),
                calls: vec![],
                stop: Some(StopReason::Yielded),
                continuation: Some(state.finish("", 0, Some(StopReason::Yielded)).unwrap()),
            }],
            RequestPurpose::Turn,
            scope(),
        )
        .unwrap();
        assert!(!body.contains("unreviewed-model-private-state"), "{model}");
    }
}
