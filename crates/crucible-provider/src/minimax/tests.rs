//! `MiniMax`'s dialect, held to the example its API reference prints.

use crucible_models::{Delta, ProviderError};
use crucible_types::StopReason;
use serde_json::{Value, json};

use crate::completions::testing::{asking, at, field, framed, question, read, sent};

use super::{MiniMax, MiniMaxChat};

/// The streamed example of the Chat Completions reference, named "Stream",
/// read 2026-10-01: the chunks it prints, in order, as JSON. The page prints
/// no framing, so the test frames them as this wire does.
const CHUNKS: &str = include_str!("fixtures/stream-chunks.json");

fn stream() -> String {
    let chunks: Vec<Value> = serde_json::from_str(CHUNKS).expect("the example is JSON");
    framed(&chunks)
}

#[test]
fn a_request_goes_to_the_site_of_the_key_and_asks_for_reasoning_apart() {
    for (site, address) in [
        (MiniMax::IO, "https://api.minimax.io/v1/chat/completions"),
        (MiniMax::CN, "https://api.minimax.cn/v1/chat/completions"),
    ] {
        let (provider, replay) = at::<MiniMaxChat>(site, 200, &stream());

        read(&provider, asking("MiniMax-M3", question(), true, None)).expect("the answer reads");

        assert_eq!(replay.sent().url, address);
        let body = sent(&replay);
        assert_eq!(field(&body, "/max_completion_tokens"), json!(512));
        assert_eq!(body.get("max_tokens"), None);
        assert_eq!(
            field(&body, "/stream_options"),
            json!({"include_usage": true})
        );
        assert_eq!(field(&body, "/reasoning_split"), json!(true));
        assert_eq!(body.get("tool_choice"), None);
    }
}

#[test]
fn the_documented_stream_reads_its_answer_and_keeps_its_reasoning_out_of_it() {
    let (provider, _) = at::<MiniMaxChat>(MiniMax::IO, 200, &stream());

    let deltas =
        read(&provider, asking("MiniMax-M3", question(), false, None)).expect("the answer reads");

    let said: String = deltas
        .iter()
        .filter_map(|delta| match delta {
            Delta::Text(text) => Some(&**text),
            _ => None,
        })
        .collect();
    assert!(
        said.starts_with("This image is a close-up portrait"),
        "{said}"
    );
    assert!(!said.contains("The user"), "{said}");
    let at = deltas
        .iter()
        .position(|delta| matches!(delta, Delta::Continuation(_)))
        .expect("the reasoning is kept");
    assert_eq!(
        deltas.get(at + 1),
        Some(&Delta::Stopped(StopReason::Yielded))
    );
}

#[test]
fn a_failure_minimax_reports_in_its_own_status_ends_the_turn_in_its_words() {
    let body = framed(&[json!({
        "choices": [],
        "base_resp": {"status_code": 1008, "status_msg": "insufficient balance"}
    })]);
    let (provider, _) = at::<MiniMaxChat>(MiniMax::IO, 200, &body);

    let failed = read(&provider, asking("MiniMax-M3", question(), false, None));

    assert!(
        matches!(
            &failed,
            Err(ProviderError::Upstream { provider: "minimax", kind, message })
                if &**kind == "1008" && &**message == "insufficient balance"
        ),
        "{failed:?}"
    );
}

#[test]
fn a_status_of_success_is_no_failure() {
    let body = framed(&[json!({
        "choices": [{"index": 0, "delta": {"content": "Hi."}, "finish_reason": "stop"}],
        "base_resp": {"status_code": 0, "status_msg": ""}
    })]);
    let (provider, _) = at::<MiniMaxChat>(MiniMax::IO, 200, &body);

    let deltas =
        read(&provider, asking("MiniMax-M2.7", question(), false, None)).expect("the answer reads");

    assert_eq!(deltas.first(), Some(&Delta::Text("Hi.".into())));
}
