//! Claude Opus 5.5 and Claude Sonnet 5.5: asked the way Fable 5.1 is, since
//! their thinking is bound to the model and the conversation as its is, and
//! cached and priced at their own reviewed figures.

use super::*;
use crate::transport::Replay;
use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{Delta, FastForm, RequestPurpose};
use crucible_types::{Message, PricingDate, StopReason, Transcript};
use serde_json::{Value, json};
use std::fmt::Write as _;
use std::sync::Arc;

const NEWER: [&str; 2] = ["claude-opus-5-5", "claude-sonnet-5-5"];

/// A signed answer with a call in it, as the stream sends one.
fn answering() -> String {
    let blocks = [
        json!({"type":"thinking","thinking":"","signature":"first-private-signature"}),
        json!({"type":"text","text":"before"}),
        json!({"type":"tool_use","id":"call-1","name":"read","input":{"path":"a"}}),
    ];
    let mut events = vec![json!({"type":"message_start","message":{"id":"msg-fixture"}})];
    for (index, block) in blocks.iter().enumerate() {
        events.push(json!({"type":"content_block_start","index":index,"content_block":block}));
        events.push(json!({"type":"content_block_stop","index":index}));
    }
    events.push(json!({"type":"message_delta","delta":{"stop_reason":"tool_use"}}));
    events.push(json!({"type":"message_stop"}));
    let mut payload = String::new();
    for event in &events {
        let _ = write!(
            payload,
            "event: {}\ndata: {event}\n\n",
            event
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        );
    }
    payload
}

fn provider(body: &str) -> (Anthropic, Arc<Replay>) {
    let replay = Arc::new(Replay::new(200, body));
    let credential = HeaderKey::new(
        ApiKey::new("synthetic-newer-key"),
        Header::bare("x-api-key"),
    );
    (
        Anthropic::at(VENDOR, Box::new(credential), Box::new(Arc::clone(&replay))),
        replay,
    )
}

fn request<'a>(model: &'a str, transcript: &'a Transcript) -> Request<'a> {
    Request {
        model,
        purpose: RequestPurpose::Turn,
        transcript,
        tools: &[],
        system: None,
        max_tokens: 8192,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

#[test]
fn opus_and_sonnet_5_5_are_asked_with_the_binding_fable_5_1_is_asked_with() {
    for model in NEWER {
        let (provider, replay) = provider(&answering());
        let mut history = Transcript::new();
        history.push(Message::said("read a")).unwrap();

        crucible_runtime::answered!(provider.stream(request(model, &history), &Cancel::new()))
            .unwrap();

        let sent = replay.sent();
        let beta = sent
            .headers
            .iter()
            .find(|(name, _)| name == "anthropic-beta")
            .map(|(_, value)| value.clone())
            .unwrap_or_default();
        assert!(
            beta.contains("thinking-binding-controls-2026-08-01"),
            "{model}: {beta}"
        );
        let body: Value = serde_json::from_str(&sent.body).unwrap();
        assert_eq!(
            body.pointer("/thinking/block_binding/prefix_mismatch_behavior"),
            Some(&json!("drop_block")),
            "{model}"
        );
    }
}

#[test]
fn a_signed_answer_goes_back_to_the_model_that_signed_it_and_to_no_other() {
    let (provider, replay) = provider(&answering());
    let mut history = Transcript::new();
    history.push(Message::said("read a")).unwrap();
    let cancel = Cancel::new();
    let mut stream =
        crucible_runtime::answered!(provider.stream(request("claude-opus-5-5", &history), &cancel))
            .unwrap();
    let mut kept = None;
    while let Some(delta) = crucible_runtime::answered!(stream.next()) {
        if let Delta::Continuation(state) = delta.unwrap() {
            kept = Some(state);
        }
    }
    let state = kept
        .expect("a signed answer is kept")
        .finish("before", 1, Some(StopReason::WantsTools))
        .unwrap();
    history
        .push(Message::Agent {
            text: "before".into(),
            calls: vec![crucible_types::ToolCall {
                id: crucible_types::ToolId::new("call-1"),
                name: "read".into(),
                args: crucible_types::ToolArgs::new(r#"{"path":"a"}"#),
            }],
            stop: Some(StopReason::WantsTools),
            continuation: Some(state),
        })
        .unwrap();
    history
        .push(Message::ToolResults(vec![crucible_types::ToolResult {
            id: crucible_types::ToolId::new("call-1"),
            output: crucible_types::RecordedToolOutput::ok("contents"),
        }]))
        .unwrap();

    crucible_runtime::answered!(provider.stream(request("claude-opus-5-5", &history), &cancel))
        .unwrap();
    assert!(replay.sent().body.contains("first-private-signature"));

    crucible_runtime::answered!(provider.stream(request("claude-sonnet-5-5", &history), &cancel))
        .unwrap();
    assert!(!replay.sent().body.contains("private"));
}

#[test]
fn opus_and_sonnet_5_5_cache_from_512_tokens_at_their_own_prices() {
    let (provider, _) = provider("");
    for model in NEWER {
        let record = provider.prompt_cache_capabilities(model);
        assert_eq!(record.model_revision(), Some(model));
        assert!(
            record
                .mechanisms()
                .iter()
                .all(|one| one.minimum_prefix_tokens() == 512),
            "{model}"
        );
    }
    let priced = |model| {
        provider
            .prompt_cache_pricing(
                model,
                Some(model),
                Some(64_000),
                PromptCacheRetentionClass::Ephemeral,
                PricingDate::new(2026, 10, 1),
            )
            .unwrap()
            .expect("a reviewed price")
    };
    assert_eq!(
        priced("claude-opus-5-5").rates(),
        anthropic_rates(4_000_000_000, 200_000_000, 5_000_000_000, 20_000_000_000)
    );
    assert_eq!(
        priced("claude-sonnet-5-5").rates(),
        anthropic_rates(2_000_000_000, 200_000_000, 2_500_000_000, 10_000_000_000)
    );
}

#[test]
fn opus_5_5_is_fast_at_its_own_price() {
    let FastForm::Field(cost) = Anthropic::fast_at_vendor("claude-opus-5-5") else {
        panic!("Opus 5.5 has a fast form")
    };
    assert_eq!(cost.price, "$8 / $40 per million input / output tokens");
    assert_eq!(
        Anthropic::fast_at_vendor("claude-sonnet-5-5"),
        FastForm::None
    );
}
