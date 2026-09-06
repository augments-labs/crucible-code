//! Server-side results must resolve one earlier call of the same kind exactly once.

use super::{PROTOCOL, body, wire};
use crate::{sse::SseEvent, stream::Wire};
use crucible_core::{
    Continuation, ContinuationData, ContinuationPart, ContinuationScope, Message, Request,
    RequestPurpose, StopReason, ToolArgs, ToolCall, ToolId, Transcript,
};
use serde_json::{Value, json};

fn scope() -> ContinuationScope {
    ContinuationScope::from_digest([1; 32])
}

fn invalid() -> Vec<Vec<Value>> {
    let call = json!({"type":"url_context_call","id":"native","arguments":{"urls":[]}});
    let result = json!({"type":"url_context_result","call_id":"native","result":[]});
    let function = json!({"type":"function_call","id":"native","name":"read","arguments":{}});
    vec![
        vec![result.clone()],
        vec![call.clone()],
        vec![
            call.clone(),
            json!({"type":"url_context_result","call_id":"other","result":[]}),
        ],
        vec![
            call.clone(),
            json!({"type":"google_search_result","call_id":"native","result":[]}),
        ],
        vec![call.clone(), result.clone(), result.clone()],
        vec![call.clone(), result.clone(), call.clone(), result.clone()],
        vec![call.clone(), call.clone(), result.clone()],
        vec![function.clone(), call.clone(), result.clone()],
        vec![call, result, function],
    ]
}

#[test]
fn native_call_pairing_is_required_before_stream_continuation() {
    let mut accepted = Vec::new();
    for (case, steps) in invalid().into_iter().enumerate() {
        let mut wire = wire::Interactions::new("gemini-3.8-flash", scope()).unwrap();
        let mut status = "completed";
        let result = steps
            .into_iter()
            .enumerate()
            .try_for_each(|(index, step)| {
                if step.get("type").and_then(Value::as_str) == Some("function_call") {
                    status = "requires_action";
                }
                for value in [
                    json!({"event_type":"step.start","index":index,"step":step}),
                    json!({"event_type":"step.stop","index":index}),
                ] {
                    wire.deltas(&SseEvent {
                        name: String::new(),
                        data: value.to_string(),
                    })?;
                }
                Ok::<_, crucible_core::ProviderError>(())
            })
            .and_then(|()| {
                wire.deltas(&SseEvent {
                    name: String::new(),
                    data: json!({
                        "event_type":"interaction.completed","interaction":{"status":status}
                    })
                    .to_string(),
                })
            });
        if result.is_ok() {
            accepted.push(case);
        }
    }
    assert!(
        accepted.is_empty(),
        "invalid native pairs accepted: {accepted:?}"
    );
}

#[test]
fn native_call_pairing_is_revalidated_before_local_replay() {
    let mut accepted = Vec::new();
    for (case, steps) in invalid().into_iter().enumerate() {
        let mut state = Continuation::new(PROTOCOL, "gemini-3.8-flash", scope()).unwrap();
        let mut calls = Vec::new();
        for mut step in steps {
            let part = if step.get("type").and_then(Value::as_str) == Some("function_call") {
                let fields = step.as_object_mut().unwrap();
                let id = fields.remove("id").unwrap();
                let name = fields.remove("name").unwrap();
                let args = fields.remove("arguments").unwrap();
                let index = calls.len();
                calls.push(ToolCall {
                    id: ToolId::new(id.as_str().unwrap()),
                    name: name.as_str().unwrap().into(),
                    args: ToolArgs::new(args.to_string()),
                });
                ContinuationPart::Call {
                    index,
                    data: ContinuationData::new(&step.to_string()).unwrap(),
                }
            } else {
                ContinuationPart::Opaque(ContinuationData::new(&step.to_string()).unwrap())
            };
            state.push(part).unwrap();
        }
        let stop = if calls.is_empty() {
            StopReason::Yielded
        } else {
            StopReason::WantsTools
        };
        let state = state.finish("", calls.len(), Some(stop)).unwrap();
        let mut transcript = Transcript::new();
        transcript
            .push(Message::Agent {
                text: "".into(),
                calls,
                stop: Some(stop),
                continuation: Some(state),
            })
            .unwrap();
        let result = body::serialize(
            &Request {
                purpose: RequestPurpose::Turn,
                model: "gemini-3.8-flash",
                transcript: &transcript,
                tools: &[],
                max_tokens: 1024,
                system: None,
                effort: None,
                attached: &[],
                prompt_cache: None,
            },
            scope(),
        );
        if result.is_ok() {
            accepted.push(case);
        }
    }
    assert!(
        accepted.is_empty(),
        "invalid native pairs replayed: {accepted:?}"
    );
}
