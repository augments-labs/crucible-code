//! `DeepSeek`'s dialect, held to the examples its API reference prints and to
//! what its thinking-mode guide says a later request carries.

use crucible_models::{Delta, Effort, ProviderError};
use crucible_types::{
    InputTokenUsage, Message, ProviderContinuation, ProviderNumericDetail, ProviderUsage,
    StopReason, ToolArgs, ToolCall, ToolId, ToolResult, Transcript,
};
use serde_json::{Value, json};

use crate::completions::testing::{asking, at, field, framed, header, question, read, sent};
use crate::fake::found;

use super::{DeepSeek, DeepSeekChat};

/// The streamed example of the API reference, "200 (Streaming)", read
/// 2026-10-01.
const STREAM: &str = include_str!("fixtures/stream-text.sse");

#[test]
fn a_request_goes_to_deepseek_signed_with_the_key_and_asks_its_rung() {
    let (provider, replay) = at::<DeepSeekChat>(DeepSeek::VENDOR, 200, STREAM);

    read(
        &provider,
        asking("deepseek-v4-pro", question(), true, Some(Effort::Max)),
    )
    .expect("the answer reads");

    assert_eq!(
        replay.sent().url,
        "https://api.deepseek.com/chat/completions"
    );
    assert_eq!(
        header(&replay, "authorization").as_deref(),
        Some("Bearer fabricated-dialect-key")
    );
    assert!(
        header(&replay, "user-agent").is_some_and(|agent| agent.starts_with("crucible/")),
        "{:?}",
        replay.sent().headers
    );
    let body = sent(&replay);
    assert_eq!(field(&body, "/max_tokens"), json!(512));
    assert_eq!(
        field(&body, "/stream_options"),
        json!({"include_usage": true})
    );
    assert_eq!(field(&body, "/reasoning_effort"), json!("max"));
    assert_eq!(body.get("tool_choice"), None);
}

#[test]
fn the_documented_stream_reads_as_its_words_and_a_finish() {
    let (provider, _) = at::<DeepSeekChat>(DeepSeek::VENDOR, 200, STREAM);

    let deltas = read(&provider, asking("deepseek-flash", question(), false, None))
        .expect("the answer reads");

    let said: String = deltas
        .iter()
        .filter_map(|delta| match delta {
            Delta::Text(text) => Some(&**text),
            _ => None,
        })
        .collect();
    assert_eq!(said, "Hello! How can I assist you today?");
    assert!(deltas.contains(&Delta::Stopped(StopReason::Yielded)));
}

#[test]
fn the_cached_part_of_a_prompt_is_read_from_the_vendors_own_field() {
    let body = framed(&[
        json!({"choices": [{"index": 0, "delta": {"content": "Hi."}, "finish_reason": "stop"}]}),
        json!({
            "choices": [],
            "usage": {
                "prompt_tokens": 100,
                "completion_tokens": 5,
                "total_tokens": 105,
                "prompt_cache_hit_tokens": 60,
                "prompt_cache_miss_tokens": 40
            }
        }),
    ]);
    let (provider, _) = at::<DeepSeekChat>(DeepSeek::VENDOR, 200, &body);

    let deltas = read(&provider, asking("deepseek-flash", question(), false, None))
        .expect("the answer reads");

    let counted = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(100), Some(60)).expect("valid counts"),
        Some(5),
        None,
        Some(105),
        &[ProviderNumericDetail::new("cached_tokens", 60).expect("a detail")],
    )
    .expect("valid counts");
    assert_eq!(deltas.last(), Some(&Delta::Usage(counted)));
}

/// The first response of a turn with tools: reasoning, then a call.
fn thinking_then_calling() -> String {
    framed(&[
        json!({"choices": [{"index": 0, "delta": {"reasoning_content": "Look it up."}}]}),
        json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "id": "call-1",
            "type": "function", "function": {"name": "lookup", "arguments": "{}"}}]}}]}),
        json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
    ])
}

/// The conversation after the call, with `continuation` beside the answer.
fn after_the_call(continuation: Option<ProviderContinuation>) -> Transcript {
    let mut transcript = question();
    transcript
        .push(Message::Agent {
            text: "".into(),
            calls: vec![ToolCall {
                id: ToolId::new("call-1"),
                name: "lookup".into(),
                args: ToolArgs::new("{}"),
            }],
            stop: Some(StopReason::WantsTools),
            continuation,
        })
        .expect("a valid transcript");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: found("A small feline.", Vec::new()),
        }]))
        .expect("a valid transcript");
    transcript
}

/// The reasoning field of the assistant message `body` sends.
fn reasoning(body: &Value) -> Option<Value> {
    field(body, "/messages")
        .as_array()?
        .iter()
        .find(|message| field(message, "/role") == "assistant")?
        .get("reasoning_content")
        .cloned()
}

#[test]
fn every_earlier_answer_carries_its_reasoning_back_once_tools_are_sent() {
    let (provider, replay) = at::<DeepSeekChat>(DeepSeek::VENDOR, 200, &thinking_then_calling());

    let deltas = read(&provider, asking("deepseek-flash", question(), true, None))
        .expect("the answer reads");
    let kept = deltas.iter().find_map(|delta| match delta {
        Delta::Continuation(state) => state
            .clone()
            .finish("", 1, Some(StopReason::WantsTools))
            .ok(),
        _ => None,
    });
    assert!(kept.is_some(), "{deltas:?}");

    read(
        &provider,
        asking("deepseek-flash", after_the_call(kept), true, None),
    )
    .expect("the answer reads");
    assert_eq!(reasoning(&sent(&replay)), Some(json!("Look it up.")));

    // An answer that kept none is sent back empty rather than refused.
    read(
        &provider,
        asking("deepseek-flash", after_the_call(None), true, None),
    )
    .expect("the answer reads");
    assert_eq!(reasoning(&sent(&replay)), Some(json!("")));
}

#[test]
fn a_refusal_mid_stream_is_said_in_deepseeks_words() {
    let body = framed(&[json!({"error": {"type": "invalid_request_error", "message": "bad"}})]);
    let (provider, _) = at::<DeepSeekChat>(DeepSeek::VENDOR, 200, &body);

    let failed = read(&provider, asking("deepseek-flash", question(), false, None));

    assert!(
        matches!(
            &failed,
            Err(ProviderError::Upstream {
                provider: "deepseek",
                ..
            })
        ),
        "{failed:?}"
    );
}
