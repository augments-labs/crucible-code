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

/// A key of one region sent to the other's address, as the international
/// guide prints the answer.
///
/// <https://www.alibabacloud.com/help/en/model-studio/compatibility-of-openai-with-dashscope>,
/// read 2026-10-01. The trailing space is as printed.
const WRONG_REGION: &str = r#"{
    "error": {
        "message": "Incorrect API key provided. ",
        "type": "invalid_request_error",
        "param": null,
        "code": "invalid_api_key"
    }
}"#;

/// The same answer as the mainland guide prints it, in other words.
///
/// <https://help.aliyun.com/zh/model-studio/compatibility-of-openai-with-dashscope>,
/// read 2026-10-01.
const WRONG_REGION_CN: &str = r#"{
    "error": {
        "message": "Invalid API-key provided.",
        "type": "invalid_request_error",
        "param": null,
        "code": "invalid_api_key"
    }
}"#;

/// What crucible says when `address` answers a key with `body`.
fn refused_at(address: crate::endpoint::Endpoint, body: &str) -> (u16, String) {
    let (provider, _) = at::<QwenChat>(address, 401, body);
    match read(&provider, asking("qwen3.8-max", question(), false, None)) {
        Err(ProviderError::Refused {
            provider: "qwen",
            status,
            message,
        }) => (status, message.into()),
        other => panic!("a refusal, not {other:?}"),
    }
}

#[test]
fn a_key_for_the_other_site_is_refused_naming_the_row_it_was_given_on() {
    // Matched on the code: the two guides word the message two ways.
    for body in [WRONG_REGION, WRONG_REGION_CN] {
        // A key given on the mainland row and sent to its address.
        let (status, said) = refused_at(Qwen::KEY_CN, body);
        assert_eq!(status, 401);
        assert!(said.contains("Qwen · aliyun.com key"), "{said}");
        assert!(said.contains("belongs to the site"), "{said}");
        assert!(said.contains("region"), "{said}");

        // A key from the environment, which is the international row's.
        let (_, said) = refused_at(Qwen::KEY_INTL, body);
        assert!(said.contains("Qwen · alibabacloud.com key"), "{said}");
        assert!(said.contains("belongs to the site"), "{said}");
    }

    // The vendor's own words still close the line.
    let (_, said) = refused_at(Qwen::KEY_INTL, WRONG_REGION);
    assert!(said.contains("Incorrect API key provided."), "{said}");
}

#[test]
fn another_refusal_of_a_key_is_left_in_the_vendor_s_words() {
    let other = r#"{"error":{"message":"Access denied.","type":"invalid_request_error","code":"AccessDenied"}}"#;

    let (status, said) = refused_at(Qwen::KEY_CN, other);

    assert_eq!(status, 401);
    assert_eq!(said, "Access denied.");
}

#[test]
fn an_answer_kept_by_3_8_stays_an_answer_when_the_session_moves_to_3_7() {
    // The model changes; the vendor, the key and the address do not, so the
    // answer is still this vendor's own and goes back as one, with nothing
    // of its reasoning, which 3.7 does not read.
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
    assert!(kept.is_some(), "{deltas:?}");

    read(
        &provider,
        asking("qwen3.7-plus", after_the_call(kept), true, None),
    )
    .expect("the answer reads");

    let body = sent(&replay);
    let roles: Vec<Value> = field(&body, "/messages")
        .as_array()
        .map(|messages| messages.iter().map(|one| field(one, "/role")).collect())
        .unwrap_or_default();
    assert_eq!(roles, [json!("user"), json!("assistant"), json!("tool")]);
    assert_eq!(reasoning(&body), None);
}
