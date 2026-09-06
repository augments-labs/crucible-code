//! The live wire and resumed private data enforce the same native-step shapes.

use super::{PROTOCOL, body, wire};
use crate::{sse::SseEvent, stream::Wire};
use crucible_core::{
    Continuation, ContinuationData, ContinuationPart, ContinuationScope, Message, Request,
    RequestPurpose, StopReason, Transcript,
};
use serde_json::{Value, json};

fn malformed() -> Vec<Value> {
    vec![
        json!({"type":"thought","signature":7}),
        json!({"type":"thought","summary":"private-canary"}),
        json!({"type":"thought","summary":[{"type":"text","text":7}]}),
        json!({"type":"thought","summary":[{"type":"image","data":7}]}),
        json!({"type":"thought","summary":[{"type":"function_call","arguments":{}}]}),
        json!({"type":"google_search_call","arguments":{}}),
        json!({"type":"google_search_call","id":"a","arguments":[]} ),
        json!({"type":"google_search_call","id":"a","arguments":{"queries":[7]}}),
        json!({"type":"google_search_call","id":"a","arguments":{},"search_type":7}),
        json!({"type":"google_search_result","call_id":"a","result":{}}),
        json!({"type":"google_search_result","call_id":"a","result":[7]}),
        json!({"type":"google_search_result","call_id":"a","result":[{"search_suggestions":7}]}),
        json!({"type":"url_context_call","id":"","arguments":{}}),
        json!({"type":"url_context_call","id":"a","arguments":{"urls":[7]}}),
        json!({"type":"url_context_result","result":[]}),
        json!({"type":"url_context_result","call_id":"a","result":[{"url":7}]}),
        json!({"type":"url_context_result","call_id":"a","result":[],"is_error":"false"}),
        json!({"type":"code_execution_call","id":"a","arguments":{"code":7}}),
        json!({"type":"code_execution_result","call_id":"a","result":[]}),
        json!({"type":"processing_call","id":"a\nprivate-canary"}),
        json!({"type":"processing_result","call_id":7}),
    ]
}

#[test]
fn malformed_native_steps_cannot_finish_into_continuation() {
    let mut admitted = Vec::new();
    for (index, step) in malformed().into_iter().enumerate() {
        let mut wire = wire::Interactions::new("gemini-3.8-flash", scope()).unwrap();
        let result = [
            json!({"event_type":"step.start","index":0,"step":step}),
            json!({"event_type":"step.stop","index":0}),
        ]
        .into_iter()
        .try_for_each(|value| {
            wire.deltas(&SseEvent {
                name: String::new(),
                data: value.to_string(),
            })
            .map(|_| ())
        });
        if result.is_ok() {
            admitted.push(index);
        }
        assert!(!format!("{result:?}").contains("private-canary"));
    }
    assert!(
        admitted.is_empty(),
        "malformed cases retained: {admitted:?}"
    );
}

fn scope() -> ContinuationScope {
    ContinuationScope::from_digest([1; 32])
}

#[test]
fn malformed_resumed_native_steps_cannot_be_posted() {
    let mut admitted = Vec::new();
    for (index, step) in malformed().into_iter().enumerate() {
        let mut state = Continuation::new(PROTOCOL, "gemini-3.8-flash", scope()).unwrap();
        state
            .push(ContinuationPart::Opaque(
                ContinuationData::new(&step.to_string()).unwrap(),
            ))
            .unwrap();
        let mut transcript = Transcript::new();
        transcript
            .push(Message::Agent {
                text: "".into(),
                calls: vec![],
                stop: Some(StopReason::Yielded),
                continuation: Some(state.finish("", 0, Some(StopReason::Yielded)).unwrap()),
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
            admitted.push(index);
        }
        if let Err(error) = result {
            assert!(!format!("{error:?}").contains("private-canary"));
        }
    }
    assert!(admitted.is_empty(), "malformed cases posted: {admitted:?}");
}
