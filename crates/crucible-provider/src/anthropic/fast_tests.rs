//! Anthropic's fast form: `speed: "fast"` and the fast mode beta header on
//! the models that have one, the speed each answer says it was served at, and
//! the refusals taken for a refusal of fast.

use super::*;
use crate::transport::Replay;
use crucible_credentials::{ApiKey, Header, HeaderKey};
use crucible_models::{FastForm, RequestPurpose, Served, Speed};
use crucible_types::{Message, Transcript};
use serde_json::Value;
use std::sync::Arc;

/// An answer whose usage says it was served at `speed`, or says nothing
/// where `speed` is empty.
fn answered(speed: &str) -> String {
    let said = if speed.is_empty() {
        String::new()
    } else {
        format!(",\"speed\":\"{speed}\"")
    };
    format!(
        "event: message_start\ndata: {{\"type\":\"message_start\",\"message\":{{\"id\":\"msg_1\",\"usage\":{{\"input_tokens\":10,\"output_tokens\":1{said}}}}}}}\n\n\
         event: content_block_start\ndata: {{\"index\":0,\"content_block\":{{\"type\":\"text\",\"text\":\"\"}}}}\n\n\
         event: content_block_delta\ndata: {{\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":\"Hi\"}}}}\n\n\
         event: content_block_stop\ndata: {{\"index\":0}}\n\n\
         event: message_delta\ndata: {{\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":2{said}}}}}\n\n\
         event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n"
    )
}

fn anthropic(endpoint: Endpoint, status: u16, body: &str) -> (Anthropic, Arc<Replay>) {
    let replay = Arc::new(Replay::new(status, body));
    let credential = HeaderKey::new(ApiKey::new("synthetic-fast-key"), Header::bare("x-api-key"));
    (
        Anthropic::at(
            endpoint,
            Box::new(credential),
            Box::new(Arc::clone(&replay)),
        ),
        replay,
    )
}

fn asking(model: &'static str) -> Request<'static> {
    let mut transcript = Transcript::new();
    transcript.push(Message::said("hello")).unwrap();
    Request {
        model,
        purpose: RequestPurpose::Turn,
        transcript: Box::leak(Box::new(transcript)),
        tools: &[],
        system: None,
        max_tokens: 8192,
        effort: None,
        attached: &[],
        prompt_cache: None,
    }
}

/// What the request carried: its `speed` field and its beta header.
fn sent(endpoint: Endpoint, model: &'static str, speed: Speed) -> (Option<String>, Option<String>) {
    let (provider, replay) = anthropic(endpoint, 200, &answered("standard"));
    let cancel = Cancel::new();
    let _ = crucible_runtime::answered!(provider.stream_at(asking(model), speed, &cancel));
    let sent = replay.sent();
    let body: Value = serde_json::from_str(&sent.body).unwrap();
    let beta = sent
        .headers
        .iter()
        .find(|(name, _)| name == "anthropic-beta")
        .map(|(_, value)| value.clone());
    (
        body.get("speed").and_then(Value::as_str).map(str::to_owned),
        beta,
    )
}

#[test]
fn a_fast_request_says_so_and_carries_the_fast_mode_beta() {
    assert_eq!(
        sent(VENDOR, "claude-opus-5", Speed::Fast),
        (
            Some("fast".to_owned()),
            Some("fast-mode-2026-02-01".to_owned())
        )
    );
    assert_eq!(sent(VENDOR, "claude-opus-5", Speed::Standard), (None, None));
}

#[test]
fn the_fast_mode_beta_joins_the_betas_a_model_already_carries() {
    // No model with a fast form carries another beta today, so this is the
    // rule rather than a case: a second value is joined, never written over.
    let mut outgoing = crucible_credentials::Outgoing::new();
    outgoing.set_header("anthropic-beta", "a-beta-2026-01-01");
    super::fast::beta(&mut outgoing);
    let beta = outgoing
        .headers()
        .iter()
        .find(|(name, _)| &**name == "anthropic-beta")
        .map(|(_, value)| value.to_string());
    assert_eq!(
        beta.as_deref(),
        Some("a-beta-2026-01-01,fast-mode-2026-02-01")
    );
}

#[test]
fn a_model_with_no_fast_form_and_a_configured_address_ask_for_nothing() {
    assert_eq!(sent(VENDOR, "claude-sonnet-5", Speed::Fast), (None, None));
    assert_eq!(
        sent(
            Endpoint::fixed("https://gateway.example/v1/messages"),
            "claude-opus-5",
            Speed::Fast
        ),
        (None, None)
    );
}

#[test]
fn only_the_models_the_vendor_serves_fast_have_a_fast_form() {
    let (api, _) = anthropic(VENDOR, 200, "");
    let (gateway, _) = anthropic(
        Endpoint::fixed("https://gateway.example/v1/messages"),
        200,
        "",
    );
    assert!(matches!(api.fast("claude-opus-5"), FastForm::Field(_)));
    for model in ["claude-sonnet-5", "claude-fable-5-1", "claude-haiku-4-5"] {
        assert_eq!(api.fast(model), FastForm::None, "{model}");
    }
    assert_eq!(gateway.fast("claude-opus-5"), FastForm::None);
}

#[test]
fn the_answer_says_the_speed_it_was_served_at() {
    for (speed, served) in [
        ("fast", Served::Fast),
        ("standard", Served::Standard),
        ("", Served::Unsaid),
    ] {
        let (provider, _) = anthropic(VENDOR, 200, &answered(speed));
        let cancel = Cancel::new();
        let mut stream = crucible_runtime::answered!(provider.stream_at(
            asking("claude-opus-5"),
            Speed::Fast,
            &cancel
        ))
        .unwrap();
        while let Some(delta) = crucible_runtime::answered!(stream.next()) {
            delta.unwrap();
        }
        assert_eq!(stream.served(), served, "{speed:?}");
    }
}

#[test]
fn the_refusals_taken_for_a_refusal_of_fast_are_the_ones_named_and_only_on_a_fast_request() {
    let not_enabled = r#"{"type":"error","error":{"type":"invalid_request_error","message":"Fast mode is not enabled for your organization. An organization admin must enable this feature."}}"#;
    let unsupported = r#"{"type":"error","error":{"type":"invalid_request_error","message":"'claude-opus-5' does not support the `speed` parameter. This feature is only available on supported models."}}"#;
    let credits = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Usage credits are required for fast mode."}}"#;
    let other = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: must be greater than 0"}}"#;
    let busy = r#"{"type":"error","error":{"type":"rate_limit_error","message":"Number of request tokens has exceeded your rate limit."}}"#;
    let cancel = Cancel::new();

    for (status, body, fast) in [
        (400, not_enabled, true),
        (400, unsupported, true),
        (429, credits, true),
        (400, other, false),
        (429, busy, false),
    ] {
        let (provider, _) = anthropic(VENDOR, status, body);
        let answer = crucible_runtime::answered!(provider.stream_at(
            asking("claude-opus-5"),
            Speed::Fast,
            &cancel
        ));
        assert_eq!(
            matches!(
                answer,
                Err(ProviderError::FastRefused {
                    provider: "anthropic",
                    ..
                })
            ),
            fast,
            "{body}"
        );
    }

    let (provider, _) = anthropic(VENDOR, 400, not_enabled);
    let standard = crucible_runtime::answered!(provider.stream_at(
        asking("claude-opus-5"),
        Speed::Standard,
        &cancel
    ));
    assert!(
        matches!(standard, Err(ProviderError::Refused { .. })),
        "{:?}",
        standard.err()
    );
}
