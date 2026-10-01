//! Qwen's dialect, held to the example its compatibility guide prints and to
//! what its thinking guide says a later request carries.

use crucible_models::{Delta, DeltaStream, Effort, Provider, ProviderError};
use crucible_runtime::Cancel;
use crucible_types::{
    Message, ProviderContinuation, StopReason, ToolArgs, ToolCall, ToolId, ToolResult, Transcript,
};
use serde_json::{Value, json};

use crate::completions::testing::{asking, at, field, framed, question, read, sent};
use crate::fake::found;

use super::{Qwen, QwenChat};

/// The streamed example of the compatibility guide, read 2026-10-01. The
/// page prints its first chunks and no finish.
const STREAM: &str = include_str!("fixtures/stream-text.sse");

/// A whole answer, for a test about the request rather than the stream.
fn answered() -> String {
    framed(&[
        json!({"choices": [{"index": 0, "delta": {"content": "Yes."},
        "finish_reason": "stop"}]}),
    ])
}

#[test]
fn each_of_the_six_rows_sends_to_its_own_address() {
    for (address, expected) in [
        (
            Qwen::KEY_INTL,
            "https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions",
        ),
        (
            Qwen::KEY_CN,
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        ),
        (
            Qwen::CODING_INTL,
            "https://coding-intl.dashscope.aliyuncs.com/v1/chat/completions",
        ),
        (
            Qwen::CODING_CN,
            "https://coding.dashscope.aliyuncs.com/v1/chat/completions",
        ),
        (
            Qwen::TOKEN_INTL,
            "https://token-plan.ap-southeast-1.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
        ),
        (
            Qwen::TOKEN_CN,
            "https://token-plan.cn-beijing.maas.aliyuncs.com/compatible-mode/v1/chat/completions",
        ),
    ] {
        let (provider, replay) = at::<QwenChat>(address, 200, &answered());
        read(
            &provider,
            asking("qwen3.8-max", question(), false, Some(Effort::Xhigh)),
        )
        .expect("the answer reads");
        assert_eq!(replay.sent().url, expected);
    }
}

#[test]
fn a_request_asks_its_rung_and_its_counts() {
    let (provider, replay) = at::<QwenChat>(Qwen::KEY_INTL, 200, &answered());

    read(
        &provider,
        asking("qwen3.8-max", question(), true, Some(Effort::Xhigh)),
    )
    .expect("the answer reads");

    let body = sent(&replay);
    assert_eq!(field(&body, "/max_tokens"), json!(512));
    assert_eq!(field(&body, "/reasoning_effort"), json!("xhigh"));
    assert_eq!(
        field(&body, "/stream_options"),
        json!({"include_usage": true})
    );
    assert_eq!(body.get("tool_choice"), None);
}

#[test]
fn the_documented_chunks_read_as_words_and_a_stream_without_a_finish_as_unfinished() {
    let (provider, _) = at::<QwenChat>(Qwen::KEY_INTL, 200, STREAM);
    let cancel = Cancel::new();
    let mut stream: Box<dyn DeltaStream> = crucible_runtime::answered!(
        provider.stream(asking("qwen3.8-max", question(), false, None), &cancel)
    )
    .expect("the request is sent");

    let first = crucible_runtime::answered!(stream.next());
    let second = crucible_runtime::answered!(stream.next());

    assert!(
        matches!(first, Some(Ok(Delta::Text(ref said))) if &**said == "我是"),
        "{first:?}"
    );
    assert!(
        matches!(second, Some(Err(ProviderError::Transport { .. }))),
        "{second:?}"
    );
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
fn a_3_8_answer_carries_its_reasoning_back_and_an_older_model_keeps_none() {
    let (provider, replay) = at::<QwenChat>(Qwen::KEY_INTL, 200, &thinking_then_calling());

    let deltas =
        read(&provider, asking("qwen3.8-max", question(), true, None)).expect("the answer reads");
    let kept = deltas.iter().find_map(|delta| match delta {
        Delta::Continuation(state) => state
            .clone()
            .finish("", 1, Some(StopReason::WantsTools))
            .ok(),
        _ => None,
    });
    read(
        &provider,
        asking("qwen3.8-max", after_the_call(kept), true, None),
    )
    .expect("the answer reads");
    assert_eq!(reasoning(&sent(&replay)), Some(json!("Look it up.")));

    // Sent back only where it was kept: the vendor answers without it.
    read(
        &provider,
        asking("qwen3.8-max", after_the_call(None), true, None),
    )
    .expect("the answer reads");
    assert_eq!(reasoning(&sent(&replay)), None);

    let deltas =
        read(&provider, asking("qwen3.7-plus", question(), true, None)).expect("the answer reads");
    assert!(
        !deltas
            .iter()
            .any(|delta| matches!(delta, Delta::Continuation(_))),
        "{deltas:?}"
    );
}
