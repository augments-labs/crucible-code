//! Mistral's dialect, held to the shapes its reasoning page prints and to
//! what it says a later request carries.

use crucible_models::{Delta, Effort};
use crucible_types::{
    InputTokenUsage, Message, ProviderContinuation, ProviderNumericDetail, ProviderUsage,
    StopReason, ToolArgs, ToolCall, ToolId, ToolResult, Transcript,
};
use serde_json::{Value, json};

use crate::completions::testing::{asking, at, field, framed, header, question, read, sent};
use crate::fake::found;

use super::{Mistral, MistralChat};

/// A stream in the shapes of the reasoning page, read 2026-11-05: a thinking
/// chunk, then one list holding the last of it and the first text, then text.
const STREAM: &str = include_str!("fixtures/stream-thinking.sse");

/// What the fixture's thinking chunks say, together.
const THOUGHT: &str = "17 * 23 is 340 + 51, which is 391.";

/// What the fixture's text says, together.
const SAID: &str = "17 * 23 = 391.";

#[test]
fn a_request_goes_to_mistral_signed_with_the_key_and_asks_its_rung() {
    let (provider, replay) = at::<MistralChat>(Mistral::VENDOR, 200, STREAM);

    read(
        &provider,
        asking("mistral-large-4", question(), true, Some(Effort::High)),
    )
    .expect("the answer reads");

    assert_eq!(
        replay.sent().url,
        "https://api.mistral.ai/v1/chat/completions"
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
    assert_eq!(field(&body, "/reasoning_effort"), json!("high"));
    // Its reference names no field that asks for the counts, and it sends
    // them unasked on the last chunk.
    assert_eq!(body.get("stream_options"), None);
}

/// The continuation a read of `deltas` handed over, finished as an answer
/// that said `said`, made `calls` calls and stopped for `stop`.
fn kept(
    deltas: &[Delta],
    said: &str,
    calls: usize,
    stop: StopReason,
) -> Option<ProviderContinuation> {
    deltas.iter().find_map(|delta| match delta {
        Delta::Continuation(state) => state.clone().finish(said, calls, Some(stop)).ok(),
        _ => None,
    })
}

/// The content of the assistant message `body` sends.
fn answer(body: &Value) -> Option<Value> {
    field(body, "/messages")
        .as_array()?
        .iter()
        .find(|message| field(message, "/role") == "assistant")
        .cloned()
}

#[test]
fn a_thinking_chunk_beside_the_text_reads_as_the_answer_and_goes_back_unchanged() {
    let (provider, replay) = at::<MistralChat>(Mistral::VENDOR, 200, STREAM);

    let deltas = read(
        &provider,
        asking("mistral-large-4", question(), false, Some(Effort::High)),
    )
    .expect("the answer reads");

    let said: String = deltas
        .iter()
        .filter_map(|delta| match delta {
            Delta::Text(text) => Some(&**text),
            _ => None,
        })
        .collect();
    assert_eq!(said, SAID, "{deltas:?}");
    assert!(deltas.contains(&Delta::Stopped(StopReason::Yielded)));
    let continuation = kept(&deltas, SAID, 0, StopReason::Yielded);
    assert!(continuation.is_some(), "{deltas:?}");

    let mut transcript = question();
    transcript
        .push(Message::Agent {
            text: SAID.into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
            continuation,
        })
        .expect("a valid transcript");
    transcript
        .push(Message::said("Now multiply that by 3."))
        .expect("a valid transcript");
    read(
        &provider,
        asking("mistral-large-4", transcript, false, None),
    )
    .expect("the answer reads");

    let assistant = answer(&sent(&replay));
    assert_eq!(
        assistant,
        Some(json!({
            "role": "assistant",
            "content": [
                {"type": "thinking", "thinking": [{"type": "text", "text": THOUGHT}]},
                {"type": "text", "text": SAID}
            ]
        }))
    );
}

/// The first response of a turn with tools: a thinking chunk, then a call.
fn thinking_then_calling() -> String {
    framed(&[
        json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": [
            {"type": "thinking", "thinking": [{"type": "text", "text": "Look it up."}]}
        ]}}]}),
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

#[test]
fn a_thinking_chunk_goes_back_on_the_answer_whose_calls_it_led_to() {
    let (provider, replay) = at::<MistralChat>(Mistral::VENDOR, 200, &thinking_then_calling());

    let deltas = read(&provider, asking("mistral-large-4", question(), true, None))
        .expect("the answer reads");
    let continuation = kept(&deltas, "", 1, StopReason::WantsTools);
    assert!(continuation.is_some(), "{deltas:?}");

    read(
        &provider,
        asking("mistral-large-4", after_the_call(continuation), true, None),
    )
    .expect("the answer reads");
    let assistant = answer(&sent(&replay)).unwrap_or(Value::Null);
    assert_eq!(
        field(&assistant, "/content"),
        json!([{"type": "thinking", "thinking": [{"type": "text", "text": "Look it up."}]}])
    );
    assert_eq!(field(&assistant, "/tool_calls/0/id"), json!("call-1"));
    assert_eq!(assistant.get("reasoning_content"), None);

    // An answer that kept none goes back as the calls alone: the page asks
    // for what the model wrote, and says nothing of a turn without it.
    read(
        &provider,
        asking("mistral-large-4", after_the_call(None), true, None),
    )
    .expect("the answer reads");
    let assistant = answer(&sent(&replay)).unwrap_or(Value::Null);
    assert_eq!(assistant.get("content"), None, "{assistant}");
    assert_eq!(assistant.get("reasoning_content"), None, "{assistant}");
}

#[test]
fn the_cached_part_of_a_prompt_is_read_from_the_field_its_page_names() {
    let (provider, _) = at::<MistralChat>(Mistral::VENDOR, 200, STREAM);

    let deltas = read(
        &provider,
        asking("mistral-large-4", question(), false, None),
    )
    .expect("the answer reads");

    let counted = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(1013), Some(1008)).expect("valid counts"),
        Some(30),
        None,
        Some(1043),
        &[ProviderNumericDetail::new("cached_tokens", 1008).expect("a detail")],
    )
    .expect("valid counts");
    assert!(deltas.contains(&Delta::Usage(counted)), "{deltas:?}");
}
