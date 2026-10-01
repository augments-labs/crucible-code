//! `MiMo`'s dialect, held to the example its thinking guide prints and to what
//! that guide says a later request carries.

use crucible_models::Delta;
use crucible_types::{
    InputTokenUsage, Message, ProviderNumericDetail, ProviderUsage, StopReason, ToolArgs, ToolCall,
    ToolId, ToolResult,
};
use serde_json::json;

use crate::completions::testing::{asking, at, field, question, read, sent};
use crate::fake::found;

use super::{Mimo, MimoChat};

/// The streamed example of the thinking guide, "Streaming Response (Thinking
/// Enabled)", read 2026-10-01, with the page's own elision line.
const STREAM: &str = include_str!("fixtures/stream-thinking.sse");

#[test]
fn a_request_names_the_fields_the_reference_documents_and_no_others() {
    let (provider, replay) = at::<MimoChat>(Mimo::VENDOR, 200, STREAM);

    read(&provider, asking("mimo-v2.6-pro", question(), true, None)).expect("the answer reads");

    assert_eq!(
        replay.sent().url,
        "https://api.xiaomimimo.com/v1/chat/completions"
    );
    let body = sent(&replay);
    assert_eq!(field(&body, "/max_completion_tokens"), json!(512));
    assert_eq!(body.get("max_tokens"), None);
    assert_eq!(body.get("stream_options"), None);
    assert_eq!(body.get("reasoning_effort"), None);
    assert_eq!(body.get("tool_choice"), None);
}

#[test]
fn the_documented_stream_keeps_its_reasoning_beside_the_answer_and_its_counts() {
    let (provider, replay) = at::<MimoChat>(Mimo::VENDOR, 200, STREAM);

    let deltas = read(&provider, asking("mimo-v2.6-pro", question(), false, None))
        .expect("the answer reads");

    let said: String = deltas
        .iter()
        .filter_map(|delta| match delta {
            Delta::Text(text) => Some(&**text),
            _ => None,
        })
        .collect();
    assert!(!said.is_empty(), "{deltas:?}");
    let at = deltas
        .iter()
        .position(|delta| matches!(delta, Delta::Continuation(_)))
        .expect("the reasoning is kept");
    assert_eq!(
        deltas.get(at + 1),
        Some(&Delta::Stopped(StopReason::Yielded))
    );
    let counted = ProviderUsage::new(
        InputTokenUsage::inclusive_read(Some(61), Some(0)).expect("valid counts"),
        Some(467),
        Some(29),
        Some(528),
        &[
            ProviderNumericDetail::new("cached_tokens", 0).expect("a detail"),
            ProviderNumericDetail::new("reasoning_tokens", 29).expect("a detail"),
        ],
    )
    .expect("valid counts");
    assert_eq!(deltas.last(), Some(&Delta::Usage(counted)));

    // The reasoning goes back on the answer it came with, once tools are sent.
    let kept = deltas.iter().find_map(|delta| match delta {
        Delta::Continuation(state) => state
            .clone()
            .finish(&said, 0, Some(StopReason::Yielded))
            .ok(),
        _ => None,
    });
    let mut transcript = question();
    transcript
        .push(Message::Agent {
            text: said.into(),
            calls: Vec::new(),
            stop: Some(StopReason::Yielded),
            continuation: kept,
        })
        .expect("a valid transcript");
    transcript
        .push(Message::said("And a dog?"))
        .expect("a valid transcript");
    read(&provider, asking("mimo-v2.6-pro", transcript, true, None)).expect("the answer reads");
    let body = sent(&replay);
    // The question, the answer, the next question.
    assert_eq!(field(&body, "/messages/1/role"), "assistant");
    let reasoning = field(&body, "/messages/1/reasoning_content");
    let reasoning = reasoning.as_str().unwrap_or_default();
    assert!(
        reasoning.starts_with("The user is asking for tips to"),
        "{reasoning}"
    );
}

#[test]
fn an_answer_with_a_call_and_no_kept_reasoning_is_sent_back_empty() {
    let (provider, replay) = at::<MimoChat>(Mimo::VENDOR, 200, STREAM);
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
            continuation: None,
        })
        .expect("a valid transcript");
    transcript
        .push(Message::ToolResults(vec![ToolResult {
            id: ToolId::new("call-1"),
            output: found("A small feline.", Vec::new()),
        }]))
        .expect("a valid transcript");

    read(&provider, asking("mimo-v2.6-pro", transcript, true, None)).expect("the answer reads");

    let body = sent(&replay);
    let assistant = field(&body, "/messages")
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .find(|one| field(one, "/role") == "assistant")
        })
        .cloned()
        .unwrap_or_default();
    assert_eq!(field(&assistant, "/reasoning_content"), json!(""));
}
